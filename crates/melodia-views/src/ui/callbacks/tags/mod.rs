//! The Edit Track Information dialog's callbacks. The `TagEditor` global
//! (`crates/melodia-ui/ui/globals/dialog-forms.slint`) is Rust-owned throughout, and numbers cross
//! the boundary as strings, so all parse and validation lives in [`commit`].
//!
//! **Every async handler is `slint::spawn_local`, not `runtime.spawn`**: the per-open session is an
//! `Rc<RefCell<_>>` and the completion toast needs the `Rc<NotificationsUi>`, both `!Send`.
//! `async_compat::Compat` supplies the tokio reactor for what they await.

mod artwork;
mod commit;
mod credits;
mod fold;
mod form;
mod lists;
mod open;
mod session;
mod toast;

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};

use crate::ui::shell::notifications::NotificationsUi;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, TagEditor};

use session::TagSession;

/// Wire every `TagEditor` callback, from boot's `install_ui` once the notifications stack the Save
/// toast needs exists.
pub fn wire_tags(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let session: Rc<RefCell<TagSession>> = Rc::new(RefCell::new(TagSession::default()));
    let te = ui.global::<TagEditor>();

    open::wire_request_edit(&te, ui, state, &session);
    credits::wire_credits(&te, ui, &session);
    lists::wire_genres(&te, ui);
    lists::wire_roles(&te, ui);
    wire_insert_resident_lyrics(&te, ui, &session);
    artwork::wire_pick_artwork(&te, ui, state, &session);
    artwork::wire_remove_artwork(&te, ui, &session);
    commit::wire_commit(&te, ui, state, &session, notifications);
}

/// `insert-resident-lyrics`: fill the field with the sheet this track already has, and stop
/// there. The write is the dialog's own Save, so the sheet diffs against the baseline like any
/// field and the same Cancel undoes it.
fn wire_insert_resident_lyrics(te: &TagEditor, ui: &AppWindow, session: &Rc<RefCell<TagSession>>) {
    let weak = ui.as_weak();
    let session = session.clone();
    te.on_insert_resident_lyrics(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(text) = session.borrow().resident_lyrics.clone() else {
            return;
        };
        ui.global::<TagEditor>().set_lyrics(SharedString::from(text));
    });
}
