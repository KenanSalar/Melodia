//! Remove: the picks handed to `library::duplicates::apply`, and the toast saying what left.

use std::rc::Rc;

use slint::ComponentHandle;

use super::{Finder, shows_finder};
use crate::ui::shell::notifications::{Completion, RowText};
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::library::duplicates::RemovalCounts;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, DuplicatesUi};

impl Finder {
    /// Removes the picked copies. Unlike the finder's other answers this one lands under any
    /// ticket: a dialog closed mid-remove still removed, and its toast says so.
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
        dup.set_removing(true);

        let ticket = self.ticket.get();
        let state = self.state.clone();
        let answer = self
            .state
            .runtime
            .spawn(async move { library::duplicates::apply(&state, removals).await });
        let finder = Rc::downgrade(self);
        let spawned = slint::spawn_local(async_compat::Compat::new(async move {
            let counts = match answer.await {
                Ok(Ok(counts)) => Some(counts),
                Ok(Err(e)) => {
                    log::warn!("duplicates: remove failed: {}", describe(&e));
                    None
                }
                Err(e) => {
                    log::warn!("duplicates: remove task failed: {}", describe(&e));
                    None
                }
            };
            let Some(finder) = finder.upgrade() else {
                return;
            };
            let Some(ui) = finder.ui.upgrade() else {
                return;
            };
            ui.global::<DuplicatesUi>().set_removing(false);
            let Some(counts) = counts else {
                finder.notifications.show_failure(&ui, |ui| {
                    let dup = ui.global::<DuplicatesUi>();
                    RowText::plain(
                        dup.invoke_remove_failed_title(),
                        dup.invoke_remove_failed_message(),
                    )
                });
                return;
            };
            if finder.ticket.get() == ticket && shows_finder(&ui) {
                ui.global::<Dialog>().set_open(false);
            }
            finder.show_removed(&ui, counts);
        }));
        if let Err(e) = spawned {
            dup.set_removing(false);
            log::warn!("duplicates: remove not spawned: {}", describe(&e));
        }
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
