//! Edit-Track-Information dialog callbacks, split by concern:
//!
//! * [`open`] — `request-edit`: fetch the selection, fold it to what it agrees on, fill the
//!   `TagEditor` global and record the baseline the commit diffs against.
//! * [`credits`] — the two artist-credit editors and the phrase picker behind them.
//! * [`lists`] — the genre field and the ten role boxes, which are one editor mounted eleven times.
//! * [`artwork`] — the cover panel and its bounded preview decode.
//! * [`commit`] — the tri-state diff into a `TagEdit`, and handing it to `library::tags`.
//! * [`toast`] — what the user is told afterwards.
//! * [`form`] / [`session`] / [`fold`] — the types the above share: what the form holds, what one
//!   open carries, and the pure folding helpers.
//!
//! The `TagEditor` global (`crates/melodia-ui/ui/globals/dialog-forms.slint`) is Rust-owned
//! throughout, and numbers cross the boundary as strings, so all parse and validation lives in
//! [`commit`].
//!
//! **All the async handlers use `slint::spawn_local` (not `runtime.spawn`)**: the per-open snapshot
//! lives in an `Rc<RefCell<_>>` and the completion toast needs the `Rc<NotificationsUi>` — both
//! `!Send`, so the work must stay on the UI thread. `async_compat::Compat` supplies the tokio
//! reactor for the awaited sqlx / `spawn_blocking` calls, exactly as `ui::playlists::wire_files`
//! does. Wired from `main.rs` after the notifications stack exists, for the same reason.

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

/// Wire every `TagEditor` callback. Needs `Rc<NotificationsUi>` for the Save completion toast, so
/// it is called from `main.rs` after the notifications stack exists (same constraint as
/// `ui::playlists::wire_files`).
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

/// `insert-resident-lyrics`: fill the field with the sheet this track already has.
///
/// **Fills the box and stops there.** The write is the dialog's own Save, so the inserted sheet is
/// a diff against the populate-time baseline like every other field — which is what makes it
/// reviewable, cancellable, and undoable by the same Cancel that covers a mistyped title.
///
/// Here rather than in a module of its own: it reads the session and writes one property, so there
/// is nothing for a file to hold but the handler.
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
