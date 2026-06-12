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
//! keystroke. Pre-computed lowercase columns in `RowSearchKey` mean the
//! filter walk allocates zero per row.

mod fetch;
mod selection;

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

pub use fetch::{apply_row_favorite, fetch_and_apply, refilter, resort_and_apply};
pub use selection::{clear_selection, handle_select_row};

/// Pre-lowered text columns for fuzzy filtering. Built once per
/// `fetch_and_apply` and aligned positionally with `TracksUi::full`, so
/// `full[i]` and `search_keys[i]` describe the same row.
///
/// Storage is a single packed `Box<str>` of `"title\0artist\0album"` —
/// one heap allocation per row instead of three separate `String`
/// headers, ~60% memory cut on libraries with 10k+ rows. `\0` is a safe
/// separator because needles come from text input (no NUL) and the rare
/// field value containing `\0` is sanitised at build time.
pub(super) struct RowSearchKey {
    packed: Box<str>,
}

impl RowSearchKey {
    pub(super) fn from_row(r: &RsTrackListRow) -> Self {
        let title = r.title.as_str();
        let artist = r.artist.as_deref().unwrap_or("");
        let album = r.album.as_deref().unwrap_or("");
        let mut buf = String::with_capacity(title.len() + artist.len() + album.len() + 2);
        push_sanitised_lower(&mut buf, title);
        buf.push('\0');
        push_sanitised_lower(&mut buf, artist);
        buf.push('\0');
        push_sanitised_lower(&mut buf, album);
        Self {
            packed: buf.into_boxed_str(),
        }
    }

    pub(super) fn matches(&self, lowered_needle: &str) -> bool {
        self.packed.contains(lowered_needle)
    }
}

/// Append `s.to_lowercase()` to `out`, replacing any embedded `\0` with a
/// space so it can't collide with the field separator. The ASCII fast-path
/// avoids per-char Unicode-table dispatch.
fn push_sanitised_lower(out: &mut String, s: &str) {
    if s.is_ascii() {
        out.reserve(s.len());
        for &b in s.as_bytes() {
            if b == 0 {
                out.push(' ');
            } else {
                out.push(b.to_ascii_lowercase() as char);
            }
        }
        return;
    }
    for ch in s.chars() {
        if ch == '\0' {
            out.push(' ');
        } else {
            out.extend(ch.to_lowercase());
        }
    }
}

/// Holds the unfiltered set of tracks the Tracks view is currently sorted by.
/// Filter changes re-derive the visible model from this Vec without a DB hit.
pub struct TracksUi {
    /// Canonical row set, kept in DB-fetch order. Never reordered — a
    /// header-click sort only rebuilds `order`, never this Vec.
    pub(super) full: Mutex<Arc<Vec<RsTrackListRow>>>,
    /// Pre-lowered filter keys, aligned positionally with `full`
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

    /// IDs of the rows that pass `filter`, in the current sort order. Used by
    /// `play-row` to replace the queue with "this track + the rest of the
    /// current view".
    pub fn current_ids_filtered(&self, filter: &str) -> Vec<i64> {
        // Hold each guard only long enough to bump the Arc refcount, then drop
        // immediately so concurrent UI callers don't queue behind us.
        let (full, keys, order) = {
            let f = self.full.lock().clone();
            let k = self.search_keys.lock().clone();
            let o = self.order.lock().clone();
            (f, k, o)
        };
        let needle = filter.trim().to_lowercase();
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

/// Finish a [`PreparedTrackRow`] into a `UiTrackListRow` by decoding (or
/// cache-hitting) its cover thumbnail. Must run on the UI thread —
/// `slint::Image` is `!Send`.
pub fn finish_track_list_row(p: PreparedTrackRow, thumbs: &CoverThumbs) -> UiTrackListRow {
    let cover_img = thumbs.get_or_load_opt(
        Some(p.artwork_path.as_str()).filter(|s| !s.is_empty()),
    );
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
        artwork_path: p.artwork_path,
        cover_img,
        display_duration: p.display_duration,
        selected: false,
        enabled: p.enabled,
        album_id: p.album_id,
        artist_id: p.artist_id,
        genre_id: p.genre_id,
    }
}

pub fn to_slint_track_list_row(r: &RsTrackListRow, thumbs: &CoverThumbs) -> UiTrackListRow {
    finish_track_list_row(prepare_track_list_row(r), thumbs)
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
