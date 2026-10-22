//! The Rust half of the in-window drag, whose meaning lives on Slint's `Drag` global.
//!
//! Three writes, each where its occasion is: the token here, the disarm every press makes in
//! `window_chrome::winit_filter`, and a revoke from each list that rebuilds its rows under a drag.

use std::rc::Rc;

use slint::{ComponentHandle, DataTransfer};

use melodia_ui::{AppWindow, Drag};

/// Gives every `DragSource` the token it carries. Slint can build a `data-transfer` only through
/// the host, and a `DragArea` holding an empty one never starts a drag. User data alone, so no
/// backend can take a drag out of the window, the payload it stands for being `Drag`'s.
pub fn install(app: &AppWindow) {
    let mut token = DataTransfer::default();
    token.set_user_data(Rc::new(()));
    app.global::<Drag>().set_token(token);
}
