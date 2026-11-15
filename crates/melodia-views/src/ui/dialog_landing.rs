//! What a dialog that fills while it is up shares: work run on the runtime whose answer lands on
//! the UI thread, and the ticket that says whether the dialog still wants it.
//!
//! The dialog is already open when its answers arrive, so `DialogClaim`, which guards a raise still
//! to come, can't tell whether one is still wanted.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use slint::ComponentHandle;
use tokio::runtime::Handle;

use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, DialogKind};

/// Moved by a dialog's next load and by its close, so an answer sent before either lands nowhere.
pub(crate) struct DialogTicket {
    current: Cell<u64>,
    kind: DialogKind,
}

impl DialogTicket {
    pub(crate) fn new(kind: DialogKind) -> Self {
        Self { current: Cell::new(0), kind }
    }

    pub(crate) fn get(&self) -> u64 {
        self.current.get()
    }

    pub(crate) fn retire(&self) {
        self.current.set(self.current.get().wrapping_add(1));
    }

    /// Whether an answer sent under `ticket` still lands: nothing has moved the ticket since, and
    /// the dialog still shows this kind.
    pub(crate) fn holds(&self, ticket: u64, ui: &AppWindow) -> bool {
        self.current.get() == ticket && self.shows(ui)
    }

    pub(crate) fn shows(&self, ui: &AppWindow) -> bool {
        let dialog = ui.global::<Dialog>();
        dialog.get_open() && dialog.get_kind() == self.kind
    }
}

/// A dialog's controller, held in an `Rc` its answers reach through a weak pointer.
pub(crate) trait TicketedDialog: Sized + 'static {
    /// What a failed [`Self::after`] is logged under.
    const LABEL: &'static str;

    fn window(&self) -> &slint::Weak<AppWindow>;
    fn ticket(&self) -> &DialogTicket;
    fn runtime(&self) -> &Handle;

    /// Runs `work` on the runtime and hands its answer to `land` on the UI thread, while the
    /// ticket it was sent under holds.
    fn after<T: Send + 'static>(
        self: &Rc<Self>,
        work: impl Future<Output = T> + Send + 'static,
        land: impl FnOnce(&AppWindow, &Self, T) + 'static,
    ) {
        let ticket = self.ticket().get();
        self.after_any_ticket(Self::LABEL, work, move |ui, dialog, answer| {
            if let Some(answer) = answer
                && dialog.ticket().holds(ticket, ui)
            {
                land(ui, dialog, answer);
            }
        });
    }

    /// [`Self::after`] for an answer that lands whatever the ticket: a write a dialog closed mid-run
    /// still made, or a count shown outside the dialog. `None` is a task that failed to join, so a
    /// busy flag set for the work still comes down. Answers whether the landing was spawned.
    fn after_any_ticket<T: Send + 'static>(
        self: &Rc<Self>,
        label: &'static str,
        work: impl Future<Output = T> + Send + 'static,
        land: impl FnOnce(&AppWindow, &Self, Option<T>) + 'static,
    ) -> bool {
        let answer = self.runtime().spawn(work);
        let dialog = Rc::downgrade(self);
        let spawned = slint::spawn_local(async_compat::Compat::new(async move {
            let answer = answer
                .await
                .inspect_err(|e| log::warn!("{label}: task failed: {}", describe(e)))
                .ok();
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            let Some(ui) = dialog.window().upgrade() else {
                return;
            };
            land(&ui, &dialog, answer);
        }));
        if let Err(e) = &spawned {
            log::warn!("{label}: not spawned: {}", describe(e));
        }
        spawned.is_ok()
    }
}
