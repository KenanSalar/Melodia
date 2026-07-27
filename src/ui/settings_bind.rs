//! Shared wiring for the audio settings: the seed each installer reads at boot,
//! and the change-handler each toggle installs.
//!
//! Every audio toggle in Settings and in the Now-Playing dialogs follows the same
//! two-phase shape: apply the new value to the live Rodio backend *synchronously*
//! (so the sound changes before the callback returns), then persist it on the
//! blocking pool (`mutate_settings` is a synchronous file rewrite we don't want
//! on the UI thread). A failed disk write must not undo the applied value — the
//! warn from [`AppState::persist_blocking`] is the only report.
//!
//! That shape was open-coded once per toggle across the crossfade, `ReplayGain`
//! and equalizer installers, and the disk read each of those opens with was
//! open-coded once per installer. These are the single copies of both.

use crate::error::AppError;
use crate::services::settings::{self, SettingsData};
use crate::state::{AppState, PlaybackContext};

/// Persisted settings for an installer to seed its global from, falling back to
/// the inert defaults if the file is missing or unreadable.
///
/// Every audio installer opened with the same `match` over
/// [`settings::read_settings`], each spelling out its own defaults in the error
/// arm — which is both a copy of the `Default` impl and a second place for it to
/// drift. Deriving from `SettingsData::default()` instead means the seed is
/// computed one way whether the file loaded or not. `what` names the read in the
/// log line.
pub fn read_or_default(state: &AppState, what: &str) -> SettingsData {
    settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("read settings for {what}: {e}");
        SettingsData::default()
    })
}

/// Build the change-handler for a boolean audio setting: apply to the backend,
/// then persist.
///
/// `apply` is one of the infallible `library::playback::player_set_*` helpers
/// (EQ / `ReplayGain` / crossfade state lives on the backend's lock-free cells,
/// not the `PlayerState` machine), and `persist` its `library::settings::set_*`
/// sibling. `label` names the write in the log line if it fails.
///
/// ```ignore
/// g.on_crossfade_manual_changed(toggle_binding(
///     state,
///     "persist crossfade_manual",
///     library::playback::player_set_crossfade_manual,
///     library::settings::set_crossfade_manual,
/// ));
/// ```
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
