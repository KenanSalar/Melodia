//! Transient in-app notification stack — Rust side.
//!
//! Owns the `Rc<VecModel<NotificationRow>>` behind the `Notifications` global's `rows`,
//! plus a monotonic id counter and a visible cap.
//!
//! `Rc<VecModel<_>>` is UI-thread-only, so every `&self` method here must be called from
//! a Slint callback context or through `upgrade_in_event_loop`.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{AppWindow, NotificationRow, Notifications};

/// Maximum visible notifications. Pushing past it drops the oldest first, so the stack
/// never grows off-screen.
const MAX_VISIBLE: usize = 5;

/// How long a transient confirmation toast stays up. Errors stay sticky instead — a
/// failure nobody was looking at is a failure nobody heard about — so this only ever
/// reaches [`NotificationsUi::show_auto_dismiss`].
pub const TOAST_AUTO_DISMISS_MS: u32 = 3000;

/// One notification's worth of data. `variant` is one of the four strings
/// `NotificationCard` dispatches on; an unknown one falls through to "info" styling, but
/// picking outside the four is drift between the two sides.
pub struct NotificationParams {
    pub variant: SharedString,
    pub title: SharedString,
    pub message: SharedString,
    /// Empty ⇒ no action button rendered.
    pub action_label: SharedString,
    /// Routing key for the optional action button, read by the default
    /// `Notifications.action` handler in `globals/updater.slint` and by
    /// [`NotificationsUi::dismiss_by_kind`] to clear lingering rows of a category.
    pub action_kind: SharedString,
}

impl NotificationParams {
    /// A toast with no action button and nothing to dismiss it by — most of them.
    /// Anything carrying an `action_kind` builds the struct literal instead: that field
    /// routes the button *and* groups rows for [`NotificationsUi::dismiss_by_kind`], and
    /// a constructor hiding it would make the two roles harder to tell apart.
    pub fn plain(variant: &str, title: SharedString, message: SharedString) -> Self {
        Self {
            variant: variant.into(),
            title,
            message,
            action_label: SharedString::default(),
            action_kind: SharedString::default(),
        }
    }
}

pub struct NotificationsUi {
    rows: Rc<VecModel<NotificationRow>>,
    next_id: Cell<i32>,
}

impl NotificationsUi {
    /// Push a new notification, returning its id so the caller can `dismiss` it later.
    /// Past `MAX_VISIBLE` the oldest is evicted before the new one lands.
    pub fn show(&self, p: NotificationParams) -> i32 {
        let id = self.next_id.get();
        // Saturating, against the wrap-to-negative on a session that somehow pushes
        // 2^31 notifications.
        self.next_id.set(id.saturating_add(1));

        // Down to `MAX_VISIBLE - 1`, leaving room for the push to settle at the cap.
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

    /// [`show`](Self::show) plus a single-shot timer, for a transient confirmation that
    /// shouldn't need a manual close — errors and actionable prompts stay sticky. If the
    /// user closes it first, or it is evicted past the cap, the by-id removal finds
    /// nothing and no-ops: ids are monotonic and never reused, so it can't dismiss the
    /// wrong row.
    pub fn show_auto_dismiss(&self, p: NotificationParams, ms: u32) -> i32 {
        let id = self.show(p);
        let rows = self.rows.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(u64::from(ms)), move || {
            if let Some(pos) = rows.iter().position(|r: NotificationRow| r.id == id) {
                rows.remove(pos);
            }
        });
        id
    }

    /// Remove the row with this id; a no-op if none matches.
    pub fn dismiss(&self, id: i32) {
        if let Some(pos) = self.rows.iter().position(|r: NotificationRow| r.id == id) {
            self.rows.remove(pos);
        }
    }

    /// Remove every row whose `action_kind` matches — the file-watching toggle clearing
    /// its "watching disabled" row. Back-to-front, so a remove doesn't invalidate the
    /// indices of pending matches.
    pub fn dismiss_by_kind(&self, kind: &str) {
        for i in (0..self.rows.row_count()).rev() {
            if self.rows.row_data(i).is_some_and(|r| r.action_kind.as_str() == kind) {
                self.rows.remove(i);
            }
        }
    }
}

/// Install the `Notifications` global's row model and wire its `dismiss` callback,
/// returning the handle the caller threads into the modules that raise toasts.
///
/// The `action` callback is deliberately left to the Slint-side dispatcher in
/// `globals/updater.slint` — a new action flow is one branch there plus a `show(…)`
/// call, with no closure threading.
pub fn install(ui: &AppWindow) -> Rc<NotificationsUi> {
    let rows: Rc<VecModel<NotificationRow>> = Rc::new(VecModel::default());
    ui.global::<Notifications>().set_rows(ModelRc::from(rows.clone()));

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
