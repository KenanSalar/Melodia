//! Transient in-app notification stack — Rust side.
//!
//! Owns the `Rc<VecModel<NotificationRow>>` backing the `Notifications`
//! global's `rows` model, plus a monotonic `next_id` counter and a
//! `max_visible` cap. Public API:
//!   - [`install`] wires the global once at startup and returns the handle.
//!   - [`NotificationsUi::show`] appends a row, returns its id.
//!   - [`NotificationsUi::dismiss`] removes by id.
//!   - [`NotificationsUi::dismiss_by_kind`] removes every row whose
//!     `action_kind` matches — used by `ui::file_watching` to clear the
//!     "watching disabled" row when the user toggles watching back on.
//!
//! Threading: `Rc<VecModel<_>>` is UI-thread-only. All public methods are
//! `&self` and must be called from a Slint callback context (or via
//! `Weak<AppWindow>::upgrade_in_event_loop`). For now the only caller is
//! the file-watching toggle whose change handler runs on the UI thread
//! already, so direct calls work without an event-loop hop.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{AppWindow, NotificationRow, Notifications};

/// Maximum visible notifications. Pushing a (`MAX_VISIBLE` + 1)-th row drops
/// the oldest (front of the model) before appending, so the stack never
/// grows off-screen. Picked to match Tauri's loose unbounded behaviour
/// without ever pushing cards past the viewport edge.
const MAX_VISIBLE: usize = 5;

/// One notification's worth of data. `variant` is one of the strings the
/// `NotificationCard` Slint component dispatches on: `"info"`, `"success"`,
/// `"warning"`, `"error"`. Unknown variants fall through to the "info"
/// styling on the Slint side, but callers should pick one of the four to
/// avoid drift between Rust and Slint expectations.
pub struct NotificationParams {
    pub variant: SharedString,
    pub title: SharedString,
    pub message: SharedString,
    /// Empty ⇒ no action button rendered.
    pub action_label: SharedString,
    /// Routing key for the optional action button. Read by the default
    /// `Notifications.action(id, kind) => { … }` handler in `globals.slint`,
    /// plus used by [`NotificationsUi::dismiss_by_kind`] to clear lingering
    /// rows for a given category (e.g. `"watcher-disabled"`).
    pub action_kind: SharedString,
}

pub struct NotificationsUi {
    rows: Rc<VecModel<NotificationRow>>,
    next_id: Cell<i32>,
}

impl NotificationsUi {
    /// Push a new notification. Returns the assigned id so the caller can
    /// later `dismiss(id)` programmatically. When more than `MAX_VISIBLE`
    /// rows are active, the oldest is evicted before the new one lands.
    pub fn show(&self, p: NotificationParams) -> i32 {
        let id = self.next_id.get();
        // Saturating bump avoids the wrap-to-negative footgun on a long-
        // running session that somehow pushes 2 ^ 31 notifications.
        self.next_id.set(id.saturating_add(1));

        // Evict oldest (front) entries until we're at `MAX_VISIBLE - 1`,
        // leaving room for the new push to settle at `MAX_VISIBLE`.
        while self.rows.row_count() >= MAX_VISIBLE {
            self.rows.remove(0);
        }

        self.rows.push(NotificationRow {
            id,
            variant: p.variant,
            title: p.title,
            message: p.message,
            action_label: p.action_label,
            action_kind: p.action_kind,
        });
        id
    }

    /// Remove the row with the given id. No-op if no row matches (already
    /// dismissed, or id never existed).
    pub fn dismiss(&self, id: i32) {
        if let Some(pos) = self
            .rows
            .iter()
            .position(|r: NotificationRow| r.id == id)
        {
            self.rows.remove(pos);
        }
    }

    /// Remove every row whose `action_kind` matches. Used by the
    /// file-watching toggle to clear the "watching disabled" row when the
    /// user toggles watching back on. Iterates back-to-front so each remove
    /// doesn't invalidate the indices of pending matches.
    pub fn dismiss_by_kind(&self, kind: &str) {
        for i in (0..self.rows.row_count()).rev() {
            if self
                .rows
                .row_data(i)
                .is_some_and(|r| r.action_kind.as_str() == kind)
            {
                self.rows.remove(i);
            }
        }
    }
}

/// Install the `Notifications` global's row model and wire its `dismiss`
/// callback. Returns the Rust-side handle the caller (typically
/// `main.rs`) threads into other modules (`ui::file_watching::install`,
/// future `Updater` task, …).
///
/// The `action` callback is intentionally left to the Slint-side default
/// dispatcher in `globals.slint` — adding a new action flow is a single
/// branch there plus a `notifications.show(...)` call from Rust, no
/// closure threading.
pub fn install(ui: &AppWindow) -> Rc<NotificationsUi> {
    let rows: Rc<VecModel<NotificationRow>> = Rc::new(VecModel::default());
    ui.global::<Notifications>()
        .set_rows(ModelRc::from(rows.clone()));

    let state = Rc::new(NotificationsUi {
        rows,
        next_id: Cell::new(0),
    });

    {
        let state = state.clone();
        ui.global::<Notifications>().on_dismiss(move |id| {
            state.dismiss(id);
        });
    }

    state
}

#[cfg(test)]
#[path = "tests/notifications_tests.rs"]
mod tests;
