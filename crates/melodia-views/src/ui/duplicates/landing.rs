//! What both duplicates dialogs add to a ticketed dialog: rows read by id while the list scrolls.

use std::future::Future;
use std::rc::Rc;

use crate::ui::dialog_landing::TicketedDialog;
use crate::ui::paged_rows::{PagedRow, RowRequest, spawn_fetch};
use melodia_core::error::AppResult;

pub(super) trait DialogRows: TicketedDialog {
    /// What a failed [`Self::fetch_rows`] is logged under.
    const ROWS_LABEL: &'static str;

    /// Reads the rows `request` asks for, `build` making them out of what `query` read. A fetch
    /// landing after the ticket moved is dropped, which frees the list to ask again.
    fn fetch_rows<R: PagedRow, T: Send + 'static>(
        self: &Rc<Self>,
        request: RowRequest<R>,
        query: impl Future<Output = AppResult<T>> + Send + 'static,
        build: impl FnOnce(&Self, &[i64], T) -> Vec<(i64, R)> + 'static,
    ) {
        let ticket = self.ticket().get();
        let dialog = Rc::downgrade(self);
        let land = move |request: RowRequest<R>, answer: T| {
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            if dialog.ticket().get() == ticket {
                let rows = build(&dialog, request.ids(), answer);
                request.land(rows);
            }
        };
        spawn_fetch(self.runtime(), request, query, land, Self::ROWS_LABEL);
    }
}
