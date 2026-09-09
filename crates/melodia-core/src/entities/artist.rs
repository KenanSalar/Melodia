use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// One name in a credit, plus the text that joins it to the next one.
///
/// The phrase *follows* its name and the last one's is empty, so a list renders by plain
/// concatenation. Free text rather than an enum: a credit is meant to read the way the release
/// prints it, and [`JOIN_PHRASES`] is a picker's worth of common answers, not the whole set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditedArtist {
    pub name: String,
    pub join_phrase: String,
}

/// A whole artist field, in the two shapes the rest of the tree needs it: the credit **as
/// printed**, which every display surface and the FTS index read, and the ordered list behind it,
/// which is what an artist-scoped query and the `track_artists` rows are built on.
///
/// The fields are private and there is no way to set one without the other, because the two are
/// one fact and drifting them apart is silent: a row that *displays* "X feat. Y" while *filing*
/// under whoever the stale list named. `line` is always [`Self::artists`] rendered, and `None`
/// exactly when the credit is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtistCredit {
    line: Option<String>,
    artists: Vec<CreditedArtist>,
}

impl ArtistCredit {
    /// What a file's two artist tags say: `printed` from `ARTIST`, `names` from `ARTISTS`.
    #[must_use]
    pub fn from_tags(printed: &str, names: &[String]) -> Self {
        Self::new(credits_from(printed, names))
    }

    /// One typed name, which is what a plain text box means.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        Self::from_tags(name, &[])
    }

    #[must_use]
    pub fn new(artists: Vec<CreditedArtist>) -> Self {
        let rendered = render(&artists);
        Self {
            line: (!rendered.is_empty()).then_some(rendered),
            artists,
        }
    }

    /// The credit as printed, for the `artist` column and everything reading it.
    #[must_use]
    pub fn line(&self) -> Option<&str> {
        self.line.as_deref()
    }

    #[must_use]
    pub fn artists(&self) -> &[CreditedArtist] {
        &self.artists
    }

    /// The name a row's `artist_id` points at: the first credited artist, `""` for none.
    ///
    /// [`Self::line`] is not it. Keying an `artists` row on "X feat. Y" is what made a featured
    /// collaboration read as a third artist nobody had ever recorded under.
    #[must_use]
    pub fn primary_name(&self) -> &str {
        self.artists.first().map_or("", |credit| credit.name.as_str())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty()
    }
}

/// Picker label paired with what it renders as. The spacing differs per phrase, so both halves
/// are spelled: a comma leads with none where every word phrase leads with a space.
///
/// Positional, like [`super::smart_criteria`]'s field arrays, and **never translated** — this is
/// tag content, so a localized `" feat. "` would land in the user's file.
pub const JOIN_PHRASES: [(&str, &str); 10] = [
    ("feat.", " feat. "),
    ("&", " & "),
    (",", ", "),
    ("and", " and "),
    ("with", " with "),
    ("vs.", " vs. "),
    ("x", " x "),
    ("+", " + "),
    ("/", " / "),
    ("presents", " presents "),
];

/// What separates names when a file lists them without saying how, which is every `ARTISTS` tag
/// whose `ARTIST` we couldn't read a phrase out of.
const DEFAULT_JOIN: &str = ", ";
const DEFAULT_FINAL_JOIN: &str = " & ";

/// The credit as one string. Private: [`ArtistCredit`] is the only thing allowed to hold the
/// result, so that nothing can render a line and file it beside a different list.
fn render(credits: &[CreditedArtist]) -> String {
    let len = credits.iter().map(|c| c.name.len() + c.join_phrase.len()).sum();
    let mut out = String::with_capacity(len);
    for credit in credits {
        out.push_str(&credit.name);
        out.push_str(&credit.join_phrase);
    }
    out
}

/// Rebuild a credit list from the two tags a file carries: `display` (`ARTIST`) and `names`
/// (`ARTISTS`).
///
/// The phrases are whatever sits between the names in `display`, which only holds when the two
/// tags agree — so the result is checked against [`render`] and falls back to the default
/// phrases when it doesn't reproduce the original. Nothing is rewritten on the strength of that
/// fallback: the dialog leaves an untouched field at `FieldEdit::Keep`.
#[must_use]
fn credits_from(display: &str, names: &[String]) -> Vec<CreditedArtist> {
    if names.is_empty() {
        return match display.trim() {
            "" => Vec::new(),
            name => vec![CreditedArtist {
                name: name.to_owned(),
                join_phrase: String::new(),
            }],
        };
    }
    derive_phrases(display, names)
        .filter(|credits| render(credits) == display)
        .unwrap_or_else(|| with_default_phrases(names))
}

/// Walk the names through `display` in order, taking the gaps as phrases. `None` as soon as one
/// name isn't where the next has to start from.
fn derive_phrases(display: &str, names: &[String]) -> Option<Vec<CreditedArtist>> {
    let mut credits = Vec::with_capacity(names.len());
    let mut rest = display;
    for (i, name) in names.iter().enumerate() {
        rest = rest.strip_prefix(name.as_str())?;
        let join_phrase = if i + 1 == names.len() {
            String::new()
        } else {
            let next = names[i + 1].as_str();
            let gap = rest.find(next)?;
            let (phrase, tail) = rest.split_at(gap);
            rest = tail;
            phrase.to_owned()
        };
        credits.push(CreditedArtist {
            name: name.clone(),
            join_phrase,
        });
    }
    Some(credits)
}

fn with_default_phrases(names: &[String]) -> Vec<CreditedArtist> {
    let last = names.len().saturating_sub(1);
    names
        .iter()
        .enumerate()
        .map(|(i, name)| CreditedArtist {
            name: name.clone(),
            join_phrase: if i == last {
                String::new()
            } else if i + 1 == last {
                DEFAULT_FINAL_JOIN.to_owned()
            } else {
                DEFAULT_JOIN.to_owned()
            },
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub musicbrainz_id: Option<String>,
    pub image_path: Option<String>,
}

/// View-backed struct with computed stats (from `artist_stats` view)
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct ArtistStats {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub musicbrainz_id: Option<String>,
    pub image_path: Option<String>,
    pub track_count: i32,
    pub album_count: i32,
    pub total_duration_ms: i64,
}

/// Lightweight struct for artists with favorited tracks.
///
/// `Hash` is what the Favorites grid compares against its last applied set —
/// derived rather than hand-listed so a new field can't silently drop out of
/// the comparison and leave a card stale.
#[derive(Clone, Debug, PartialEq, Hash, FromRow, Serialize, Deserialize)]
pub struct FavoriteArtist {
    pub id: i64,
    pub name: String,
    pub image_path: Option<String>,
    pub favorite_count: i32,
}
