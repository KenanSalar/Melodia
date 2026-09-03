//! Push-side glue: subscribes to the player's `tokio::sync::watch` channels
//! and writes converted `ViewModels` into the Slint `Player` global.
//!
//! Each subscriber is a `slint::spawn_local(async_compat::Compat::new(...))`
//! future. `spawn_local` runs on the Slint event-loop thread (so it can write
//! to UI properties directly) and `Compat` provides a tokio reactor for the
//! `watch::Receiver::changed` await.

use std::path::Path;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use crate::{AppWindow, Player, PlayerVm, QueueVm, RadioVm, TrackSummaryRow};
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{PlayerViewModelLight, PositionTick, QueueViewModel};
use melodia_engine::player::engine::types::RadioNowPlaying;

/// Subscribe to the lightweight player `ViewModel` (status, current track,
/// volume, `has_next/prev`). Fires on every state change; replaces the whole
/// `Player.vm` property each time.
pub fn spawn_view_model_subscriber(
    ui: Weak<AppWindow>,
    sinks: &Arc<PlayerSinks>,
    cover_thumbs: Arc<CoverThumbs>,
    runtime: Handle,
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
                let cover_path = reuse_or_warm(
                    &new_vm.track.artwork_path,
                    &prev_vm.track.artwork_path,
                    &mut new_vm.track.cover_img,
                    &prev_vm.track.cover_img,
                );
                let logo_path = reuse_or_warm(
                    &new_vm.radio.artwork_path,
                    &prev_vm.radio.artwork_path,
                    &mut new_vm.radio.logo_img,
                    &prev_vm.radio.logo_img,
                );
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
                // `Property::set` is value-compared, so this guard spares only the move into the
                // setter and the binding-handle access it opens with, and pays a second compare
                // whenever the VM *did* change. Worth it because most emits are value-identical —
                // a seek carries its position outside `vm`.
                if new_vm != prev_vm {
                    player.set_vm(new_vm);
                }
                if let Some(path) = cover_path {
                    warm_vm_cover(ui.as_weak(), &runtime, &cover_thumbs, path, VmCoverSlot::Track);
                }
                if let Some(path) = logo_path {
                    warm_vm_cover(
                        ui.as_weak(),
                        &runtime,
                        &cover_thumbs,
                        path,
                        VmCoverSlot::Station,
                    );
                }
            }
        }
        log::debug!("ui::shell::bridge view-model subscriber stopped");
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
        log::debug!("ui::shell::bridge queue subscriber stopped");
    }))?;
    Ok(())
}

/// Subscribe to ~500 ms position ticks and write only the three position
/// scalars on the `Player` global. Crucially this no longer touches
/// `Player.vm`: dirtying that struct invalidated every binding that read
/// any field (title, artist, artwork, …) — at 7 200 ticks/hour that path
/// produced a monotonic RSS climb. See the comment on `PlayerVm` in
/// `crates/melodia-ui/ui/models.slint` for the full history.
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
        log::debug!("ui::shell::bridge position subscriber stopped");
    }))?;
    Ok(())
}

// ---------- conversions ----------

/// **Cache-only on the cover**, because both callers run on the event loop: `get_or_load_opt` would
/// decode the full source there, and `decode_thumb_buffer` takes the large-decode gate the prewarm
/// pool holds while it works — so a miss parks the loop behind a background decode rather than
/// merely paying for its own. A cold cover comes back as the empty [`slint::Image`] and
/// [`warm_vm_cover`] fills the slot in a moment later.
pub fn to_slint_track(t: &TrackSummary, cover_thumbs: &CoverThumbs) -> TrackSummaryRow {
    let cover_img = cover_thumbs.get_cached_opt(t.artwork_path.as_deref());
    TrackSummaryRow {
        id: clamp_i64_to_i32(t.id),
        file_path: SharedString::from(t.file_path.as_str()),
        title: SharedString::from(t.title.as_str()),
        artist: SharedString::from(t.artist.as_deref().unwrap_or("")),
        album: SharedString::from(t.album.as_deref().unwrap_or("")),
        duration_ms: clamp_i64_to_i32(t.duration_ms.max(0)),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        cover_img,
        is_favorite: t.is_favorite,
        rating: t.rating,
    }
}

/// Both halves of one artwork slot's contract: keep the handle where the path stands still, and
/// answer the path to warm where it moved onto a tier that had nothing.
///
/// **Stable identity is the first half and it is invisible until profiled.** Slint compares
/// `Image` by identity, so a fresh handle per emit dirties every binding reading `vm` and makes
/// `FemtoVG` treat the artwork as a brand-new texture on each volume step, seek or queue edit. Same
/// pixels either way; both handles wrap the same cached RGB8 buffer.
///
/// The second is gated on the path having *moved* rather than on the slot being empty, since the
/// reuse hands a cover that failed to decode straight back — an is-it-empty test would re-ask on
/// every volume step for the rest of the track.
fn reuse_or_warm(
    new_path: &SharedString,
    prev_path: &SharedString,
    new_image: &mut slint::Image,
    prev_image: &slint::Image,
) -> Option<String> {
    if new_path == prev_path {
        new_image.clone_from(prev_image);
        return None;
    }
    (new_image.size().width == 0).then(|| new_path.to_string())
}

/// Convert the station on the deck for the Slint boundary.
///
/// The logo is resolved exactly as [`to_slint_track`] resolves a cover — cache-only, so nothing
/// decodes on the event loop, with [`warm_vm_cover`] filling a miss. The tile comes from
/// `ui::radio::station_tile`, the same derivation the grid card and the station hero take theirs
/// from, so one station cannot wear two different tiles on two pages.
fn to_slint_radio_vm(station: &RadioNowPlaying, cover_thumbs: &CoverThumbs) -> RadioVm {
    let tile = crate::ui::radio::station_tile(&station.name);
    RadioVm {
        station_id: clamp_i64_to_i32(station.station_id),
        uuid: opt_shared(station.station_uuid.as_deref()),
        name: SharedString::from(station.name.as_str()),
        live_title: SharedString::from(station.live_title.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(station.artwork_path.as_deref().unwrap_or("")),
        logo_img: cover_thumbs.get_cached_opt(station.artwork_path.as_deref()),
        monogram: tile.monogram,
        tile_color_1: tile.color_1,
        tile_color_2: tile.color_2,
        buffering: station.buffering,
        stream_url: SharedString::from(station.stream_url.as_str()),
        country: opt_shared(station.country.as_deref()),
        // Trimmed and joined here rather than carried that way, so the bar's second line and a
        // station card show the same few tags off the same rule.
        tags: SharedString::from(crate::ui::radio::display_tags(
            station.tags.as_deref().unwrap_or_default(),
        )),
        homepage: opt_shared(station.homepage.as_deref()),
        codec: opt_shared(station.codec.as_deref()),
        bitrate: station.bitrate,
        play_count: station.play_count,
    }
}

/// Slint has no `Option<T>`, so an absent fact crosses as the empty string every consumer already
/// gates on.
fn opt_shared(value: Option<&str>) -> SharedString {
    SharedString::from(value.unwrap_or(""))
}

/// Which half of `Player.vm` a warm is filling. The two paths differ only in the field they guard
/// on and the field they write, so a parallel `warm_vm_logo` would be the same round trip, the
/// same staleness check and the same bail spelled twice.
#[derive(Clone, Copy)]
pub enum VmCoverSlot {
    Track,
    Station,
}

/// Decode `path` into the row tier off the event loop, then write it into `Player.vm`.
///
/// [`to_slint_track`]'s other half: it answers cache-only so nothing decodes on the UI thread, and
/// this is what makes a cold cover arrive at all. Fire-and-forget — the buffer crosses back rather
/// than a second lookup, `SharedPixelBuffer` being `Send` where [`slint::Image`] is not. Keyed on
/// the path on the way back in, so a track change that landed while this decoded keeps its own
/// cover; nothing is written for an empty path or a failed decode.
pub fn warm_vm_cover(
    weak: Weak<AppWindow>,
    runtime: &Handle,
    cover_thumbs: &Arc<CoverThumbs>,
    path: String,
    slot: VmCoverSlot,
) {
    if path.is_empty() {
        return;
    }
    let thumbs = cover_thumbs.clone();
    runtime.spawn_blocking(move || {
        // A decode that failed has nothing to write — the slot is already the empty `Image`, that
        // emptiness being what asked for this. Bailing here skips the whole round trip: the write
        // would repaint nothing, but the hop, the `PlayerVm` clone `get_vm` hands back and the
        // compare over it are all paid on the UI thread before it can decide that.
        let Some(buffer) = thumbs.get_or_load_rgb8(Path::new(&path)) else {
            return;
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let player = ui.global::<Player>();
            let mut vm = player.get_vm();
            let held = match slot {
                VmCoverSlot::Track => &vm.track.artwork_path,
                VmCoverSlot::Station => &vm.radio.artwork_path,
            };
            if held != path.as_str() {
                return;
            }
            let image = slint::Image::from_rgb8(buffer);
            match slot {
                VmCoverSlot::Track => vm.track.cover_img = image,
                VmCoverSlot::Station => vm.radio.logo_img = image,
            }
            player.set_vm(vm);
        });
    });
}

/// Convert a backend view-model snapshot to the Slint `PlayerVm` struct.
/// Position-related scalars (`position_ms`, `duration_ms`, `progress`) are
/// deliberately *not* on `PlayerVm` — they live as standalone properties on
/// the `Player` global and are written directly by the position-tick
/// subscriber. See `crates/melodia-ui/ui/models.slint` for the rationale.
pub fn to_slint_player_vm(vm: &PlayerViewModelLight, cover_thumbs: &CoverThumbs) -> PlayerVm {
    let track = vm
        .current_track
        .as_ref()
        .map(|t| to_slint_track(t.as_ref(), cover_thumbs))
        .unwrap_or_default();
    let radio = vm.radio.as_ref().map(|r| to_slint_radio_vm(r, cover_thumbs)).unwrap_or_default();
    PlayerVm {
        has_track: vm.current_track.is_some(),
        track,
        has_station: vm.radio.is_some(),
        radio,
        status: SharedString::from(vm.status),
        is_playing: vm.status == "playing",
        volume: i32::try_from(vm.volume).unwrap_or(i32::MAX),
        is_muted: vm.is_muted,
        playback_speed: speed_to_f32(vm.playback_speed),
        gapless: vm.gapless_enabled,
        sleep_at_track_end: vm.sleep_at_track_end,
        has_next: vm.has_next,
        has_previous: vm.has_previous,
    }
}

/// Narrow playback speed (f64 backend, f32 Slint) for UI consumption.
#[allow(
    clippy::cast_possible_truncation,
    reason = "playback speed values (0.25..=2.0) round-trip through f32 without observable loss"
)]
fn speed_to_f32(speed: f64) -> f32 {
    speed as f32
}

pub fn to_slint_queue_vm(qvm: &QueueViewModel) -> QueueVm {
    QueueVm {
        length: len_as_i32(qvm.queue_tracks.len()),
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
