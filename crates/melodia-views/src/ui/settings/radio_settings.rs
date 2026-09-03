//! Wire the Radio switches to Rust.
//!
//! No credentials and no status watch, so what distinguishes this from
//! [`super::discord_settings`] is the master switch's teardown: turning the feature off has
//! to stop a station and forget where the page was, which [`crate::ui::radio::disable`]
//! owns.
//!
//! Every shadow is written **before** its persist, not after it, so a directory call racing
//! the disk write reads the new answer. `library::radio` refuses on the master shadow, which
//! is what makes "off" mean no traffic rather than a hidden sidebar row; the other two are
//! read the same way, per page and per play.

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Settings};

pub fn install_radio(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();
    g.set_radio_enabled(state.radio_enabled.get());
    g.set_radio_hide_segmented(state.radio_hide_segmented.get());
    g.set_radio_send_clicks(state.radio_send_clicks.get());

    {
        let s = state.clone();
        let weak = ui.as_weak();
        g.on_radio_enabled_changed(move |on| {
            s.radio_enabled.set(on);
            if !on && let Some(app) = weak.upgrade() {
                crate::ui::radio::disable(&app, &s);
            }
            s.persist_blocking("set_radio_enabled", move |st| {
                library::settings::set_radio_enabled(st, on)
            });
        });
    }

    {
        // No refetch behind it: the filter applies to the next page rather than re-thinning
        // the one on screen, so a flip mid-browse takes effect at the next search or
        // load-more. Re-running the query here would discard the user's paging to change
        // which stations are offered, not whether any of them play.
        //
        // The chip lists are dropped rather than left, because `FacetIndex::prime` skips a list
        // it already holds: without this the Format chip would keep offering the pre-flip set for
        // the rest of the session.
        let s = state.clone();
        let weak = ui.as_weak();
        g.on_radio_hide_segmented_changed(move |hide| {
            s.radio_hide_segmented.set(hide);
            if let Some(app) = weak.upgrade() {
                crate::ui::radio::forget_facets(&app);
            }
            s.persist_blocking("set_radio_hide_segmented", move |st| {
                library::settings::set_radio_hide_segmented(st, hide)
            });
        });
    }

    {
        let s = state.clone();
        g.on_radio_send_clicks_changed(move |send| {
            s.radio_send_clicks.set(send);
            s.persist_blocking("set_radio_send_clicks", move |st| {
                library::settings::set_radio_send_clicks(st, send)
            });
        });
    }
}
