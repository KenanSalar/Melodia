//! Shared wiring for the audio settings: the seed each installer reads at boot,
//! and the change-handler each toggle installs.
//!
//! Every audio toggle in Settings and in the Now-Playing dialogs is two-phase: apply to
//! the live playback engine *synchronously*, so the sound changes before the callback
//! returns, then persist on the blocking pool. A failed disk write must not undo the
//! applied value — the warn from [`AppState::persist_blocking`] is the only report.

use crate::error::AppError;
use crate::services::settings::{self, SettingsData};
use crate::state::{AppState, PlaybackContext};

/// Persisted settings for an installer to seed its global from, falling back to the inert
/// defaults if the file is missing or unreadable. Deriving that fallback from
/// `SettingsData::default()` keeps the seed computed one way either side of the error arm,
/// which would otherwise be a second copy of the `Default` impl.
pub fn read_or_default(state: &AppState, what: &str) -> SettingsData {
    settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("read settings for {what}: {e}");
        SettingsData::default()
    })
}

/// The change-handler for a boolean audio setting: apply to the backend, then persist.
/// `apply` is one of the infallible `library::playback::player_set_*` helpers — EQ,
/// `ReplayGain` and crossfade state living on the backend's lock-free cells rather than
/// the `PlayerState` machine — and `persist` its `library::settings::set_*` sibling.
pub fn toggle_binding(
    state: &AppState,
    label: &'static str,
    apply: fn(&PlaybackContext, bool),
    persist: fn(&AppState, bool) -> Result<(), AppError>,
) -> impl FnMut(bool) + 'static {
    let state = state.clone();
    move |on| {
        apply(&state.playback_ctx(), on);
        state.persist_blocking(label, move |s| persist(s, on));
    }
}
