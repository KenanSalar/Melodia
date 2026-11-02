//! The kept-out list's dialog, the files Remove left out of the library each with its Bring Back,
//! and the Settings row's count, which follows every library change.

use std::cell::Cell;
use std::future::Future;
use std::path::Path;
use std::rc::{Rc, Weak};

use slint::{ComponentHandle, ModelRc};

use super::landing::{DialogTicket, spawn_landing};
use super::rows::{day_of, file_and_folder};
use crate::ui::callbacks::clipboard::spawn_copy;
use crate::ui::paged_rows::{Keyset, PagedRow, PagedRows, ReadStamp, RowRequest, spawn_fetch};
use crate::ui::shell::notifications::{Completion, NotificationsUi, RowText};
use crate::ui::signal::on_signal;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_app::library;
use melodia_app::library::duplicates::kept_out::{self, BroughtBack};
use melodia_app::state::AppState;
use melodia_core::entities::duplicates::KeptOutFile;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, DialogKind, KeptOutRow, KeptOutUi};

/// Installs the list's model, wires `KeptOutUi` and keeps its count current. UI thread.
pub(super) fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let list = KeptOutList::new(ui, state, notifications);
    let kept = ui.global::<KeptOutUi>();
    kept.set_rows(ModelRc::from(list.rows.clone()));

    let l = list.clone();
    kept.on_load(move || l.load());
    let l = list.clone();
    kept.on_bring_back(move |id| l.bring_back(i64::from(id)));
    let l = list.clone();
    kept.on_bring_all_back(move || l.bring_all_back());
    let l = list.clone();
    kept.on_open_folder(move |path| l.open_folder(&path));
    let l = list.clone();
    kept.on_copy_all_paths(move || l.copy_all_paths());
    let l = list.clone();
    kept.on_released(move || l.release());

    list.recount();
    let followed = on_signal(&state.library_changed, ui.as_weak(), "kept-out files", move |ui| {
        list.recount();
        if list.ticket.shows(ui) {
            list.list();
        }
    });
    if let Err(e) = followed {
        log::warn!("The kept-out files won't follow the library: {}", describe(&e));
    }
}

struct KeptOutList {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    notifications: Rc<NotificationsUi>,
    rows: Rc<PagedRows<KeptOutRow>>,
    ticket: DialogTicket,
    /// The latest count read sent, so an earlier one landing late can't overwrite it.
    recounts: Cell<u64>,
}

impl KeptOutList {
    fn new(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            ui: ui.as_weak(),
            state: state.clone(),
            notifications: Rc::clone(notifications),
            rows: PagedRows::new(fetcher(this.clone())),
            ticket: DialogTicket::new(DialogKind::KeptOutFiles),
            recounts: Cell::new(0),
        })
    }

    /// The opener has already raised the dialog and set `busy`.
    fn load(self: &Rc<Self>) {
        self.rows.clear();
        self.list();
    }

    /// Reads the keyset under a new ticket, so a read or a row fetch sent before lands nowhere.
    fn list(self: &Rc<Self>) {
        self.ticket.retire();
        let db = self.state.db.clone();
        let read = ReadStamp::now();
        self.after(async move { kept_out::ids(&db).await }, move |ui, list, ids| {
            let kept = ui.global::<KeptOutUi>();
            match ids {
                Ok(ids) => {
                    list.rows.show(Keyset { ids: ids.into(), seed: Vec::new(), read });
                    // A rename since re-points a row the same id still names.
                    list.rows.reread();
                    kept.set_failed(false);
                }
                Err(e) => {
                    log::warn!("kept-out files: list failed: {}", describe(&e));
                    kept.set_failed(true);
                }
            }
            kept.set_busy(false);
        });
    }

    fn recount(self: &Rc<Self>) {
        let recount = self.recounts.get().wrapping_add(1);
        self.recounts.set(recount);
        let db = self.state.db.clone();
        let list = Rc::downgrade(self);
        let work = async move { kept_out::count(&db).await };
        spawn_landing(&self.state.runtime, "kept-out count", work, move |count| {
            let Some(list) = list.upgrade() else {
                return;
            };
            let Some(ui) = list.ui.upgrade() else {
                return;
            };
            match count {
                Some(Ok(count)) if list.recounts.get() == recount => {
                    ui.global::<KeptOutUi>().set_count(clamp_i64_to_i32(count));
                }
                Some(Ok(_)) | None => {}
                Some(Err(e)) => log::warn!("kept-out files: count failed: {}", describe(&e)),
            }
        });
    }

    fn bring_back(self: &Rc<Self>, id: i64) {
        let state = self.state.clone();
        self.bring(1, async move { kept_out::bring_back(&state, &[id]).await });
    }

    fn bring_all_back(self: &Rc<Self>) {
        let state = self.state.clone();
        let asked = self.rows.ids().len();
        self.bring(asked, async move { kept_out::bring_all_back(&state).await });
    }

    /// Runs a Bring Back and says what came of it. Like Remove's, its answer lands under any
    /// ticket: a list closed mid-run still brought the files back, and its toast says so.
    fn bring(
        self: &Rc<Self>,
        asked: usize,
        work: impl Future<Output = AppResult<BroughtBack>> + Send + 'static,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let kept = ui.global::<KeptOutUi>();
        if kept.get_bringing_back() {
            return;
        }
        let list = Rc::downgrade(self);
        let spawned =
            spawn_landing(&self.state.runtime, "kept-out bring back", work, move |brought| {
                let Some(list) = list.upgrade() else {
                    return;
                };
                let Some(ui) = list.ui.upgrade() else {
                    return;
                };
                ui.global::<KeptOutUi>().set_bringing_back(false);
                match brought {
                    Some(Ok(brought)) => list.show_brought_back(&ui, brought),
                    Some(Err(e)) => {
                        log::warn!("kept-out files: bring back failed: {}", describe(&e));
                        list.show_bring_back_failed(&ui, asked);
                    }
                    None => list.show_bring_back_failed(&ui, asked),
                }
            });
        if spawned {
            kept.set_bringing_back(true);
        }
    }

    /// Nothing to say where every file asked for had already left the list, the watcher having
    /// seen it deleted.
    fn show_brought_back(&self, ui: &AppWindow, brought: BroughtBack) {
        let missing = len_as_i32(brought.not_returned);
        if brought.returned == 0 {
            if missing > 0 {
                self.notifications.show_failure(ui, move |ui| {
                    let kept = ui.global::<KeptOutUi>();
                    RowText::plain(
                        kept.invoke_bring_back_failed_title(missing),
                        kept.invoke_not_brought_back_message(missing),
                    )
                });
            }
            return;
        }
        let kept = ui.global::<KeptOutUi>();
        self.notifications.show_completion(
            Completion::partial_if(missing > 0),
            kept.invoke_brought_back_title(len_as_i32(brought.returned)),
            kept.invoke_not_brought_back_message(missing),
        );
    }

    fn show_bring_back_failed(&self, ui: &AppWindow, asked: usize) {
        let asked = len_as_i32(asked);
        self.notifications.show_failure(ui, move |ui| {
            let kept = ui.global::<KeptOutUi>();
            RowText::plain(
                kept.invoke_bring_back_failed_title(asked),
                kept.invoke_bring_back_failed_message(),
            )
        });
    }

    fn open_folder(&self, path: &str) {
        let Some(folder) = Path::new(path).parent() else {
            return;
        };
        let folder = folder.to_path_buf();
        self.state.runtime.spawn(async move {
            if let Err(e) = library::tracks::reveal_folder(folder).await {
                log::warn!("kept-out files: reveal failed: {}", describe(&e));
            }
        });
    }

    fn copy_all_paths(&self) {
        spawn_copy(&self.state, &self.ui, "copy kept-out paths", |state| async move {
            Ok(kept_out::paths(&state.db).await?.join("\n"))
        });
    }

    /// The close's: the list reads as never opened, and nothing it asked for lands.
    fn release(&self) {
        self.ticket.retire();
        self.rows.clear();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let kept = ui.global::<KeptOutUi>();
        kept.set_busy(false);
        kept.set_failed(false);
    }

    /// The finder's `after`, under this list's ticket.
    fn after<T: Send + 'static>(
        self: &Rc<Self>,
        work: impl Future<Output = AppResult<T>> + Send + 'static,
        land: impl FnOnce(&AppWindow, &Self, AppResult<T>) + 'static,
    ) {
        let ticket = self.ticket.get();
        let list = Rc::downgrade(self);
        spawn_landing(&self.state.runtime, "kept-out files", work, move |answer| {
            let Some(answer) = answer else {
                return;
            };
            let Some(list) = list.upgrade() else {
                return;
            };
            let Some(ui) = list.ui.upgrade() else {
                return;
            };
            if list.ticket.holds(ticket, &ui) {
                land(&ui, &list, answer);
            }
        });
    }
}

impl PagedRow for KeptOutRow {
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// Reads the rows on screen by id. A fetch landing after the ticket moved is dropped, which frees
/// the list to ask again.
fn fetcher(list: Weak<KeptOutList>) -> impl Fn(RowRequest<KeptOutRow>) + 'static {
    move |request| {
        let Some(this) = list.upgrade() else {
            return;
        };
        let ids = request.ids().to_vec();
        let db = this.state.db.clone();
        let ticket = this.ticket.get();
        let list = list.clone();
        let land = move |request: RowRequest<KeptOutRow>, files: Vec<KeptOutFile>| {
            let Some(this) = list.upgrade() else {
                return;
            };
            if this.ticket.get() != ticket {
                return;
            }
            request.land(files.iter().map(|file| (file.id, row(file))).collect());
        };
        let files = async move { kept_out::files(&db, &ids).await };
        spawn_fetch(&this.state.runtime, request, files, land, "kept-out rows");
    }
}

fn row(file: &KeptOutFile) -> KeptOutRow {
    let (file_name, folder) = file_and_folder(&file.path);
    KeptOutRow {
        id: clamp_i64_to_i32(file.id),
        file_name,
        folder,
        path: file.path.as_str().into(),
        removed: day_of(&file.excluded_at).into(),
    }
}
