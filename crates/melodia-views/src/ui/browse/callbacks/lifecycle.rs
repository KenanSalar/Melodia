//! Section enter/leave for Browse, and the `library_changed` subscriber behind it.

use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::ui::browse::{self as browse_ui_mod, BrowseUi, NAV_BROWSE};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::signal::on_signal;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Browse, Nav};

pub(super) fn wire(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // Seeded here, the gate firing only on a later difference (`ui-patterns.md`'s
    // `SectionActiveGate` bullet).
    browse_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_BROWSE);
    // The boot seed fetches wherever the launch lands but warms no covers off screen, so a
    // Browse it didn't land on re-fetches once on its first visit.
    if !browse_ui.section_active() {
        browse_ui.mark_dirty();
    }

    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            bu.set_section_active(active);
            if !active {
                // Browse has no enter-time fetch of its own, so nothing else would refresh the
                // listing (`tracks/callbacks/lifecycle.rs`'s rule). Its card model survives the
                // leave, so the covers under it go back as proxies rather than dropped, or the
                // re-entry would paint placeholders.
                bu.mark_dirty();
                if let Some(ui) = weak.upgrade() {
                    // A selection the user can't see still answers `Selection.any-live()`,
                    // swallowing an Escape meant for the next page. Guarded, the unstamp walking
                    // every row.
                    if ui.global::<Browse>().get_selected_ids().row_count() > 0 {
                        browse_ui_mod::clear_selection(&ui);
                    }
                }
                s.runtime.spawn_blocking(crate::ui::grid_prewarm::hand_back_covers);
                return;
            }
            if bu.take_dirty() {
                let path = bu.current_path();
                let s = s.clone();
                let bu = bu.clone();
                let weak = weak.clone();
                spawn_logged!(
                    s,
                    "browse::section_enter",
                    browse_ui_mod::fetch_and_apply(&s, &bu, weak, path)
                );
            }
        });
    }

    install_library_refresher(ui, state, browse_ui);
}

/// Re-fetches the current path on `library_changed`, which a watcher batch, a scan or a folder
/// change bumps.
fn install_library_refresher(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let s = state.clone();
    let bu = browse_ui.clone();
    let installed =
        on_signal(&state.library_changed, ui.as_weak(), "browse-library-changed", move |ui| {
            // Hidden, a re-fetch costs a directory read and a library-wide scan per bump, so the
            // next enter does it once instead.
            if !bu.section_active() {
                bu.mark_dirty();
                return;
            }
            let path = bu.current_path();
            let s = s.clone();
            let bu = bu.clone();
            let weak = ui.as_weak();
            spawn_logged!(
                s,
                "browse::library_changed",
                browse_ui_mod::fetch_and_apply(&s, &bu, weak, path)
            );
        });
    if let Err(e) = installed {
        log::warn!("Browse won't follow library changes: {}", describe(&e));
    }
}
