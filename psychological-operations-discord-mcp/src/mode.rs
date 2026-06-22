/// `readonly` exposes only the read tools; `full` adds the mutating set.
/// The queue tools (`read_queue` / `mark_handled`) are exposed in both modes.
/// (Read/write tools are not implemented yet — the gate is wired so they slot
/// in later.)
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Readonly,
    Full,
}
