//! Publishes the hero band's metadata chips into the `HeroChips` global.
//!
//! The thin half of [`crate::ui::chips`]: that module wraps a list of chips
//! into rows, this one decides which chips a given hero has and hands them
//! over. Six views share one global — see `globals/hero-chips.slint` for why
//! one is enough — so every hero opens by calling exactly one `publish_*`.
//!
//! **No chip costs a query, and no `publish_*` walks a `Vec`.** Every fact is
//! either on a stats struct the caller already holds at its existing push site,
//! or folded out of a `Vec` the same fetch just walked — by the fetch, *on that
//! worker*, so a 20 000-track genre never hashes its track list inside an
//! `upgrade_in_event_loop`. `AlbumStats::disc_count` and `is_compilation` are
//! the clearest case of the first: both fetched today and dropped on the floor
//! by `to_slint_album_row`, because nothing painted them. [`fold_tracks`] is
//! the second — defined here, called from there.
//!
//! A publisher also reads no Slint property it doesn't own. Two of them used
//! to, and the cost was an ordering constraint the call site had to honour and
//! a comment had to state — "after the counts, never before". Taking the facts
//! as arguments (or off the section's own handle) makes the write order
//! irrelevant, which is the difference between a rule and a hazard.
//!
//! **A band states facts about the set the page is about, never about the
//! current filter.** That is forced rather than chosen: an album's chips cannot
//! follow its track filter without lying about the album, so the alternative
//! rule would hold on two heroes out of six. The counts that *do* track a
//! filter already exist — they gate the grids' empty states.
//!
//! Order matters, because the band wraps at [`HERO_MAX_ROWS`] and drops what
//! still doesn't fit. Each builder puts the fact a user is most likely to be
//! looking for first, so a narrow window loses the least.

use std::cell::RefCell;
use std::collections::HashSet;

use slint::{ComponentHandle, SharedString};

use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::genre::GenreStats;
use crate::entities::playlist::PlaylistStats;
use crate::entities::track::{MostPlayedFavorite, TrackListRow};
use crate::ui::chips;
use crate::ui::favorites::{FavoritesTab, FavoritesUi};
use crate::ui::recently_played::{RecentlyPlayedTab, RecentlyPlayedUi};
use crate::ui::tracks::format_duration_ms;
use crate::ui::util::len_as_i32;
use crate::{AppWindow, HeroChips};

/// How many rows a hero band gives its chips before dropping the rest.
///
/// Two, not one: every hero leaves slack between its meta block and its action
/// pill (the trailing `Rectangle { vertical-stretch: 1 }`), and a second 26 px
/// row plus its 4 px gap fits inside it on all six — so a narrow window wraps
/// into space that was already there rather than growing the band.
///
/// The two heroes carrying a second text line (Album's artist, Playlist's
/// description) are the ones that had to be argued for, since their band is
/// `Theme.hero-artwork` plus padding and nothing else. It fits because that
/// line sits *under the title, inside the title row*, where the `SearchBar`
/// beside it has already claimed the height — a subtitle on a row of its own
/// spent about a chip row's worth of the tile, and the wrap then overran it.
/// Moving either one back out is what breaks this, not the row count.
///
/// That makes `Theme.hero-title-size` and `font-size-md` a *pair*: the title
/// row is as tall as those two stacked, and the `SearchBar` only covers the
/// first. Raising either spends the same slack the second chip row needs.
///
/// Not unbounded like the Now Playing strip (which passes `None`), because a
/// third row overruns the tile on all six and pushes the pill out of the
/// banner.
const HERO_MAX_ROWS: usize = 2;

/// The translated labels a builder needs.
///
/// A trait rather than the Slint global directly, so the builders — which are
/// the part with the actual decisions in them — can be tested without standing
/// up an `AppWindow`. `@tr` folds literals at codegen and nothing else, so
/// production has to route through the global's callbacks either way.
trait ChipLabels {
    fn tracks(&self, count: i32) -> SharedString;
    fn albums(&self, count: i32) -> SharedString;
    fn artists(&self, count: i32) -> SharedString;
    fn favorites(&self, count: i32) -> SharedString;
    fn discs(&self, count: i32) -> SharedString;
    fn plays(&self, count: i32) -> SharedString;
    fn compilation(&self) -> SharedString;
}

impl ChipLabels for HeroChips<'_> {
    fn tracks(&self, count: i32) -> SharedString {
        self.invoke_tracks(count)
    }
    fn albums(&self, count: i32) -> SharedString {
        self.invoke_albums(count)
    }
    fn artists(&self, count: i32) -> SharedString {
        self.invoke_artists(count)
    }
    fn favorites(&self, count: i32) -> SharedString {
        self.invoke_favorites(count)
    }
    fn discs(&self, count: i32) -> SharedString {
        self.invoke_discs(count)
    }
    fn plays(&self, count: i32) -> SharedString {
        self.invoke_plays(count)
    }
    fn compilation(&self) -> SharedString {
        self.invoke_compilation()
    }
}

/// How many distinct artists and albums a track list spans.
///
/// The two facts a list-shaped hero can state that its stats row can't: a genre
/// or a playlist knows how many *tracks* it holds and nothing about their
/// spread. `Copy` and two words wide, so it crosses an `upgrade_in_event_loop`
/// for free.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct HeroFold {
    pub artists: i32,
    pub albums: i32,
}

/// What the Most Played tab sums to.
///
/// Its own totals, never the Songs tab's: the query behind it is
/// `is_favorite = TRUE AND play_count > 0`, a strict subset, so borrowing the
/// Songs duration would overstate it by every favourite never played.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct MostPlayedTotals {
    pub tracks: i32,
    pub duration_ms: i64,
    pub plays: i32,
}

/// Everything the Favorites band can state, across its three tabs. Which fields
/// are read follows the tab — the band describes whatever the body below it is
/// listing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FavoritesFacts {
    pub tab: FavoritesTab,
    pub tracks: i32,
    pub duration_ms: i64,
    pub songs: HeroFold,
    pub most_played: MostPlayedTotals,
    pub artists: i32,
}

/// The same, for Recently Played's two tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RecentlyPlayedFacts {
    pub tab: RecentlyPlayedTab,
    pub tracks: i32,
    pub duration_ms: i64,
    pub songs: HeroFold,
    pub most_played: MostPlayedTotals,
}

/// Count the distinct artists and albums a track list spans.
///
/// Keyed on the ids rather than the names: an `i64` hashes far cheaper than a
/// `String` over a list this size, and a track with no album genuinely belongs
/// to none, so `None` is skipped rather than pooled into an "unknown" bucket
/// that would read as one more album.
///
/// Belongs on the worker that fetched the rows, never inside an
/// `upgrade_in_event_loop`.
pub fn fold_tracks(rows: &[TrackListRow]) -> HeroFold {
    let mut artists: HashSet<i64> = HashSet::new();
    let mut albums: HashSet<i64> = HashSet::new();
    for row in rows {
        if let Some(id) = row.artist_id {
            artists.insert(id);
        }
        if let Some(id) = row.album_id {
            albums.insert(id);
        }
    }
    HeroFold {
        artists: len_as_i32(artists.len()),
        albums: len_as_i32(albums.len()),
    }
}

/// Sum the Most Played tab's own totals off its cached rows.
pub fn fold_most_played(rows: &[MostPlayedFavorite]) -> MostPlayedTotals {
    MostPlayedTotals {
        tracks: len_as_i32(rows.len()),
        duration_ms: rows.iter().map(|r| r.duration_ms).sum(),
        plays: rows.iter().map(|r| r.play_count).sum(),
    }
}

/// The genre most of a track list is tagged with, or `None` when it is split
/// evenly enough that naming one would misrepresent the rest.
///
/// An album is usually single-genre, so this reads as "the album's genre"
/// there; a compilation that genuinely spans several gets no chip rather than
/// whichever one happened to win by a track.
pub fn dominant_genre(rows: &[TrackListRow]) -> Option<String> {
    /// Share of the tracks the winner has to hold to be worth stating.
    const MAJORITY: usize = 2;

    let mut tally: Vec<(&str, usize)> = Vec::new();
    let mut tagged = 0usize;
    for genre in rows.iter().filter_map(|r| r.genre.as_deref()) {
        if genre.is_empty() {
            continue;
        }
        tagged += 1;
        match tally.iter_mut().find(|(name, _)| *name == genre) {
            Some((_, count)) => *count += 1,
            None => tally.push((genre, 1)),
        }
    }
    let (name, count) = tally.into_iter().max_by_key(|&(_, count)| count)?;
    (count * MAJORITY > tagged).then(|| name.to_owned())
}

/// The span of release years across an artist's albums, or `None` when no album
/// carries one. A single year answers `(y, y)` and is rendered without a dash.
pub fn year_span(albums: &[AlbumStats]) -> Option<(i32, i32)> {
    let mut years = albums.iter().filter_map(|a| a.year).filter(|y| *y > 0);
    let first = years.next()?;
    Some(years.fold((first, first), |(lo, hi), y| (lo.min(y), hi.max(y))))
}

/// The published chips plus the width they were chunked against.
///
/// UI-thread state, so a `thread_local` rather than something threaded through
/// six call sites that only ever hold an `&AppWindow`. Keeping the width across
/// a hero swap is deliberate: the incoming strip re-reports it a frame later
/// anyway, and until it does the cached value is a far better guess than zero.
struct PublishedChips {
    width: f32,
    chips: Vec<SharedString>,
    /// Row lengths of the split last handed to Slint — see
    /// [`chips::split_shape`] for what it buys and why the chips aren't in it.
    shape: Vec<usize>,
}

thread_local! {
    static PUBLISHED: RefCell<PublishedChips> = const {
        RefCell::new(PublishedChips { width: 0.0, chips: Vec::new(), shape: Vec::new() })
    };
}

/// Wire the width channel. Called once, at boot.
pub fn install(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<HeroChips>().on_recompute(move |width| {
        let Some(ui) = weak.upgrade() else { return };
        PUBLISHED.with_borrow_mut(|p| p.width = width);
        // The one caller that may re-chunk to the shape already on screen, and
        // the one that fires per pointer motion of a resize drag.
        write_rows(&ui, false);
    });
}

/// Replace the band's chips and re-chunk — but only when the publishing view
/// is the one on screen. Callers reach for a `publish_*` wrapper below; this is
/// the seam they share, and the single place the gate lives.
///
/// **The gate is not optional.** `HeroChips` is one global for six heroes, and
/// the boot path fetches every persisted detail id regardless of which section
/// is being restored — so at cold start up to four of these land while a
/// *different* hero owns the band, and the last one to finish wins. A view that
/// drops its publish here re-publishes on section-enter, which always re-fetches
/// (the leave marks it dirty). Exactly the contract
/// `hero_backdrop::apply` is held to by `apply_detail_artwork`.
fn publish(ui: &AppWindow, chips: Vec<SharedString>, section_active: bool) {
    if !section_active {
        return;
    }
    PUBLISHED.with_borrow_mut(|p| p.chips = chips);
    write_rows(ui, true);
}

/// Drop the band's chips on hero teardown, so backing out of one hero and into
/// another can't leave the previous entity's counts under the new title.
///
/// Belongs beside a *teardown* `hero_backdrop::reset`, not beside every one:
/// the two mosaic heroes also reset the backdrop when their mosaic empties out
/// while the view is still on screen, and clearing there would blank a live
/// band.
pub fn clear(ui: &AppWindow) {
    PUBLISHED.with_borrow_mut(|p| p.chips.clear());
    write_rows(ui, true);
}

/// Re-chunk and hand the split to Slint.
///
/// `force` is the caller answering "did my chips move?" — `true` from [`publish`]
/// and [`clear`], which is the only way they can, and `false` from the width
/// channel, where they cannot. Unforced, an unchanged shape skips the write:
/// `set_rows` is a model reset, so repainting a strip that re-chunked to the
/// same two rows rebuilds every chip for nothing, once per pointer motion.
fn write_rows(ui: &AppWindow, force: bool) {
    let Some(rows) = PUBLISHED.with_borrow_mut(|p| {
        let rows = chips::chunk_chips_to_rows(&p.chips, p.width, Some(HERO_MAX_ROWS));
        let shape = chips::split_shape(&rows);
        if !force && shape == p.shape {
            return None;
        }
        p.shape = shape;
        Some(rows)
    }) else {
        return;
    };
    ui.global::<HeroChips>()
        .set_rows(chips::rows_to_model(rows));
}

// --- Per-hero publishers ------------------------------------------------
//
// Each reads the global once and hands it to the matching builder, so a call
// site is one line and the label plumbing stays in this file. The extra
// argument each takes is the fold its own fetch produced — see the module doc
// for why that is the caller's job and not this module's.

pub fn publish_album(
    ui: &AppWindow,
    album: &AlbumStats,
    genre: Option<&str>,
    section_active: bool,
) {
    let chips = album_chips(&ui.global::<HeroChips>(), album, genre);
    publish(ui, chips, section_active);
}

pub fn publish_artist(
    ui: &AppWindow,
    artist: &ArtistStats,
    years: Option<(i32, i32)>,
    section_active: bool,
) {
    let chips = artist_chips(&ui.global::<HeroChips>(), artist, years);
    publish(ui, chips, section_active);
}

pub fn publish_genre(ui: &AppWindow, genre: &GenreStats, fold: HeroFold, section_active: bool) {
    let chips = genre_chips(&ui.global::<HeroChips>(), genre, fold);
    publish(ui, chips, section_active);
}

pub fn publish_playlist(
    ui: &AppWindow,
    playlist: &PlaylistStats,
    fold: HeroFold,
    section_active: bool,
) {
    let chips = playlist_chips(&ui.global::<HeroChips>(), playlist, fold);
    publish(ui, chips, section_active);
}

/// Favorites is the one hero assembled from three fetches rather than one, so
/// it takes the handle and gathers rather than being handed a struct — no call
/// site holds all of it. Every field is a finished value the fetch that owns it
/// already folded on its own worker (`FavoritesUiState::songs_fold`,
/// `::most_played_totals`), which is what keeps this cheap enough to call from
/// wherever an input lands: a publish that beats a sibling fetch is a tick
/// stale, never half-built.
///
/// Nothing here is read back off a Slint property, and nothing walks a `Vec`.
/// Both mattered: the first made the caller's write order load-bearing, and the
/// second put a fold over every favourite on the UI thread.
pub fn publish_favorites(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let state = fav_ui.state();
    // Taken and released one at a time. These are four sibling locks with no
    // ordering anyone has argued, and a struct literal would hold every guard
    // it built until the statement ended — nothing here needs two at once.
    let (tracks, duration_ms) = {
        let stats = state.stats.lock();
        (
            i32::try_from(stats.count).unwrap_or(i32::MAX),
            stats.total_duration_ms,
        )
    };
    let songs = *state.songs_fold.lock();
    let most_played = *state.most_played_totals.lock();
    // The whole set, not the filtered grid: a band names the page it sits on,
    // and the filtered count is the one gating that tab's empty state.
    let artists = len_as_i32(state.fav_artists.lock().len());

    let facts = FavoritesFacts {
        tab: fav_ui.active_tab(),
        tracks,
        duration_ms,
        songs,
        most_played,
        artists,
    };
    let chips = favorites_chips(&ui.global::<HeroChips>(), &facts);
    publish(ui, chips, fav_ui.section_active());
}

/// Recently Played is the second hero assembled from more than one fetch — the
/// recency list and the Most Played grid land independently — so like Favorites
/// it takes the handle and gathers rather than being handed a struct. Every
/// field is a finished value the fetch that owns it already folded on its own
/// worker (`RecentlyPlayedUiState::songs_totals`, `::songs_fold`,
/// `::most_played_totals`), which is what keeps this cheap enough to call from
/// wherever an input lands.
///
/// Nothing here is read back off a Slint property, and nothing walks a `Vec`.
pub fn publish_recently_played(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let state = rp_ui.state();
    // Taken and released one at a time, the `publish_favorites` reason: these
    // are sibling locks with no ordering anyone has argued, and a struct literal
    // would hold every guard it built until the statement ended.
    let songs_totals = *state.songs_totals.lock();
    let songs = *state.songs_fold.lock();
    let most_played = *state.most_played_totals.lock();

    let facts = RecentlyPlayedFacts {
        tab: rp_ui.active_tab(),
        tracks: songs_totals.tracks,
        duration_ms: songs_totals.duration_ms,
        songs,
        most_played,
    };
    let chips = recently_played_chips(&ui.global::<HeroChips>(), &facts);
    publish(ui, chips, rp_ui.section_active());
}

// --- Builders -----------------------------------------------------------

/// Year first — it is what distinguishes two pressings of the same record, and
/// the only chip here a user might be scanning a page for.
fn album_chips(
    labels: &impl ChipLabels,
    album: &AlbumStats,
    genre: Option<&str>,
) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(6);
    if let Some(year) = album.year.filter(|y| *y > 0) {
        out.push(SharedString::from(year.to_string()));
    }
    out.push(labels.tracks(album.track_count));
    push_duration(&mut out, album.total_duration_ms);
    // A single disc is every album's default and says nothing.
    if let Some(discs) = album.disc_count.filter(|d| *d > 1) {
        out.push(labels.discs(discs));
    }
    if album.is_compilation {
        out.push(labels.compilation());
    }
    // Last: it is the one chip here that isn't about *this* release, so it is
    // the one worth losing first on a narrow band.
    if let Some(genre) = genre {
        out.push(SharedString::from(genre));
    }
    out
}

fn artist_chips(
    labels: &impl ChipLabels,
    artist: &ArtistStats,
    years: Option<(i32, i32)>,
) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(4);
    out.push(labels.tracks(artist.track_count));
    if artist.album_count > 0 {
        out.push(labels.albums(artist.album_count));
    }
    push_duration(&mut out, artist.total_duration_ms);
    if let Some(span) = years {
        out.push(SharedString::from(format_year_span(span)));
    }
    out
}

/// The shape four of the six heroes share: what the list holds, how long it
/// runs, and how far it spreads. Album and Artist are the two that don't —
/// they lead with a year and gate on facts the others have no equivalent of.
///
/// `count` arrives already rendered because it is the one chip whose *noun*
/// varies: Favorites' Songs tab counts favorites, everyone else counts tracks.
fn list_chips(
    labels: &impl ChipLabels,
    count: SharedString,
    total_duration_ms: i64,
    fold: HeroFold,
) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(4);
    out.push(count);
    push_duration(&mut out, total_duration_ms);
    push_fold(&mut out, labels, fold);
    out
}

fn genre_chips(labels: &impl ChipLabels, genre: &GenreStats, fold: HeroFold) -> Vec<SharedString> {
    list_chips(
        labels,
        labels.tracks(genre.track_count),
        genre.total_duration_ms,
        fold,
    )
}

/// No "Smart" chip — the title already carries the `auto_awesome` badge, and
/// saying it twice in one band reads as a bug.
fn playlist_chips(
    labels: &impl ChipLabels,
    playlist: &PlaylistStats,
    fold: HeroFold,
) -> Vec<SharedString> {
    list_chips(
        labels,
        labels.tracks(playlist.track_count),
        playlist.total_duration_ms,
        fold,
    )
}

/// Empty on an empty tab, whichever of the three it is: each paints its own
/// "nothing here yet" copy — a sentence on Songs, a `GridEmptyState` on the
/// other two — and a lone "0 favorites" chip beside one of those states the same
/// thing far more bleakly.
///
/// The guards read the *unfiltered* counts, so a filter that matches nothing
/// still leaves the band stating what the page holds. That is the same split as
/// everywhere else here: the empty states are the surfaces that follow a filter.
fn favorites_chips(labels: &impl ChipLabels, facts: &FavoritesFacts) -> Vec<SharedString> {
    match facts.tab {
        FavoritesTab::MostPlayed => most_played_chips(labels, facts.most_played),
        FavoritesTab::Artists if facts.artists > 0 => vec![labels.artists(facts.artists)],
        FavoritesTab::Songs if facts.tracks > 0 => list_chips(
            labels,
            labels.favorites(facts.tracks),
            facts.duration_ms,
            facts.songs,
        ),
        _ => Vec::new(),
    }
}

/// Same empty-state split as Favorites, for the same reason, over two tabs
/// instead of three. The Songs count is `tracks`, not `favorites` — the noun is
/// the one chip whose wording follows the page.
fn recently_played_chips(
    labels: &impl ChipLabels,
    facts: &RecentlyPlayedFacts,
) -> Vec<SharedString> {
    match facts.tab {
        RecentlyPlayedTab::MostPlayed => most_played_chips(labels, facts.most_played),
        RecentlyPlayedTab::Songs if facts.tracks > 0 => list_chips(
            labels,
            labels.tracks(facts.tracks),
            facts.duration_ms,
            facts.songs,
        ),
        RecentlyPlayedTab::Songs => Vec::new(),
    }
}

/// What a Most Played tab states about itself. Shared by the two pages that have
/// one — the tab is the same list under a different predicate, so it says the
/// same three things.
///
/// **It sums itself.** Favorites' query is `is_favorite = TRUE AND play_count > 0`,
/// a strict subset of its Songs tab, and Recently Played's is the whole library
/// where its Songs tab is the last 200 played — so on both pages borrowing the
/// Songs duration would be a different set's total.
fn most_played_chips(labels: &impl ChipLabels, totals: MostPlayedTotals) -> Vec<SharedString> {
    if totals.tracks == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(3);
    out.push(labels.tracks(totals.tracks));
    push_duration(&mut out, totals.duration_ms);
    // The tab ranks by this and states it nowhere else.
    if totals.plays > 0 {
        out.push(labels.plays(totals.plays));
    }
    out
}

/// `1994` for a single year, `1994–2003` for a span — an en dash, the range
/// punctuation, not a hyphen.
fn format_year_span((first, last): (i32, i32)) -> String {
    if first == last {
        first.to_string()
    } else {
        format!("{first}\u{2013}{last}")
    }
}

/// The spread chips, in the order they'd be missed: a list of one artist or one
/// album says nothing, so neither is stated at one.
fn push_fold(out: &mut Vec<SharedString>, labels: &impl ChipLabels, fold: HeroFold) {
    if fold.artists > 1 {
        out.push(labels.artists(fold.artists));
    }
    if fold.albums > 1 {
        out.push(labels.albums(fold.albums));
    }
}

/// A zero-length entity has nothing to say about its running time, and "0:00"
/// beside "0 tracks" is noise rather than information.
fn push_duration(out: &mut Vec<SharedString>, total_duration_ms: i64) {
    if total_duration_ms > 0 {
        out.push(SharedString::from(format_duration_ms(total_duration_ms)));
    }
}

#[cfg(test)]
#[path = "tests/hero_chips_tests.rs"]
mod tests;
