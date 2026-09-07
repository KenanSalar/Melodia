//! Wire the Settings page's online-lyrics switch to Rust.
//!
//! Seeds `Settings.lyrics-online-enabled` off the shadow and registers the change callback. The
//! shadow moves **first** and synchronously: `library::lyrics` reads it on a worker on every track
//! change, so a change landing between the click and the disk write has to see the new answer
//! rather than the file's old one.
//!
//! Off by default, which is what the shipped package description promises of every online feature.

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // Seeded off the shadow rather than off `settings.json`, as `radio_settings` does: `AppState`
    // already read the file at boot, and a second read here would answer the same question twice.
    ui.global::<Settings>().set_lyrics_online_enabled(state.lyrics_online_enabled.get());

    let state = state.clone();
    ui.global::<Settings>().on_lyrics_online_enabled_changed(move |on| {
        state.lyrics_online_enabled.set(on);
        state.persist_blocking("set_lyrics_online_enabled", move |st| {
            library::settings::set_lyrics_online_enabled(st, on)
        });
    });
}
