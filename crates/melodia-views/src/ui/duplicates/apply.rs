//! Remove: the picks handed to `library::duplicates::apply`, and the toast saying what left.

use std::rc::Rc;

use slint::ComponentHandle;

use super::Finder;
use super::landing::TicketedDialog;
use crate::ui::shell::notifications::{Completion, RowText};
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::library::duplicates::RemovalCounts;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, DuplicatesUi};

impl Finder {
    /// Removes the picked copies, the dialog closing on the answer only if it still shows them.
    pub(super) fn remove(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let dup = ui.global::<DuplicatesUi>();
        if dup.get_removing() {
            return;
        }
        let removals = self.set.borrow().removals();
        if removals.is_empty() {
            return;
        }

        let ticket = self.ticket.get();
        let state = self.state.clone();
        let work = async move { library::duplicates::apply(&state, removals).await };
        let spawned =
            self.after_any_ticket("duplicates remove", work, move |ui, finder, removed| {
                ui.global::<DuplicatesUi>().set_removing(false);
                let counts = match removed {
                    Some(Ok(counts)) => counts,
                    Some(Err(e)) => {
                        log::warn!("duplicates: remove failed: {}", describe(&e));
                        finder.show_remove_failed(ui);
                        return;
                    }
                    None => {
                        finder.show_remove_failed(ui);
                        return;
                    }
                };
                if finder.ticket.holds(ticket, ui) {
                    ui.global::<Dialog>().set_open(false);
                }
                finder.show_removed(ui, counts);
            });
        if spawned {
            dup.set_removing(true);
        }
    }

    fn show_remove_failed(&self, ui: &AppWindow) {
        self.notifications.show_failure(ui, |ui| {
            let dup = ui.global::<DuplicatesUi>();
            RowText::plain(dup.invoke_remove_failed_title(), dup.invoke_remove_failed_message())
        });
    }

    fn show_removed(&self, ui: &AppWindow, counts: RemovalCounts) {
        let dup = ui.global::<DuplicatesUi>();
        self.notifications.show_completion(
            Completion::partial_if(counts.skipped > 0),
            dup.invoke_removed_title(len_as_i32(counts.removed)),
            dup.invoke_removed_message(len_as_i32(counts.skipped)),
        );
    }
}
