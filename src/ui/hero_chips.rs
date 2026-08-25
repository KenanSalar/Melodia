//! Publishes the hero band's metadata chips into the `HeroChips` global.
//!
//! The thin half of [`crate::ui::chips`]: that module wraps a list of chips into rows,
//! this one decides which chips a hero has. Six views share one global, so every hero
//! opens by calling exactly one `publish_*`.
//!
//! **No chip costs a query, and no `publish_*` walks a `Vec`.** Every fact is either on a
//! stats struct the caller already holds or folded out of a `Vec` the same fetch walked —
//! by the fetch, *on that worker*, so a 20 000-track genre never hashes its track list
//! inside an `upgrade_in_event_loop`. Those pure folds are [`crate::ui::hero_folds`]. A
//! publisher also reads no Slint property it doesn't own: taking the facts as arguments
//! makes the caller's write order irrelevant.
//!
//! **The gate is the live tab, and the row remembers who filled it.** Both halves exist
//! for the same event — a cross-section drill moves the tab from inside the very closure
//! that publishes. Read off the `section_active` shadow, which `SectionActiveGate` only
//! updates next frame, the gate answers for the tab being *left* and drops the publish;
//! and once it lands, the departing tab's own leave arrives in the same change-handler
//! drain and would clear it, a leave being unable to tell a hand-off whose destination
//! has already published from one still fetching. [`ChipOwner`] is what can.
//!
//! **A band states facts about the set the page is about, never the current filter** —
//! forced rather than chosen, an album's chips being unable to follow its track filter
//! without lying about the album.
//!
//! Order matters: the band wraps at [`HERO_MAX_ROWS`] and drops what still doesn't fit,
//! so each builder leads with the fact a user is most likely scanning for.

use std::cell::RefCell;

use slint::{ComponentHandle, SharedString};

use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::genre::GenreStats;
use crate::entities::playlist::PlaylistStats;
use crate::ui::chips;
use crate::ui::favorites::{FavoritesTab, FavoritesUi, NAV_FAVORITES};
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::my_library::{MyLibraryTab, NAV_MY_LIBRARY, tab_from_index};
use crate::ui::radio::NAV_RADIO;
use crate::ui::recently_played::{NAV_RECENTLY_PLAYED, RecentlyPlayedTab, RecentlyPlayedUi};
use crate::ui::tracks::format_duration_ms;
use crate::ui::util::len_as_i32;
use crate::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, HeroChips, MyLibrary, Nav, PlaylistDetail,
    Radio,
};

/// How many rows a hero band gives its chips before dropping the rest.
///
/// Two, measured rather than picked: a second row plus its gap fits the slack every
/// hero's trailing spacer already leaves. A third overruns the tile on all six and pushes
/// the action pill out of the banner — unlike the Now Playing strip, which passes `None`.
///
/// What makes the second row fit on the two heroes carrying a subtitle (Album's artist,
/// Playlist's description) is that the line sits *under the title, inside the title row*,
/// where the `SearchBar` beside it has already claimed the height. Moving either onto a
/// row of its own is what breaks this, not the row count — and it makes
/// `Theme.hero-title-size` and `font-size-md` a pair, raising either spending the slack.
const HERO_MAX_ROWS: usize = 2;

/// The translated labels a builder needs. A trait rather than the global directly, so the
/// builders — the part with the decisions in them — are testable without an `AppWindow`.
/// `@tr` folds literals at codegen and nothing else, so production routes through the
/// global's callbacks either way.
trait ChipLabels {
    fn tracks(&self, count: i32) -> SharedString;
    fn albums(&self, count: i32) -> SharedString;
    fn artists(&self, count: i32) -> SharedString;
    fn favorites(&self, count: i32) -> SharedString;
    fn discs(&self, count: i32) -> SharedString;
    fn plays(&self, count: i32) -> SharedString;
    fn compilation(&self) -> SharedString;
    fn bitrate(&self, kbps: i32) -> SharedString;
    fn votes(&self, count: i32) -> SharedString;
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
    fn bitrate(&self, kbps: i32) -> SharedString {
        self.invoke_bitrate(kbps)
    }
    fn votes(&self, count: i32) -> SharedString {
        self.invoke_votes(count)
    }
}

/// Everything the Favorites band can state, across its three tabs. Which fields get
/// read follows the tab — the band describes whatever the body below it is listing.
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

/// Whose facts the band is currently stating. **A teardown clears only chips it owns**,
/// which is the whole of what this is for — see [`clear_if_stale`]. The four details
/// carry their id because a band holds its banner across a tab switch, so "is this
/// still the Album Detail's row" has to mean *that* album's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChipOwner {
    Album(i64),
    Artist(i64),
    Genre(i64),
    Playlist(i64),
    Favorites,
    RecentlyPlayed,
    /// The station detail, and the one owner carrying no id. It cannot: a browsed station
    /// has no database row, so every one of them would record `Station(0)` and a hand-off
    /// between two of them would read as the same banner. Nothing is lost — Radio has one
    /// detail and every open republishes, so the worst a collapsed owner costs is a clear
    /// that didn't need to happen.
    Station,
}

/// The published chips, who published them, and the width they were chunked against.
///
/// UI-thread state, so a `thread_local` rather than something threaded through six call
/// sites that only hold an `&AppWindow`. The width deliberately survives a hero swap: the
/// incoming strip re-reports it a frame later, and until then it beats zero.
struct PublishedChips {
    width: f32,
    owner: Option<ChipOwner>,
    chips: Vec<SharedString>,
    /// Row lengths of the split last handed to Slint — see
    /// [`chips::split_shape`] for what it buys and why the chips aren't in it.
    shape: Vec<usize>,
}

thread_local! {
    static PUBLISHED: RefCell<PublishedChips> = const {
        RefCell::new(PublishedChips {
            width: 0.0,
            owner: None,
            chips: Vec::new(),
            shape: Vec::new(),
        })
    };
}

/// Wire the width channel. Called once, at boot.
pub fn install(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<HeroChips>().on_recompute(move |width| {
        let Some(ui) = weak.upgrade() else { return };
        PUBLISHED.with_borrow_mut(|p| p.width = width);
        // The one caller that may re-chunk to the shape already on screen, and the one
        // that fires per pointer motion of a resize drag.
        write_rows(&ui, false);
    });
}

/// Replace the band's chips and re-chunk, but only when the publishing view is on screen.
/// Callers reach for a `publish_*` wrapper below; this is the seam they share.
///
/// **The gate is not optional.** One global serves six heroes and the boot path fetches
/// every persisted detail id whichever section is restored, so at cold start up to four
/// of these land while a *different* hero owns the band and the last to finish wins. A
/// view that drops its publish here re-publishes on section-enter, which always
/// re-fetches — the contract `apply_detail_artwork` holds `hero_backdrop::apply` to.
fn publish(ui: &AppWindow, owner: ChipOwner, chips: Vec<SharedString>, section_active: bool) {
    if !section_active {
        return;
    }
    PUBLISHED.with_borrow_mut(|p| {
        p.owner = Some(owner);
        p.chips = chips;
    });
    write_rows(ui, true);
}

/// Drop the band's chips unconditionally — the page-level teardown, where nothing can
/// bring a banner back without a fetch that republishes all of it.
///
/// Every *other* teardown reaches for [`clear_if_stale`]: the two mosaic heroes also
/// reset the backdrop when their mosaic empties while the view is still on screen, and
/// clearing there would blank a live band.
pub fn clear(ui: &AppWindow) {
    PUBLISHED.with_borrow_mut(|p| {
        p.owner = None;
        p.chips.clear();
    });
    write_rows(ui, true);
}

/// Drop the band's chips **unless the band is still painting the hero that published
/// them**, so backing out of one hero and into another can't leave the previous entity's
/// counts under the new title.
///
/// The record's question to answer rather than the departing view's: a predicate taking
/// the *departing tab* cannot tell a hand-off whose destination has already published
/// from one still waiting on a fetch, and a cross-tab drill fills the strip in the tick
/// that moves the tab. Two ways the band is still painting these, per [`should_clear`]:
///
/// * **Nothing took over** — the mounted view would publish this same owner. Now Playing
///   *covering* a band lands here correctly, the same hero coming back underneath.
/// * **The band is collapsing out of it.** Nothing clears a detail id on a tab leave, so
///   an owner whose id is still set is a banner the band is mid-morph over.
///   `MyLibrary.hero-collapsed` asks again at the end, by which point either the detail
///   really closed (id `-1`, so this clears) or a tab switch collapsed it and picking
///   that tab again morphs the banner back open with its counts intact.
pub fn clear_if_stale(ui: &AppWindow) {
    let recorded = PUBLISHED.with_borrow(|p| p.owner);
    let Some(recorded) = recorded else {
        return;
    };
    if should_clear(recorded, band_owner(ui), is_open(ui, recorded)) {
        clear(ui);
    }
}

/// The pure half of [`clear_if_stale`], split out so the decision is testable without
/// an `AppWindow`.
fn should_clear(recorded: ChipOwner, band: Option<ChipOwner>, still_open: bool) -> bool {
    if band == Some(recorded) {
        return false;
    }
    // No hero is mounted and the owner's own id survives: the band is collapsing over
    // the banner these belong to, and `hero-collapsed` asks again at the end.
    !(band.is_none() && still_open)
}

/// The owner the view on screen right now *would* publish, or [`None`] where no
/// band is mounted at all — a My Library grid, or any of the tabless sections.
/// UI thread only.
fn band_owner(ui: &AppWindow) -> Option<ChipOwner> {
    let nav = ui.global::<Nav>().get_selected_index();
    if nav == NAV_FAVORITES {
        return Some(ChipOwner::Favorites);
    }
    if nav == NAV_RECENTLY_PLAYED {
        return Some(ChipOwner::RecentlyPlayed);
    }
    if nav == NAV_RADIO {
        // Unlike the two curated pages, this band has an idle state: with no detail open
        // the Radio band states a count and no chips at all.
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

/// Which detail the mounted My Library tab has open, given the four live ids — the pure
/// half of [`band_owner`]. **The tab is what discriminates, not the id**:
/// `seed_detail_from_settings` runs for all four detail views at boot whichever tab is
/// restored, so more than one can be `>= 0` at a time.
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

/// Whether `owner`'s own detail id is still set. A curated page has no id, so it
/// is never something the band can be collapsing over.
fn is_open(ui: &AppWindow, owner: ChipOwner) -> bool {
    match owner {
        ChipOwner::Album(id) => i64::from(ui.global::<AlbumDetail>().get_album_id()) == id,
        ChipOwner::Artist(id) => i64::from(ui.global::<ArtistDetail>().get_artist_id()) == id,
        ChipOwner::Genre(id) => i64::from(ui.global::<GenreDetail>().get_genre_id()) == id,
        ChipOwner::Playlist(id) => i64::from(ui.global::<PlaylistDetail>().get_playlist_id()) == id,
        // **Nav too, where the four above need none.** Their page-level teardown clears
        // unconditionally, so `is_open` is only ever asked while My Library is on screen;
        // Radio's leave hands its hero back through the same macro every detail close takes,
        // and a station page left standing on an unmounted page is not a band mid-collapse.
        ChipOwner::Station => {
            ui.global::<Nav>().get_selected_index() == NAV_RADIO
                && ui.global::<Radio>().get_detail_open()
        }
        ChipOwner::Favorites | ChipOwner::RecentlyPlayed => false,
    }
}

/// Re-chunk and hand the split to Slint.
///
/// `force` is the caller answering "did my chips move?" — `true` from [`publish`] and
/// [`clear`], the only two that can. Unforced, an unchanged shape skips the write:
/// `set_rows` is a model reset, so a strip re-chunking to the same two rows would rebuild
/// every chip once per pointer motion.
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
    ui.global::<HeroChips>().set_rows(chips::rows_to_model(rows));
}

// --- Per-hero publishers ------------------------------------------------
//
// Each reads the global once and hands it to the matching builder, so a call site is
// one line and the label plumbing stays here. The extra argument each takes is the fold
// its own fetch produced, for the module doc's reason.

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

/// The station hero's facts, gathered rather than read off one entity: a station opened from
/// Browse and one opened from a kept tab come from different types, and the directory refresh
/// behind the open fills in what neither had.
///
/// `votes` is `None` until the directory has answered, which for a station it no longer lists
/// is forever. `0` is a real number of votes and would print as one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationFacts {
    /// Already split and capped by the caller, which is where the tag-display policy lives —
    /// this module decides layout, not how many genres are worth naming.
    pub tags: Vec<String>,
    pub country: String,
    pub state: String,
    pub language: String,
    pub codec: String,
    pub bitrate: i32,
    pub votes: Option<i64>,
}

pub fn publish_station(ui: &AppWindow, facts: &StationFacts, section_active: bool) {
    let chips = station_chips(&ui.global::<HeroChips>(), facts);
    publish(ui, ChipOwner::Station, chips, section_active);
}

/// Favorites is assembled from three fetches, so it takes the handle and gathers rather
/// than being handed a struct — no call site holds all of it. Every field is a finished
/// value its owning fetch already folded, so a publish beating a sibling fetch is a tick
/// stale, never half-built.
pub fn publish_favorites(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let state = fav_ui.state();
    // One at a time: four sibling locks with no argued ordering, and a struct literal
    // would hold every guard it built until the statement ended.
    let (tracks, duration_ms) = {
        let stats = state.stats.lock();
        (i32::try_from(stats.count).unwrap_or(i32::MAX), stats.total_duration_ms)
    };
    let songs = *state.songs_fold.lock();
    let most_played = *state.most_played_totals.lock();
    // The whole set, not the filtered grid: a band names the page it sits on, and the
    // filtered count is the one gating that tab's empty state.
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
    publish(ui, ChipOwner::Favorites, chips, fav_ui.section_active());
}

/// The second hero assembled from more than one fetch — the recency list and the Most
/// Played grid land independently — so it gathers like Favorites, under the same
/// contract.
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

// --- Builders -----------------------------------------------------------

/// Year first — what distinguishes two pressings of the same record, and the only chip
/// here a user might be scanning a page for.
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
    // Last: the one chip that isn't about *this* release, so the one worth losing first
    // on a narrow band.
    if let Some(genre) = genre {
        out.push(SharedString::from(genre));
    }
    out
}

/// **Tags last, for `album_chips`' reason**: they say what a station plays rather than what it
/// *is*, so on a band narrow enough to drop a row they are what should go before the country and
/// the codec do. Every field is omitted when blank — the directory leaves most of them blank on
/// some station or other, and a hand-typed one arrives with almost nothing.
fn station_chips(labels: &impl ChipLabels, facts: &StationFacts) -> Vec<SharedString> {
    let mut out = Vec::with_capacity(6 + facts.tags.len());
    for field in [&facts.country, &facts.state, &facts.language, &facts.codec] {
        if !field.is_empty() {
            out.push(SharedString::from(field.as_str()));
        }
    }
    // Zero on a large share of live stations, where it means "the directory doesn't know"
    // rather than silence.
    if facts.bitrate > 0 {
        out.push(labels.bitrate(facts.bitrate));
    }
    if let Some(votes) = facts.votes {
        out.push(labels.votes(i32::try_from(votes).unwrap_or(i32::MAX)));
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

/// The shape four of the six heroes share: what the list holds, how long it runs, how far
/// it spreads. `count` arrives already rendered, being the one chip whose *noun* varies —
/// Favorites' Songs tab counts favorites, everyone else tracks.
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

/// No "Smart" chip — the title already carries the `auto_awesome` badge, and
/// saying it twice in one band reads as a bug.
fn playlist_chips(
    labels: &impl ChipLabels,
    playlist: &PlaylistStats,
    fold: HeroFold,
) -> Vec<SharedString> {
    list_chips(labels, labels.tracks(playlist.track_count), playlist.total_duration_ms, fold)
}

/// Empty on an empty tab, whichever of the three: each paints its own "nothing here yet"
/// copy, and a lone "0 favorites" chip beside one says it far more bleakly. The guards
/// read the *unfiltered* counts, the empty states being the surfaces that follow a
/// filter.
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

/// Same empty-state split as Favorites over two tabs instead of three. The Songs count
/// is `tracks`, not `favorites` — the noun is the one chip whose wording follows the page.
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

/// What a Most Played tab states about itself, shared by the two pages that have one.
///
/// **It sums itself.** Favorites' query is a strict subset of its Songs tab and Recently
/// Played's is the whole library where its Songs tab is the last 200 played, so borrowing
/// the Songs duration would be a different set's total on either page.
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

/// The spread chips, in the order they'd be missed. A spread of one says nothing, so
/// neither is stated at one.
fn push_fold(out: &mut Vec<SharedString>, labels: &impl ChipLabels, fold: HeroFold) {
    if fold.artists > 1 {
        out.push(labels.artists(fold.artists));
    }
    if fold.albums > 1 {
        out.push(labels.albums(fold.albums));
    }
}

/// A zero-length entity has nothing to say about its running time, and "0:00" beside
/// "0 tracks" is noise rather than information.
fn push_duration(out: &mut Vec<SharedString>, total_duration_ms: i64) {
    if total_duration_ms > 0 {
        out.push(SharedString::from(format_duration_ms(total_duration_ms)));
    }
}

#[cfg(test)]
#[path = "tests/hero_chips_tests.rs"]
mod tests;
