use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A track's genres, in the two shapes the rest of the tree needs: the line every display surface
/// and the FTS index read, and the ordered names the `track_genres` rows are built on.
///
/// Private fields for [`super::artist::ArtistCredit`]'s reason, and the same invariant — `line` is
/// always [`Self::names`] rendered, `None` exactly when there are none. What it deliberately does
/// *not* borrow from `ArtistCredit` is the join phrases: a release prints its artists the way it
/// chooses, and nothing prints genres at all, so they join with one separator and need no picker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenreList {
    line: Option<String>,
    names: Vec<String>,
}

/// What separates genres in the rendered line. Not [`super::artist`]'s final `" & "`: that reads as
/// a credit ("Alice & Bob"), and "Rock & Pop" would read as one genre named that.
const GENRE_JOIN: &str = ", ";

impl GenreList {
    /// **Each name once.** `genres.name` is `UNIQUE COLLATE NOCASE`, so `"Rock; rock"` and a
    /// repeated `GENRE` frame both resolve to one `genres` row and would write two `track_genres`
    /// rows for one track. The primary key is `(track_id, position)`, so nothing downstream
    /// rejects that, and the stats triggers *and* `recalculate_all_stats` would agree on a count
    /// that is twice what it should be. Kept first-seen, which is the order the tag wrote.
    #[must_use]
    pub fn new(names: Vec<String>) -> Self {
        let mut kept: Vec<String> = Vec::with_capacity(names.len());
        for name in names {
            if !kept.iter().any(|seen| seen.eq_ignore_ascii_case(&name)) {
                kept.push(name);
            }
        }

        let rendered = kept.join(GENRE_JOIN);
        Self {
            line: (!rendered.is_empty()).then_some(rendered),
            names: kept,
        }
    }

    /// One typed name, which is what a plain text box means.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        if name.is_empty() {
            return Self::default();
        }
        Self::new(vec![name.to_owned()])
    }

    /// The genres as printed, for the `genre` column and everything reading it.
    #[must_use]
    pub fn line(&self) -> Option<&str> {
        self.line.as_deref()
    }

    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The name a row's `genre_id` points at: the first genre, `None` for none.
    #[must_use]
    pub fn primary(&self) -> Option<&str> {
        self.names.first().map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Split a line [`Self::line`] rendered back into its names, for a caller holding the column
    /// rather than the rows.
    ///
    /// **Display only.** A genre whose own name contains the separator ("Chanson, Francaise") is
    /// indistinguishable from two once rendered, so this can hand back a name nobody tagged. That
    /// costs a wrong label on a chip and nothing else, which is why anything that *persists* a
    /// genre reads the rows through `queries::track::get_track_genres_by_ids` instead. Lives here
    /// so the split and `GENRE_JOIN` cannot drift apart.
    pub fn names_in_line(line: &str) -> impl Iterator<Item = &str> {
        line.split(GENRE_JOIN).map(str::trim).filter(|name| !name.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct GenreStats {
    pub id: i64,
    pub name: String,
    pub track_count: i32,
    pub total_duration_ms: i64,
}

#[cfg(test)]
#[path = "tests/genre_tests.rs"]
mod tests;
