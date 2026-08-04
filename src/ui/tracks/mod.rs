//! Tracks-view glue between Rust and Slint.
//!
//! The Slint `Tracks` global owns a `Rc<VecModel<TrackListRow>>` set
//! once at startup via `install_tracks_model`. The canonical (unfiltered) list
//! lives in `TracksUi::full` so client-side filter changes don't hit the DB.
//!
//! Cross-thread layout:
//! * `TracksUi` is `Send + Sync` — `Arc<TracksUi>` cloned into callback
//!   closures and tokio tasks.
//! * Slint properties / model can only be touched from the UI thread; we
//!   reach back via `Weak<AppWindow>::upgrade_in_event_loop`.
//!
//! Allocation strategy: `full` and `search_keys` are stored behind
//! `Mutex<Arc<Vec<…>>>`. Refilter takes a cheap `Arc::clone` instead of
//! deep-cloning a 10 000-element `Vec` of `String`-bearing rows on every
//! keystroke. Pre-folded columns in `RowSearchKey` mean the filter walk
//! allocates zero per row.

mod fetch;
mod selection;

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::row_match::{self, Needle};
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

pub use fetch::{apply_row_favorite, apply_row_rating, fetch_and_apply, refilter, resort_and_apply};
pub use selection::{clear_selection, handle_select_row};

/// Pre-folded text columns for fuzzy filtering. Built once per
/// `fetch_and_apply` and aligned positionally with `TracksUi::full`, so
/// `full[i]` and `search_keys[i]` describe the same row.
///
/// Storage is a single packed `Box<str>` of [`row_match::search_fields`]
/// joined by `\0` — one heap allocation per row instead of one `String`
/// header per column. `\0` is a safe separator because needles come from
/// text input (no NUL) and `push_folded` maps away the rare field value
/// carrying one.
///
/// `year` stays an integer beside the packed text rather than being
/// rendered into it, so the Tracks view and every `track_matches` surface
/// run the *same* [`row_match::Needle::matches_year`] rule instead of two spellings
/// of it.
pub(super) struct RowSearchKey {
    packed: Box<str>,
    year: Option<i32>,
}

impl RowSearchKey {
    pub(super) fn from_row(r: &RsTrackListRow) -> Self {
        let fields = row_match::search_fields(r);
        let text_len: usize = fields.iter().map(|f| f.len()).sum();
        // One separator between each pair, so one fewer than there are fields.
        let mut buf = String::with_capacity(text_len + fields.len() - 1);
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                buf.push('\0');
            }
            row_match::push_folded(&mut buf, field);
        }
        Self {
            packed: buf.into_boxed_str(),
            year: r.year,
        }
    }

    /// A plain `str::contains` on the packed text, because both sides are
    /// already folded — this is the one matcher that doesn't go through
    /// [`Needle::contains`], and `Needle::as_str` exists for it.
    pub(super) fn matches(&self, needle: &Needle) -> bool {
        self.packed.contains(needle.as_str()) || needle.matches_year(self.year)
    }
}

/// Holds the unfiltered set of tracks the Tracks view is currently sorted by.
/// Filter changes re-derive the visible model from this Vec without a DB hit.
pub struct TracksUi {
    /// Canonical row set, kept in DB-fetch order. Never reordered — a
    /// header-click sort only rebuilds `order`, never this Vec.
    pub(super) full: Mutex<Arc<Vec<RsTrackListRow>>>,
    /// Pre-folded filter keys, aligned positionally with `full`
    /// (`full[i]` ↔ `search_keys[i]`).
    pub(super) search_keys: Mutex<Arc<Vec<RowSearchKey>>>,
    /// Display-order permutation into `full` / `search_keys`. A sort
    /// change recomputes only this index `Vec` in memory — no DB
    /// round-trip and no `search_keys` rebuild. After a fresh fetch this
    /// is the identity `0..full.len()` (the DB `ORDER BY` already
    /// produced display order on the cold path).
    pub(super) order: Mutex<Arc<Vec<usize>>>,
    /// Path-keyed thumbnail cache. Many tracks share an album cover, so
    /// hitting this from `to_slint_track_list_row` avoids re-decoding the
    /// same file per track. Shared with the now-playing-bar bridge so
    /// the bar's artwork tile reuses cached thumbnails warmed by the
    /// Tracks view.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Visibility and staleness bookkeeping (`section-active-changed`
    /// shadow plus dirty flag). Unlike the entity-grid views, Tracks
    /// releases nothing on leave — `dirty` is set only by the
    /// `library_changed` refresher when a bump arrives while the section
    /// is hidden, and consumed on re-enter to run one deferred re-fetch.
    section: SectionState,
}

impl TracksUi {
    pub fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            full: Mutex::new(Arc::new(Vec::new())),
            search_keys: Mutex::new(Arc::new(Vec::new())),
            order: Mutex::new(Arc::new(Vec::new())),
            cover_thumbs,
            section: SectionState::new(),
        }
    }

    /// Mirror the Tracks-section-visible flag (`section-active-changed`).
    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    /// Whether the Tracks section is currently on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Mark the cached row data stale — a `library_changed` bump arrived
    /// while the section was hidden. See [`Self::take_dirty`].
    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. `true` on section-enter
    /// means a library mutation happened while hidden and the row set must
    /// be re-fetched.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// IDs of the rows that pass `filter`, in the current sort order.
    /// `play-row` hands these to `player_play_tracks`, so the queue becomes
    /// exactly the view the user was looking at.
    pub fn current_ids_filtered(&self, filter: &str) -> Vec<i64> {
        // Hold each guard only long enough to bump the Arc refcount, then drop
        // immediately so concurrent UI callers don't queue behind us.
        let (full, keys, order) = {
            let f = self.full.lock().clone();
            let k = self.search_keys.lock().clone();
            let o = self.order.lock().clone();
            (f, k, o)
        };
        let needle = row_match::fold_needle(filter);
        // Walk `order` so ids come back in the current display sort order.
        // `.get()` keeps this panic-safe if a fetch swapped `full` between
        // the three locks above — the next rebuild restores consistency.
        if needle.is_empty() {
            return order.iter().filter_map(|&i| Some(full.get(i)?.id)).collect();
        }
        order
            .iter()
            .filter_map(|&i| {
                let r = full.get(i)?;
                keys.get(i)?.matches(&needle).then_some(r.id)
            })
            .collect()
    }

    /// Surgical mutation of `is_favorite` on the canonical Vec. Combined with
    /// `apply_row_favorite` below, lets us avoid re-fetching the whole list
    /// on a single-row favourite toggle (preserves scroll position + no flash).
    ///
    /// `Arc::make_mut` performs copy-on-write: when no other reader holds a
    /// clone, the mutation happens in place; if a refilter is mid-flight
    /// holding an `Arc` clone, we get a fresh Vec so the in-flight read
    /// keeps its consistent view.
    pub fn flip_favorite(&self, id: i64, fav: bool) {
        let mut full = self.full.lock();
        let v = Arc::make_mut(&mut *full);
        if let Some(r) = v.iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// Surgical mutation of `rating` on the canonical Vec — the star-rating
    /// analogue of [`Self::flip_favorite`]. Paired with `apply_row_rating` so a
    /// hover-set rating doesn't re-fetch the whole list.
    pub fn flip_rating(&self, id: i64, rating: i32) {
        let mut full = self.full.lock();
        let v = Arc::make_mut(&mut *full);
        if let Some(r) = v.iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }
}

/// Build an empty `VecModel<TrackListRow>`, hand it to the Slint `Tracks`
/// global as a `ModelRc`. Subsequent updates locate it by downcasting
/// `Tracks.rows` back to `VecModel<TrackListRow>`.
pub fn install_tracks_model(ui: &AppWindow) {
    let model: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_rows(ModelRc::from(model));
}

/// Install a persistent `VecModel<i32>` for `Tracks.selected-ids` so
/// selection mutations can `set_vec` into the same model instead of
/// allocating a fresh `ModelRc + VecModel` on every click.
pub fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_selected_ids(ModelRc::from(model));
}

/// A `UiTrackListRow` minus its `cover_img` — every field that can be built
/// **off** the UI thread. `slint::Image` is `!Send`, so the cover decode
/// must stay on the UI thread, but the `SharedString`s and the formatted
/// duration are all `Send` and are far cheaper to produce on a tokio
/// worker. See [`prepare_track_list_row`] / [`finish_track_list_row`].
pub struct PreparedTrackRow {
    id: i32,
    title: SharedString,
    artist: SharedString,
    album: SharedString,
    genre: SharedString,
    year: i32,
    track_number: i32,
    duration_ms: i32,
    is_favorite: bool,
    rating: i32,
    artwork_path: SharedString,
    display_duration: SharedString,
    enabled: bool,
    album_id: i32,
    artist_id: i32,
    genre_id: i32,
}

/// Build the `Send` half of a track-list row — every field but the cover
/// decode. Safe (and intended) to run on a tokio worker so the UI thread
/// only pays for the `!Send` cover lookup in [`finish_track_list_row`].
pub fn prepare_track_list_row(r: &RsTrackListRow) -> PreparedTrackRow {
    PreparedTrackRow {
        id: clamp_i64_to_i32(r.id),
        title: SharedString::from(r.title.as_str()),
        artist: SharedString::from(r.artist.as_deref().unwrap_or("")),
        album: SharedString::from(r.album.as_deref().unwrap_or("")),
        genre: SharedString::from(r.genre.as_deref().unwrap_or("")),
        year: r.year.unwrap_or(0),
        track_number: r.track_number.unwrap_or(0),
        duration_ms: i32::try_from(r.duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        is_favorite: r.is_favorite,
        rating: r.rating,
        artwork_path: SharedString::from(r.artwork_path.as_deref().unwrap_or("")),
        display_duration: SharedString::from(format_duration_ms(r.duration_ms.max(0))),
        // Always interactive for real DB-backed rows. Only the Browse view
        // overrides this `false` — for disk-only files not yet in the library.
        enabled: true,
        album_id: r.album_id.map_or(0, clamp_i64_to_i32),
        artist_id: r.artist_id.map_or(0, clamp_i64_to_i32),
        genre_id: r.genre_id.map_or(0, clamp_i64_to_i32),
    }
}

/// Finish a [`PreparedTrackRow`] into a `UiTrackListRow`. Since covers
/// went lazy (`RowCovers.request` resolves the thumbnail per instantiated
/// row on the Slint side), this is a plain field move — the prepare/finish
/// split survives only because the established view pipelines hop threads
/// between the two stages.
pub fn finish_track_list_row(p: PreparedTrackRow) -> UiTrackListRow {
    UiTrackListRow {
        id: p.id,
        title: p.title,
        artist: p.artist,
        album: p.album,
        genre: p.genre,
        year: p.year,
        track_number: p.track_number,
        duration_ms: p.duration_ms,
        is_favorite: p.is_favorite,
        rating: p.rating,
        artwork_path: p.artwork_path,
        display_duration: p.display_duration,
        selected: false,
        enabled: p.enabled,
        album_id: p.album_id,
        artist_id: p.artist_id,
        genre_id: p.genre_id,
    }
}

pub fn to_slint_track_list_row(r: &RsTrackListRow) -> UiTrackListRow {
    finish_track_list_row(prepare_track_list_row(r))
}

/// `mm:ss` for tracks under one hour, `h:mm:ss` otherwise. Matches the Slint
/// `Theme.format-duration` helper exactly so any UI surface that still calls
/// the Slint version (the now-playing bar's drag tooltip + total length) and
/// the precomputed row strings stay byte-identical.
pub(crate) fn format_duration_ms(ms: i64) -> String {
    let secs_total = ms / 1000;
    let hours = secs_total / 3600;
    let mins = (secs_total / 60) % 60;
    let secs = secs_total % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

#[cfg(test)]
#[path = "tests/tracks_tests.rs"]
mod tests;
