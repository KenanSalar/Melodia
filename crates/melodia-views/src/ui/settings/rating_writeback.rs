//! The Save Ratings to Files toggle, through [`shadow_toggle`]: `tasks::rating_writeback` reads the
//! shadow on a worker at the end of every quiet period, so a flush between the click and the
//! disk write has to see the new answer.
//!
//! On by default (`LibraryFlags::default()`): a star living only in the database is one a library
//! rebuild loses and no other player can see.

use slint::ComponentHandle;

use crate::ui::settings_bind::shadow_toggle;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState) {
    // Off the shadow `AppState` filled at boot, as `radio_settings` seeds, not a second file read.
    ui.global::<Settings>().set_write_ratings_to_tags(state.write_ratings_to_tags.get());

    let persist = shadow_toggle(
        state,
        &state.write_ratings_to_tags,
        "set_write_ratings_to_tags",
        library::settings::set_write_ratings_to_tags,
    );
    ui.global::<Settings>().on_write_ratings_to_tags_changed(persist);
}
