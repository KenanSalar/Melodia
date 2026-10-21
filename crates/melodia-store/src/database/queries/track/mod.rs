//! Queries over `tracks`, one file per kind of answer a caller wants back. Everything is re-exported,
//! so a call site still names `queries::track::…`.

pub(crate) mod columns;
mod credits;
mod entity_ids;
#[cfg(test)]
mod full_rows;
mod keyset;
mod list;
mod lookup;
mod membership;
mod ranking;
mod scope_facts;
mod search_text;
mod write;

pub use credits::*;
pub use entity_ids::*;
#[cfg(test)]
pub use full_rows::*;
pub use keyset::*;
pub use list::*;
pub use lookup::*;
pub use ranking::*;
pub use scope_facts::*;
pub use search_text::*;
pub use write::*;

/// `ids` as the JSON array `json_each` reads, so a set of any size is one bind.
fn json_array(ids: &[i64]) -> String {
    let items: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!("[{}]", items.join(","))
}

#[cfg(test)]
#[path = "../tests/track_tests.rs"]
mod tests;
