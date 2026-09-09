//! Wire the Settings page's two lyrics switches to Rust.
//!
//! Each seeds its Slint property off the shadow and registers the change callback through
//! [`shadow_toggle`], which moves the shadow before it spawns the write: `library::lyrics` reads
//! the online one on a worker on every track change, and the Now Playing menu carries both
//! switches itself, so a change landing between the click and the disk write has to see the new
//! answer rather than the file's old one. What is left here is the mirror each row owes the
//! menu's copy of it.
//!
//! The lookup is off by default, which is what the shipped package description promises of every
//! online feature. Romanization is on: it reaches nothing, and it draws nothing at all unless the
//! sheet is in a script the reader cannot sound out.

use slint::ComponentHandle;

use crate::ui::settings_bind::shadow_toggle;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Lyrics, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // Seeded off the shadow rather than off `settings.json`, as `radio_settings` does: `AppState`
    // already read the file at boot, and a second read here would answer the same question twice.
    ui.global::<Settings>().set_lyrics_online_enabled(state.lyrics_online_enabled.get());

    {
        let persist = shadow_toggle(
            state,
            &state.lyrics_online_enabled,
            "set_lyrics_online_enabled",
            library::settings::set_lyrics_online_enabled,
        );
        let weak = ui.as_weak();
        ui.global::<Settings>().on_lyrics_online_enabled_changed(move |on| {
            persist(on);
            // The Now Playing menu carries the same switch and reads the `Lyrics` global, seeded
            // once at boot. Without this it spends the session showing what the flag was at launch.
            //
            // **And no re-ask, unlike that menu's own half**: reaching this page closed Now
            // Playing, which hands the sheet and its claim back, so the re-open looks up again.
            if let Some(ui) = weak.upgrade() {
                ui.global::<Lyrics>().set_online_enabled(on);
            }
        });
    }

    ui.global::<Settings>()
        .set_lyrics_romanization_shown(state.lyrics_romanization_shown.get());
    {
        let persist = shadow_toggle(
            state,
            &state.lyrics_romanization_shown,
            "set_lyrics_romanization_shown",
            library::settings::set_lyrics_romanization_shown,
        );
        let weak = ui.as_weak();
        ui.global::<Settings>().on_lyrics_romanization_shown_changed(move |shown| {
            persist(shown);
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
