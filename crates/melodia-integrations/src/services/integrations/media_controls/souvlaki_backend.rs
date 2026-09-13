//! SMTC on Windows and `MediaPlayer` on macOS, through souvlaki.

use std::time::{Duration, Instant};

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tokio::sync::mpsc;

use melodia_engine::player::engine::event_sink::{MediaControlsSync, PlayerEvent};
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_engine::player::engine::types::PlaybackStatus;

use super::published::Published;
use super::{cover_url, forward, volume_percent};

/// How often a playing position is handed over. Both panels advance their own clock between
/// updates, so this only has to keep the two from visibly diverging.
const TIMELINE_REFRESH: Duration = Duration::from_secs(5);

/// Wrapper around souvlaki's `MediaControls`.
/// `Option` allows graceful degradation when OS media controls are unavailable.
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
    event_tx: mpsc::Sender<PlayerEvent>,
    /// Last-synced state to avoid redundant SMTC calls on queue-only changes.
    published: Published,
    last_timeline_push: Option<Instant>,
}

impl MediaControlsHandle {
    /// macOS creates the controls eagerly, since it needs no window handle. On Windows,
    /// souvlaki's SMTC backend `expect`s a non-null `HWND` in `PlatformConfig` and panics without
    /// one; no OS window exists at `AppState::init` time, so creation is deferred to
    /// `attach_smtc`, called from the event loop once the Slint window is shown. Until then — and
    /// wherever init fails — the handle is an inert no-op.
    pub(super) fn new(tx: mpsc::Sender<PlayerEvent>) -> Self {
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

        Self {
            inner: std::sync::Mutex::new(MediaControlsInner {
                controls,
                #[cfg(target_os = "windows")]
                event_tx: tx,
                published: Published::default(),
                last_timeline_push: None,
            }),
        }
    }
}

/// Build souvlaki controls and attach the event callback, folding any failure
/// into `None` with a warning. A `None` result leaves the handle an inert no-op.
fn try_create_controls(
    hwnd: Option<*mut std::ffi::c_void>,
    tx: mpsc::Sender<PlayerEvent>,
) -> Option<MediaControls> {
    match create_controls(hwnd, tx) {
        Ok(controls) => Some(controls),
        Err(e) => {
            log::warn!(
                "Failed to initialize OS media controls: {}",
                melodia_core::error::describe(&e)
            );
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
    tx: mpsc::Sender<PlayerEvent>,
) -> Result<MediaControls, souvlaki::Error> {
    let config = PlatformConfig { dbus_name: "melodia", display_name: "Melodia", hwnd };

    let mut controls = MediaControls::new(config)?;

    controls.attach(move |event: MediaControlEvent| {
        if let Some(event) = translate_event(event) {
            forward(&tx, event);
        }
    })?;

    Ok(controls)
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
        MediaControlEvent::SetVolume(vol) => Some(PlayerEvent::SetVolume(volume_percent(vol))),
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
            log::debug!("Media controls OpenUri requested (ignored): {uri}");
            None
        }
    }
}

impl MediaControlsSync for MediaControlsHandle {
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus) {
        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let inner = &mut *guard;

        let changes = inner.published.changes(vm, status);
        let Some(controls) = inner.controls.as_mut() else {
            return;
        };

        if changes.metadata {
            let source = vm.source();
            if let Some(source) = source.as_ref() {
                let artwork_url = source.artwork_path.map(cover_url);
                if let Err(e) = controls.set_metadata(MediaMetadata {
                    title: Some(source.title),
                    artist: source.secondary,
                    album: source.album,
                    duration: source.duration_ms.map(Duration::from_millis),
                    cover_url: artwork_url.as_deref(),
                }) {
                    log::debug!("Failed to set media metadata: {e}");
                }
            } else if let Err(e) = controls.set_metadata(MediaMetadata::default()) {
                log::debug!("Failed to clear media metadata: {e}");
            }
        }

        if changes.status || changes.metadata || changes.position {
            let progress = Some(MediaPosition(Duration::from_millis(vm.position_ms)));
            let playback = match status {
                PlaybackStatus::Playing => MediaPlayback::Playing { progress },
                PlaybackStatus::Paused => MediaPlayback::Paused { progress },
                PlaybackStatus::Stopped | PlaybackStatus::Loading => MediaPlayback::Stopped,
            };
            if let Err(e) = controls.set_playback(playback) {
                log::debug!("Failed to set media playback status: {e}");
            }
            inner.last_timeline_push = Some(Instant::now());
        }

        inner.published.record(vm, status, changes);
    }

    fn update_position(&self, position_ms: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let inner = &mut *guard;
        let Some(controls) = inner.controls.as_mut() else {
            return;
        };
        if inner.last_timeline_push.is_some_and(|at| at.elapsed() < TIMELINE_REFRESH) {
            return;
        }
        let progress = Some(MediaPosition(Duration::from_millis(position_ms)));
        if let Err(e) = controls.set_playback(MediaPlayback::Playing { progress }) {
            log::debug!("Failed to update media position: {e}");
        }
        inner.last_timeline_push = Some(Instant::now());
    }
}

#[cfg(test)]
#[path = "tests/souvlaki_backend_tests.rs"]
mod tests;
