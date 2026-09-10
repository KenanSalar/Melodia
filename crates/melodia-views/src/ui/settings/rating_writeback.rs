//! Wire the Settings page's "Save ratings to files" toggle to Rust.
//!
//! Seeds `Settings.write-ratings-to-tags` from `settings.json` and registers the change callback
//! through [`shadow_toggle`], which moves the shadow before it spawns the write:
//! `tasks::rating_writeback` reads it on a tokio worker at the end of every quiet period, so a
//! flush landing between the click and the disk write has to see the new answer, not the old one.
//!
//! The default is ON, enforced by `LibraryFlags::default()`: a star that lives only in the
//! database is one a library rebuild loses and no other player can see.

use slint::ComponentHandle;

use crate::ui::settings_bind::shadow_toggle;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // Seeded off the shadow rather than off `settings.json`, as `radio_settings` does: `AppState`
    // already read the file at boot, and a second read here would answer the same question twice.
    ui.global::<Settings>().set_write_ratings_to_tags(state.write_ratings_to_tags.get());

    let persist = shadow_toggle(
        state,
        &state.write_ratings_to_tags,
        "set_write_ratings_to_tags",
        library::settings::set_write_ratings_to_tags,
    );
    ui.global::<Settings>().on_write_ratings_to_tags_changed(persist);
}
