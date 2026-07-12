//! Shared wiring for the audio-settings toggles.
//!
//! Every audio toggle in Settings and in the Now-Playing dialogs follows the same
//! two-phase shape: apply the new value to the live Rodio backend *synchronously*
//! (so the sound changes before the callback returns), then persist it on the
//! blocking pool (`mutate_settings` is a synchronous file rewrite we don't want
//! on the UI thread). A failed disk write must not undo the applied value — the
//! warn from [`AppState::persist_blocking`] is the only report.
//!
//! That shape was open-coded once per toggle across the crossfade, `ReplayGain`
//! and equalizer installers. This is the single copy.

use crate::error::AppError;
use crate::state::{AppState, PlaybackContext};

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
