use sqlx::FromRow;

/// A track's genres as the line displays and the FTS index read, and as the ordered names the
/// `track_genres` rows are built on. Private fields for [`super::artist::ArtistCredit`]'s
/// invariant: `line` is always [`Self::names`] rendered, `None` exactly when there are none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenreList {
    line: Option<String>,
    names: Vec<String>,
}

/// What separates genres in the rendered line. Not [`super::artist`]'s final `" & "`: that reads as
/// a credit ("Alice & Bob"), and "Rock & Pop" would read as one genre named that.
const GENRE_JOIN: &str = ", ";

impl GenreList {
    /// Each name once, first-seen: `genres.name` is `UNIQUE COLLATE NOCASE`, so `"Rock; rock"`
    /// would write two `track_genres` rows for one genre, which nothing rejects and every count
    /// then doubles.
    #[must_use]
    pub fn new(names: Vec<String>) -> Self {
        let mut kept: Vec<String> = Vec::with_capacity(names.len());
        for name in names {
            if !kept.iter().any(|seen| seen.eq_ignore_ascii_case(&name)) {
                kept.push(name);
            }
        }

        let rendered = kept.join(GENRE_JOIN);
        Self { line: (!rendered.is_empty()).then_some(rendered), names: kept }
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

    /// Display only: a name containing the separator ("Chanson, Francaise") splits in two, so
    /// anything persisting a genre reads the rows instead. Here so the split and `GENRE_JOIN`
    /// can't drift.
    pub fn names_in_line(line: &str) -> impl Iterator<Item = &str> {
        line.split(GENRE_JOIN).map(str::trim).filter(|name| !name.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct GenreStats {
    pub id: i64,
    pub name: String,
    pub track_count: i32,
    pub total_duration_ms: i64,
}

#[cfg(test)]
#[path = "tests/genre_tests.rs"]
mod tests;
