//! Shared agent selector for `agents {login,browser,enqueue}`.
//!
//! Exactly one of `--agent-tag` / `--me` / `--agent-instance` is
//! required; `--parent-agent-instance-hierarchy` is only valid alongside
//! `--agent-instance`. [`AgentRef::resolve`] collapses the selection to a
//! single agent `name`, which becomes a filesystem path segment
//! (`…/browser/agent/<name>/…`) and the flat CEF profile dir
//! `agent-<name>` — so every form except a verbatim tag replaces '/' with
//! '_'. A tag containing '/' is rejected at parse time instead.

use clap::{ArgGroup, Args};

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
    /// Collapse the selector to the resolved agent `name`. The clap
    /// `agent_ref` group guarantees exactly one selector is set; the
    /// final `unreachable!` mirrors `browser::args::Args::initial_mode`.
    ///
    /// Every form except a verbatim `--agent-tag` replaces '/' with '_'
    /// so the name is a single path segment (see the module docs).
    pub fn resolve(&self, cfg: &crate::run::Config) -> String {
        if let Some(tag) = self.agent_tag.as_deref() {
            tag.to_string()
        } else if self.me {
            cfg.objectiveai_agent_instance_hierarchy.replace('/', "_")
        } else if let Some(inst) = self.agent_instance.as_deref() {
            let full = match self.parent_agent_instance_hierarchy.as_deref() {
                Some(parent) => format!("{parent}/{inst}"),
                None => format!("{}/{}", cfg.objectiveai_agent_instance_hierarchy, inst),
            };
            full.replace('/', "_")
        } else {
            unreachable!("clap ArgGroup agent_ref required=true, multiple=false")
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
    fn tag_is_verbatim() {
        assert_eq!(sel(Some("my-tag"), false, None, None).resolve(&cfg("h")), "my-tag");
    }

    #[test]
    fn me_collapses_hierarchy() {
        assert_eq!(sel(None, true, None, None).resolve(&cfg("a/b/c")), "a_b_c");
    }

    #[test]
    fn me_default_hierarchy() {
        assert_eq!(
            sel(None, true, None, None).resolve(&cfg("psychological-operations")),
            "psychological-operations"
        );
    }

    #[test]
    fn instance_no_parent_uses_hierarchy() {
        assert_eq!(sel(None, false, Some("inst"), None).resolve(&cfg("a/b")), "a_b_inst");
    }

    #[test]
    fn instance_with_parent_ignores_hierarchy() {
        assert_eq!(
            sel(None, false, Some("inst"), Some("p/q")).resolve(&cfg("ignored")),
            "p_q_inst"
        );
    }

    #[test]
    fn instance_value_slash_collapses() {
        assert_eq!(sel(None, false, Some("x/y"), None).resolve(&cfg("h")), "h_x_y");
    }

    #[test]
    fn tag_parser_rejects_slash() {
        assert!(tag_without_slash("a/b").is_err());
        assert_eq!(tag_without_slash("a-b").unwrap(), "a-b");
    }
}
