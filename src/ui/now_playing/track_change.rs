//! `sinks.view_model` subscriber + the (decode + metadata fetch + write)
//! apply step. Skipped while the view is closed; seeded on open.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Weak};

use super::metadata::to_slint_track_meta;
use super::write_crossfade_slot;
use super::NowPlayingState;
use crate::entities::track::TrackSummary;
use crate::library;
use crate::state::AppState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::{AppWindow, Player, Theme as ThemeGlobal, TrackMetaRow};

/// Subscribe to `sinks.view_model`, react only to actual track changes.
/// Always stashes the current track into `NowPlayingState::current_track`;
/// then, *only while the view is open*, decodes + blurs the new cover into
/// the inactive slot + flips and fetches the metadata chips. While the view
/// is closed it stashes and stops there — `wire_now_playing_open` does the
/// decode on the next open.
pub(super) fn spawn_track_change_subscriber(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: Arc<NowPlayingArtwork>,
    np_state: Rc<NowPlayingState>,
    initial_track_id: Option<i64>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
    let mut rx = state.sinks.view_model.subscribe();
    // Seeded from the snapshot in `install` so the subscriber doesn't
    // re-fire for the already-seeded restored track.
    let mut last_track_id = initial_track_id;
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // Pull *only* `current_track` out of the snapshot. The
            // subscriber fires on every `view_model` push (play/pause,
            // volume, mute, speed, …) but acts solely on a track change,
            // so cloning the whole `ViewModel` per push is pure waste —
            // this clone is just an `Option<Arc<_>>` refcount bump. The
            // borrow guard is a statement-scoped temporary, dropped before
            // the `.await` below.
            let new_track = rx
                .borrow_and_update()
                .as_ref()
                .map(|vm| vm.current_track.clone());
            let Some(new_track) = new_track else { continue };
            let new_id = new_track.as_ref().map(|t| t.id);
            // Ignore play/pause/volume-only pushes — only a track change
            // touches the artwork slots or refetches metadata.
            if new_id == last_track_id {
                continue;
            }
            last_track_id = new_id;
            // Stash the current track regardless of visibility so a later
            // open can seed the artwork from it. `borrow_mut` guard is a
            // statement-scoped temporary, dropped before the `.await`.
            np_state.current_track.borrow_mut().clone_from(&new_track);
            // The decode + blur + metadata DB read only produces something
            // on screen while a surface renders it — the full Now Playing
            // view or the square miniplayer (whose ~150 px artwork reads
            // from the same `Player.np-cover-{a,b}` dual-slot). Skip
            // otherwise and let `wire_now_playing_open` /
            // `NowPlayingState::kick_artwork` seed on next open / first
            // square paint instead.
            if np_state.open.get() || (np_state.mini_visible.get() && np_state.mini_square.get()) {
                apply_track_change(&weak, &state, &np_artwork, &np_state, new_track, true).await;
            }
        }
        log::debug!("ui::now_playing track-change subscriber stopped");
    }))?;
    Ok(())
}

/// Fetch metadata + decode/blur the cover for `track`, then write both into
/// the `Player` global on the UI thread. Runs inside a `Compat` future, so
/// it stays on the UI thread across the `.await`s.
///
/// `animate` picks the cross-fade behaviour (see `write_crossfade_slot`):
/// `true` for a live track change while the view is open, `false` for the
/// seed-on-open path. Records the applied track id in
/// `NowPlayingState::applied_track_id` so a redundant re-seed is skipped.
pub(super) async fn apply_track_change(
    weak: &Weak<AppWindow>,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &NowPlayingState,
    track: Option<Arc<TrackSummary>>,
    animate: bool,
) {
    let track_id = track.as_ref().map(|t| t.id);

    // --- Metadata: DB read, awaited inline (sqlx has a reactor here).
    // `get_track_meta` reads only the 8 chip columns, not a full `Track`.
    let meta = match track.as_ref() {
        Some(t) => match library::tracks::get_track_meta(state, t.id).await {
            Ok(Some(m)) => to_slint_track_meta(&m),
            // Missing row → clear the chips (default = all-empty strings).
            Ok(None) => TrackMetaRow::default(),
            Err(e) => {
                log::warn!("ui::now_playing get_track_meta({}): {e}", t.id);
                TrackMetaRow::default()
            }
        },
        // No track → clear the chips.
        None => TrackMetaRow::default(),
    };

    // --- Blur + high-res cover: CPU-bound, off-loaded to the runtime's
    // blocking pool. A *single* decode of the source file derives both the
    // sharp foreground tile and the blurred backdrop (see
    // `now_playing_artwork`) — the cover is the largest image in the app's
    // hot path, so decoding it once instead of twice halves the per-skip
    // CPU cost.
    let artwork = track
        .as_ref()
        .and_then(|t| t.artwork_path.clone())
        .filter(|p| !p.is_empty());

    let (cover, blurred, accent_argb): (Option<Image>, Option<Image>, Option<u32>) = match artwork
    {
        Some(path) => {
            let np = np_artwork.clone();
            match state
                .runtime
                .spawn_blocking(move || np.get_or_decode(Path::new(&path)))
                .await
            {
                Ok(Some(pair)) => (
                    Some(Image::from_rgb8(pair.cover)),
                    Some(Image::from_rgb8(pair.blur)),
                    pair.accent_argb,
                ),
                Ok(None) => (None, None, None),
                Err(e) => {
                    log::warn!("ui::now_playing artwork task join: {e}");
                    (None, None, None)
                }
            }
        }
        None => (None, None, None),
    };

    // --- Write to Slint (UI thread) ---
    let Some(ui) = weak.upgrade() else { return };

    // A newer track change may have landed while we were decoding — the
    // open-seed task (spawned by `wire_now_playing_open`) and the track-change
    // subscriber can both have an `apply_track_change` in flight. If the track
    // we decoded is no longer current, drop the result; the newer call owns
    // the `Player` slots and `applied_track_id`. `borrow()` is a
    // statement-scoped temporary — not held across an `.await` (there is none
    // after this point).
    if track_id != np_state.current_track.borrow().as_ref().map(|t| t.id) {
        return;
    }

    let player = ui.global::<Player>();
    // Chip strip: refresh the shadow from the just-fetched `meta` and push
    // a freshly chunked 2D model so the view reflects the new track without
    // waiting on a width-change fire. The chunk uses the chip-area width
    // cached by `Player.recompute-chip-rows`; if the view hasn't laid out
    // yet (`chip_last_width == 0.0`), `chunk_chips_to_rows` collapses to a
    // single row and the view's mount Timer fires a real width immediately.
    let chip_texts = super::metadata::visible_chip_texts(&meta);
    let chip_rows =
        super::metadata::chunk_chips_to_rows(&chip_texts, np_state.chip_last_width.get());
    player.set_chip_rows(super::metadata::rows_to_model(chip_rows));
    *np_state.chip_texts.borrow_mut() = chip_texts;
    player.set_track_meta(meta);

    // Per-artwork accent → `Player.np-accent`. Falls back to the live
    // `Theme.accent` so non-MY users keep a static-accent tint and a missing-
    // artwork / failed-decode track doesn't strand the slot on the previous
    // track's colour. Theme changes naturally propagate via this fallback on
    // the next track change.
    player.set_np_accent(match accent_argb {
        Some(argb) => crate::themes::brush(argb),
        None => ui.global::<ThemeGlobal>().get_accent(),
    });

    write_crossfade_slot(
        blurred,
        animate,
        player.get_blur_use_a(),
        |img| player.set_blur_img_a(img),
        |img| player.set_blur_img_b(img),
        |v| player.set_blur_use_a(v),
        |v| player.set_blur_has_image(v),
    );
    write_crossfade_slot(
        cover,
        animate,
        player.get_np_cover_use_a(),
        |img| player.set_np_cover_a(img),
        |img| player.set_np_cover_b(img),
        |v| player.set_np_cover_use_a(v),
        |v| player.set_np_cover_has_image(v),
    );

    np_state.applied_track_id.set(track_id);
}
