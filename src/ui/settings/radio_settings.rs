//! Wire the Radio master switch to Rust.
//!
//! The thinnest of the section installers — one toggle, no credentials, no status watch.
//! What it does have that [`super::discord_settings`] doesn't is a teardown: turning the
//! feature off has to stop a station and forget where the page was, which
//! [`crate::ui::radio::disable`] owns.
//!
//! The shadow is written **before** the persist, not after it, so a directory call racing
//! the disk write reads the new answer. `library::radio` refuses on that shadow, which is
//! what makes "off" mean no traffic rather than a hidden sidebar row.

use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppWindow, Settings, library};

pub fn install_radio(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();
    g.set_radio_enabled(state.radio_enabled());

    let s = state.clone();
    let weak = ui.as_weak();
    g.on_radio_enabled_changed(move |on| {
        s.set_radio_enabled(on);
        if !on && let Some(app) = weak.upgrade() {
            crate::ui::radio::disable(&app, &s);
        }
        s.persist_blocking("set_radio_enabled", move |st| {
            library::settings::set_radio_enabled(st, on)
        });
    });
}
