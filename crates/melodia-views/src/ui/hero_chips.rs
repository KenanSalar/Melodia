//! Publishes the hero band's metadata chips into the `HeroChips` global, one `publish_*` per hero.
//!
//! No chip costs a query and no publisher walks a `Vec`: every fact is on a stats struct the
//! caller holds or was folded on the fetch's own worker ([`crate::ui::hero_folds`]), and arrives as
//! an argument rather than read back off a Slint property.
//!
//! The gate is the live tab, and the row remembers who filled it ([`ChipOwner`]). A cross-section
//! drill moves the tab inside the closure that publishes, so a `section_active` shadow answers for
//! the tab being left, and the departing leave can't tell a hand-off that already published.
//!
//! The band clips at two rows, so each builder leads with what a user most likely scans for.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui::favorites::{FavoritesTab, FavoritesUi, NAV_FAVORITES};
use crate::ui::hero_folds::HeroFold;
use crate::ui::my_library::{MyLibraryTab, NAV_MY_LIBRARY, tab_from_index};
use crate::ui::radio::NAV_RADIO;
use crate::ui::recently_played::{NAV_RECENTLY_PLAYED, RecentlyPlayedTab, RecentlyPlayedUi};
use crate::ui::tracks::format_duration_ms;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::track_list::MostPlayedTotals;
use melodia_ui::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, HeroChips, MyLibrary, Nav, PlaylistDetail,
    Radio,
};

/// The translated labels, as a trait so the builders test without an `AppWindow`.
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

/// Everything the Favorites band can state; the tab picks which fields are read.
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

/// Whose facts the band is stating, so a teardown clears only chips it owns ([`clear_if_stale`]).
/// The details carry their id, a band holding its banner across a tab switch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChipOwner {
    Album(i64),
    Artist(i64),
    Genre(i64),
    Playlist(i64),
    Favorites,
    RecentlyPlayed,
    /// No id: a browsed station has no row, so every one would be `Station(0)`. Each open
    /// republishes, so the worst this costs is a needless clear.
    Station,
}

thread_local! {
    /// Who published the chips now on the band.
    static PUBLISHED_BY: Cell<Option<ChipOwner>> = const { Cell::new(None) };
}

/// Replaces the band's chips if the publishing view is on screen; the seam every `publish_*`
/// shares. One global serves six heroes and the boot fetches every persisted detail, so ungated
/// the last to finish would win. A dropped publish comes back with section-enter's re-fetch.
fn publish(ui: &AppWindow, owner: ChipOwner, chips: Vec<SharedString>, section_active: bool) {
    if !section_active {
        return;
    }
    PUBLISHED_BY.set(Some(owner));
    ui.global::<HeroChips>().set_chips(ModelRc::from(Rc::new(VecModel::from(chips))));
}

/// Drops the band's chips unconditionally, the page-level teardown. Every other teardown takes
/// [`clear_if_stale`], which can't blank a live band.
pub fn clear(ui: &AppWindow) {
    PUBLISHED_BY.set(None);
    ui.global::<HeroChips>().set_chips(ModelRc::default());
}

/// Drops the band's chips unless the band is still painting the hero that published them, so the
/// previous entity's counts never sit under a new title.
///
/// Still painting, per [`should_clear`], means nothing took over (Now Playing covering the band
/// lands here), or the band is collapsing out of a detail whose id is still set, which
/// `MyLibrary.hero-collapsed` asks about again once the morph ends.
pub fn clear_if_stale(ui: &AppWindow) {
    let Some(recorded) = PUBLISHED_BY.get() else {
        return;
    };
    if should_clear(recorded, band_owner(ui), is_open(ui, recorded)) {
        clear(ui);
    }
}

/// The pure half of [`clear_if_stale`].
fn should_clear(recorded: ChipOwner, band: Option<ChipOwner>, still_open: bool) -> bool {
    if band == Some(recorded) {
        return false;
    }
    // No hero mounted and the owner's id survives: the band is collapsing over its banner.
    !(band.is_none() && still_open)
}

/// The owner the view on screen would publish, or `None` where no band is mounted.
fn band_owner(ui: &AppWindow) -> Option<ChipOwner> {
    let nav = ui.global::<Nav>().get_selected_index();
    if nav == NAV_FAVORITES {
        return Some(ChipOwner::Favorites);
    }
    if nav == NAV_RECENTLY_PLAYED {
        return Some(ChipOwner::RecentlyPlayed);
    }
    if nav == NAV_RADIO {
        // Radio's band idles with no chips while no detail is open.
        return ui.global::<Radio>().get_detail_open().then_some(ChipOwner::Station);
    }
    if nav != NAV_MY_LIBRARY {
        return None;
    }
    let g = ui.global::<MyLibrary>();
    my_library_owner(
        tab_from_index(&g, g.get_tab_idx()),
        ui.global::<AlbumDetail>().get_album_id(),
        ui.global::<ArtistDetail>().get_artist_id(),
        ui.global::<GenreDetail>().get_genre_id(),
        ui.global::<PlaylistDetail>().get_playlist_id(),
    )
}

/// The pure half of [`band_owner`] for My Library. The tab discriminates, not the id: a tab left
/// with its detail open keeps it.
fn my_library_owner(
    tab: MyLibraryTab,
    album_id: i32,
    artist_id: i32,
    genre_id: i32,
    playlist_id: i32,
) -> Option<ChipOwner> {
    let (id, wrap): (i32, fn(i64) -> ChipOwner) = match tab {
        MyLibraryTab::Songs => return None,
        MyLibraryTab::Albums => (album_id, ChipOwner::Album),
        MyLibraryTab::Artists => (artist_id, ChipOwner::Artist),
        MyLibraryTab::Genres => (genre_id, ChipOwner::Genre),
        MyLibraryTab::Playlists => (playlist_id, ChipOwner::Playlist),
    };
    (id >= 0).then(|| wrap(i64::from(id)))
}

/// Whether `owner`'s detail id is still set; a curated page has none.
fn is_open(ui: &AppWindow, owner: ChipOwner) -> bool {
    match owner {
        ChipOwner::Album(id) => i64::from(ui.global::<AlbumDetail>().get_album_id()) == id,
        ChipOwner::Artist(id) => i64::from(ui.global::<ArtistDetail>().get_artist_id()) == id,
        ChipOwner::Genre(id) => i64::from(ui.global::<GenreDetail>().get_genre_id()) == id,
        ChipOwner::Playlist(id) => i64::from(ui.global::<PlaylistDetail>().get_playlist_id()) == id,
        // The nav as well, unlike the four, which are only asked with My Library on screen:
        // Radio's leave hands its hero back by a close's path while the page is still seated.
        ChipOwner::Station => {
            ui.global::<Nav>().get_selected_index() == NAV_RADIO
                && ui.global::<Radio>().get_detail_open()
        }
        ChipOwner::Favorites | ChipOwner::RecentlyPlayed => false,
    }
}

pub fn publish_album(
    ui: &AppWindow,
    album: &AlbumStats,
    genre: Option<&str>,
    section_active: bool,
) {
    let chips = album_chips(&ui.global::<HeroChips>(), album, genre);
    publish(ui, ChipOwner::Album(album.id), chips, section_active);
}

pub fn publish_artist(
    ui: &AppWindow,
    artist: &ArtistStats,
    years: Option<(i32, i32)>,
    section_active: bool,
) {
    let chips = artist_chips(&ui.global::<HeroChips>(), artist, years);
    publish(ui, ChipOwner::Artist(artist.id), chips, section_active);
}

pub fn publish_genre(ui: &AppWindow, genre: &GenreStats, fold: HeroFold, section_active: bool) {
    let chips = genre_chips(&ui.global::<HeroChips>(), genre, fold);
    publish(ui, ChipOwner::Genre(genre.id), chips, section_active);
}

pub fn publish_playlist(
    ui: &AppWindow,
    playlist: &PlaylistStats,
    fold: HeroFold,
    section_active: bool,
) {
    let chips = playlist_chips(&ui.global::<HeroChips>(), playlist, fold);
    publish(ui, ChipOwner::Playlist(playlist.id), chips, section_active);
}

/// The station hero's facts, gathered because a browsed and a kept station are different types.
///
/// What the station is, never how it streams: codec, bitrate and URL are rows on the page below.
/// No country either, the band's own subtitle already naming it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationFacts {
    /// Split and capped by the caller, which owns how many genres are worth naming.
    pub tags: Vec<String>,
    pub state: String,
    pub language: String,
}

pub fn publish_station(ui: &AppWindow, facts: &StationFacts, section_active: bool) {
    publish(ui, ChipOwner::Station, station_chips(facts), section_active);
}

/// Gathers from three fetches off the handle, no call site holding them all. Each field is folded
/// already, so a publish beating a sibling fetch is a tick stale, never half-built.
pub fn publish_favorites(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let state = fav_ui.state();
    // One at a time: three sibling locks with no argued ordering, and a struct literal
    // would hold every guard it built until the statement ended.
    let (tracks, duration_ms, songs) = {
        let stats = state.stats.lock();
        let spread = HeroFold {
            artists: i32::try_from(stats.artists).unwrap_or(i32::MAX),
            albums: i32::try_from(stats.albums).unwrap_or(i32::MAX),
        };
        (i32::try_from(stats.count).unwrap_or(i32::MAX), stats.total_duration_ms, spread)
    };
    let most_played = *state.most_played_totals.lock();
    // The whole set, not the filtered grid, whose count gates the tab's empty state.
    let artists = *state.artist_total.lock();

    let facts = FavoritesFacts {
        tab: fav_ui.active_tab(),
        tracks,
        duration_ms,
        songs,
        most_played,
        artists,
    };
    let chips = favorites_chips(&ui.global::<HeroChips>(), &facts);
    publish(ui, ChipOwner::Favorites, chips, fav_ui.section_active());
}

/// Gathers like [`publish_favorites`], the recency list and the Most Played grid landing apart.
pub fn publish_recently_played(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let state = rp_ui.state();
    // One at a time, the `publish_favorites` reason.
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
    publish(ui, ChipOwner::RecentlyPlayed, chips, rp_ui.section_active());
}

/// Year first, which tells two pressings of one record apart.
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
    // Each omitted when blank: a chip reading "Digital Media" on every row states nothing.
    for field in [&album.release_type, &album.media, &album.label] {
        if let Some(value) = field.as_deref().filter(|v| !v.is_empty()) {
            out.push(SharedString::from(value));
        }
    }
    // Last, the one chip not about this release, so the first a narrow band drops.
    if let Some(genre) = genre {
        out.push(SharedString::from(genre));
    }
    out
}

/// Tags last, for `album_chips`' reason. No labels: every chip is a name the directory supplied.
fn station_chips(facts: &StationFacts) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(2 + facts.tags.len());
    for field in [&facts.state, &facts.language] {
        if !field.is_empty() {
            out.push(SharedString::from(field.as_str()));
        }
    }
    out.extend(facts.tags.iter().map(|tag| SharedString::from(tag.as_str())));
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

/// The shape four heroes share. `count` arrives rendered, its noun being the one that varies.
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
    list_chips(labels, labels.tracks(genre.track_count), genre.total_duration_ms, fold)
}

/// No "Smart" chip: the title already carries the badge.
fn playlist_chips(
    labels: &impl ChipLabels,
    playlist: &PlaylistStats,
    fold: HeroFold,
) -> Vec<SharedString> {
    list_chips(labels, labels.tracks(playlist.track_count), playlist.total_duration_ms, fold)
}

/// Empty on an empty tab, which paints its own copy. The guards read the unfiltered counts.
fn favorites_chips(labels: &impl ChipLabels, facts: &FavoritesFacts) -> Vec<SharedString> {
    match facts.tab {
        FavoritesTab::MostPlayed => most_played_chips(labels, facts.most_played),
        FavoritesTab::Artists if facts.artists > 0 => vec![labels.artists(facts.artists)],
        FavoritesTab::Songs if facts.tracks > 0 => {
            list_chips(labels, labels.favorites(facts.tracks), facts.duration_ms, facts.songs)
        }
        _ => Vec::new(),
    }
}

/// Favorites' split over two tabs; Songs counts `tracks` here, not `favorites`.
fn recently_played_chips(
    labels: &impl ChipLabels,
    facts: &RecentlyPlayedFacts,
) -> Vec<SharedString> {
    match facts.tab {
        RecentlyPlayedTab::MostPlayed => most_played_chips(labels, facts.most_played),
        RecentlyPlayedTab::Songs if facts.tracks > 0 => {
            list_chips(labels, labels.tracks(facts.tracks), facts.duration_ms, facts.songs)
        }
        RecentlyPlayedTab::Songs => Vec::new(),
    }
}

/// What a Most Played tab states. It sums itself, its set differing from the Songs tab's on both
/// pages.
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

/// `1994`, or `1994–2003` with an en dash.
fn format_year_span((first, last): (i32, i32)) -> String {
    if first == last { first.to_string() } else { format!("{first}\u{2013}{last}") }
}

/// The spread chips, neither stated at one.
fn push_fold(out: &mut Vec<SharedString>, labels: &impl ChipLabels, fold: HeroFold) {
    if fold.artists > 1 {
        out.push(labels.artists(fold.artists));
    }
    if fold.albums > 1 {
        out.push(labels.albums(fold.albums));
    }
}

/// Nothing for a zero-length entity, where "0:00" is noise.
fn push_duration(out: &mut Vec<SharedString>, total_duration_ms: i64) {
    if total_duration_ms > 0 {
        out.push(SharedString::from(format_duration_ms(total_duration_ms)));
    }
}

#[cfg(test)]
#[path = "tests/hero_chips_tests.rs"]
mod tests;
