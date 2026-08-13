use std::sync::Arc;
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::player::event_sink::{EventSink, MediaControlsSync, PlayerEvent};
use crate::player::state::PlayerViewModelLight;
use crate::player::types::PlaybackStatus;

/// Wrapper around souvlaki's `MediaControls`.
/// `Option` allows graceful degradation when OS media controls are unavailable
/// (e.g., no D-Bus session, headless server).
pub struct MediaControlsHandle {
    inner: std::sync::Mutex<MediaControlsInner>,
}

struct MediaControlsInner {
    controls: Option<MediaControls>,
    /// Retained on Windows so the deferred `attach_smtc` can wire souvlaki's
    /// event callback into the same channel `spawn_event_receiver` drains.
    /// SMTC can't be built until the OS window — and its `HWND` — exists,
    /// which is well after `AppState::init`.
    #[cfg(target_os = "windows")]
    event_tx: mpsc::Sender<MediaControlEvent>,
    /// Last-synced state to avoid redundant D-Bus/SMTC calls on queue-only changes.
    last_track_id: Option<i64>,
    last_status: Option<PlaybackStatus>,
    last_position_ms: u64,
    last_volume: u32,
    last_is_muted: bool,
}

/// Initialize OS media controls (MPRIS on Linux, SMTC on Windows, `MediaPlayer`
/// on macOS).
///
/// Linux and macOS create the controls eagerly — neither needs a window
/// handle. On Windows, souvlaki's SMTC backend `expect`s a non-null `HWND` in
/// `PlatformConfig` and panics without one; no OS window exists at
/// `AppState::init` time, so creation is deferred to `attach_smtc`, called
/// from the event loop once the Slint window is shown. Until then — and on
/// any platform where init fails — the handle is an inert no-op.
pub fn init_media_controls() -> (MediaControlsHandle, mpsc::Receiver<MediaControlEvent>) {
    let (tx, rx) = mpsc::channel(32);

    #[cfg(not(target_os = "windows"))]
    let controls = {
        let controls = try_create_controls(None, tx);
        if controls.is_some() {
            log::info!("OS media controls initialized");
        }
        controls
    };

    #[cfg(target_os = "windows")]
    let controls: Option<MediaControls> = {
        log::info!("Windows SMTC init deferred until the OS window is shown");
        None
    };

    let handle = MediaControlsHandle {
        inner: std::sync::Mutex::new(MediaControlsInner {
            controls,
            #[cfg(target_os = "windows")]
            event_tx: tx,
            last_track_id: None,
            last_status: None,
            last_position_ms: 0,
            last_volume: 0,
            last_is_muted: false,
        }),
    };

    (handle, rx)
}

/// Build souvlaki controls and attach the event callback, folding any failure
/// into `None` with a warning. A `None` result leaves the handle an inert
/// no-op (headless session, missing D-Bus, etc.).
fn try_create_controls(
    hwnd: Option<*mut std::ffi::c_void>,
    tx: mpsc::Sender<MediaControlEvent>,
) -> Option<MediaControls> {
    match create_controls(hwnd, tx) {
        Ok(controls) => Some(controls),
        Err(e) => {
            log::warn!("Failed to initialize OS media controls: {e}");
            None
        }
    }
}

#[cfg(target_os = "windows")]
impl MediaControlsHandle {
    /// Attach Windows System Media Transport Controls now that the OS window —
    /// and therefore a valid `HWND` — exists. Called from the event loop once
    /// the Slint window is shown (see `main`).
    ///
    /// Returns `true` only when this call newly attached the controls.
    /// Idempotent: a call made when controls already exist (or when souvlaki
    /// fails to build them) returns `false` without rebuilding. The caller
    /// uses a `true` result to push a one-off state re-sync — `sync()` no-op'd
    /// every call while the handle was inert, so nothing has reached the OS
    /// panel yet.
    #[must_use]
    pub fn attach_smtc(&self, hwnd: *mut std::ffi::c_void) -> bool {
        // Clone the event sender under a short lock; bail if SMTC is already up.
        let tx = {
            let guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.controls.is_some() {
                return false;
            }
            guard.event_tx.clone()
        };

        // Build off-lock — `MediaControls::new` does COM/SMTC setup that should
        // not serialise behind `sync()` calls arriving from playback threads.
        let Some(controls) = try_create_controls(Some(hwnd), tx) else {
            return false;
        };

        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.controls.is_some() {
            // Lost a race with another `attach_smtc` — drop the spare controls.
            return false;
        }
        guard.controls = Some(controls);
        log::info!("Windows SMTC attached");
        true
    }
}

fn create_controls(
    hwnd: Option<*mut std::ffi::c_void>,
    tx: mpsc::Sender<MediaControlEvent>,
) -> Result<MediaControls, String> {
    let config = PlatformConfig {
        dbus_name: "melodia",
        display_name: "Melodia",
        hwnd,
    };

    let mut controls = MediaControls::new(config).map_err(|e| format!("{e}"))?;

    controls
        .attach(move |event: MediaControlEvent| {
            if let Err(e) = tx.try_send(event) {
                log::warn!("Dropped media control event due to full channel: {e}");
            }
        })
        .map_err(|e| format!("{e}"))?;

    Ok(controls)
}

/// Spawn a background task that receives OS media control events and translates
/// them into `PlayerEvent` values fed into the application's `EventSink`.
///
/// The mpsc channel decouples souvlaki's internal D-Bus/SMTC callback thread
/// from the player state — the callback never blocks on `PlayerState` or
/// `MediaControlsHandle` locks, avoiding deadlocks.
pub fn spawn_event_receiver(
    tracker: &TaskTracker,
    shutdown_token: CancellationToken,
    mut rx: mpsc::Receiver<MediaControlEvent>,
    sink: Arc<dyn EventSink>,
) {
    tracker.spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = shutdown_token.cancelled() => break,
                maybe_event = rx.recv() => match maybe_event {
                    Some(event) => {
                        if let Some(pe) = translate_event(event) {
                            sink.handle(pe);
                        }
                    }
                    None => break,
                },
            }
        }
        log::info!("Media control event receiver stopped");
    });
}

fn translate_event(event: MediaControlEvent) -> Option<PlayerEvent> {
    match event {
        MediaControlEvent::Play => Some(PlayerEvent::Play),
        MediaControlEvent::Pause => Some(PlayerEvent::Pause),
        MediaControlEvent::Toggle => Some(PlayerEvent::PlayPause),
        MediaControlEvent::Next => Some(PlayerEvent::Next),
        MediaControlEvent::Previous => Some(PlayerEvent::Previous),
        MediaControlEvent::Stop => Some(PlayerEvent::Stop),
        MediaControlEvent::SetPosition(MediaPosition(pos)) => {
            Some(PlayerEvent::SeekTo(u64::try_from(pos.as_millis()).unwrap_or(u64::MAX)))
        }
        MediaControlEvent::SetVolume(vol) => {
            let vol = vol.clamp(0.0, 1.0);
            // vol is bounded [0, 1.0] → (vol * 100).round() ∈ [0, 100], always
            // representable as u32. The `as` cast is the trailing narrowing
            // step from f64 to u32; vol/round handle the value range.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "vol is clamped to [0, 1] before scale"
            )]
            let scaled = (vol * 100.0).round() as u32;
            Some(PlayerEvent::SetVolume(scaled))
        }
        // Relative seeks need current position; revisit when the library API lands a
        // SeekRelative variant or the sink can resolve it from state.
        MediaControlEvent::Seek(_) | MediaControlEvent::SeekBy(_, _) => {
            log::debug!("Relative seek media event ignored (needs library API support)");
            None
        }
        MediaControlEvent::Raise | MediaControlEvent::Quit => {
            // Window control needs a Slint window handle, which this layer
            // deliberately doesn't hold — see the `EventSink` split.
            log::debug!("Window control media event ignored");
            None
        }
        MediaControlEvent::OpenUri(uri) => {
            log::debug!("MPRIS OpenUri requested (ignored): {uri}");
            None
        }
    }
}

impl MediaControlsSync for MediaControlsHandle {
    /// Sync OS media controls with current player state.
    /// Called from `with_state_emit()` after `ViewModel` emission.
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus) {
        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        let track_id = vm.current_track.as_ref().map(|t| t.id);
        let track_changed = guard.last_track_id != track_id;
        let status_changed = guard.last_status != Some(status);
        let position_changed = guard.last_position_ms != vm.position_ms;
        let volume_changed = guard.last_volume != vm.volume || guard.last_is_muted != vm.is_muted;

        if !track_changed && !status_changed && !position_changed && !volume_changed {
            return;
        }

        let Some(controls) = guard.controls.as_mut() else {
            return;
        };

        if track_changed {
            if let Some(ref track) = vm.current_track {
                let cover_url = track.artwork_path.as_ref().map(|p| format!("file://{p}"));

                if let Err(e) = controls.set_metadata(MediaMetadata {
                    title: Some(&track.title),
                    artist: track.artist.as_deref(),
                    album: track.album.as_deref(),
                    duration: Some(Duration::from_millis(vm.duration_ms)),
                    cover_url: cover_url.as_deref(),
                }) {
                    log::debug!("Failed to set media metadata: {e}");
                }
            } else if let Err(e) = controls.set_metadata(MediaMetadata::default()) {
                log::debug!("Failed to clear media metadata: {e}");
            }
        }

        if status_changed || track_changed || position_changed {
            let progress = Some(MediaPosition(Duration::from_millis(vm.position_ms)));
            let playback = match status {
                PlaybackStatus::Playing => MediaPlayback::Playing { progress },
                PlaybackStatus::Paused => MediaPlayback::Paused { progress },
                PlaybackStatus::Stopped | PlaybackStatus::Loading => MediaPlayback::Stopped,
            };
            if let Err(e) = controls.set_playback(playback) {
                log::debug!("Failed to set media playback status: {e}");
            }
        }

        #[cfg(target_os = "linux")]
        if volume_changed {
            let vol = crate::player::state::volume_to_amplitude(vm.volume, vm.is_muted);
            if let Err(e) = controls.set_volume(vol) {
                log::debug!("Failed to set media volume: {e}");
            }
        }

        guard.last_track_id = track_id;
        guard.last_status = Some(status);
        guard.last_position_ms = vm.position_ms;
        guard.last_volume = vm.volume;
        guard.last_is_muted = vm.is_muted;
    }

    /// Periodic position refresh used by the playback monitor on Windows / macOS.
    /// Linux MPRIS relies on state-change syncs and doesn't need this.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn update_position(&self, position_ms: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(controls) = guard.controls.as_mut() {
            let progress = Some(MediaPosition(Duration::from_millis(position_ms)));
            if let Err(e) = controls.set_playback(MediaPlayback::Playing { progress }) {
                log::debug!("Failed to update media position: {e}");
            }
        }
    }
}
