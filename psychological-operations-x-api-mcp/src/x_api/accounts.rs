//! `list_accounts` — the bootstrap tool that discovers which identities
//! the session's agent may act as, plus the object-safe shim that lets
//! the (otherwise generic) server reach the objectiveai
//! [`CommandExecutor`].
//!
//! A session's agent can act as several identities: its own agent
//! instance hierarchy (AIH) and any tags bound to it. `list_accounts`
//! enumerates the AIH + its tags, then reports the subset that are
//! actually usable — i.e. have a stored token carrying a `refresh_token`
//! (everything needed to mint a fresh access token), and only when the
//! X-App OAuth client is configured at all. The agent then passes one of
//! the returned names as the required `account` argument to every other
//! tool. `list_accounts` itself takes no `account` and is quota-free —
//! it's the bootstrap that makes the other calls possible.
//!
//! [`CommandExecutor`] is generic (and so not object-safe), but the
//! server is a concrete type embedded in `StreamableHttpService<…>`. So
//! the one operation we need — "fetch the tags bound to an AIH" — is
//! wrapped behind the object-safe [`AgentTagLister`], with a blanket impl
//! over every executor.

use std::future::Future;
use std::pin::Pin;

use futures::StreamExt;
use objectiveai_sdk::cli::command::CommandExecutor;
use objectiveai_sdk::cli::command::agents::instances::get as instances_get;
use psychological_operations_sdk::x::client::AuthMode;
use psychological_operations_sdk::x::x_app;
use rmcp::model::{CallToolResult, Content, Extensions};
use rmcp::{ErrorData, handler::server::wrapper::Parameters, schemars, tool, tool_router};

use super::PsychologicalOperationsXApiMcp;

/// Object-safe shim over the (generic, non-object-safe)
/// [`CommandExecutor`], exposing the single operation `list_accounts`
/// needs: fetch the tag names bound to a given agent instance hierarchy.
pub trait AgentTagLister: Send + Sync {
    /// Tags currently bound to `aih` (newest-bound first); empty when the
    /// agent has none. The concrete executor's error type isn't
    /// object-safe to surface, so failures come back stringified.
    fn agent_tags<'a>(
        &'a self,
        aih: String,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>>;
}

impl<E> AgentTagLister for E
where
    E: CommandExecutor + Send + Sync + 'static,
    E::Error: std::fmt::Display,
{
    fn agent_tags<'a>(
        &'a self,
        aih: String,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>> {
        Box::pin(async move {
            // Fetch the EXACT agent (not its children): split the AIH at
            // its last '/' into instance leaf + optional parent, mirroring
            // `notify::selector_for`. `agents instances get` always yields
            // an item for an explicitly-named target.
            let target = match aih.rsplit_once('/') {
                Some((parent, leaf)) => instances_get::Target::Direct {
                    parent_agent_instance_hierarchy: Some(parent.to_string()),
                    agent_instance: leaf.to_string(),
                },
                None => instances_get::Target::Direct {
                    parent_agent_instance_hierarchy: None,
                    agent_instance: aih.clone(),
                },
            };
            let request = instances_get::Request {
                path_type: instances_get::Path::AgentsInstancesGet,
                targets: vec![target],
                base: Default::default(),
            };
            let stream = instances_get::execute(self, request, None)
                .await
                .map_err(|e| e.to_string())?;
            futures::pin_mut!(stream);
            match stream.next().await {
                Some(Ok(item)) => Ok(item.tags),
                Some(Err(e)) => Err(e.to_string()),
                None => Ok(Vec::new()),
            }
        })
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListAccountsRequest {}

#[tool_router(router = accounts_tools, vis = "pub")]
impl PsychologicalOperationsXApiMcp {
    #[tool(
        name = "list_accounts",
        description = "List the X accounts (identities) you may act as: your own agent instance \
                       hierarchy plus any tags bound to it that have a refreshable token. Pass one \
                       of the returned names as the required `account` argument to every other tool."
    )]
    async fn list_accounts(
        &self,
        Parameters(_req): Parameters<ListAccountsRequest>,
        extensions: Extensions,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.resolve_session(&extensions).await?;
        let aih = state.agent_instance_hierarchy.clone();

        let tags = self
            .accounts
            .agent_tags(aih.clone())
            .await
            .map_err(|e| ErrorData::internal_error(format!("list agent tags: {e}"), None))?;

        // Candidate identities: the agent's own AIH plus each bound tag,
        // de-duplicated with original order preserved (AIH first).
        let mut candidates: Vec<String> = Vec::with_capacity(tags.len() + 1);
        for name in std::iter::once(aih).chain(tags) {
            if !candidates.contains(&name) {
                candidates.push(name);
            }
        }

        // An account is usable only if the X-App OAuth client is
        // configured at all (without it, no token can be refreshed).
        let x_app = x_app::config::load(&self.db)
            .await
            .map_err(|e| ErrorData::internal_error(format!("load x_app config: {e}"), None))?;
        let usable_app = x_app.is_complete();

        // Probe each candidate concurrently: available iff the X-App is
        // configured AND the candidate has a stored token carrying a
        // `refresh_token` (everything needed to refresh it).
        let http = self.build_client();
        let checks = candidates.into_iter().map(|name| {
            let http = http.clone();
            async move {
                if !usable_app {
                    return None;
                }
                match http.read_auth(&AuthMode::Agent(name.clone())).await {
                    Ok(Some(t)) if t.refresh_token.is_some() => Some(name),
                    _ => None,
                }
            }
        });
        let available: Vec<String> = futures::future::join_all(checks)
            .await
            .into_iter()
            .flatten()
            .collect();

        let body = serde_json::to_string(&available)
            .map_err(|e| ErrorData::internal_error(format!("serialize accounts: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }
}
