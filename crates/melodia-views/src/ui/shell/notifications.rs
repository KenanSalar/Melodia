//! The toast stack's Rust half: the `Notifications` rows, their cap, and the recipes localized rows
//! are rebuilt from after a language switch, which reaches only live `@tr` bindings.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use melodia_ui::{
    AppWindow, MelodiaUpdater, NotificationKind, NotificationRow, NotificationVariant,
    Notifications, Settings, SettingsPage,
};

/// Past it the oldest is dropped, so the stack never grows off-screen.
const MAX_VISIBLE: usize = 5;

/// How long a confirmation stays up. Errors stay sticky: a failure nobody was looking at is one
/// nobody heard about.
pub const TOAST_AUTO_DISMISS_MS: u32 = 3000;

/// A row's timeout when only a close or Rust removes it.
const STICKY: u32 = 0;

pub struct NotificationParams {
    pub variant: NotificationVariant,
    pub title: SharedString,
    pub message: SharedString,
    /// Empty ⇒ no action button rendered.
    pub action_label: SharedString,
    /// Routes the optional action button through `run_action` and groups rows for
    /// [`NotificationsUi::dismiss_by_kind`].
    pub kind: NotificationKind,
}

impl NotificationParams {
    /// A toast with no action button and nothing to dismiss it by, most of them. One with a `kind`
    /// spells the literal, so that field's two roles stay in view.
    pub fn plain(variant: NotificationVariant, title: SharedString, message: SharedString) -> Self {
        Self {
            variant,
            title,
            message,
            action_label: SharedString::default(),
            kind: NotificationKind::None,
        }
    }
}

/// What a localized row is besides its text.
#[derive(Clone, Copy)]
pub struct LocalizedPush {
    pub variant: NotificationVariant,
    pub kind: NotificationKind,
    /// `0` ⇒ sticky.
    pub timeout_ms: u32,
}

/// The three translated strings a row paints, rebuilt in whatever language is active now.
pub struct RowText {
    pub title: SharedString,
    pub message: SharedString,
    /// Empty ⇒ no action button, as on [`NotificationParams`].
    pub action_label: SharedString,
}

impl RowText {
    /// A row with no action button — most of them.
    pub fn plain(title: SharedString, message: SharedString) -> Self {
        Self { title, message, action_label: SharedString::default() }
    }
}

/// How much of an action landed, which is what colours its completion toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Complete,
    /// Something was left out beside what landed.
    Partial,
}

impl Completion {
    pub fn partial_if(left_out_any: bool) -> Self {
        if left_out_any { Self::Partial } else { Self::Complete }
    }
}

/// Renders a row's strings from the `Settings.*` trampolines, so it can be run again after
/// a language switch.
type Relabel = Box<dyn Fn(&AppWindow) -> RowText>;

pub struct NotificationsUi {
    rows: Rc<VecModel<NotificationRow>>,
    /// Keyed by row id rather than position, positions moving under an eviction.
    recipes: RefCell<HashMap<i32, Relabel>>,
    next_id: Cell<i32>,
    /// Told every row that goes, by whatever path: the undo slot has to drop its hold with its
    /// notice.
    row_removed: OnceCell<Box<dyn Fn(i32)>>,
}

impl NotificationsUi {
    /// **Private, which is the whole enforcement of the locale rule**: the public pushes are
    /// [`show_auto_dismiss`](Self::show_auto_dismiss), gone before anyone reaches the language
    /// picker, and [`show_localized`](Self::show_localized) and
    /// [`show_localized_timed`](Self::show_localized_timed), which keep the recipe. A sticky push
    /// of rendered strings fails only in a locale nobody reviews in, so it is put out of reach.
    fn show(&self, p: NotificationParams, timeout_ms: u32) -> i32 {
        let id = self.next_id.get();
        self.next_id.set(id.saturating_add(1));

        // Down to `MAX_VISIBLE - 1`, leaving room for the push to settle at the cap.
        while self.rows.row_count() >= MAX_VISIBLE {
            self.remove_at(0);
        }

        self.rows.push(NotificationRow {
            id,
            variant: p.variant,
            title: p.title,
            message: p.message,
            action_label: p.action_label,
            kind: p.kind,
            timeout_ms: i32::try_from(timeout_ms).unwrap_or(i32::MAX),
            restarts: 0,
        });
        id
    }

    /// For a confirmation that shouldn't need a close. The card runs the clock, holding it while
    /// the card is hovered or the window is in the background.
    pub fn show_auto_dismiss(&self, p: NotificationParams, ms: u32) -> i32 {
        self.show(p, ms)
    }

    /// For a sticky row, which has to survive a language switch: the recipe renders the push and
    /// is kept for [`refresh_for_locale`](Self::refresh_for_locale).
    pub fn show_localized<F>(
        &self,
        ui: &AppWindow,
        variant: NotificationVariant,
        kind: NotificationKind,
        relabel: F,
    ) -> i32
    where
        F: Fn(&AppWindow) -> RowText + 'static,
    {
        self.show_localized_timed(ui, LocalizedPush { variant, kind, timeout_ms: STICKY }, relabel)
    }

    /// For a timed row that can still be up across a language switch, its clock standing still
    /// while it is read.
    pub fn show_localized_timed<F>(&self, ui: &AppWindow, push: LocalizedPush, relabel: F) -> i32
    where
        F: Fn(&AppWindow) -> RowText + 'static,
    {
        let text = relabel(ui);
        let id = self.show(
            NotificationParams {
                variant: push.variant,
                title: text.title,
                message: text.message,
                action_label: text.action_label,
                kind: push.kind,
            },
            push.timeout_ms,
        );
        self.recipes.borrow_mut().insert(id, Box::new(relabel));
        id
    }

    /// Swaps a live row's recipe and re-renders it in place, so its card and clock carry on.
    pub fn relabel<F>(&self, ui: &AppWindow, id: i32, relabel: F)
    where
        F: Fn(&AppWindow) -> RowText + 'static,
    {
        let Some((pos, row)) = self.find(id) else { return };
        let text = relabel(ui);
        self.recipes.borrow_mut().insert(id, Box::new(relabel));
        self.rows.set_row_data(
            pos,
            NotificationRow {
                title: text.title,
                message: text.message,
                action_label: text.action_label,
                ..row
            },
        );
    }

    /// Starts a live row's clock over.
    pub fn restart(&self, id: i32) {
        let Some((pos, row)) = self.find(id) else { return };
        let restarts = row.restarts.wrapping_add(1);
        self.rows.set_row_data(pos, NotificationRow { restarts, ..row });
    }

    /// The one listener, which the undo slot registers.
    pub fn on_row_removed(&self, listener: impl Fn(i32) + 'static) {
        if self.row_removed.set(Box::new(listener)).is_err() {
            log::warn!("notifications: a second row-removed listener was ignored");
        }
    }

    /// The transient toast for an action that landed, whole or in part.
    pub fn show_completion(
        &self,
        completion: Completion,
        title: SharedString,
        message: SharedString,
    ) -> i32 {
        let variant = match completion {
            Completion::Complete => NotificationVariant::Success,
            Completion::Partial => NotificationVariant::Warning,
        };
        self.show_auto_dismiss(
            NotificationParams::plain(variant, title, message),
            TOAST_AUTO_DISMISS_MS,
        )
    }

    /// The sticky error for an action that landed nothing.
    pub fn show_failure<F>(&self, ui: &AppWindow, relabel: F) -> i32
    where
        F: Fn(&AppWindow) -> RowText + 'static,
    {
        self.show_localized(ui, NotificationVariant::Error, NotificationKind::None, relabel)
    }

    /// Re-renders every row carrying a recipe, collected before any write: `set_row_data` notifies
    /// Slint, and a handler raising a toast would re-enter the `RefCell` this walk holds.
    pub fn refresh_for_locale(&self, ui: &AppWindow) {
        let updated: Vec<(usize, NotificationRow)> = {
            let recipes = self.recipes.borrow();
            self.rows
                .iter()
                .enumerate()
                .filter_map(|(pos, row): (usize, NotificationRow)| {
                    let text = recipes.get(&row.id)?(ui);
                    Some((
                        pos,
                        NotificationRow {
                            title: text.title,
                            message: text.message,
                            action_label: text.action_label,
                            ..row
                        },
                    ))
                })
                .collect()
        };
        for (pos, row) in updated {
            self.rows.set_row_data(pos, row);
        }
    }

    pub fn dismiss(&self, id: i32) {
        if let Some((pos, _)) = self.find(id) {
            self.remove_at(pos);
        }
    }

    /// Back to front, so a remove doesn't shift the matches still to come.
    pub fn dismiss_by_kind(&self, kind: NotificationKind) {
        for i in (0..self.rows.row_count()).rev() {
            if self.rows.row_data(i).is_some_and(|r| r.kind == kind) {
                self.remove_at(i);
            }
        }
    }

    /// A row closed or evicted first is simply not found, ids never being reused.
    fn find(&self, id: i32) -> Option<(usize, NotificationRow)> {
        self.rows.iter().enumerate().find(|(_, row)| row.id == id)
    }

    /// **Every removal goes through here**: a recipe outliving its row is a closure kept for the
    /// session, and an undo hold outliving its notice an undo nobody can reach. The listener runs
    /// with nothing borrowed, the eviction inside `show` reaching it too.
    fn remove_at(&self, pos: usize) {
        let Some(row) = self.rows.row_data(pos) else { return };
        self.recipes.borrow_mut().remove(&row.id);
        self.rows.remove(pos);
        if let Some(listener) = self.row_removed.get() {
            listener(row.id);
        }
    }
}

/// Installs the row model and both callbacks, answering the handle toast raisers take.
pub fn install(ui: &AppWindow) -> Rc<NotificationsUi> {
    let rows: Rc<VecModel<NotificationRow>> = Rc::new(VecModel::default());
    ui.global::<Notifications>().set_rows(ModelRc::from(rows.clone()));

    let state = Rc::new(NotificationsUi {
        rows,
        recipes: RefCell::new(HashMap::new()),
        next_id: Cell::new(0),
        row_removed: OnceCell::new(),
    });

    {
        let state = state.clone();
        ui.global::<Notifications>().on_dismiss(move |id| {
            state.dismiss(id);
        });
    }
    {
        let state = state.clone();
        let weak = ui.as_weak();
        ui.global::<Notifications>().on_action(move |id, kind| {
            if let Some(ui) = weak.upgrade() {
                run_action(&ui, id, kind);
            }
            state.dismiss(id);
        });
    }

    state
}

/// Runs a row's action through the callback its control in Settings or the update card fires.
/// Here rather than in Slint because only Rust matches exhaustively: a new kind doesn't build until
/// it has an arm, where a missing Slint branch paints a button that does nothing.
fn run_action(ui: &AppWindow, id: i32, kind: NotificationKind) {
    match kind {
        NotificationKind::InstallUpdate => ui.global::<MelodiaUpdater>().invoke_install(),
        NotificationKind::UpdateRestart => ui.global::<MelodiaUpdater>().invoke_restart(),
        NotificationKind::CrashReport => ui.global::<Settings>().invoke_open_log_folder(),
        NotificationKind::SupportMelodia => ui.global::<Settings>().invoke_open_kofi(),
        NotificationKind::ExclusiveRefused => {
            let page = ui.global::<SettingsPage>();
            page.invoke_open_category(page.get_category_output());
        }
        // The slot acts only on its own row, so a stale card's button does nothing.
        NotificationKind::Undo => ui.global::<Notifications>().invoke_undo(id),
        // Kinds that only group rows; their rows carry no button.
        NotificationKind::None
        | NotificationKind::UpdateFailed
        | NotificationKind::WatcherDisabled
        | NotificationKind::LibraryResyncing
        | NotificationKind::AudioDeviceLost
        | NotificationKind::ArtworkRestoring => {}
    }
}

#[cfg(test)]
#[path = "tests/notifications_tests.rs"]
mod tests;
