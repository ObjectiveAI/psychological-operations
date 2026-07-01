/// `readonly` exposes only the read tools; `full` adds the mutating set
/// ([`FULL_ONLY_TOOLS`]).
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Readonly,
    Full,
}

/// Tools only listed + callable when the session's mode is [`Mode::Full`].
/// (The read tools are exposed in both modes.)
pub(crate) const FULL_ONLY_TOOLS: &[&str] = &["send_message"];
