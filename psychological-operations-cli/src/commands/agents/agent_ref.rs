//! Shared agent selector for `agents {login,browser,enqueue}`.
//!
//! Exactly one of `--agent-tag` / `--me` / `--agent-instance` is
//! required; `--parent-agent-instance-hierarchy` is only valid alongside
//! `--agent-instance`. Two resolutions, intentionally different:
//!
//! - [`AgentRef::resolve_name`] — for `agents login` / `agents browser`:
//!   collapses '/' to '_' so the result is a single filesystem path
//!   segment (`…/browser/agent/<name>/`, flat CEF profile `agent-<name>`).
//! - [`AgentRef::resolve_raw`] — for `agents enqueue`: verbatim, '/'
//!   preserved (the queue stores the agent and deliverer raw).
//!
//! In both, the tag is used verbatim; a tag containing '/' is rejected at
//! parse time.

use clap::{ArgGroup, Args};
use psychological_operations_sdk::x::queue::AgentKind;

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("agent_ref")
    .required(true)
    .multiple(false)
    .args(["agent_tag", "me", "agent_instance"]))]
pub struct AgentRef {
    /// Select the agent by tag, used verbatim as the name. Rejected if
    /// it contains '/' (the name becomes a filesystem path segment).
    #[arg(long, group = "agent_ref", value_name = "TAG", value_parser = tag_without_slash)]
    pub agent_tag: Option<String>,

    /// Select the configured agent instance hierarchy itself
    /// (`OBJECTIVEAI_AGENT_INSTANCE_HIERARCHY`).
    #[arg(long, group = "agent_ref")]
    pub me: bool,

    /// Select `<hierarchy>/<INSTANCE>` (or `<PARENT>/<INSTANCE>` when
    /// `--parent-agent-instance-hierarchy` is given).
    #[arg(long, group = "agent_ref", value_name = "INSTANCE")]
    pub agent_instance: Option<String>,

    /// Explicit parent hierarchy for `--agent-instance`. Only valid
    /// alongside it; when omitted the configured hierarchy is the parent.
    #[arg(long, requires = "agent_instance", value_name = "PARENT")]
    pub parent_agent_instance_hierarchy: Option<String>,
}

impl AgentRef {
    /// The resolved agent string **without** path-safety collapsing —
    /// the tag verbatim; `--me` / `--agent-instance` keep their '/'
    /// separators. Used by `agents enqueue`, where the queue stores the
    /// agent (and the deliverer) verbatim. The clap `agent_ref` group
    /// guarantees exactly one selector is set; the final `unreachable!`
    /// mirrors `browser::args::Args::initial_mode`.
    pub fn resolve_raw(&self, cfg: &crate::run::Config) -> String {
        if let Some(tag) = self.agent_tag.as_deref() {
            tag.to_string()
        } else if self.me {
            cfg.objectiveai_agent_instance_hierarchy.clone()
        } else if let Some(inst) = self.agent_instance.as_deref() {
            match self.parent_agent_instance_hierarchy.as_deref() {
                Some(parent) => format!("{parent}/{inst}"),
                None => format!("{}/{}", cfg.objectiveai_agent_instance_hierarchy, inst),
            }
        } else {
            unreachable!("clap ArgGroup agent_ref required=true, multiple=false")
        }
    }

    /// The resolved persona **name** for `agents login` / `agents
    /// browser`: [`resolve_raw`](Self::resolve_raw) with '/' collapsed to
    /// '_' so it is a single filesystem path segment (it becomes
    /// `…/browser/agent/<name>/` and the flat CEF profile `agent-<name>`).
    /// The tag is already verbatim and slash-free (rejected at parse).
    pub fn resolve_name(&self, cfg: &crate::run::Config) -> String {
        self.resolve_raw(cfg).replace('/', "_")
    }

    /// Which `agent_kind` this selector resolves to: `--agent-tag`
    /// yields [`AgentKind::AgentTag`]; `--me` / `--agent-instance`
    /// yield [`AgentKind::AgentInstanceHierarchy`].
    pub fn kind(&self) -> AgentKind {
        if self.agent_tag.is_some() {
            AgentKind::AgentTag
        } else {
            AgentKind::AgentInstanceHierarchy
        }
    }
}

/// clap value-parser for `--agent-tag`. The tag is used verbatim as the
/// agent `name`, which becomes a filesystem path segment / CEF profile
/// dir, so a '/' must be rejected rather than silently mangled.
fn tag_without_slash(s: &str) -> Result<String, String> {
    if s.contains('/') {
        Err(format!("agent tag must not contain '/': {s:?}"))
    } else {
        Ok(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::Config;

    fn cfg(hierarchy: &str) -> Config {
        Config {
            objectiveai_agent_instance_hierarchy: hierarchy.into(),
            ..Config::default()
        }
    }

    fn sel(
        tag: Option<&str>,
        me: bool,
        inst: Option<&str>,
        parent: Option<&str>,
    ) -> AgentRef {
        AgentRef {
            agent_tag: tag.map(Into::into),
            me,
            agent_instance: inst.map(Into::into),
            parent_agent_instance_hierarchy: parent.map(Into::into),
        }
    }

    #[test]
    fn tag_verbatim_both_resolutions() {
        let s = sel(Some("my-tag"), false, None, None);
        assert_eq!(s.resolve_name(&cfg("h")), "my-tag");
        assert_eq!(s.resolve_raw(&cfg("h")), "my-tag");
    }

    #[test]
    fn me_name_collapses_raw_keeps_slashes() {
        let s = sel(None, true, None, None);
        assert_eq!(s.resolve_name(&cfg("a/b/c")), "a_b_c");
        assert_eq!(s.resolve_raw(&cfg("a/b/c")), "a/b/c");
    }

    #[test]
    fn me_default_hierarchy() {
        let s = sel(None, true, None, None);
        assert_eq!(s.resolve_name(&cfg("psychological-operations")), "psychological-operations");
        assert_eq!(s.resolve_raw(&cfg("psychological-operations")), "psychological-operations");
    }

    #[test]
    fn instance_no_parent_name_vs_raw() {
        let s = sel(None, false, Some("inst"), None);
        assert_eq!(s.resolve_name(&cfg("a/b")), "a_b_inst");
        assert_eq!(s.resolve_raw(&cfg("a/b")), "a/b/inst");
    }

    #[test]
    fn instance_with_parent_name_vs_raw() {
        let s = sel(None, false, Some("inst"), Some("p/q"));
        assert_eq!(s.resolve_name(&cfg("ignored")), "p_q_inst");
        assert_eq!(s.resolve_raw(&cfg("ignored")), "p/q/inst");
    }

    #[test]
    fn tag_parser_rejects_slash() {
        assert!(tag_without_slash("a/b").is_err());
        assert_eq!(tag_without_slash("a-b").unwrap(), "a-b");
    }

    #[test]
    fn kind_reflects_selector() {
        assert_eq!(sel(Some("t"), false, None, None).kind(), AgentKind::AgentTag);
        assert_eq!(sel(None, true, None, None).kind(), AgentKind::AgentInstanceHierarchy);
        assert_eq!(sel(None, false, Some("i"), None).kind(), AgentKind::AgentInstanceHierarchy);
        assert_eq!(
            sel(None, false, Some("i"), Some("p")).kind(),
            AgentKind::AgentInstanceHierarchy
        );
    }
}
