//! The lyrics feature's three switches, and the one place each is answered.
//!
//! Each has a Settings row **and** a Now Playing control, and neither view re-reads the other's
//! global after boot, so whichever is used writes its twin ([`mirrored`]), over
//! [`shadow_toggle`]'s shadow-before-persist order: `library::lyrics` reads the feature and
//! online switches on a worker every track change, and a lookup between the click and the disk
//! write has to see the new answer.
//!
//! Lyrics and the lookup are off by default, the second as the package description promises of
//! every online feature. Romanization is on: it reaches nothing, and draws nothing unless the
//! sheet is in a script the reader cannot sound out.

use slint::ComponentHandle;

use crate::ui::settings_bind::shadow_toggle;
use melodia_app::library;
use melodia_app::state::{AppState, SharedFlag};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Lyrics, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();
    // Off the shadows `AppState` filled at boot, as `radio_settings` seeds, not a second file read.
    g.set_lyrics_enabled(state.lyrics_enabled.get());
    g.set_lyrics_online_enabled(state.lyrics_online_enabled.get());
    g.set_lyrics_romanization_shown(state.lyrics_romanization_shown.get());

    g.on_lyrics_enabled_changed(enabled_handler(ui, state));
    g.on_lyrics_online_enabled_changed(online_handler(ui, state));
    g.on_lyrics_romanization_shown_changed(romanization_handler(ui, state));
}

/// The handler both controls of the feature switch register.
///
/// The card's row owes Now Playing nothing, for [`online_handler`]'s reason: reaching this page
/// closed that view, and its close already handed the sheet back.
pub(crate) fn enabled_handler(ui: &AppWindow, state: &AppState) -> impl Fn(bool) + 'static {
    mirrored(
        ui,
        state,
        &state.lyrics_enabled,
        "set_lyrics_enabled",
        library::settings::set_lyrics_enabled,
        |ui, on| {
            ui.global::<Settings>().set_lyrics_enabled(on);
            ui.global::<Lyrics>().set_enabled(on);
        },
    )
}

/// The handler both rows of the online-lookup switch register.
///
/// **The card's row owes no re-ask where the menu's does**: reaching this page closed Now Playing,
/// which hands the sheet and its claim back, so the re-open looks up again on its own.
pub(crate) fn online_handler(ui: &AppWindow, state: &AppState) -> impl Fn(bool) + 'static {
    mirrored(
        ui,
        state,
        &state.lyrics_online_enabled,
        "set_lyrics_online_enabled",
        library::settings::set_lyrics_online_enabled,
        |ui, on| {
            ui.global::<Settings>().set_lyrics_online_enabled(on);
            ui.global::<Lyrics>().set_online_enabled(on);
        },
    )
}

/// The handler both rows of the romanization switch register.
///
/// The two views are never on screen together, so nothing has to be redrawn for the card's row;
/// the menu's wraps this to re-lay the sheet it is standing on.
pub(crate) fn romanization_handler(ui: &AppWindow, state: &AppState) -> impl Fn(bool) + 'static {
    mirrored(
        ui,
        state,
        &state.lyrics_romanization_shown,
        "set_lyrics_romanization_shown",
        library::settings::set_lyrics_romanization_shown,
        |ui, shown| {
            ui.global::<Settings>().set_lyrics_romanization_shown(shown);
            ui.global::<Lyrics>().set_romanization_shown(shown);
        },
    )
}

/// [`shadow_toggle`] plus the write keeping the switch's other row in step. `mirror` writes
/// **both** globals, so one handler serves either row; a global written from its own change
/// handler is a value-compared no-op, and a `ToggleSwitch`'s `<=>` survives a write from Rust.
fn mirrored(
    ui: &AppWindow,
    state: &AppState,
    shadow: &SharedFlag,
    label: &'static str,
    persist: fn(&Paths, bool) -> Result<(), AppError>,
    mirror: fn(&AppWindow, bool),
) -> impl Fn(bool) + 'static {
    let persist = shadow_toggle(state, shadow, label, persist);
    let weak = ui.as_weak();
    move |on| {
        persist(on);
        if let Some(ui) = weak.upgrade() {
            mirror(&ui, on);
        }
    }
}
