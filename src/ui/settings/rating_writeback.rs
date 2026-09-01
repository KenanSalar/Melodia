//! Wire the Settings page's "Save ratings to files" toggle to Rust.
//!
//! Seeds `Settings.write-ratings-to-tags` from `settings.json` and registers the change
//! callback. The shadow on [`AppState`] moves **first** and synchronously — `tasks::rating_writeback`
//! reads it on a tokio worker at the end of every quiet period, so a flush landing between the
//! click and the disk write has to see the new answer, not the old one.
//!
//! The default is ON, enforced by `LibraryFlags::default()`: a star that lives only in the
//! database is one a library rebuild loses and no other player can see.

use slint::ComponentHandle;

use crate::library;
use crate::services::settings;
use crate::state::AppState;
use crate::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // A missing or unreadable file leaves the Slint default in place, matching the
    // first-launch path.
    if let Ok(s) = settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_write_ratings_to_tags(s.library.write_ratings_to_tags);
    }

    let state = state.clone();
    ui.global::<Settings>().on_write_ratings_to_tags_changed(move |on| {
        state.set_write_ratings_to_tags(on);
        state.persist_blocking("set_write_ratings_to_tags", move |st| {
            library::settings::set_write_ratings_to_tags(st, on)
        });
    });
}
