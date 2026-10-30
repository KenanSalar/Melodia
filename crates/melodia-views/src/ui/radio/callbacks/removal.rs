//! Taking stations out of a kept tab: at once with an undo, or, with undo off in Settings or past
//! what an undo may hold, asked first and then for good. The Favorites heart, the trash and the
//! menu all arrive here.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::ui::callbacks::DialogClaim;
use crate::ui::radio::{RadioTab, RadioUi, kept, rows, tab_from_index};
use crate::ui::undo::{Removal, UndoWindow, stations_removed};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::library;
use melodia_app::library::radio::RemovedStations;
use melodia_app::state::AppState;
use melodia_core::entities::radio::KeptList;
use melodia_core::entities::undo::{RemovalSize, UNDO_HOLD_CAP, UndoCapture, UndoKind};
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, CardSelection, Dialog, NoUndoReason, Radio};

/// What a run of removals from one kept tab took, put back together.
pub struct StationRemovalUndo {
    removed: RemovedStations,
    radio_ui: Arc<RadioUi>,
}

impl StationRemovalUndo {
    pub fn same_list(&self, other: &Self) -> bool {
        self.removed.list() == other.removed.list()
    }

    pub fn absorb(&mut self, newer: Self) {
        self.removed.absorb(newer.removed);
    }

    pub fn list(&self) -> KeptList {
        self.removed.list()
    }

    pub fn count(&self) -> usize {
        self.removed.count()
    }

    pub fn name(&self) -> SharedString {
        SharedString::from(self.removed.newest_name())
    }

    /// Writes the stations back and re-reads both tabs. A station page the removal closed stays
    /// closed, navigation not being part of what went.
    pub async fn restore(self, state: &AppState, weak: &Weak<AppWindow>) -> Result<(), AppError> {
        library::radio::restore_stations(&state.db, self.removed).await?;
        if let Some(ui) = weak.upgrade() {
            kept::refresh(&ui, state, &self.radio_ui);
        }
        Ok(())
    }
}

pub(super) fn wire(cx: ViewCtx<'_>, radio_ui: &Arc<RadioUi>) {
    let remover = Remover {
        state: cx.state.clone(),
        radio_ui: radio_ui.clone(),
        undo: cx.undo.clone(),
        weak: cx.app.as_weak(),
    };
    let g = cx.app.global::<Radio>();
    {
        let remover = remover.clone();
        g.on_remove_stations(move |list, ids| remover.request(list, ids.iter().collect()));
    }
    g.on_remove_stations_confirmed(move |list, ids| {
        remover.for_good(list, ids.iter().map(i64::from).collect());
    });
}

/// What a removal reaches. Every path runs on the UI thread, where the undo slot lives.
#[derive(Clone)]
struct Remover {
    state: AppState,
    radio_ui: Arc<RadioUi>,
    undo: Rc<UndoWindow>,
    weak: Weak<AppWindow>,
}

impl Remover {
    /// Removes at once and offers the undo, or asks first where Settings turned undo off for it or
    /// there are more stations than an undo may hold.
    fn request(&self, list: i32, ids: Vec<i32>) {
        let Some(from) = self.kept_list(list) else { return };
        if ids.is_empty() {
            return;
        }
        if !self.undo.covers(UndoKind::Radio, RemovalSize::Items(ids.len())) {
            self.confirm_first(from, ids, NoUndoReason::TurnedOff);
            return;
        }
        if ids.len() > UNDO_HOLD_CAP {
            self.confirm_first(from, ids, NoUndoReason::TooLarge);
            return;
        }
        self.remove(from, ids.into_iter().map(i64::from).collect());
    }

    /// The confirmation's accept, holding nothing, then the button-less notice where the
    /// confirmation was the cap's. The dialog is modal, so Settings answers here as it did at the
    /// request.
    fn for_good(&self, list: i32, ids: Vec<i64>) {
        let Some(from) = self.kept_list(list) else { return };
        if ids.is_empty() {
            return;
        }
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            let removed =
                library::radio::remove_stations_for_good(&this.state.db, &ids, from).await;
            this.finish();
            match removed {
                Ok(0) => {}
                Ok(count) => this.tell_for_good(from, count),
                Err(e) => log::warn!("radio::remove for good: {}", describe(&e)),
            }
        }));
        if let Err(e) = spawned {
            log::warn!("radio::remove for good spawn_local: {}", describe(&e));
        }
    }

    /// A tick later, a `Dialog` write inside the click tripping Slint's recursion guard, and on a
    /// model of its own: the menu's ids can be the card selection's, rewritten on every pick. One
    /// station still on its tab is named.
    fn confirm_first(&self, from: KeptList, ids: Vec<i32>, reason: NoUndoReason) {
        let Some(claim) = DialogClaim::take_from(&self.weak) else { return };
        let station = match ids.as_slice() {
            &[id] => kept::resolve(&self.radio_ui, kept_tab(from), i64::from(id)),
            _ => None,
        };
        let raised = self.weak.upgrade_in_event_loop(move |ui| {
            if !claim.holds(&ui) {
                return;
            }
            let dialog = ui.global::<Dialog>();
            let from_recent = from == KeptList::Recent;
            if let Some(station) = station {
                let row = rows::to_slint_kept_station_row(&station);
                dialog.invoke_open_remove_station(row, from_recent);
            } else {
                let ids = ModelRc::new(VecModel::from(ids));
                dialog.invoke_open_remove_stations(ids, from_recent, reason);
            }
        });
        if let Err(e) = raised {
            log::warn!("radio::remove confirm: {}", describe(&e));
        }
    }

    fn remove(&self, from: KeptList, ids: Vec<i64>) {
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            let removed = library::radio::remove_stations(&this.state.db, ids, from).await;
            this.finish();
            match removed {
                Ok(capture) => this.offer(from, capture),
                Err(e) => log::warn!("radio::remove: {}", describe(&e)),
            }
        }));
        if let Err(e) = spawned {
            log::warn!("radio::remove spawn_local: {}", describe(&e));
        }
    }

    fn offer(&self, from: KeptList, capture: UndoCapture<RemovedStations>) {
        match capture {
            UndoCapture::Held(removed) => {
                log::info!("radio::remove: {} station(s)", removed.count());
                let radio_ui = self.radio_ui.clone();
                self.undo.offer(Removal::RadioStations(StationRemovalUndo { removed, radio_ui }));
            }
            UndoCapture::OverCap { rows } => self.tell_for_good(from, rows),
            UndoCapture::Nothing => {}
        }
    }

    /// The button-less notice, past the cap no one station being named.
    fn tell_for_good(&self, from: KeptList, count: usize) {
        log::info!("radio::remove for good: {count} station(s)");
        let title = stations_removed(from, count, SharedString::default());
        self.undo.notify_over_cap(UndoKind::Radio, count, title);
    }

    /// After every removal, a failed one included, so both tabs show what survived.
    fn finish(&self) {
        let Some(ui) = self.weak.upgrade() else { return };
        // A removed id left in the card selection would still be counted by the menu.
        ui.global::<CardSelection>().invoke_clear();
        kept::refresh(&ui, &self.state, &self.radio_ui);
    }

    /// The kept tab `list` names, or `None` for Browse, whose stations have no row to remove.
    fn kept_list(&self, list: i32) -> Option<KeptList> {
        let ui = self.weak.upgrade()?;
        match tab_from_index(&ui.global::<Radio>(), list) {
            RadioTab::Favorites => Some(KeptList::Favorites),
            RadioTab::Recent => Some(KeptList::Recent),
            RadioTab::Browse => None,
        }
    }
}

fn kept_tab(list: KeptList) -> RadioTab {
    match list {
        KeptList::Favorites => RadioTab::Favorites,
        KeptList::Recent => RadioTab::Recent,
    }
}
