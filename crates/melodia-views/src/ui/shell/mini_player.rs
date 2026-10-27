//! The `MiniPlayer` global: Up Next's visibility gate, the large artwork's and the lyrics sheet's
//! lifecycle, the backdrop switch and the height a stopped resize settles at.
//!
//! The miniplayer is a window size, engaged when the window shrinks past the threshold
//! `app-window.slint` derives; its Full Player caption only resizes the window. Every artwork arm
//! moves the mirror `NowPlayingState` keeps and then follows
//! [`NowPlayingState::renders_artwork`], so no caller learns which surface answered.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::now_playing::{NowPlayingState, release_lyrics};
use melodia_app::state::AppState;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, MiniPlayer};

/// Hydrate `np_state.mini_layout` from the live global, because `layout-changed` only fires on
/// actual changes — a window already in the column before entering mini would leave the mirror
/// on the strip and `kick_artwork()` skipped on entry.
fn sync_mini_layout(weak: &slint::Weak<AppWindow>, np_state: &NowPlayingState) {
    let Some(ui) = weak.upgrade() else { return };
    np_state.mini_layout.set(ui.global::<MiniPlayer>().get_layout());
}

/// Seeds the large artwork if anything on screen still draws from it, and hands it back otherwise.
fn follow_artwork(np_state: &NowPlayingState) {
    if np_state.renders_artwork() {
        np_state.kick_artwork();
    } else {
        np_state.release_artwork();
    }
}

/// Runs between `AppWindow::new()` and `app.show()`.
pub fn install(
    app: &AppWindow,
    state: &AppState,
    np_state: &Rc<NowPlayingState>,
) -> Result<(), AppError> {
    let mini = app.global::<MiniPlayer>();

    // On exit the artwork and the sheet go back unless Now Playing draws them, no track change
    // coming to hand the sheet over.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let weak = app.as_weak();
        mini.on_active_changed(move |is_active| {
            let was_visible = np_state.mini_visible.replace(is_active);
            if is_active {
                crate::ui::window_chrome::geometry::hold_full_player();
                sync_mini_layout(&weak, &np_state);
                np_state.kick_up_next();
                if np_state.renders_artwork() {
                    np_state.kick_artwork();
                }
                // Outside the artwork's guard: entering force-closes Now Playing, before or after
                // this arm, so the sheet may be gone, and asking costs nothing, `reseed` deduping.
                np_state.kick_lyrics();
            } else {
                crate::ui::window_chrome::geometry::release_full_player();
                // A launch settles through an exit too, with nothing to hand back, and `f` opens Now
                // Playing under the miniplayer, where the swap mounts it with no edge left to reseed
                // what was released.
                if !was_visible {
                    return;
                }
                if !np_state.renders_artwork() {
                    np_state.release_artwork();
                }
                if !np_state.renders_panel()
                    && let Some(ui) = weak.upgrade()
                {
                    release_lyrics(&ui, &state, &np_state);
                }
            }
        });
    }

    // The card and the column draw the high-res cover where the strip takes the row tier's thumb,
    // and the column alone has a slot for the sheet.
    {
        let np_state = np_state.clone();
        mini.on_layout_changed(move |layout| {
            np_state.mini_layout.set(layout);
            if !np_state.mini_visible.get() {
                return;
            }
            follow_artwork(&np_state);
            np_state.kick_lyrics();
        });
    }

    // The strip paints the backdrop without the large tile, so switching it on is what makes that
    // variant need the decode.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        mini.on_set_backdrop_shown(move |on| {
            np_state.mini_backdrop.set(on);
            if np_state.mini_visible.get() {
                follow_artwork(&np_state);
            }
            state.persist_blocking("mini backdrop", move |paths| {
                melodia_app::library::window::set_mini_backdrop(paths, on)
            });
        });
    }

    // The switch decides the height from a layout Rust can't see, and only Rust can resize the
    // window. It asks from the winit filter's posted release, outside any dispatch.
    {
        let weak = app.as_weak();
        mini.on_request_height(move |height| {
            let Some(ui) = weak.upgrade() else { return };
            let window = ui.window();
            let width = window.size().to_logical(window.scale_factor()).width;
            window.set_size(slint::LogicalSize::new(width, height));
        });
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/mini_player_tests.rs"]
mod tests;
