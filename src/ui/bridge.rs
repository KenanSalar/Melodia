//! Push-side glue: subscribes to the player's `tokio::sync::watch` channels
//! and writes converted `ViewModels` into the Slint `Player` global.
//!
//! Each subscriber is a `slint::spawn_local(async_compat::Compat::new(...))`
//! future. `spawn_local` runs on the Slint event-loop thread (so it can write
//! to UI properties directly) and `Compat` provides a tokio reactor for the
//! `watch::Receiver::changed` await.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};
use tokio::sync::watch;

use crate::entities::track::TrackSummary;
use crate::player::event_sink::PlayerSinks;
use crate::player::state::{
    PlayerViewModelLight, PositionTick, QueueViewModel,
};
use crate::media::cover_thumbs::CoverThumbs;
use crate::{AppWindow, Player, PlayerVm, QueueVm, TrackSummaryRow};

/// Subscribe to the lightweight player `ViewModel` (status, current track,
/// volume, `has_next/prev`). Fires on every state change; replaces the whole
/// `Player.vm` property each time.
pub fn spawn_view_model_subscriber(
    ui: Weak<AppWindow>,
    sinks: &Arc<PlayerSinks>,
    cover_thumbs: Arc<CoverThumbs>,
) -> Result<(), slint::EventLoopError> {
    let mut rx = sinks.view_model.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            let Some(ui) = ui.upgrade() else { break };
            if let Some(vm) = snapshot {
                let player = ui.global::<Player>();
                let prev_vm = player.get_vm();
                let mut new_vm = to_slint_player_vm(&vm, &cover_thumbs);
                // Stable artwork identity: when the artwork path is
                // unchanged, reuse the previous `Image` handle instead of
                // the fresh one minted by `to_slint_track`. Slint compares
                // `Image` by identity, so a new handle per emit dirties
                // every binding reading `vm` and forces FemtoVG to treat
                // the cover as a brand-new texture on each volume step /
                // seek / queue edit. Same pixels either way — both handles
                // wrap the same cached RGB8 buffer.
                if new_vm.track.artwork_path == prev_vm.track.artwork_path {
                    new_vm.track.cover_img = prev_vm.track.cover_img.clone();
                }
                let new_position_ms = clamp_to_i32(vm.position_ms);
                let new_duration_ms = clamp_to_i32(vm.duration_ms);
                let new_progress = if vm.duration_ms > 0 {
                    ms_to_progress(vm.position_ms, vm.duration_ms)
                } else {
                    0.0
                };
                // Clear any pending seek hold once the backend's position
                // catches up (±2s) or the current track changes.
                let pending = player.get_seek_pending_ms();
                if pending >= 0
                    && (new_vm.track.id != prev_vm.track.id
                        || (new_position_ms - pending).abs() < 2000)
                {
                    player.set_seek_pending_ms(-1);
                }
                // Position scalars are top-level on the Player global (not
                // fields of `vm`) so the position-tick path can update them
                // every 500 ms without invalidating the metadata bindings.
                // We push them here too so a track change resets the slider
                // immediately instead of waiting for the next tick.
                player.set_position_ms(new_position_ms);
                player.set_duration_ms(new_duration_ms);
                player.set_progress(new_progress);
                // Slint's property set dirties dependents unconditionally —
                // skip the write when the VM is value-identical (e.g. a
                // seek emit: position lives outside `vm`, so nothing the
                // struct carries actually changed).
                if new_vm != prev_vm {
                    player.set_vm(new_vm);
                }
            }
        }
        log::debug!("ui::bridge view-model subscriber stopped");
    }))?;
    Ok(())
}

/// Subscribe to queue-summary updates (length, shuffle, `repeat_mode`).
pub fn spawn_queue_subscriber(
    ui: Weak<AppWindow>,
    sinks: &Arc<PlayerSinks>,
) -> Result<(), slint::EventLoopError> {
    let mut rx = sinks.queue.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            let Some(ui) = ui.upgrade() else { break };
            if let Some(qvm) = snapshot {
                ui.global::<Player>().set_queue(to_slint_queue_vm(&qvm));
            }
        }
        log::debug!("ui::bridge queue subscriber stopped");
    }))?;
    Ok(())
}

/// Subscribe to ~500 ms position ticks and write only the three position
/// scalars on the `Player` global. Crucially this no longer touches
/// `Player.vm`: dirtying that struct invalidated every binding that read
/// any field (title, artist, artwork, …) — at 7 200 ticks/hour that path
/// produced a monotonic RSS climb. See the comment on `PlayerVm` in
/// `ui/models.slint` for the full history.
pub fn spawn_position_subscriber(
    ui: Weak<AppWindow>,
    position_tx: &watch::Sender<Option<PositionTick>>,
) -> Result<(), slint::EventLoopError> {
    let mut rx = position_tx.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            let Some(ui) = ui.upgrade() else { break };
            if let Some(tick) = snapshot {
                let player = ui.global::<Player>();
                let new_position_ms = clamp_to_i32(tick.position_ms);
                player.set_position_ms(new_position_ms);
                if tick.duration_ms > 0 {
                    player.set_duration_ms(clamp_to_i32(tick.duration_ms));
                    player.set_progress(ms_to_progress(tick.position_ms, tick.duration_ms));
                } else {
                    player.set_progress(0.0);
                }
                // Clear seek-pending once the backend's position catches
                // up to the requested target (±2s window).
                let pending = player.get_seek_pending_ms();
                if pending >= 0 && (new_position_ms - pending).abs() < 2000 {
                    player.set_seek_pending_ms(-1);
                }
            }
        }
        log::debug!("ui::bridge position subscriber stopped");
    }))?;
    Ok(())
}

// ---------- conversions ----------

pub fn to_slint_track(t: &TrackSummary, cover_thumbs: &CoverThumbs) -> TrackSummaryRow {
    let cover_img = cover_thumbs.get_or_load_opt(t.artwork_path.as_deref());
    TrackSummaryRow {
        id: clamp_to_i32(u64::try_from(t.id).unwrap_or(0)),
        file_path: SharedString::from(t.file_path.as_str()),
        title: SharedString::from(t.title.as_str()),
        artist: SharedString::from(t.artist.as_deref().unwrap_or("")),
        album: SharedString::from(t.album.as_deref().unwrap_or("")),
        duration_ms: clamp_to_i32(u64::try_from(t.duration_ms.max(0)).unwrap_or(0)),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        cover_img,
        is_favorite: t.is_favorite,
    }
}

/// Convert a backend view-model snapshot to the Slint `PlayerVm` struct.
/// Position-related scalars (`position_ms`, `duration_ms`, `progress`) are
/// deliberately *not* on `PlayerVm` — they live as standalone properties on
/// the `Player` global and are written directly by the position-tick
/// subscriber. See `ui/models.slint` for the rationale.
pub fn to_slint_player_vm(
    vm: &PlayerViewModelLight,
    cover_thumbs: &CoverThumbs,
) -> PlayerVm {
    let track = vm
        .current_track
        .as_ref()
        .map(|t| to_slint_track(t.as_ref(), cover_thumbs))
        .unwrap_or_default();
    PlayerVm {
        has_track: vm.current_track.is_some(),
        track,
        status: SharedString::from(vm.status),
        is_playing: vm.status == "playing",
        volume: i32::try_from(vm.volume).unwrap_or(i32::MAX),
        is_muted: vm.is_muted,
        playback_speed: speed_to_f32(vm.playback_speed),
        gapless: vm.gapless_enabled,
        has_next: vm.has_next,
        has_previous: vm.has_previous,
    }
}

/// Narrow playback speed (f64 backend, f32 Slint) for UI consumption.
#[allow(
    clippy::cast_possible_truncation,
    reason = "playback speed values (0.25..=4.0) round-trip through f32 without observable loss"
)]
fn speed_to_f32(speed: f64) -> f32 {
    speed as f32
}

pub fn to_slint_queue_vm(qvm: &QueueViewModel) -> QueueVm {
    QueueVm {
        length: i32::try_from(qvm.queue_tracks.len()).unwrap_or(i32::MAX),
        current_index: qvm.queue_index,
        shuffle: qvm.shuffle_enabled,
        repeat_mode: SharedString::from(qvm.repeat_mode.as_str()),
        has_next: qvm.has_next,
        has_previous: qvm.has_previous,
    }
}

fn clamp_to_i32(v: u64) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Convert a u64 ms position into f32 progress in [0.0, 1.0]. Used purely for
/// UI display — sub-millisecond precision is irrelevant.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "u64 → f64 → f32 path: ms positions stay below f53 mantissa range, and f32 progress is for UI display only"
)]
fn ms_to_progress(position_ms: u64, duration_ms: u64) -> f32 {
    (position_ms as f64 / duration_ms as f64) as f32
}
