//! The `Player` global's three `watch` subscribers, each a `spawn_local` future on the UI thread
//! with `Compat` lending it a tokio reactor.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::ui::util::{clamp_i64_to_i32, len_as_i32, opt_shared};
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{PlayerViewModelLight, PositionTick, QueueViewModel};
use melodia_engine::player::engine::types::RadioNowPlaying;
use melodia_ui::{AppWindow, Player, PlayerVm, QueueVm, RadioVm, TrackSummaryRow};

/// How near a reported position has to land to a held seek target to release it. The seek's own
/// echo carries the target exactly; the slack keeps a target the state adjusts from pinning the
/// thumb until the track changes.
const SEEK_HOLD_SLACK_MS: i32 = 2_000;

/// The stamp of the position on screen, shared by the two subscribers that write one. Their
/// channels wake independently, so a view model can land ahead of a tick read before it; only a
/// newer engine stamp is shown, the duration with it.
#[derive(Clone, Default)]
pub struct ShownPosition(Rc<Cell<u64>>);

impl ShownPosition {
    /// Whether a position stamped `stamp` is newer than the one on screen, recording it if so.
    fn admits(&self, stamp: u64) -> bool {
        if stamp <= self.0.get() {
            return false;
        }
        self.0.set(stamp);
        true
    }
}

/// Show a reported position and the length it is out of, releasing a seek hold the position has
/// caught up with.
pub fn show_position(player: &Player<'_>, position_ms: u64, duration_ms: u64) {
    let position_ms = clamp_to_i32(position_ms);
    player.set_duration_ms(clamp_to_i32(duration_ms));
    player.set_position_ms(position_ms);
    let pending = player.get_seek_pending_ms();
    if pending >= 0 && (position_ms - pending).abs() < SEEK_HOLD_SLACK_MS {
        player.set_seek_pending_ms(-1);
    }
}

pub fn spawn_view_model_subscriber(
    ui: Weak<AppWindow>,
    sinks: &Arc<PlayerSinks>,
    cover_thumbs: Arc<CoverThumbs>,
    runtime: Handle,
    shown: ShownPosition,
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
                // A hold names a point on the track it was set against, whatever the stamp says.
                if new_vm.track.id != prev_vm.track.id {
                    player.set_seek_pending_ms(-1);
                }
                // Here too, so a track change resets the slider without waiting for a tick.
                if shown.admits(vm.position_stamp) {
                    show_position(&player, vm.position_ms, vm.duration_ms);
                }
                // `Property::set` compares too, so this spares only the move and the binding-handle
                // access, at a second compare when the VM did change. Most emits are identical.
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

/// Writes only the position scalars, never `Player.vm` (`models.slint` argues why).
pub fn spawn_position_subscriber(
    ui: Weak<AppWindow>,
    position_tx: &watch::Sender<Option<PositionTick>>,
    shown: ShownPosition,
) -> Result<(), slint::EventLoopError> {
    let mut rx = position_tx.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            let Some(ui) = ui.upgrade() else { break };
            let Some(tick) = snapshot else { continue };
            // A tick read before a stop, a seek or a track change carries an older stamp than
            // that op's view model, and would put the position it moved off back on screen.
            if !shown.admits(tick.stamp) {
                continue;
            }
            show_position(&ui.global::<Player>(), tick.position_ms, tick.duration_ms);
        }
        log::debug!("ui::shell::bridge position subscriber stopped");
    }))?;
    Ok(())
}

/// **Cache-only on the cover**, both callers running on the event loop: a load there would decode
/// the full source behind the large-decode gate the prewarm pool holds, parking the loop behind a
/// background decode. A cold cover comes back empty and [`warm_vm_cover`] fills it.
pub fn to_slint_track(t: &TrackSummary, cover_thumbs: &CoverThumbs) -> TrackSummaryRow {
    let cover_img = cover_thumbs.get_cached_opt(t.artwork_path.as_deref());
    TrackSummaryRow {
        id: clamp_i64_to_i32(t.id),
        file_path: SharedString::from(t.file_path.as_str()),
        title: SharedString::from(t.title.as_str()),
        artist: opt_shared(t.artist.as_deref()),
        album: opt_shared(t.album.as_deref()),
        duration_ms: clamp_i64_to_i32(t.duration_ms.max(0)),
        artwork_path: opt_shared(t.artwork_path.as_deref()),
        cover_img,
        is_favorite: t.is_favorite,
        rating: t.rating,
    }
}

/// Keeps the slot's handle where its path stands still, and answers the path to warm where it moved
/// onto a tier that had nothing.
///
/// Slint compares `Image` by identity, so a fresh handle per emit dirties every binding reading `vm`
/// and makes `FemtoVG` upload the artwork as a new texture on each volume step, seek or queue edit.
/// The warm is gated on the path moving rather than the slot being empty: the reuse hands back a
/// cover that failed to decode, and an emptiness test would re-ask on every volume step.
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

/// The logo resolves cache-only, as [`to_slint_track`]'s cover does. The tile is
/// `ui::radio::station_tile`'s, as the grid card's and the hero's are, so a station wears one tile
/// everywhere.
fn to_slint_radio_vm(station: &RadioNowPlaying, cover_thumbs: &CoverThumbs) -> RadioVm {
    let tile = crate::ui::radio::station_tile(&station.name);
    let announcement = station.announcement();
    RadioVm {
        station_id: clamp_i64_to_i32(station.station_id),
        uuid: opt_shared(station.station_uuid.as_deref()),
        name: SharedString::from(station.name.as_str()),
        live_title: opt_shared(
            announcement.map(|song| song.title).or(station.live_title.as_deref()),
        ),
        live_artist: opt_shared(announcement.map(|song| song.artist)),
        artwork_path: opt_shared(station.artwork_path.as_deref()),
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

/// Which half of `Player.vm` a warm fills.
#[derive(Clone, Copy)]
pub enum VmCoverSlot {
    Track,
    Station,
}

/// Decodes `path` into the row tier off the event loop and writes it into `Player.vm`, which is
/// how a cold cover arrives at all. Checked against the path on the way back, so a track change
/// that landed meanwhile keeps its own cover.
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
        // A failed decode bails before the hop: the write would repaint nothing, but the hop, the
        // `get_vm` clone and its compare would all be paid on the UI thread.
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

#[allow(
    clippy::cast_possible_truncation,
    reason = "playback speed values (0.25..=2.0) round-trip through f32 without observable loss"
)]
fn speed_to_f32(speed: f64) -> f32 {
    speed as f32
}

pub fn to_slint_queue_vm(qvm: &QueueViewModel) -> QueueVm {
    QueueVm {
        length: len_as_i32(qvm.ids.len()),
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

#[cfg(test)]
#[path = "tests/bridge_tests.rs"]
mod tests;
