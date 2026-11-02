//! The duplicate finder's dialog: a find as it opens, the removal picks over its groups, and Not
//! Duplicates.
//!
//! Every answer lands under the ticket it was sent with, which the next find and the dialog's
//! close both move. The dialog is already up while it fills, so `DialogClaim`, which guards a
//! raise still to come, can't tell whether this one is still wanted.

mod picks;
mod rows;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::future::Future;
use std::rc::{Rc, Weak};

use slint::{ComponentHandle, ModelRc};

use crate::ui::paged_rows::{Keyset, PagedRows, ReadStamp, RowRequest, spawn_fetch};
use crate::ui::util::{format_size, len_as_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, Dialog, DialogKind, DuplicateRow, DuplicatesUi};
use picks::DuplicateSet;

/// Installs the finder's model and wires `DuplicatesUi`. UI thread.
pub fn install(ui: &AppWindow, state: &AppState) {
    let finder = Finder::new(ui, state);
    let dup = ui.global::<DuplicatesUi>();
    dup.set_rows(ModelRc::from(finder.rows.clone()));

    let f = finder.clone();
    dup.on_find(move || f.find());
    let f = finder.clone();
    dup.on_toggle(move |id| f.repick(|set| set.toggle(i64::from(id))));
    let f = finder.clone();
    dup.on_keep_copy(move |id| f.repick(|set| set.keep(i64::from(id))));
    let f = finder.clone();
    dup.on_keep_folder(move |id| f.repick(|set| set.keep_folder(i64::from(id))));
    let f = finder.clone();
    dup.on_suggest(move || f.repick(DuplicateSet::suggest_all));
    let f = finder.clone();
    dup.on_clear_picks(move || f.repick(DuplicateSet::clear_picks));
    let f = finder.clone();
    dup.on_set_show_dismissed(move |show| {
        f.show_dismissed.set(show);
        f.publish();
    });
    let f = finder.clone();
    dup.on_dismiss_group(move |id| f.dismiss(i64::from(id)));
    let f = finder.clone();
    dup.on_restore_group(move |id| f.restore(i64::from(id)));
    dup.on_released(move || finder.release());
}

struct Finder {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    set: RefCell<DuplicateSet>,
    rows: Rc<PagedRows<DuplicateRow>>,
    ticket: Cell<u64>,
    show_dismissed: Cell<bool>,
}

impl Finder {
    fn new(ui: &AppWindow, state: &AppState) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            ui: ui.as_weak(),
            state: state.clone(),
            set: RefCell::default(),
            rows: PagedRows::new(fetcher(this.clone())),
            ticket: Cell::new(0),
            show_dismissed: Cell::new(false),
        })
    }

    /// The opener has already raised the dialog and set `busy`, so only Rust's side starts over.
    fn find(self: &Rc<Self>) {
        self.forget();
        let db = self.state.db.clone();
        self.after(async move { library::duplicates::find(&db).await }, |ui, finder, found| {
            let dup = ui.global::<DuplicatesUi>();
            match found {
                Ok(groups) => {
                    let mut set = DuplicateSet::new(&groups);
                    set.suggest_all();
                    *finder.set.borrow_mut() = set;
                    finder.publish();
                    dup.set_failed(false);
                }
                Err(e) => {
                    log::warn!("duplicates: find failed: {}", describe(&e));
                    finder.write_counts(&dup);
                    dup.set_failed(true);
                }
            }
            dup.set_busy(false);
        });
    }

    fn dismiss(self: &Rc<Self>, header_id: i64) {
        let Some(ids) = self.set.borrow().copy_ids(header_id) else {
            return;
        };
        let db = self.state.db.clone();
        self.after(
            async move { library::duplicates::dismiss(&db, &ids).await },
            move |_, finder, dismissed| match dismissed {
                Ok(set) => finder.regroup(header_id, |picks| picks.mark_dismissed(header_id, set)),
                Err(e) => log::warn!("duplicates: Not Duplicates failed: {}", describe(&e)),
            },
        );
    }

    fn restore(self: &Rc<Self>, header_id: i64) {
        let Some(set) = self.set.borrow().dismissal(header_id) else {
            return;
        };
        let db = self.state.db.clone();
        self.after(
            async move { library::duplicates::restore(&db, set).await },
            move |_, finder, restored| match restored {
                Ok(()) => finder.regroup(header_id, |picks| picks.mark_restored(header_id)),
                Err(e) => log::warn!("duplicates: Count as Duplicates failed: {}", describe(&e)),
            },
        );
    }

    /// The close's: everything a find held goes, and the surface reads as never opened.
    fn release(&self) {
        self.forget();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let dup = ui.global::<DuplicatesUi>();
        dup.set_busy(false);
        dup.set_failed(false);
        self.write_counts(&dup);
    }

    fn forget(&self) {
        self.ticket.set(self.ticket.get().wrapping_add(1));
        self.show_dismissed.set(false);
        *self.set.borrow_mut() = DuplicateSet::default();
        self.rows.clear();
        self.rows.select(&HashSet::new());
    }

    fn repick(&self, edit: impl FnOnce(&mut DuplicateSet)) {
        edit(&mut self.set.borrow_mut());
        self.show_picks();
    }

    /// A group's dismissal moved: its cached rows repaint the flag, and the list drops or keeps it.
    fn regroup(&self, header_id: i64, edit: impl FnOnce(&mut DuplicateSet)) {
        edit(&mut self.set.borrow_mut());
        let (ids, dismissed) = {
            let set = self.set.borrow();
            (set.group_row_ids(header_id), set.dismissal(header_id).is_some())
        };
        for id in ids {
            self.rows.patch(id, |row| row.dismissed = dismissed);
        }
        self.publish();
    }

    fn publish(&self) {
        let ids = self.set.borrow().listed_ids(self.show_dismissed.get());
        self.rows.show(Keyset { ids, seed: Vec::new(), read: ReadStamp::now() });
        self.show_picks();
    }

    fn show_picks(&self) {
        let picked = self.set.borrow().picked_ids();
        self.rows.select(&picked);
        if let Some(ui) = self.ui.upgrade() {
            self.write_counts(&ui.global::<DuplicatesUi>());
        }
    }

    fn write_counts(&self, dup: &DuplicatesUi<'_>) {
        let set = self.set.borrow();
        let show_dismissed = self.show_dismissed.get();
        dup.set_group_count(len_as_i32(set.listed_count(show_dismissed)));
        dup.set_dismissed_count(len_as_i32(set.dismissed_count()));
        dup.set_show_dismissed(show_dismissed);
        dup.set_picked_count(len_as_i32(set.picked_count()));
        dup.set_picked_size(format_size(set.picked_bytes()).into());
    }

    /// Runs `work` on the runtime and hands its answer to `land` on the UI thread, while the
    /// ticket it was sent under is current and the dialog still shows the finder.
    fn after<T: Send + 'static>(
        self: &Rc<Self>,
        work: impl Future<Output = AppResult<T>> + Send + 'static,
        land: impl FnOnce(&AppWindow, &Self, AppResult<T>) + 'static,
    ) {
        let ticket = self.ticket.get();
        let answer = self.state.runtime.spawn(work);
        let finder = Rc::downgrade(self);
        let spawned = slint::spawn_local(async_compat::Compat::new(async move {
            let answer = match answer.await {
                Ok(answer) => answer,
                Err(e) => {
                    log::warn!("duplicates: task failed: {}", describe(&e));
                    return;
                }
            };
            let Some(finder) = finder.upgrade() else {
                return;
            };
            let Some(ui) = finder.ui.upgrade() else {
                return;
            };
            if finder.ticket.get() == ticket && shows_finder(&ui) {
                land(&ui, &finder, answer);
            }
        }));
        if let Err(e) = spawned {
            log::warn!("duplicates: not spawned: {}", describe(&e));
        }
    }
}

fn shows_finder(ui: &AppWindow) -> bool {
    let dialog = ui.global::<Dialog>();
    dialog.get_open() && dialog.get_kind() == DialogKind::Duplicates
}

/// Reads the rows on screen by id, a header through its best copy. A fetch landing after the
/// ticket moved is dropped, which frees the list to ask again.
fn fetcher(finder: Weak<Finder>) -> impl Fn(RowRequest<DuplicateRow>) + 'static {
    move |request| {
        let Some(this) = finder.upgrade() else {
            return;
        };
        let ids = rows::track_ids_for(&this.set.borrow(), request.ids());
        let db = this.state.db.clone();
        let ticket = this.ticket.get();
        let finder = finder.clone();
        let land = move |request: RowRequest<DuplicateRow>, copies: Vec<_>| {
            let Some(this) = finder.upgrade() else {
                return;
            };
            if this.ticket.get() != ticket {
                return;
            }
            let rows = rows::build(&this.set.borrow(), request.ids(), &copies);
            request.land(rows);
        };
        let copies = async move { library::duplicates::copies(&db, &ids).await };
        spawn_fetch(&this.state.runtime, request, copies, land, "duplicate rows");
    }
}
