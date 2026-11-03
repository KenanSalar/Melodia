//! What both duplicates dialogs share: work run on the runtime whose answer lands on the UI thread,
//! and the ticket that says whether the dialog still wants it.
//!
//! Each dialog is already up while it fills, so `DialogClaim`, which guards a raise still to come,
//! can't tell whether an answer is still wanted.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;

use slint::ComponentHandle;
use tokio::runtime::Handle;

use crate::ui::paged_rows::{PagedRow, RowRequest, spawn_fetch};
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, Dialog, DialogKind};

/// Moved by a dialog's next load and by its close, so an answer sent before either lands nowhere.
pub(super) struct DialogTicket {
    current: Cell<u64>,
    kind: DialogKind,
}

impl DialogTicket {
    pub(super) fn new(kind: DialogKind) -> Self {
        Self { current: Cell::new(0), kind }
    }

    pub(super) fn get(&self) -> u64 {
        self.current.get()
    }

    pub(super) fn retire(&self) {
        self.current.set(self.current.get().wrapping_add(1));
    }

    /// Whether an answer sent under `ticket` still lands: nothing has moved the ticket since, and
    /// the dialog still shows this kind.
    pub(super) fn holds(&self, ticket: u64, ui: &AppWindow) -> bool {
        self.current.get() == ticket && self.shows(ui)
    }

    pub(super) fn shows(&self, ui: &AppWindow) -> bool {
        let dialog = ui.global::<Dialog>();
        dialog.get_open() && dialog.get_kind() == self.kind
    }
}

/// A dialog's controller, held in an `Rc` its answers reach through a weak pointer.
pub(super) trait TicketedDialog: Sized + 'static {
    /// What a failed [`Self::after`] is logged under.
    const LABEL: &'static str;
    /// What a failed [`Self::fetch_rows`] is logged under.
    const ROWS_LABEL: &'static str;

    fn window(&self) -> &slint::Weak<AppWindow>;
    fn ticket(&self) -> &DialogTicket;
    fn runtime(&self) -> &Handle;

    /// Runs `work` on the runtime and hands its answer to `land` on the UI thread, while the
    /// ticket it was sent under holds.
    fn after<T: Send + 'static>(
        self: &Rc<Self>,
        work: impl Future<Output = AppResult<T>> + Send + 'static,
        land: impl FnOnce(&AppWindow, &Self, AppResult<T>) + 'static,
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
        let dialog = Rc::downgrade(self);
        spawn_landing(self.runtime(), label, work, move |answer| {
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            let Some(ui) = dialog.window().upgrade() else {
                return;
            };
            land(&ui, &dialog, answer);
        })
    }

    /// Reads the rows `request` asks for, `build` making them out of what `query` read. A fetch
    /// landing after the ticket moved is dropped, which frees the list to ask again.
    fn fetch_rows<R: PagedRow, T: Send + 'static>(
        self: &Rc<Self>,
        request: RowRequest<R>,
        query: impl Future<Output = AppResult<T>> + Send + 'static,
        build: impl FnOnce(&Self, &[i64], T) -> Vec<(i64, R)> + 'static,
    ) {
        let ticket = self.ticket().get();
        let dialog = Rc::downgrade(self);
        let land = move |request: RowRequest<R>, answer: T| {
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            if dialog.ticket().get() == ticket {
                let rows = build(&dialog, request.ids(), answer);
                request.land(rows);
            }
        };
        spawn_fetch(self.runtime(), request, query, land, Self::ROWS_LABEL);
    }
}

/// Runs `work` on `runtime` and hands its answer to `land` on the UI thread, `None` for a task that
/// failed to join, answering whether the landing was spawned. Either failure is logged under
/// `label`.
fn spawn_landing<T: Send + 'static>(
    runtime: &Handle,
    label: &'static str,
    work: impl Future<Output = T> + Send + 'static,
    land: impl FnOnce(Option<T>) + 'static,
) -> bool {
    let answer = runtime.spawn(work);
    let spawned = slint::spawn_local(async_compat::Compat::new(async move {
        let answer =
            answer.await.inspect_err(|e| log::warn!("{label}: task failed: {}", describe(e))).ok();
        land(answer);
    }));
    if let Err(e) = &spawned {
        log::warn!("{label}: not spawned: {}", describe(e));
    }
    spawned.is_ok()
}
