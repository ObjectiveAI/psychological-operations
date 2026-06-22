//! The Discord psyop family. Mirrors [`crate::cli::psyops::x`] but for
//! Discord messages; ingestion sources are not modeled yet, so for now it
//! carries only the scoring/delivery shape. The stage types are duplicated
//! from `x` (identical for now) so the family stays self-contained and can
//! diverge freely.

pub mod all_dms;
pub mod channel;
pub mod dm;
pub mod psyop;
pub mod server;
pub mod sort_by;
pub mod stage;
pub mod trigger;

pub use all_dms::AllDms;
pub use channel::Channel;
pub use dm::Dm;
pub use psyop::{PsyOp, PsyopType};
pub use server::Server;
pub use sort_by::SortBy;
pub use trigger::Trigger;
pub use stage::{is_vector_function, OutputTop, Stage, StageBase};
