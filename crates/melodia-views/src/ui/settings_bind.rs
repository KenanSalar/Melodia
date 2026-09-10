//! Shared wiring for the settings toggles: the seed each installer reads at boot, and the
//! change-handler each toggle installs.
//!
//! **Every toggle here is two-phase, and the first phase is synchronous.** [`toggle_binding`]
//! applies to the live playback engine, so the sound changes before the callback returns;
//! [`shadow_toggle`] moves a [`SharedFlag`], so a worker reading it mid-write sees the new answer.
//! Both then persist on the blocking pool, and a failed disk write must not undo what was already
//! applied: the warn from [`AppState::persist_blocking`] is the only report.

use melodia_app::services::settings::{self, SettingsData};
use melodia_app::state::{AppState, PlaybackContext, SharedFlag};
use melodia_core::error::AppError;

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

/// The change-handler for a boolean setting a worker also reads: move the shadow, then persist.
///
/// **The order is the whole of it.** A task firing between the two has to see the new answer
/// rather than the one still on disk, which is what [`SharedFlag`] is for and what its own doc
/// asks of every writer. Spelled once here so no call site can get it the other way round, and so
/// a handler with an extra effect of its own has only that effect left to spell.
pub fn shadow_toggle(
    state: &AppState,
    shadow: &SharedFlag,
    label: &'static str,
    persist: fn(&AppState, bool) -> Result<(), AppError>,
) -> impl Fn(bool) + 'static {
    let state = state.clone();
    let shadow = shadow.clone();
    move |on| {
        shadow.set(on);
        state.persist_blocking(label, move |s| persist(s, on));
    }
}
