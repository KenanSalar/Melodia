//! The Search view's Top Result card: what wins it, and why.
//!
//! Pure — no Slint, no DB, no `AppState`. That is what lets the nine-step
//! ranking below be exhaustively unit-tested, and it is why the payload is a
//! discriminator plus scalars rather than a rendered card: two of the three
//! subtitles are translated plurals, and `@tr` only reaches literals inside
//! `.slint`, so the text is resolved on the UI thread by
//! `apply::write_top_result`.

use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::genre::GenreStats;
use crate::library::search::SearchResults;
use crate::ui::row_match::fold_needle;

/// Top Result discriminator. Matches the `top-kind` string slot in the
/// Slint `Search` global ("album" / "artist" / "genre" / ""). A genre
/// takes this card rather than a strip of its own — it is a route to a
/// page, not a row of things to browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    Album,
    Artist,
    Genre,
}

/// What the card's second line says, as a discriminator rather than a
/// formatted string: two of the three are translated plurals, and `@tr`
/// only reaches literals inside `.slint`, so the text has to be resolved
/// on the UI thread. Keeping the *choice* here leaves [`compute_top_result`]
/// pure and its tests free of locale strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopSubtitle {
    /// Album → its artist's name. A proper noun; nothing to translate.
    Text(String),
    /// Artist → `{n} albums`.
    AlbumCount(i32),
    /// Genre → `{n} tracks`.
    TrackCount(i32),
}

/// Top Result payload — the scalar fields the Slint `Search` global
/// holds (no struct on the Slint side; the discriminator + scalars
/// avoid baking a `kind` field into `EntityStripRow`).
#[derive(Debug, Clone)]
pub struct TopResult {
    pub kind: TopKind,
    pub id: i64,
    pub title: String,
    pub subtitle: TopSubtitle,
    pub artwork_path: Option<String>,
}

/// Compute the Top Result for a query against a `SearchResults`,
/// using a 9-step ranking. Pure function — exhaustively unit-tested.
///
/// Ranking (first match wins):
/// 1. Exact album name
/// 2. Exact artist name
/// 3. Exact genre name
/// 4. Album name starts-with
/// 5. Artist name starts-with
/// 6. Genre name starts-with
/// 7. First album in results
/// 8. First artist in results
/// 9. First genre in results
///
/// Genre slots in *below* album and artist within each band rather than
/// getting a band of its own: that leaves every album-vs-artist outcome
/// exactly as it was, and only lets a genre win where the card would
/// otherwise show a weaker match — an exact "Rock" beats an album merely
/// *starting* with it, which is the same exactness-first rule the two
/// original bands already encode.
///
/// **Both comparisons fold case *and accents*, through
/// [`crate::ui::row_match`].** Every other surface on this page reaches the
/// results through FTS, whose `remove_diacritics 2` tokenizer folds both — so
/// a plain `to_lowercase` here meant `bjork` filled the Songs list and both
/// strips while the card silently fell through to rule 7.
///
/// Returns `None` for a blank query, or when all three result lists are
/// empty.
pub fn compute_top_result(results: &SearchResults, query: &str) -> Option<TopResult> {
    let needle = fold_needle(query);
    if needle.is_empty() {
        return None;
    }

    let exact = |name: &str| needle.equals(name);
    let prefix = |name: &str| needle.starts_with(name);

    // 1-3. Exact name, album → artist → genre.
    if let Some(a) = results.albums.iter().find(|a| exact(&a.name)) {
        return Some(album_to_top(a));
    }
    if let Some(a) = results.artists.iter().find(|a| exact(&a.name)) {
        return Some(artist_to_top(a));
    }
    if let Some(g) = results.genres.iter().find(|g| exact(&g.name)) {
        return Some(genre_to_top(g));
    }
    // 4-6. Starts-with, same order.
    if let Some(a) = results.albums.iter().find(|a| prefix(&a.name)) {
        return Some(album_to_top(a));
    }
    if let Some(a) = results.artists.iter().find(|a| prefix(&a.name)) {
        return Some(artist_to_top(a));
    }
    if let Some(g) = results.genres.iter().find(|g| prefix(&g.name)) {
        return Some(genre_to_top(g));
    }
    // 7-9. Whatever came back first, same order.
    if let Some(a) = results.albums.first() {
        return Some(album_to_top(a));
    }
    if let Some(a) = results.artists.first() {
        return Some(artist_to_top(a));
    }
    results.genres.first().map(genre_to_top)
}

fn album_to_top(a: &AlbumStats) -> TopResult {
    TopResult {
        kind: TopKind::Album,
        id: a.id,
        title: a.name.clone(),
        subtitle: TopSubtitle::Text(a.artist_name.clone()),
        artwork_path: a.artwork_path.clone(),
    }
}

fn artist_to_top(a: &ArtistStats) -> TopResult {
    TopResult {
        kind: TopKind::Artist,
        id: a.id,
        title: a.name.clone(),
        // Album count by Tauri parity. Handed over as a count, not a
        // sentence — see [`TopSubtitle`].
        subtitle: TopSubtitle::AlbumCount(a.album_count),
        artwork_path: a.image_path.clone(),
    }
}

fn genre_to_top(g: &GenreStats) -> TopResult {
    TopResult {
        kind: TopKind::Genre,
        id: g.id,
        title: g.name.clone(),
        subtitle: TopSubtitle::TrackCount(g.track_count),
        // Genres have no artwork; the card paints its fallback glyph.
        artwork_path: None,
    }
}

#[cfg(test)]
#[path = "tests/top_result_tests.rs"]
mod tests;
