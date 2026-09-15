//! Role credits: who else worked on a recording, and in what capacity.
//!
//! Deliberately not in [`super::artist`], which owns a different shape. An artist credit is a
//! *printed line* — ordered names with the phrases the release joins them by — and all of that
//! file's machinery exists to keep the line and the list from drifting apart. A role credit has no
//! line to print: it is a set per role, which is how every tagger writes one and how every player
//! reads one.

/// A capacity someone is credited in.
///
/// Ten rather than the dozen a tag mapping lists, and the two omissions are the interesting part.
/// **`writer` folds onto [`Self::Composer`]** instead of being its own role: `ID3v2` gives both
/// `writer` and `lyricist` the `TEXT` frame, so the two cannot be told apart on an MP3 and a round
/// trip through one would silently consume the other. **`director`** is video metadata with no
/// audio surface here.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CreditRole {
    Composer,
    Lyricist,
    Conductor,
    Remixer,
    Arranger,
    Producer,
    Engineer,
    Mixer,
    DjMixer,
    Performer,
}

/// Every role, in the order a credits list reads best: the writing credits, then the performing
/// ones, then the studio. Positional, and the `track_credits.role` strings below are the stored
/// form, so reordering this is free and renaming one is a migration.
pub const ROLES: [CreditRole; 10] = [
    CreditRole::Composer,
    CreditRole::Lyricist,
    CreditRole::Arranger,
    CreditRole::Conductor,
    CreditRole::Performer,
    CreditRole::Remixer,
    CreditRole::Producer,
    CreditRole::Engineer,
    CreditRole::Mixer,
    CreditRole::DjMixer,
];

impl CreditRole {
    /// The `track_credits.role` value. Stable — a rename is a data migration, not an edit here.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Lyricist => "lyricist",
            Self::Conductor => "conductor",
            Self::Remixer => "remixer",
            Self::Arranger => "arranger",
            Self::Producer => "producer",
            Self::Engineer => "engineer",
            Self::Mixer => "mixer",
            Self::DjMixer => "dj_mixer",
            Self::Performer => "performer",
        }
    }

    /// The inverse of [`Self::as_db_str`]. `None` for a role this build doesn't know, which a
    /// database written by a newer version can hold.
    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        ROLES.into_iter().find(|role| role.as_db_str() == value)
    }

    /// Whether [`RoleCredit::detail`] means anything for this role. Only a performer has an
    /// instrument or a voice; every other role's detail is empty and a writer must not invent one.
    #[must_use]
    pub fn carries_detail(self) -> bool {
        matches!(self, Self::Performer)
    }
}

/// One person in one role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleCredit {
    pub role: CreditRole,
    pub name: String,
    /// Instrument or voice, for [`CreditRole::Performer`] only. Empty otherwise.
    pub detail: String,
}

/// A track's whole role credit set, plus the one string that renders it.
///
/// Private fields for [`super::artist::ArtistCredit`]'s reason: `tracks.credits` is the rendered
/// half and the rows are the structured half, and a surface showing one while the index searches
/// the other is a bug nothing reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleCredits {
    line: Option<String>,
    credits: Vec<RoleCredit>,
}

impl RoleCredits {
    #[must_use]
    pub fn new(credits: Vec<RoleCredit>) -> Self {
        let rendered = render(&credits);
        Self { line: (!rendered.is_empty()).then_some(rendered), credits }
    }

    /// The names as one string, for the `credits` column and the FTS index behind it.
    #[must_use]
    pub fn line(&self) -> Option<&str> {
        self.line.as_deref()
    }

    #[must_use]
    pub fn all(&self) -> &[RoleCredit] {
        &self.credits
    }

    /// The names credited in one role, in tag order.
    pub fn for_role(&self, role: CreditRole) -> impl Iterator<Item = &RoleCredit> + Clone {
        self.credits.iter().filter(move |credit| credit.role == role)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.credits.is_empty()
    }
}

/// The names as one searchable string, **each once**.
///
/// One person is routinely both composer and producer, and fts5's bm25 counts a repeated token
/// twice — the same argument the index already makes about `file_name` echoing the title. Order is
/// first appearance, so the line reads as the tag wrote it.
fn render(credits: &[RoleCredit]) -> String {
    let mut seen: Vec<&str> = Vec::with_capacity(credits.len());
    for credit in credits {
        if !seen.iter().any(|name| name.eq_ignore_ascii_case(&credit.name)) {
            seen.push(&credit.name);
        }
    }
    seen.join(", ")
}

#[cfg(test)]
#[path = "tests/credits_tests.rs"]
mod tests;
