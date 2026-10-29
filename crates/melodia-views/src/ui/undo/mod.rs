//! The one undo slot. A removal writes at once and offers what puts it back; the slot shows that
//! as a notice with an Undo button, which Ctrl+Z also reaches while it is up.
//!
//! One slot rather than a stack: a run of the same removal merges into the notice already up,
//! and anything else replaces it, the earlier removal standing. The hold goes with its notice,
//! however that leaves.

mod removal;

use std::cell::RefCell;
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::ui::shell::notifications::{LocalizedPush, NotificationsUi, RowText};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, NotificationKind, NotificationVariant, Notifications, UndoText};

use removal::Absorbed;
pub use removal::{Removal, playlists_deleted, tracks_removed_from};

/// How long the notice waits for a press. It runs only while nobody is reading it: hover and a
/// window in the background both hold the clock.
pub const UNDO_NOTICE_MS: u32 = 6_000;

/// The state and the window are handed to each restore, so a hold carries only what it took.
pub struct UndoWindow {
    notifications: Rc<NotificationsUi>,
    state: AppState,
    ui: slint::Weak<AppWindow>,
    slot: RefCell<Option<Slot>>,
}

struct Slot {
    row: i32,
    removal: Removal,
}

/// How an offer lands, decided with the slot borrowed and acted on once it isn't.
enum Landing {
    Merged { row: i32, recipe: removal::Recipe },
    Fresh(Removal),
    OverCap(removal::Title),
}

impl UndoWindow {
    /// Raises the notice for a removal that has already been written. **No slot borrow is held
    /// across a `NotificationsUi` call**, any of which can remove a row and call back in.
    pub fn offer(&self, removal: Removal) {
        let Some(ui) = self.ui.upgrade() else { return };
        let landing = match self.slot.borrow_mut().as_mut() {
            Some(slot) => match slot.removal.absorb(removal) {
                Absorbed::Merged => {
                    Landing::Merged { row: slot.row, recipe: slot.removal.recipe() }
                }
                Absorbed::Distinct(removal) => Landing::Fresh(removal),
                Absorbed::OverCap(title) => Landing::OverCap(title),
            },
            None => Landing::Fresh(removal),
        };

        match landing {
            Landing::Merged { row, recipe } => {
                self.notifications.relabel(&ui, row, recipe);
                self.notifications.restart(row);
            }
            Landing::OverCap(title) => self.notify_over_cap(title),
            Landing::Fresh(removal) => {
                self.replace_held();
                let push = LocalizedPush {
                    variant: NotificationVariant::Info,
                    kind: NotificationKind::Undo,
                    timeout_ms: UNDO_NOTICE_MS,
                };
                let row = self.notifications.show_localized_timed(&ui, push, removal.recipe());
                *self.slot.borrow_mut() = Some(Slot { row, removal });
                ui.global::<Notifications>().set_undo_row(row);
            }
        }
    }

    /// Raises the notice for a removal, or a run of them, too large to hold: what went, that it
    /// can't be undone, and no button. It still replaces the held one, so the card up is always the
    /// latest removal and Ctrl+Z never reaches past it to an older one.
    pub fn notify_over_cap(&self, title: impl Fn(&AppWindow) -> SharedString + 'static) {
        let Some(ui) = self.ui.upgrade() else { return };
        self.replace_held();
        let push = LocalizedPush {
            variant: NotificationVariant::Info,
            kind: NotificationKind::None,
            timeout_ms: UNDO_NOTICE_MS,
        };
        self.notifications.show_localized_timed(&ui, push, move |ui| {
            RowText::plain(title(ui), ui.global::<UndoText>().invoke_too_many_to_undo())
        });
    }

    /// The card's button and Ctrl+Z both arrive here. Only the slot's own row acts, so a press
    /// racing a replace does nothing. The restore runs on this thread, where a failure can raise
    /// its notice.
    fn undo(&self, row: i32) {
        if !self.holds(row) {
            return;
        }
        let Some(slot) = self.release() else { return };
        self.notifications.dismiss(slot.row);

        let (state, ui) = (self.state.clone(), self.ui.clone());
        let notifications = self.notifications.clone();
        let restoring = slint::spawn_local(Compat::new(async move {
            let Err(e) = slot.removal.restore(&state, &ui).await else { return };
            log::warn!("undo: {}", describe(&e));
            if let Some(ui) = ui.upgrade() {
                notifications.show_failure(&ui, |ui| {
                    let text = ui.global::<UndoText>();
                    RowText::plain(text.invoke_couldnt_undo(), text.invoke_couldnt_undo_message())
                });
            }
        }));
        if let Err(e) = restoring {
            log::warn!("undo spawn_local: {}", describe(&e));
        }
    }

    /// Takes the held notice down with its hold, for a removal that replaces it.
    fn replace_held(&self) {
        if let Some(old) = self.release() {
            self.notifications.dismiss(old.row);
        }
    }

    fn row_removed(&self, row: i32) {
        if self.holds(row) {
            self.release();
        }
    }

    fn holds(&self, row: i32) -> bool {
        self.slot.borrow().as_ref().is_some_and(|slot| slot.row == row)
    }

    /// The one place the slot empties, so Ctrl+Z never names a row that is gone.
    fn release(&self) -> Option<Slot> {
        let slot = self.slot.borrow_mut().take()?;
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<Notifications>().set_undo_row(-1);
        }
        Some(slot)
    }
}

/// Wires the slot to the notice stack and to `Notifications.undo`. Both hold it weakly, the
/// listener living inside the `NotificationsUi` the slot holds.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    notifications: &Rc<NotificationsUi>,
) -> Rc<UndoWindow> {
    let window = Rc::new(UndoWindow {
        notifications: notifications.clone(),
        state: state.clone(),
        ui: ui.as_weak(),
        slot: RefCell::new(None),
    });

    let weak = Rc::downgrade(&window);
    notifications.on_row_removed(move |row| {
        if let Some(window) = weak.upgrade() {
            window.row_removed(row);
        }
    });

    let weak = Rc::downgrade(&window);
    ui.global::<Notifications>().on_undo(move |row| {
        if let Some(window) = weak.upgrade() {
            window.undo(row);
        }
    });

    window
}
