//! The Rust half of the in-window drag, whose meaning lives on Slint's `Drag` global.
//!
//! Three writes, each where its occasion is: the token here, the disarm every press makes in
//! `window_chrome::winit_filter` with whether a touch made it, and a revoke from each list that
//! rebuilds its rows under a drag. [`lifted_run`] answers the one question Slint can't, where a
//! paged list's picks sit, for the press that lifts them.

use std::rc::Rc;

use slint::{ComponentHandle, DataTransfer};

use crate::ui::util::len_as_i32;
use melodia_ui::{AppWindow, Drag, LiftedRun};

/// Gives every `DragSource` the token it carries. Slint can build a `data-transfer` only through
/// the host, and a `DragArea` holding an empty one never starts a drag. User data alone, so no
/// backend can take a drag out of the window, the payload it stands for being `Drag`'s.
pub fn install(app: &AppWindow) {
    let mut token = DataTransfer::default();
    token.set_user_data(Rc::new(()));
    app.global::<Drag>().set_token(token);
}

/// Where a drag of picked `rows`, ascending, lifts them from, for the drop target to draw no line
/// where letting go would leave them be. Rows with a gap between them say so with a count of 0: a
/// drop anywhere gathers them, which changes the order.
pub fn lifted_run(rows: &[usize]) -> LiftedRun {
    match (rows.first(), rows.last()) {
        (Some(&first), Some(&last)) if first + rows.len() == last + 1 => {
            LiftedRun { first: len_as_i32(first), count: len_as_i32(rows.len()) }
        }
        _ => LiftedRun { first: -1, count: 0 },
    }
}
