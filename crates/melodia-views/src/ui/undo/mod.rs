//! The one undo slot. A removal writes at once and offers what puts it back; the slot shows that
//! as a notice with an Undo button, which Ctrl+Z also reaches while it is up.
//!
//! One slot rather than a stack: a run of the same removal merges into the notice already up,
//! and anything else replaces it, the earlier removal standing. The hold goes with its notice,
//! however that leaves.

mod removal;

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::shell::notifications::{LocalizedPush, NotificationsUi};
use melodia_ui::{AppWindow, NotificationKind, NotificationVariant, Notifications};

pub use removal::Removal;

/// Material's long duration, its value for a snackbar carrying an action. It has to outlast every
/// other timed toast, the exclusive-output refusal's button included.
pub const UNDO_NOTICE_MS: u32 = 10_000;

pub struct UndoWindow {
    notifications: Rc<NotificationsUi>,
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
}

impl UndoWindow {
    /// Raises the notice for a removal that has already been written. **No slot borrow is held
    /// across a `NotificationsUi` call**, any of which can remove a row and call back in.
    pub fn offer(&self, removal: Removal) {
        let Some(ui) = self.ui.upgrade() else { return };
        let landing = match self.slot.borrow_mut().as_mut() {
            Some(slot) => match slot.removal.absorb(removal) {
                None => Landing::Merged { row: slot.row, recipe: slot.removal.recipe() },
                Some(removal) => Landing::Fresh(removal),
            },
            None => Landing::Fresh(removal),
        };

        match landing {
            Landing::Merged { row, recipe } => {
                self.notifications.relabel(&ui, row, recipe);
                self.notifications.restart(row);
            }
            Landing::Fresh(removal) => {
                if let Some(old) = self.release() {
                    self.notifications.dismiss(old.row);
                }
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

    /// The card's button and Ctrl+Z both arrive here. Only the slot's own row acts, so a press
    /// racing a replace does nothing.
    fn undo(&self, row: i32) {
        if !self.holds(row) {
            return;
        }
        let Some(slot) = self.release() else { return };
        self.notifications.dismiss(slot.row);
        slot.removal.restore();
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
pub fn install(ui: &AppWindow, notifications: &Rc<NotificationsUi>) -> Rc<UndoWindow> {
    let window = Rc::new(UndoWindow {
        notifications: notifications.clone(),
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
