//! The Import dialog: a paste or a file read into a profile, the lines it won't use listed, then
//! its filters set for the card's device. The profile under review is held here until Apply or
//! the close.

use std::cell::RefCell;
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;

use crate::ui::dialog_landing::{DialogTicket, TicketedDialog};
use crate::ui::file_dialog;
use crate::ui::headphones::Card;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::library::correction::{ImportError, Imported};
use melodia_app::state::AppState;
use melodia_core::entities::correction::{
    CorrectionOrigin, FilterError, FilterSetError, ProfileRefusal, SkipReason, SkippedLine,
};
use melodia_core::error::describe;
use melodia_ui::{
    AppWindow, CorrectionImport, Dialog, DialogKind, ImportRefusal, ImportStep, SkipWhy,
    SkippedLineRow,
};

pub(super) fn install(ui: &AppWindow, card: &Rc<Card>) {
    let import = Rc::new(Import {
        ui: ui.as_weak(),
        state: card.state().clone(),
        card: Rc::clone(card),
        ticket: DialogTicket::new(DialogKind::CorrectionImport),
        skipped: Rc::new(VecModel::default()),
        reviewed: RefCell::new(None),
    });
    let g = ui.global::<CorrectionImport>();
    g.set_skipped(ModelRc::from(import.skipped.clone()));

    let i = Rc::clone(&import);
    g.on_choose_file(move || i.choose_file());
    let i = Rc::clone(&import);
    g.on_review(move |text| i.review_paste(&text));
    let i = Rc::clone(&import);
    g.on_apply(move || i.apply());
    g.on_released(move || import.release());
}

struct Import {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    card: Rc<Card>,
    ticket: DialogTicket,
    skipped: Rc<VecModel<SkippedLineRow>>,
    reviewed: RefCell<Option<Imported>>,
}

impl Import {
    /// Parsed on the UI thread, a paste being capped small.
    fn review_paste(&self, text: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        self.land(&ui, library::correction::import_paste(text));
    }

    /// The ticket is taken before the native dialog, so a file chosen after the dialog closed or
    /// reopened lands nowhere.
    fn choose_file(self: &Rc<Self>) {
        let ticket = self.ticket.get();
        let picking = file_dialog::parented(&self.ui, "Import a Correction")
            .add_filter("Parametric EQ", &["txt"])
            .pick_file();
        let import = Rc::downgrade(self);
        let spawned = slint::spawn_local(Compat::new(async move {
            let Some(file) = picking.await else { return };
            let Some(import) = import.upgrade() else { return };
            let Some(ui) = import.ui.upgrade() else { return };
            if !import.ticket.holds(ticket, &ui) {
                return;
            }
            ui.global::<CorrectionImport>().set_busy(true);
            let path = file.path().to_path_buf();
            let read = import
                .state
                .runtime
                .spawn_blocking(move || library::correction::import_file(&path));
            import.after_any_ticket("correction import", read, move |ui, import, read| {
                ui.global::<CorrectionImport>().set_busy(false);
                if !import.ticket.holds(ticket, ui) {
                    return;
                }
                match read {
                    Some(Ok(read)) => import.land(ui, read),
                    Some(Err(e)) => log::warn!("correction import task failed: {}", describe(&e)),
                    None => {}
                }
            });
        }));
        if let Err(e) = spawned {
            log::warn!("correction import: file dialog not spawned: {}", describe(&e));
        }
    }

    /// A read profile goes to the review; a refused one stays on the paste step saying why.
    fn land(&self, ui: &AppWindow, read: Result<Imported, ImportError>) {
        let g = ui.global::<CorrectionImport>();
        let imported = match read {
            Ok(imported) => imported,
            Err(e) => {
                log::info!("correction import refused: {}", describe(&e));
                let (refusal, count) = refusal(&e);
                g.set_refusal(refusal);
                g.set_refused_count(len_as_i32(count));
                return;
            }
        };
        let file_name = match &imported.origin {
            CorrectionOrigin::File { name } => name.as_str(),
            CorrectionOrigin::AutoEq { .. } | CorrectionOrigin::Paste => "",
        };
        g.set_file_name(file_name.into());
        g.set_filter_count(len_as_i32(imported.profile.filters.filters().len()));
        self.skipped.set_vec(imported.profile.skipped.iter().map(skipped_row).collect::<Vec<_>>());
        g.set_refusal(ImportRefusal::None);
        g.set_step(ImportStep::Review);
        *self.reviewed.borrow_mut() = Some(imported);
    }

    fn apply(&self) {
        let (Some(imported), Some(target)) = (self.reviewed.take(), self.card.target()) else {
            return;
        };
        library::correction::apply(&self.state, target, imported);
        self.card.paint();
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<Dialog>().set_open(false);
        }
    }

    fn release(&self) {
        self.ticket.retire();
        self.reviewed.take();
        self.skipped.set_vec(Vec::new());
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<CorrectionImport>();
        g.set_busy(false);
        g.set_refusal(ImportRefusal::None);
        g.set_refused_count(0);
        g.set_file_name("".into());
        g.set_filter_count(0);
    }
}

/// The refusal, and the filter count a file holding too many states.
fn refusal(error: &ImportError) -> (ImportRefusal, usize) {
    match error {
        ImportError::Read(_) => (ImportRefusal::Unread, 0),
        ImportError::TooLarge => (ImportRefusal::TooLarge, 0),
        ImportError::Refused(ProfileRefusal::OneEar) => (ImportRefusal::OneEar, 0),
        ImportError::Refused(ProfileRefusal::Filters(FilterSetError::TooMany(count))) => {
            (ImportRefusal::TooMany, *count)
        }
        // A filter out of range is a skipped line by then, so only an empty set reaches here.
        ImportError::Refused(ProfileRefusal::Filters(
            FilterSetError::Empty | FilterSetError::Filter { .. },
        )) => (ImportRefusal::NoFilters, 0),
    }
}

fn skipped_row(line: &SkippedLine) -> SkippedLineRow {
    SkippedLineRow {
        number: len_as_i32(line.number),
        text: line.text.as_ref().into(),
        reason: skip_why(line.reason),
    }
}

fn skip_why(reason: SkipReason) -> SkipWhy {
    match reason {
        SkipReason::Off => SkipWhy::Off,
        SkipReason::Unsupported => SkipWhy::Unsupported,
        SkipReason::OutOfRange(FilterError::Frequency(_)) => SkipWhy::Frequency,
        SkipReason::OutOfRange(FilterError::Gain(_)) => SkipWhy::Gain,
        SkipReason::OutOfRange(FilterError::Q(_)) => SkipWhy::Q,
        SkipReason::Unreadable => SkipWhy::Unreadable,
    }
}

impl TicketedDialog for Import {
    const LABEL: &'static str = "correction import";

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
