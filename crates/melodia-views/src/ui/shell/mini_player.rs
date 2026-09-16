//! Wires the `MiniPlayer` global to the Up Next subscriber's visibility gate, the artwork cache's
//! lifecycle and the backdrop switch's persistence.
//!
//! **Resize-only trigger.** The miniplayer engages purely on the window being shrunk
//! past the threshold `app-window.slint` derives; there is no entry or exit button
//! anywhere, and the user grows the window again to leave. The backdrop switch is the one thing
//! here a button does move, and it is a repaint rather than a mode.
//!
//! **Every arm below is the same two steps**: move the mirror `NowPlayingState` keeps, then seed
//! or release against what that changes [`NowPlayingState::renders_artwork`] to. Which surface the
//! answer came from is exactly what the callers must not have to know.
//!
//! Sibling to `window_chrome` rather than under it: that module owns OS-frame concerns —
//! the titlebar, the drag region, the restart flow — where this is a responsive *layout*
//! concern, and mixing the two axes makes neither greppable.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::now_playing::NowPlayingState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
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

/// Off-thread release of the [`NowPlayingArtwork`] LRU plus a `malloc_trim` to hand the
/// pages back, mirroring `wire_now_playing_open`'s: the heavy `(cover, blur)` buffers
/// are pinned only while a surface renders them.
fn release_artwork_off_thread(state: &AppState, np_artwork: &Arc<NowPlayingArtwork>) {
    let np = np_artwork.clone();
    state.runtime.spawn_blocking(move || {
        np.clear();
        melodia_platform::services::platform::allocator::trim();
    });
}

/// Wire the callbacks to the Up Next subscriber's `mini_visible` gate and the large artwork's
/// cache lifecycle. Runs on the event-loop thread between `AppWindow::new()` and `app.run()`,
/// the same window as `now_playing` and `window_chrome`.
pub fn install(
    app: &AppWindow,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &Rc<NowPlayingState>,
) -> Result<(), AppError> {
    let mini = app.global::<MiniPlayer>();

    // active-changed: on enter, flip the gates, re-seed Up Next so the column doesn't render an
    // empty list, and seed the high-res cover if anything on screen draws from it. On exit, release
    // the artwork LRU — nothing in full-UI mode needs those buffers.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        let weak = app.as_weak();
        mini.on_active_changed(move |is_active| {
            np_state.mini_visible.set(is_active);
            if is_active {
                crate::ui::window_chrome::geometry::hold_full_player();
                sync_mini_layout(&weak, &np_state);
                np_state.kick_up_next();
                if np_state.renders_artwork() {
                    np_state.kick_artwork();
                }
                // **Beside the artwork kick rather than inside its guard.** Entering force-closes
                // Now Playing, and whether that close ran before or after this arm decides whether
                // the sheet is still there; asking for it either way costs nothing, `reseed`
                // deduping on the claim it already holds.
                np_state.kick_lyrics();
            } else {
                crate::ui::window_chrome::geometry::release_full_player();
                release_artwork_off_thread(&state, &np_artwork);
            }
        });
    }

    // layout-changed: the card and the column draw the high-res cover where the strip takes the row
    // tier's thumb — so seed on the way in, and on the way out release only if the backdrop isn't
    // still solving its colours off that same decode. The sheet is asked too, the column being the
    // one layout with a slot for it: a crossing into it would otherwise mount an empty sheet until
    // the next track, and one out of it hands the sheet back.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        mini.on_layout_changed(move |layout| {
            np_state.mini_layout.set(layout);
            if !np_state.mini_visible.get() {
                return;
            }
            if np_state.renders_artwork() {
                np_state.kick_artwork();
            } else {
                release_artwork_off_thread(&state, &np_artwork);
            }
            np_state.kick_lyrics();
        });
    }

    // set-backdrop-shown: the button has already written the property, so this moves the mirror
    // and answers what that changed. **The strip is why there is anything to do here**: it paints
    // the backdrop without drawing the large tile, so switching one on is the only thing that
    // makes that variant need the decode at all.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        mini.on_set_backdrop_shown(move |on| {
            np_state.mini_backdrop.set(on);
            if np_state.mini_visible.get() {
                if np_state.renders_artwork() {
                    np_state.kick_artwork();
                } else {
                    release_artwork_off_thread(&state, &np_artwork);
                }
            }
            state.persist_blocking("mini backdrop", move |s| {
                melodia_app::library::window::set_mini_backdrop(s, on)
            });
        });
    }

    Ok(())
}
