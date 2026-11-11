//! Cue sheets: a whole-disc rip's sheet read into the tracks it carves out of its image, and a tag
//! edit on those tracks written back into it ([`write`]).
//!
//! Every sheet is read outside the write: [`plan`] reads the sheets a folder scan, a watcher batch
//! or an import touches before its write opens, and `queries::scan::cue_units` writes what it
//! settled in that transaction.

mod around;
mod layout;
mod merge;
mod parse;
mod plan;
mod resolve;
mod text;
mod track_tags;
mod write;

pub use around::{named_files, settle_dirs};
pub(crate) use merge::{CueRow, rows};
pub(crate) use plan::{Claim, SheetJob, SheetRead};
pub use plan::{CueChunk, CuePlan, PlanInput, settle};
pub use track_tags::read_track_tags;
pub use write::{SheetEdit, replace_sheet, rewritten_sheet};
