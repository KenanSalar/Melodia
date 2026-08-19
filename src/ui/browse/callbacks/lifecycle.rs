//! Section enter/leave for Browse, and the `library_changed` subscriber behind it.

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi, NAV_BROWSE};
use crate::ui::callbacks::macros::spawn_logged;
use crate::{AppWindow, Browse, Nav};

/// Seed the section shadow, wire the gate, and subscribe to `library_changed`.
pub(super) fn wire(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // Seed the shadow from the current nav state: the gate's `ChangeTracker` baselines
    // inside `AppWindow::new()` and fires only on a later difference, so a section the
    // boot doesn't land on gets no edge at all, and the one it does land on gets its edge
    // a frame late — after boot has already read this shadow. See the `SectionActiveGate`
    // bullet in `.claude/rules/ui-patterns.md`.
    browse_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_BROWSE);
    // `browse::seed_from_settings` fetches whatever section the launch lands on, and off
    // screen that fetch *releases* what it warmed — `warm_card_tier` hands its buffers
    // back and reports `false`, so the card tier stays cold and nothing bumps the
    // generation. Seeding the flag here costs one re-fetch on the first visit to a Browse
    // the boot didn't land on, and nothing at all on the one it did. Same shape, same
    // reason, as the four detail lifecycles'.
    if !browse_ui.section_active() {
        browse_ui.mark_dirty();
    }

    // section-active-changed: mirror visibility into the synchronous shadow and, on
    // re-enter, re-fetch the current directory once if a `library_changed` bump arrived
    // while the section was hidden (the subscriber below marks dirty instead of
    // re-fetching a view the user can't see).
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            bu.set_section_active(active);
            if !active {
                // The card tier is Browse's only cache, and it is worth a section's
                // release: at grid-tier size a full LRU is tens of megabytes. The generation
                // rewinds beside it so `0` keeps meaning "cold" rather than "first toggle
                // of the session".
                //
                // **The release is only honest beside the `mark_dirty`** — the
                // `tracks/callbacks/lifecycle.rs` rule, and Browse is the other view with
                // no enter-time fetch of its own, so without it the re-enter paints every
                // card on its placeholder. Landed synchronously, *before* the release task
                // is spawned, so a re-enter can never read `false` off a tier the spawn is
                // about to empty.
                bu.mark_dirty();
                if let Some(ui) = weak.upgrade() {
                    ui.global::<Browse>().set_covers_generation(0);
                }
                let bu = bu.clone();
                s.runtime.spawn_blocking(move || bu.release_grid_covers());
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

    // library_changed subscriber: watcher / scan completion / folder add+remove all bump
    // this counter. Re-fetch the current path so new files appear, removed files
    // disappear, and the root view updates when folders are added/removed. Mirrors the
    // `ui::settings::library_settings` pattern.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        let mut rx = state.library_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            // The initial `borrow()` value is whatever the channel started at; mark it
            // seen so we don't re-fetch immediately.
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                // Skip the directory re-fetch (read_dir + a full-index LIKE scan) while
                // the section is hidden — a scan or a busy watcher can bump this channel
                // repeatedly, and re-fetching a view nobody is looking at is O(library)
                // per bump. Mark dirty so the next section-enter re-fetches once instead.
                if !bu.section_active() {
                    bu.mark_dirty();
                    continue;
                }
                let path = bu.current_path();
                let s = s.clone();
                let bu = bu.clone();
                let weak = weak.clone();
                spawn_logged!(
                    s,
                    "browse::library_changed",
                    browse_ui_mod::fetch_and_apply(&s, &bu, weak, path)
                );
            }
        }));
    }
}
