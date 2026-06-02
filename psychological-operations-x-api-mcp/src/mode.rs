/// `readonly` exposes only the read tools (search, get_tweet, …).
/// `full` adds the mutating set (`post_tweet`, `reply_to_tweet`,
/// `quote_tweet`, `like`, `retweet`, `bookmark`). The `whoami` tool
/// is exposed in both modes.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Readonly,
    Full,
}
