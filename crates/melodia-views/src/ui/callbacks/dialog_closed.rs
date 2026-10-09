//! THE `Dialog.closed` handler — there is exactly one, and there must stay
//! exactly one. `on_closed` is `Callback::set_handler`, which has a single slot:
//! a second registration anywhere would silently clobber this one (and a default
//! `closed => { … }` body in `globals/dialog.slint` would be clobbered BY it,
//! which is precisely the leak this shape replaced). A new dialog kind that pins
//! an `image` extends this handler; it does not add another.

use slint::{ComponentHandle, Image};

use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Dialog, TagEditor};

/// Two halves, fired once the close animation completes:
///
///   1. `invoke_closed_teardown()` — the Slint-side `public function`
///      that resets every scalar / list / chrome property (`kind` /
///      `target-id` / `input-text*` / `mosaic-*` / `pending-track-ids`
///      / the two picker row models / `title` / `message` / labels /
///      `destructive`). A `public function` has no handler slot, so it
///      cannot be registered away the way a callback body can.
///   2. `current-artwork` — the one `image`-typed property, which has
///      no Slint default literal and so can only be reset from Rust.
///      This is the `SharedPixelBuffer` Arc the dialog opened with, the
///      shared grid tier's or the detail hero's; dropping it here
///      releases it on the same tick the body branch unmounts.
///
/// Pair the Arc drop with an off-thread `allocator::trim()` (parity with
/// `release_detail_artwork`) so glibc returns the freed pages instead of
/// holding them in the arena. Trim must stay off the UI thread — it
/// walks arena free lists.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Dialog>().on_closed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        // Before the teardown, which clears `kind`. Opens reach no Rust
        // seam, so this is the only trace a dialog leaves — which one, not
        // whether it was accepted, the accept dispatcher being pure Slint.
        log::debug!("dialog closed: {:?}", dlg.get_kind());
        dlg.invoke_closed_teardown();
        dlg.set_current_artwork(Image::default());
        // The Edit-Tags dialog pins a decoded cover in `TagEditor.cover`
        // (another `image`-typed property with no Slint default literal);
        // release it here — this is the one `on_closed`, extended not
        // duplicated.
        ui.global::<TagEditor>().set_cover(Image::default());
        s.runtime.spawn_blocking(melodia_platform::services::platform::allocator::trim);
    });
}
