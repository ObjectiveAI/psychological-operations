//! Shared agent selector for `agents {login,browser,enqueue}`.
//!
//! In the current model agents are addressed by **tag** (`--agent-tag`);
//! the legacy `--me` / `--agent-instance` AIH selectors remain for
//! compatibility. Exactly one is required;
//! `--parent-agent-instance-hierarchy` is only valid alongside
//! `--agent-instance`. The selector resolves to a single agent string
//! via [`AgentRef::resolve_raw`], used **verbatim** — the queue stores
//! it raw, and the browser maps it to ONE flat CEF profile dir per
//! persona (`Mode::cache_subdir` collapses any separator, so the profile
//! is always a direct child of `cef-root`).

use clap::{ArgGroup, Args};

#[derive(Debug, Args)]
#[command(group = ArgGroup::new("agent_ref")
    .required(true)
    .multiple(false)
    .args(["agent_tag", "me", "agent_instance"]))]
pub struct AgentRef {
    /// Select the agent by tag, used verbatim as the name.
    #[arg(long, group = "agent_ref", value_name = "TAG")]
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
    /// The resolved agent string, used **verbatim** by every `agents`
    /// subcommand — the tag as-is; `--me` / `--agent-instance` keep their
    /// '/' separators. The queue stores it raw; `agents login` / `browser`
    /// use it as the persona name, which maps to a single flat CEF profile
    /// dir (separators collapsed — see `Mode::cache_subdir`).
    /// The clap `agent_ref` group guarantees exactly one selector is set;
    /// the final `unreachable!` mirrors `browser::args::Args::initial_mode`.
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

    fn sel(tag: Option<&str>, me: bool, inst: Option<&str>, parent: Option<&str>) -> AgentRef {
        AgentRef {
            agent_tag: tag.map(Into::into),
            me,
            agent_instance: inst.map(Into::into),
            parent_agent_instance_hierarchy: parent.map(Into::into),
        }
    }

    #[test]
    fn tag_verbatim() {
        // Tags are used as-is, including any '/'.
        assert_eq!(
            sel(Some("my-tag"), false, None, None).resolve_raw(&cfg("h")),
            "my-tag"
        );
        assert_eq!(
            sel(Some("a/b"), false, None, None).resolve_raw(&cfg("h")),
            "a/b"
        );
    }

    #[test]
    fn me_keeps_slashes() {
        let s = sel(None, true, None, None);
        assert_eq!(s.resolve_raw(&cfg("a/b/c")), "a/b/c");
    }

    #[test]
    fn me_default_hierarchy() {
        let s = sel(None, true, None, None);
        assert_eq!(
            s.resolve_raw(&cfg("psychological-operations")),
            "psychological-operations"
        );
    }

    #[test]
    fn instance_no_parent_keeps_slashes() {
        let s = sel(None, false, Some("inst"), None);
        assert_eq!(s.resolve_raw(&cfg("a/b")), "a/b/inst");
    }

    #[test]
    fn instance_with_parent_keeps_slashes() {
        let s = sel(None, false, Some("inst"), Some("p/q"));
        assert_eq!(s.resolve_raw(&cfg("ignored")), "p/q/inst");
    }
}
