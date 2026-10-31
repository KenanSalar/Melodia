//! `Player.toggle-favorite` and `Player.set-current-rating`, each mirrored into every list's rows
//! through [`RowFlagMirrors`].

use slint::ComponentHandle;

use super::RowFlagMirrors;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Player};

/// Wires `Player.toggle-favorite`, mirroring the result into every list.
pub fn wire_now_playing_favorite(ui: &AppWindow, state: &AppState, mirrors: &RowFlagMirrors) {
    let s = state.clone();
    let m = mirrors.clone();
    ui.global::<Player>().on_toggle_favorite(move || {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::favorites::toggle_current_favorite(&s).await {
                Ok(Some((id, fav))) => m.favorites(&[id], fav),
                Ok(None) => {}
                Err(e) => log::warn!("toggle_favorite: {}", describe(&e)),
            }
        });
    });
}

/// [`wire_now_playing_favorite`] for `Player.set-current-rating`, wired at the same point.
pub fn wire_now_playing_rating(ui: &AppWindow, state: &AppState, mirrors: &RowFlagMirrors) {
    let s = state.clone();
    let m = mirrors.clone();
    ui.global::<Player>().on_set_current_rating(move |rating| {
        let s = s.clone();
        let m = m.clone();
        s.runtime.clone().spawn(async move {
            match library::ratings::set_current_rating(&s, rating).await {
                Ok(Some((id, rating))) => m.rating(id, rating),
                Ok(None) => {}
                Err(e) => log::warn!("set_current_rating: {}", describe(&e)),
            }
        });
    });
}
