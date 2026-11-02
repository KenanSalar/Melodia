//! What both duplicates dialogs share: work run on the runtime whose answer lands on the UI thread,
//! and the ticket that says whether the dialog still wants it.
//!
//! Each dialog is already up while it fills, so `DialogClaim`, which guards a raise still to come,
//! can't tell whether an answer is still wanted.

use std::cell::Cell;
use std::future::Future;

use slint::ComponentHandle;
use tokio::runtime::Handle;

use melodia_core::error::describe;
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

/// Runs `work` on `runtime` and hands its answer to `land` on the UI thread, `None` for a task that
/// failed to join, answering whether the landing was spawned. Either failure is logged under
/// `label`, and a landing still runs for the first, so a busy flag set for the work comes down.
pub(super) fn spawn_landing<T: Send + 'static>(
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
