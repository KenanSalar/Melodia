//! The Find headphones dialog: `AutoEq`'s list off the cache at once, rechecked behind it when due,
//! narrowed by the needle, and the pick fetched and set for the card's device.
//!
//! The list is held only while the dialog is open, and a close drops a fetch still running, so a
//! pick the user walked away from sets nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use crate::ui::dialog_landing::{DialogTicket, TicketedDialog};
use crate::ui::headphones::Card;
use crate::ui::row_match::PICKER_ROW_CAP;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::library::correction::{CachedList, ListAnswer, ListCheck};
use melodia_app::state::AppState;
use melodia_core::entities::correction::AutoEqEntry;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::needle::fold_needle;
use melodia_ui::{
    AppWindow, ApplyFailure, Dialog, DialogKind, HeadphonePicker, PickRow, PickerPhase,
};

/// `PickRow.badge` for a model, which carries none.
const NO_BADGE: i32 = -1;

pub(super) fn install(ui: &AppWindow, card: &Rc<Card>) {
    let picker = Rc::new(Picker {
        ui: ui.as_weak(),
        state: card.state().clone(),
        card: Rc::clone(card),
        ticket: DialogTicket::new(DialogKind::HeadphonePicker),
        rows: Rc::new(VecModel::default()),
        entries: RefCell::new(Vec::new()),
        fetch: RefCell::new(None),
    });
    let g = ui.global::<HeadphonePicker>();
    g.set_rows(ModelRc::from(picker.rows.clone()));

    let p = Rc::clone(&picker);
    g.on_load(move || p.load());
    let p = Rc::clone(&picker);
    g.on_filter_changed(move |needle| p.write_filtered(&needle));
    let p = Rc::clone(&picker);
    g.on_apply(move |index| p.apply(index));
    g.on_released(move || picker.release());
}

struct Picker {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    card: Rc<Card>,
    ticket: DialogTicket,
    rows: Rc<VecModel<PickRow>>,
    /// The whole list while the dialog is open, which a row's `index` points into.
    entries: RefCell<Vec<AutoEqEntry>>,
    /// The pick's fetch, aborted by a close.
    fetch: RefCell<Option<AbortHandle>>,
}

impl Picker {
    /// Shows the cached list, then asks `AutoEq` whether it moved once the check is due.
    fn load(self: &Rc<Self>) {
        let paths = Arc::clone(&self.state.paths);
        let read =
            self.state.runtime.spawn_blocking(move || library::correction::cached_list(&paths));
        let picker = Rc::clone(self);
        self.after(read, move |ui, _, cached| match cached {
            Ok(CachedList { entries, check }) => {
                if !entries.is_empty() {
                    picker.show(ui, entries);
                }
                if check.is_due(SystemTime::now()) {
                    picker.recheck(check);
                }
            }
            Err(e) => {
                log::warn!("AutoEq list read task failed: {}", describe(&e));
                ui.global::<HeadphonePicker>().set_phase(PickerPhase::Unreadable);
            }
        });
    }

    fn recheck(self: &Rc<Self>, check: ListCheck) {
        let state = self.state.clone();
        let answer = async move { library::correction::recheck_list(&state, check).await };
        self.after(answer, |ui, picker, answer| match answer {
            ListAnswer::Unchanged => {}
            ListAnswer::Fresh(entries) => picker.show(ui, entries),
            ListAnswer::Unreachable => {
                ui.global::<HeadphonePicker>().set_phase(PickerPhase::Unreachable);
            }
            ListAnswer::Unreadable => {
                ui.global::<HeadphonePicker>().set_phase(PickerPhase::Unreadable);
            }
        });
    }

    /// A fresh list may order the models differently, so a pick into the old one goes.
    fn show(&self, ui: &AppWindow, entries: Vec<AutoEqEntry>) {
        *self.entries.borrow_mut() = entries;
        let g = ui.global::<HeadphonePicker>();
        g.set_picked(-1);
        g.set_phase(PickerPhase::Listing);
        self.write_filtered(&g.get_filter());
    }

    /// The models matching `needle` by name or by whose measurement it is, up to the cap.
    fn write_filtered(&self, needle: &str) {
        let needle = fold_needle(needle);
        let rows: Vec<_> = self
            .entries
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, entry)| needle.contains(&entry.name) || needle.contains(&entry.source))
            .take(PICKER_ROW_CAP)
            .map(|(index, entry)| PickRow {
                index: len_as_i32(index),
                title: entry.name.as_ref().into(),
                subtitle: entry.source.as_ref().into(),
                badge: NO_BADGE,
                ..PickRow::default()
            })
            .collect();
        self.rows.set_vec(rows);
    }

    /// Lands whatever the ticket: a fetch that finished just as the dialog closed still set the
    /// correction, and the card has to show it.
    fn apply(self: &Rc<Self>, index: i32) {
        let entry = usize::try_from(index).ok().and_then(|i| self.entries.borrow().get(i).cloned());
        let (Some(entry), Some(target)) = (entry, self.card.target()) else {
            return;
        };
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<HeadphonePicker>();
        g.set_failure(ApplyFailure::None);

        let state = self.state.clone();
        let work =
            async move { library::correction::apply_from_list(&state, target, &entry).await };
        let ticket = self.ticket.get();
        let fetch = self.after_any_ticket("AutoEq profile", work, move |ui, picker, applied| {
            picker.card.paint();
            // A close already took this handle, and the slot may hold a later fetch's.
            if !picker.ticket.holds(ticket, ui) {
                return;
            }
            picker.fetch.take();
            let g = ui.global::<HeadphonePicker>();
            g.set_applying(false);
            match applied {
                Some(Ok(())) => ui.global::<Dialog>().set_open(false),
                Some(Err(e)) => {
                    log::warn!("AutoEq profile: {}", describe(&e));
                    g.set_failure(failure(&e));
                }
                None => g.set_failure(ApplyFailure::Failed),
            }
        });
        g.set_applying(fetch.is_some());
        *self.fetch.borrow_mut() = fetch;
    }

    fn release(&self) {
        self.ticket.retire();
        if let Some(fetch) = self.fetch.take() {
            fetch.abort();
        }
        self.entries.take();
        self.rows.set_vec(Vec::new());
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<HeadphonePicker>();
        g.set_applying(false);
        g.set_failure(ApplyFailure::None);
    }
}

fn failure(error: &AppError) -> ApplyFailure {
    match error {
        AppError::Network { .. } => ApplyFailure::Unreachable,
        AppError::Validation { .. } => ApplyFailure::Refused,
        _ => ApplyFailure::Failed,
    }
}

impl TicketedDialog for Picker {
    const LABEL: &'static str = "headphone picker";

    fn window(&self) -> &slint::Weak<AppWindow> {
        &self.ui
    }

    fn ticket(&self) -> &DialogTicket {
        &self.ticket
    }

    fn runtime(&self) -> &Handle {
        &self.state.runtime
    }
}
