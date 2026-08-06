//! `Browse.*` callbacks: folder navigation, breadcrumbs, sort, play actions,
//! library-changed re-fetch.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, SharedString};

use super::{collect_nonzero_track_ids, next_sort, play_row_start};
use super::macros::{spawn_blocking_logged, spawn_logged, wire_row_flag};
use crate::library;
use crate::state::AppState;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::tab_bar::should_announce_warm;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, Browse};

/// Wire every `Browse.*` callback to its `library::*` counterpart and the
/// `browse_ui` shared state, plus a `library_changed_tx` subscriber that
/// re-fetches the current path on watcher events. Call once after
/// `wire_all` and after `browse::install_browse_models`.
pub fn wire_browse(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // Seed the sort header + the `BrowseUi` sort cache from the persisted
    // `view_sort["browse"]` so the first folder navigation sorts with it.
    if let Some((field, dir)) = super::persisted_sort(state, view_id::BROWSE) {
        g.set_sort_field(SharedString::from(field.as_str()));
        g.set_sort_dir(SharedString::from(dir));
        browse_ui.set_sort(field, dir.to_owned());
    }

    // section-active-changed: mirror visibility into the synchronous shadow
    // and, on re-enter, re-fetch the current directory once if a
    // `library_changed` bump arrived while the section was hidden (the
    // subscriber below marks dirty instead of re-fetching a view the user
    // can't see). Seed the shadow from the current nav state (sidebar index
    // 1): the gate's `ChangeTracker` baselines inside `AppWindow::new()` and
    // fires only on a later difference, so a section the boot doesn't land on
    // gets no edge at all, and the one it does land on gets its edge a frame
    // late — after boot has already read this shadow. See the
    // `SectionActiveGate` bullet in `.claude/rules/ui-patterns.md`.
    browse_ui.set_section_active(ui.global::<crate::Nav>().get_selected_index() == 1);
    // `browse::seed_from_settings` fetches whatever section the launch lands on,
    // and off screen that fetch *releases* what it warmed — `warm_card_tier`
    // hands its buffers back and reports `false`, so the card tier stays cold
    // and nothing bumps the generation. Seeding the flag here costs one
    // re-fetch on the first visit to a Browse the boot didn't land on, and
    // nothing at all on the one it did. Same shape, same reason, as the four
    // detail lifecycles'.
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
                // The card tier is Browse's only cache, and it is worth a
                // section's release: at 448 px a full LRU is tens of megabytes.
                // The generation rewinds beside it so `0` keeps meaning "cold"
                // rather than "first toggle of the session".
                //
                // **The release is only honest beside the `mark_dirty`** — the
                // `callbacks/tracks.rs` rule, and Browse is the other view with
                // no enter-time fetch of its own, so without it the re-enter
                // paints every card on its placeholder. Landed synchronously,
                // *before* the release task is spawned, so a re-enter can never
                // read `false` off a tier the spawn is about to empty.
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
                spawn_logged!(s, "browse::section_enter",
                    browse_ui_mod::fetch_and_apply(&s, &bu, weak, path));
            }
        });
    }

    // open-folder: push current path onto history, spawn fetch for the
    // new path, persist the new browse_path. The Slint TouchArea fires
    // this on a folder-row click.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_open_folder(move |path| {
            let path = path.to_string();
            let leaving = bu.current_path();
            bu.push_history(leaving, path.clone());

            let s_fetch = s.clone();
            let bu_fetch = bu.clone();
            let weak_fetch = weak.clone();
            let path_fetch = path.clone();
            spawn_logged!(s_fetch, "browse::open_folder",
                browse_ui_mod::fetch_and_apply(&s_fetch, &bu_fetch, weak_fetch, path_fetch));

            // Persist the new browse_path on the blocking pool.
            let s_disk = s.clone();
            let path_disk = path;
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_browse_path(&s_disk, Some(path_disk))
                {
                    log::warn!("browse::set_browse_path: {e}");
                }
            });
        });
    }

    // go-back: pop history, spawn fetch for the popped path, persist.
    // No-op when history is empty.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_go_back(move || {
            let Some(target) = bu.pop_history() else {
                return;
            };

            let s_fetch = s.clone();
            let bu_fetch = bu.clone();
            let weak_fetch = weak.clone();
            let target_fetch = target.clone();
            spawn_logged!(s_fetch, "browse::go_back",
                browse_ui_mod::fetch_and_apply(&s_fetch, &bu_fetch, weak_fetch, target_fetch));

            let s_disk = s.clone();
            let target_disk = (!target.is_empty()).then_some(target);
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_browse_path(&s_disk, target_disk)
                {
                    log::warn!("browse::set_browse_path: {e}");
                }
            });
        });
    }

    // navigate-to: breadcrumb segment click. Truncate history down to
    // (and including) the target if it's in the stack — gives "click
    // ancestor → jump back N levels" semantics. Empty string = root.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_navigate_to(move |path| {
            let path = path.to_string();
            bu.truncate_history_to(&path);

            let s_fetch = s.clone();
            let bu_fetch = bu.clone();
            let weak_fetch = weak.clone();
            let path_fetch = path.clone();
            spawn_logged!(s_fetch, "browse::navigate_to",
                browse_ui_mod::fetch_and_apply(&s_fetch, &bu_fetch, weak_fetch, path_fetch));

            let s_disk = s.clone();
            let path_disk = (!path.is_empty()).then_some(path);
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_browse_path(&s_disk, path_disk)
                {
                    log::warn!("browse::set_browse_path: {e}");
                }
            });
        });
    }

    // play-row: double-click loads every in-library file in this folder into
    // the queue and starts on the clicked one. Disk-only rows (`id == 0`)
    // aren't in the library and are ignored — they also *displace* the row
    // index, since `current_in_library_ids` drops them, which is the case
    // `play_row_start`'s lookup-by-id fallback exists for.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        g.on_play_row(move |track_id, idx| {
            let id = i64::from(track_id);
            if id == 0 {
                return;
            }
            let ids = bu.current_in_library_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, id, idx);
            let s = s.clone();
            spawn_logged!(s, "browse::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }

    // play-next / add-to-queue: context-menu actions. Single-row and
    // multi-select both flow through the same callback as `[int]`.
    // Disk-only rows (`id == 0`) are filtered out — they aren't in the
    // library and can't be queued. (Selection-level gate at
    // `browse/selection.rs` already keeps disk-only ids out of
    // `Browse.selected-ids`; this filter is a belt-and-braces for
    // single-row mode if anything ever changes upstream.)
    {
        let s = state.clone();
        g.on_play_next(move |ids| {
            let id_vec = collect_nonzero_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        g.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> =
                ids.iter().map(i64::from).filter(|&id| id != 0).collect();
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::add_to_queue",
                library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each row (no re-fetch, so scroll position holds and there's no
    // flash). Disk-only rows have id 0 and are filtered out by
    // `collect_nonzero_track_ids`; rating never changes list membership.
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "browse::set_favorite",
            library::favorites::set_favorite, collect_nonzero_track_ids,
            captures: [weak, bu],
            after: |id_vec, fav| {
                for id in &id_vec {
                    bu.flip_favorite(*id, fav);
                    browse_ui_mod::apply_row_favorite(&weak, *id, fav);
                }
            });
    }
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "browse::set_rating",
            library::ratings::set_rating, collect_nonzero_track_ids,
            captures: [weak, bu],
            after: |id_vec, rating| {
                for id in &id_vec {
                    bu.flip_rating(*id, rating);
                    browse_ui_mod::apply_row_rating(&weak, *id, rating);
                }
            });
    }

    // request-sort: clicking a header column. Same field flips dir; new
    // field resets to ascending. Browse sorts in-memory (it mixes
    // disk-only + DB files) — no DB round-trip.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Browse>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            bu.set_sort(new_field.clone(), new_dir.as_str().to_owned());
            super::persist_view_sort(&s, view_id::BROWSE, new_field, new_dir);
            browse_ui_mod::resort_and_apply(&ui, &bu);
        });
    }

    // select-row / clear-selection: modifier-aware selection. The new
    // selected set is computed in Rust (Slint expressions can't iterate
    // a model for a membership check); disk-only rows are never
    // selectable. Mirrors the Tracks view.
    {
        let weak = weak.clone();
        let bu = browse_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::handle_select_row(&ui, &bu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::clear_selection(&ui);
        });
    }

    // toggle-column: the popup already flipped the matching `show-*`
    // flag for instant visual feedback. Persist the new visible-column
    // list to `views.json`'s `view_columns["browse"]` — a separate key from
    // the Tracks view, so Browse keeps its own column layout.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<Browse>().snapshot_visible();
            let s = s.clone();
            spawn_blocking_logged!(s, "browse::toggle_column",
                library::settings::update_view_columns(&s, "browse".to_string(), columns));
        });
    }

    // toggle-view-mode: the pill means "switch", so Rust negates. The card model
    // is rebuilt from the cached listing rather than re-fetched, and **without
    // hopping the event loop** — `invoke_from_event_loop` posts even when called
    // from the UI thread, and a redraw winning that race paints an empty grid.
    //
    // A toggle is the one path with no fetch to await before the grid mounts, so
    // it takes the `covers-generation` pair: rewind to 0 so the mounting cards
    // ask the tier cache-only, warm a screenful off-thread, then bump — gated on
    // the view still being where the prewarm left it.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_toggle_view_mode(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Browse>();
            let mode = bu.view_mode().toggled();
            let mode_idx = browse_ui_mod::mode_index(&g, mode);
            bu.set_view_mode(mode);
            g.set_view_mode(mode_idx);
            g.set_covers_generation(0);
            browse_ui_mod::rebuild_cards(&ui, &bu);

            let bu_work = bu.clone();
            let weak_bump = weak.clone();
            if mode == browse_ui_mod::BrowseViewMode::Card {
                s.runtime.spawn(async move {
                    let bu_prewarm = bu_work.clone();
                    // A `JoinError` is the same "we don't know" as a prewarm that
                    // handed its buffers back.
                    // `Some(Card)` is "we decoded for the card tier and still hold
                    // it" — the shape `should_announce_warm` takes, with the mode
                    // standing in for the tab a grid page would pass.
                    let warmed = tokio::task::spawn_blocking(move || {
                        bu_prewarm.prewarm_card_covers()
                    })
                    .await
                    .unwrap_or(false)
                    .then_some(browse_ui_mod::BrowseViewMode::Card);
                    let _ = weak_bump.upgrade_in_event_loop(move |ui| {
                        // Both shadows are written on this thread, so this is the
                        // same re-check the prewarm made, against anything that
                        // landed after it returned.
                        if should_announce_warm(
                            warmed,
                            bu_work.section_active(),
                            bu_work.view_mode(),
                        ) {
                            let g = ui.global::<Browse>();
                            g.set_covers_generation(g.get_covers_generation() + 1);
                        }
                    });
                });
            } else {
                let bu_release = bu.clone();
                s.runtime
                    .spawn_blocking(move || bu_release.release_grid_covers());
            }

            let s_disk = s.clone();
            spawn_blocking_logged!(s_disk, "browse::set_view_mode",
                library::settings::set_browse_view_mode(&s_disk, mode_idx));
        });
    }

    // columns-changed: the grid re-flowed, so re-chunk the same cards into rows
    // of the new width. No fetch, no DB — and a no-op while the list is mounted,
    // `GridColumnsSync` firing at mount regardless of which body is up.
    {
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::rebuild_cards(&ui, &bu);
        });
    }

    // request-card-cover: one card's thumbnail off Browse's own 448 px tier,
    // decoded only once `covers-generation` says the tier is warm.
    {
        let bu = browse_ui.clone();
        g.on_request_card_cover(move |path, generation| bu.grid_cover(&path, generation));
    }

    // library_changed subscriber: watcher / scan completion / folder
    // add+remove all bump this counter. Re-fetch the current path so
    // new files appear, removed files disappear, and the root view
    // updates when folders are added/removed. Mirrors the
    // `ui::library_settings.rs:74` pattern.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        let mut rx = state.library_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            // The initial `borrow()` value is whatever the channel started
            // at; mark it seen so we don't re-fetch immediately.
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                // Skip the directory re-fetch (read_dir + a full-index LIKE
                // scan) while the section is hidden — a scan or a busy
                // watcher can bump this channel repeatedly, and re-fetching
                // a view nobody is looking at is O(library) per bump. Mark
                // dirty so the next section-enter re-fetches once instead.
                if !bu.section_active() {
                    bu.mark_dirty();
                    continue;
                }
                let path = bu.current_path();
                let s = s.clone();
                let bu = bu.clone();
                let weak = weak.clone();
                spawn_logged!(s, "browse::library_changed",
                    browse_ui_mod::fetch_and_apply(&s, &bu, weak, path));
            }
        }));
    }
}
