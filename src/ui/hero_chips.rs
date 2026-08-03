//! Publishes the hero band's metadata chips into the `HeroChips` global.
//!
//! The thin half of [`crate::ui::chips`]: that module wraps a list of chips
//! into rows, this one decides which chips a given hero has and hands them
//! over. Six views share one global — see `globals/hero-chips.slint` for why
//! one is enough — so every hero opens by calling exactly one `publish_*`.
//!
//! **Nothing here costs a query.** Every fact is either on a stats struct the
//! caller already holds at its existing push site, or folded out of a `Vec` the
//! same fetch just walked. `AlbumStats::disc_count` and `is_compilation` are
//! the clearest case of the first — both fetched today and dropped on the floor
//! by `to_slint_album_row`, because nothing painted them; [`fold_tracks`] is
//! the second, and it runs on the worker beside the fetch rather than on the UI
//! thread, so a 20 000-track genre doesn't hash its whole track list inside an
//! `upgrade_in_event_loop`.
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
use crate::ui::tracks::format_duration_ms;
use crate::{AppWindow, Favorites, HeroChips, RecentlyPlayed};

/// How many rows a hero band gives its chips before dropping the rest.
///
/// Two, not one: every hero leaves slack between its meta block and its action
/// pill (the trailing `Rectangle { vertical-stretch: 1 }`), and on the two
/// mosaic heroes a second 26 px row plus its 8 px gap fits inside it — so a
/// narrow window wraps into space that was already there. The four detail
/// heroes are tighter, because their band is `Theme.hero-artwork` plus padding
/// and their column carries a subtitle the mosaic pair doesn't: there the
/// second row grows the band by the few pixels it overruns the tile by, which
/// is still the better trade against dropping a fact the user came for.
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
pub trait ChipLabels {
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
pub struct FavoritesFacts {
    pub tab: FavoritesTab,
    pub tracks: i32,
    pub duration_text: SharedString,
    pub songs: HeroFold,
    pub most_played: MostPlayedTotals,
    pub artists: i32,
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

fn len_as_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
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
}

thread_local! {
    static PUBLISHED: RefCell<PublishedChips> = const {
        RefCell::new(PublishedChips { width: 0.0, chips: Vec::new() })
    };
}

/// Wire the width channel. Called once, at boot.
pub fn install(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<HeroChips>().on_recompute(move |width| {
        let Some(ui) = weak.upgrade() else { return };
        PUBLISHED.with_borrow_mut(|p| p.width = width);
        write_rows(&ui);
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
pub fn publish(ui: &AppWindow, chips: Vec<SharedString>, section_active: bool) {
    if !section_active {
        return;
    }
    PUBLISHED.with_borrow_mut(|p| p.chips = chips);
    write_rows(ui);
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
    write_rows(ui);
}

fn write_rows(ui: &AppWindow) {
    let rows = PUBLISHED.with_borrow(|p| {
        chips::chunk_chips_to_rows(&p.chips, p.width, Some(HERO_MAX_ROWS))
    });
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

/// Favorites reads its counts back off the `Favorites` global rather than
/// taking them as arguments: they are set from three different places (the
/// stats fetch, the grid build, the tab pick) and the global is the one point
/// all three have already passed through, so the chips can't disagree with the
/// grid under them. The two folds come off the handle's own caches for the same
/// reason — no call site holds both.
pub fn publish_favorites(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let g = ui.global::<Favorites>();
    let facts = FavoritesFacts {
        tab: fav_ui.active_tab(),
        tracks: g.get_track_count(),
        // Already formatted on the global — both mosaic heroes run
        // `format_duration_ms` Rust-side — so it is passed through rather than
        // re-derived from a millisecond count the global doesn't carry.
        duration_text: g.get_duration_text(),
        songs: fold_tracks(&fav_ui.state().tracks_all.lock()),
        most_played: fold_most_played(&fav_ui.state().most_played.lock()),
        artists: g.get_artist_count(),
    };
    let chips = favorites_chips(&ui.global::<HeroChips>(), &facts);
    publish(ui, chips, fav_ui.section_active());
}

pub fn publish_recently_played(ui: &AppWindow, fold: HeroFold, section_active: bool) {
    let r = ui.global::<RecentlyPlayed>();
    let chips = recently_played_chips(
        &ui.global::<HeroChips>(),
        r.get_track_count(),
        &r.get_duration_text(),
        fold,
    );
    publish(ui, chips, section_active);
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

fn genre_chips(labels: &impl ChipLabels, genre: &GenreStats, fold: HeroFold) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(4);
    out.push(labels.tracks(genre.track_count));
    push_duration(&mut out, genre.total_duration_ms);
    push_fold(&mut out, labels, fold);
    out
}

/// No "Smart" chip — the title already carries the `auto_awesome` badge, and
/// saying it twice in one band reads as a bug.
fn playlist_chips(
    labels: &impl ChipLabels,
    playlist: &PlaylistStats,
    fold: HeroFold,
) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(4);
    out.push(labels.tracks(playlist.track_count));
    push_duration(&mut out, playlist.total_duration_ms);
    push_fold(&mut out, labels, fold);
    out
}

/// Empty on an empty Songs tab: the view paints "No favorites yet" instead, and
/// a lone "0 favorites" chip beside it would be both redundant and bleak.
fn favorites_chips(labels: &impl ChipLabels, facts: &FavoritesFacts) -> Vec<SharedString> {
    match facts.tab {
        FavoritesTab::MostPlayed => {
            let mut out = Vec::with_capacity(3);
            out.push(labels.tracks(facts.most_played.tracks));
            push_duration(&mut out, facts.most_played.duration_ms);
            // The tab ranks by this and states it nowhere else.
            if facts.most_played.plays > 0 {
                out.push(labels.plays(facts.most_played.plays));
            }
            out
        }
        FavoritesTab::Artists => vec![labels.artists(facts.artists)],
        FavoritesTab::Songs if facts.tracks > 0 => {
            let mut out = Vec::with_capacity(4);
            out.push(labels.favorites(facts.tracks));
            push_duration_text(&mut out, &facts.duration_text);
            push_fold(&mut out, labels, facts.songs);
            out
        }
        FavoritesTab::Songs => Vec::new(),
    }
}

/// Same empty-state split as Favorites, for the same reason.
fn recently_played_chips(
    labels: &impl ChipLabels,
    track_count: i32,
    duration_text: &str,
    fold: HeroFold,
) -> Vec<SharedString> {
    if track_count == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(4);
    out.push(labels.tracks(track_count));
    push_duration_text(&mut out, duration_text);
    push_fold(&mut out, labels, fold);
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

fn push_duration_text(out: &mut Vec<SharedString>, duration_text: &str) {
    if !duration_text.is_empty() {
        out.push(SharedString::from(duration_text));
    }
}

#[cfg(test)]
#[path = "tests/hero_chips_tests.rs"]
mod tests;
