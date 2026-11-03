//! The duplicate finder's dialog: a find as it opens, the removal picks over its groups, Not
//! Duplicates, and the Remove behind its Confirm step. `kept_out` is the list of what Remove left.
//!
//! Every answer but Remove's lands under the ticket it was sent with (`landing`).

mod apply;
mod kept_out;
mod landing;
mod picks;
mod rows;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use slint::{ComponentHandle, ModelRc};
use tokio::runtime::Handle;

use crate::ui::paged_rows::{Keyset, PagedRows, ReadStamp, RowRequest};
use crate::ui::shell::notifications::NotificationsUi;
use crate::ui::util::{format_size, len_as_i32};
use landing::{DialogTicket, TicketedDialog};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, DialogKind, DuplicateRow, DuplicatesUi};
use picks::DuplicateSet;

/// Installs the finder's and the kept-out list's models and wires their globals. UI thread.
pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    kept_out::install(ui, state, notifications);
    let finder = Finder::new(ui, state, notifications);
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
    let f = finder.clone();
    dup.on_remove(move || f.remove());
    dup.on_released(move || finder.release());
}

struct Finder {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    notifications: Rc<NotificationsUi>,
    set: RefCell<DuplicateSet>,
    rows: Rc<PagedRows<DuplicateRow>>,
    ticket: DialogTicket,
    show_dismissed: Cell<bool>,
}

impl Finder {
    fn new(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            ui: ui.as_weak(),
            state: state.clone(),
            notifications: Rc::clone(notifications),
            set: RefCell::default(),
            rows: PagedRows::new(fetcher(this.clone())),
            ticket: DialogTicket::new(DialogKind::Duplicates),
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
        self.ticket.retire();
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
}

impl TicketedDialog for Finder {
    const LABEL: &'static str = "duplicates";
    const ROWS_LABEL: &'static str = "duplicate rows";

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

/// Reads the rows on screen by id, a header through its best copy.
fn fetcher(finder: Weak<Finder>) -> impl Fn(RowRequest<DuplicateRow>) + 'static {
    move |request| {
        let Some(finder) = finder.upgrade() else {
            return;
        };
        let ids = rows::track_ids_for(&finder.set.borrow(), request.ids());
        let db = finder.state.db.clone();
        let copies = async move { library::duplicates::copies(&db, &ids).await };
        finder.fetch_rows(request, copies, |finder, asked, copies| {
            rows::build(&finder.set.borrow(), asked, &copies)
        });
    }
}
