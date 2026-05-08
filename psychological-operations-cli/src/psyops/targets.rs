use clap::Subcommand;

use crate::targets::destinations::Destination;

#[derive(Subcommand)]
pub enum Commands {
    /// Get this psyop's per-name targets. With `--commit <sha>` reads
    /// the commit-specific list; otherwise reads the base list.
    Get {
        name: String,
        index: Option<usize>,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Add a target to this psyop. With `--commit <sha>` adds
    /// to the commit-specific list; otherwise adds to base.
    Add {
        name: String,
        json: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Remove a target by index. `--commit` selects the layer.
    Del {
        name: String,
        index: usize,
        #[arg(long)]
        commit: Option<String>,
    },
}

impl Commands {
    pub fn handle(self, cfg: &crate::run::Config) -> Result<crate::Output, crate::error::Error> {
        match self {
            Commands::Get { name, index, commit } => {
                let json_cfg = crate::config::load(cfg);
                let list: Vec<Destination> = json_cfg.psyops.get(&name).map(|o| match commit.as_deref() {
                    Some(sha) => o.commits.get(sha).map(|c| c.targets.clone()).unwrap_or_default(),
                    None => o.base.targets.clone(),
                }).unwrap_or_default();
                match index {
                    Some(i) => {
                        let entry = list.get(i)
                            .ok_or_else(|| crate::error::Error::Other(format!("no target at index {i}")))?;
                        Ok(crate::Output::ConfigGet(serde_json::to_string(entry)?))
                    }
                    None => Ok(crate::Output::ConfigGet(serde_json::to_string(&list)?)),
                }
            }
            Commands::Add { name, json, commit } => {
                let parsed: Destination = serde_json::from_str(&json)?;
                let mut json_cfg = crate::config::load(cfg);
                let overrides = json_cfg.psyops.entry(name).or_default();
                match commit.as_deref() {
                    Some(sha) => overrides.commits.entry(sha.to_string()).or_default().targets.push(parsed),
                    None => overrides.base.targets.push(parsed),
                }
                crate::config::save(&json_cfg, cfg)?;
                Ok(crate::Output::ConfigSet)
            }
            Commands::Del { name, index, commit } => {
                let mut json_cfg = crate::config::load(cfg);
                {
                    let overrides = json_cfg.psyops.get_mut(&name)
                        .ok_or_else(|| crate::error::Error::Other(format!("no psyop config entry for \"{name}\"")))?;
                    let target = match commit.as_deref() {
                        Some(sha) => &mut overrides.commits.get_mut(sha)
                            .ok_or_else(|| crate::error::Error::Other(format!("no commit override \"{sha}\" for psyop \"{name}\"")))?
                            .targets,
                        None => &mut overrides.base.targets,
                    };
                    if index >= target.len() {
                        return Err(crate::error::Error::Other(format!("no target at index {index}")));
                    }
                    target.remove(index);
                    if let Some(sha) = commit.as_deref() {
                        if overrides.commits.get(sha).is_some_and(|c| c.is_empty()) {
                            overrides.commits.remove(sha);
                        }
                    }
                }
                if json_cfg.psyops.get(&name).is_some_and(|o| o.is_empty()) {
                    json_cfg.psyops.remove(&name);
                }
                crate::config::save(&json_cfg, cfg)?;
                Ok(crate::Output::ConfigSet)
            }
        }
    }
}
