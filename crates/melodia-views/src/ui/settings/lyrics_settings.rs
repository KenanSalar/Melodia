//! Wire the Settings page's two lyrics switches to Rust.
//!
//! Each seeds its Slint property off the shadow and registers the change callback. The shadow
//! moves **first** and synchronously: `library::lyrics` reads the online one on a worker on every
//! track change, and the Now Playing menu reads the romanization one to seed its own row, so a
//! change landing between the click and the disk write has to see the new answer rather than the
//! file's old one.
//!
//! The lookup is off by default, which is what the shipped package description promises of every
//! online feature. Romanization is on: it reaches nothing, and it draws nothing at all unless the
//! sheet is in a script the reader cannot sound out.

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Lyrics, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // Seeded off the shadow rather than off `settings.json`, as `radio_settings` does: `AppState`
    // already read the file at boot, and a second read here would answer the same question twice.
    ui.global::<Settings>().set_lyrics_online_enabled(state.lyrics_online_enabled.get());

    {
        let state = state.clone();
        ui.global::<Settings>().on_lyrics_online_enabled_changed(move |on| {
            state.lyrics_online_enabled.set(on);
            state.persist_blocking("set_lyrics_online_enabled", move |st| {
                library::settings::set_lyrics_online_enabled(st, on)
            });
        });
    }

    ui.global::<Settings>()
        .set_lyrics_romanization_shown(state.lyrics_romanization_shown.get());
    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.global::<Settings>().on_lyrics_romanization_shown_changed(move |shown| {
            state.lyrics_romanization_shown.set(shown);
            state.persist_blocking("set_lyrics_romanization_shown", move |st| {
                library::settings::set_lyrics_romanization_shown(st, shown)
            });
            // **The Now Playing menu row reads the `Lyrics` global, not this one**, and the panel
            // lays a sheet out against it. The two views are never on screen together, so nothing
            // has to be redrawn here; what this keeps true is that the menu opens showing what
            // Settings last said rather than what it was seeded with at boot.
            if let Some(ui) = weak.upgrade() {
                ui.global::<Lyrics>().set_romanization_shown(shown);
            }
        });
    }
}
