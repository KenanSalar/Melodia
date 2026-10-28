//! What wins the Search view's Top Result card. Pure, so the ranking is unit-tested whole; the
//! card's text is resolved later, on the UI thread.

use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::search::SearchResults;
use melodia_core::utils::needle::fold_needle;

/// Written to `Search.top-kind` by its lower-case name. A genre takes this card and no strip,
/// being a route to a page rather than a row of things to browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    Album,
    Artist,
    Genre,
}

/// The card's second line as a choice rather than a string: two of the three are plurals only the
/// UI thread can translate, and choosing here keeps [`compute_top_result`]'s tests free of locale
/// strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopSubtitle {
    /// Album → its artist's name, with nothing to translate.
    Text(String),
    /// Artist → `{n} albums`.
    AlbumCount(i32),
    /// Genre → `{n} tracks`.
    TrackCount(i32),
}

/// The scalars `Search` holds for the card, sparing `EntityStripRow` a `kind` field.
#[derive(Debug, Clone)]
pub struct TopResult {
    pub kind: TopKind,
    pub id: i64,
    pub title: String,
    pub subtitle: TopSubtitle,
    pub artwork_path: Option<String>,
}

/// Ranks an exact name first, then a name prefix, then whatever came back first, each band trying
/// album, artist and genre in that order. Genre sits last in each band rather than in one of its
/// own, so it only wins over a weaker match: an exact "Rock" beats an album merely starting with
/// it.
///
/// Both comparisons fold case and accents through `Needle`, as the FTS tokenizer behind every
/// other surface on the page does; a plain lowercase would let `bjork` fill the page while the
/// card fell through to the first album.
///
/// Returns `None` for a blank query, or when all three result lists are empty.
pub fn compute_top_result(results: &SearchResults, query: &str) -> Option<TopResult> {
    let needle = fold_needle(query);
    if needle.is_empty() {
        return None;
    }

    let exact = |name: &str| needle.equals(name);
    let prefix = |name: &str| needle.starts_with(name);

    if let Some(a) = results.albums.iter().find(|a| exact(&a.name)) {
        return Some(album_to_top(a));
    }
    if let Some(a) = results.artists.iter().find(|a| exact(&a.name)) {
        return Some(artist_to_top(a));
    }
    if let Some(g) = results.genres.iter().find(|g| exact(&g.name)) {
        return Some(genre_to_top(g));
    }
    if let Some(a) = results.albums.iter().find(|a| prefix(&a.name)) {
        return Some(album_to_top(a));
    }
    if let Some(a) = results.artists.iter().find(|a| prefix(&a.name)) {
        return Some(artist_to_top(a));
    }
    if let Some(g) = results.genres.iter().find(|g| prefix(&g.name)) {
        return Some(genre_to_top(g));
    }
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
        artwork_path: None,
    }
}

#[cfg(test)]
#[path = "tests/top_result_tests.rs"]
mod tests;
