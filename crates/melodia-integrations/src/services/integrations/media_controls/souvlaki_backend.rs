//! SMTC on Windows and `MediaPlayer` on macOS, through souvlaki.

use std::time::{Duration, Instant};

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use tokio::sync::mpsc;

use melodia_core::error::describe;
use melodia_engine::player::engine::event_sink::{MediaControlsSync, PlayerEvent};
use melodia_engine::player::engine::state::{PlayerViewModelLight, amplitude_to_volume};
use melodia_engine::player::engine::types::PlaybackStatus;

use super::published::Published;
use super::{cover_url, forward};

/// How often a playing position is handed over. Both panels advance their own clock between
/// updates, so this only has to keep the two from visibly diverging.
const TIMELINE_REFRESH: Duration = Duration::from_secs(5);

/// `controls` is `None` wherever the OS controls are unavailable, leaving the handle an inert
/// no-op.
pub struct MediaControlsHandle {
    inner: std::sync::Mutex<MediaControlsInner>,
}

struct MediaControlsInner {
    controls: Option<MediaControls>,
    /// Kept for `attach_smtc`, which wires the late controls into the same channel.
    #[cfg(target_os = "windows")]
    event_tx: mpsc::Sender<PlayerEvent>,
    published: Published,
    last_timeline_push: Option<Instant>,
}

impl MediaControlsHandle {
    /// macOS creates the controls eagerly. On Windows souvlaki's SMTC backend panics without a
    /// non-null `HWND`, and no window exists at `AppState::init`, so the controls wait for
    /// `attach_smtc` once the Slint window is shown.
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

fn try_create_controls(
    hwnd: Option<*mut std::ffi::c_void>,
    tx: mpsc::Sender<PlayerEvent>,
) -> Option<MediaControls> {
    match create_controls(hwnd, tx) {
        Ok(controls) => Some(controls),
        Err(e) => {
            log::warn!("Failed to initialize OS media controls: {}", describe(&e));
            None
        }
    }
}

#[cfg(target_os = "windows")]
impl MediaControlsHandle {
    /// `true` only when this call attached the controls, the caller's cue to re-sync: nothing
    /// reached the panel while the handle was inert.
    #[must_use]
    pub fn attach_smtc(&self, hwnd: *mut std::ffi::c_void) -> bool {
        let tx = {
            let guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.controls.is_some() {
                return false;
            }
            guard.event_tx.clone()
        };

        // Off the lock: COM and SMTC setup shouldn't hold up `sync()` from playback threads.
        let Some(controls) = try_create_controls(Some(hwnd), tx) else {
            return false;
        };

        let mut guard = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.controls.is_some() {
            // Lost a race with another `attach_smtc`.
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
        MediaControlEvent::SetVolume(vol) => Some(PlayerEvent::SetVolume(amplitude_to_volume(vol))),
        // Relative seeks need the current position, which no `PlayerEvent` resolves.
        MediaControlEvent::Seek(_) | MediaControlEvent::SeekBy(_, _) => {
            log::debug!("Relative seek media event ignored (needs library API support)");
            None
        }
        MediaControlEvent::Raise | MediaControlEvent::Quit => {
            // Window control needs a Slint handle this layer deliberately doesn't hold.
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
                    log::debug!("Failed to set media metadata: {}", describe(&e));
                }
            } else if let Err(e) = controls.set_metadata(MediaMetadata::default()) {
                log::debug!("Failed to clear media metadata: {}", describe(&e));
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
                log::debug!("Failed to set media playback status: {}", describe(&e));
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
            log::debug!("Failed to update media position: {}", describe(&e));
        }
        inner.last_timeline_push = Some(Instant::now());
    }
}

#[cfg(test)]
#[path = "tests/souvlaki_backend_tests.rs"]
mod tests;
