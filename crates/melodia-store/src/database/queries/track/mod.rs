//! Queries over `tracks`, one file per kind of answer a caller wants back. Everything is re-exported,
//! so a call site still names `queries::track::…`.

mod credits;
#[cfg(test)]
mod full_rows;
mod list;
mod lookup;
mod ranking;
mod write;

pub use credits::*;
#[cfg(test)]
pub use full_rows::*;
pub use list::*;
pub use lookup::*;
pub use ranking::*;
pub use write::*;

#[cfg(test)]
#[path = "../tests/track_tests.rs"]
mod tests;
