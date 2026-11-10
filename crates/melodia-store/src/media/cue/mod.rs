//! Cue sheets: a whole-disc rip's sheet read into the tracks it carves out of its image.
//!
//! Every sheet is read outside the write: [`plan`] reads the sheets a folder scan or a watcher
//! batch touches before its write opens, and `queries::scan::cue_units` writes what it settled in
//! that transaction.

mod layout;
mod merge;
mod parse;
mod plan;
mod resolve;
mod text;

pub(crate) use merge::{CueRow, rows};
pub(crate) use plan::{Claim, SheetJob, SheetRead};
pub use plan::{CueChunk, CuePlan, PlanInput, settle};
