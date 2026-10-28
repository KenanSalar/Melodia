//! The one `Dialog.closed` handler. `on_closed` has a single handler slot, so a second
//! registration would silently clobber this one, and a `closed => { … }` body in
//! `globals/dialog.slint` would be clobbered by it. A dialog kind that pins an `image` extends
//! this handler rather than adding another.

use slint::{ComponentHandle, Image};

use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Dialog, TagEditor};

/// Fired once the close animation completes. `invoke_closed_teardown()` resets every scalar,
/// list and chrome property, being a `public function` with no handler slot to register away;
/// the `image` properties have no Slint default literal and are reset here, so the buffer the
/// dialog opened with drops on the tick its body unmounts. The trim runs off the UI thread, as it
/// walks the arena's free lists.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Dialog>().on_closed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        // Before the teardown clears `kind`: opens reach no Rust seam, so this is the only trace
        // a dialog leaves.
        log::debug!("dialog closed: {:?}", dlg.get_kind());
        dlg.invoke_closed_teardown();
        dlg.set_current_artwork(Image::default());
        ui.global::<TagEditor>().set_cover(Image::default());
        s.runtime.spawn_blocking(melodia_platform::services::platform::allocator::trim);
    });
}
