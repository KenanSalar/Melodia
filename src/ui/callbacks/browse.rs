//! `Browse.*` callbacks: folder navigation, breadcrumbs, sort, play actions,
//! library-changed re-fetch.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, SharedString};

use super::collect_nonzero_track_ids;
use super::macros::{spawn_logged, spawn_logged_sync};
use crate::library;
use crate::state::AppState;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
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
    // can't see). Seed the shadow from the current nav state — `changed`
    // in `AppWindow` won't fire for a session that *starts* on Browse
    // (sidebar index 1).
    browse_ui.set_section_active(ui.global::<crate::Nav>().get_selected_index() == 1);
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            bu.set_section_active(active);
            if active && bu.take_dirty() {
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

    // play-row: double-click on a library track appends just that track
    // to the queue (skipping duplicates). Disk-only rows (`id == 0`)
    // aren't in the library and are ignored. Use `play-all` for "load
    // every in-library file in this folder".
    {
        let s = state.clone();
        g.on_play_row(move |track_id, _idx| {
            let id = i64::from(track_id);
            if id == 0 {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::play_row",
                library::queue::queue_append_unique(&s, id));
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

    // toggle-row-favorite: write through, then surgically update each
    // row (no re-fetch, so scroll position holds and there's no flash).
    // Disk-only rows can't be favorited.
    {
        let s = state.clone();
        let weak = weak.clone();
        let bu = browse_ui.clone();
        g.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_nonzero_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let weak = weak.clone();
            let bu = bu.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::favorites::set_favorite(&s, id_vec.clone(), fav).await
                {
                    log::warn!("browse::set_favorite: {e}");
                    return;
                }
                for id in &id_vec {
                    bu.flip_favorite(*id, fav);
                    browse_ui_mod::apply_row_favorite(&weak, *id, fav);
                }
            });
        });
    }

    // set-row-rating: mirror the favorite path (disk-only rows have id 0 and
    // are filtered out by `collect_nonzero_track_ids`). Rating never changes
    // list membership, so a surgical per-row patch suffices.
    {
        let s = state.clone();
        let weak = weak.clone();
        let bu = browse_ui.clone();
        g.on_set_row_rating(move |ids, rating| {
            let id_vec = collect_nonzero_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let weak = weak.clone();
            let bu = bu.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::ratings::set_rating(&s, id_vec.clone(), rating).await {
                    log::warn!("browse::set_rating: {e}");
                    return;
                }
                for id in &id_vec {
                    bu.flip_rating(*id, rating);
                    browse_ui_mod::apply_row_rating(&weak, *id, rating);
                }
            });
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
            let cur_field = g.get_sort_field();
            let cur_dir = g.get_sort_dir();
            let (new_field, new_dir) = if cur_field.as_str() == field.as_str() {
                let nd = if cur_dir.as_str() == "asc" { "desc" } else { "asc" };
                (field.to_string(), nd.to_string())
            } else {
                (field.to_string(), "asc".to_string())
            };
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            super::persist_view_sort(&s, view_id::BROWSE, new_field.clone(), &new_dir);
            bu.set_sort(new_field, new_dir);
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
            spawn_logged_sync!(s, "browse::toggle_column",
                library::settings::update_view_columns(&s, "browse".to_string(), columns));
        });
    }

    // play-all: all in-library ids in display order, start at 0.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        g.on_play_all(move || {
            let ids = bu.current_in_library_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::play_all",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)));
        });
    }

    // refresh: re-fetch the current path, no history change, no persist.
    // Fired by the BrowseView when nav lands on it (so a fresh activation
    // surfaces watcher-driven additions even when nothing else triggers a
    // refresh), and by the library-changed subscriber below.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_refresh(move || {
            let path = bu.current_path();
            let s = s.clone();
            let bu = bu.clone();
            let weak = weak.clone();
            spawn_logged!(s, "browse::refresh",
                browse_ui_mod::fetch_and_apply(&s, &bu, weak, path));
        });
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
                // scan) while the section is hidden — play-count flushes
                // bump this channel after every track completion, so an
                // ungated re-fetch would run O(library) work per song
                // during plain listening. Mark dirty so the next
                // section-enter re-fetches once instead.
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
