//! Playlist dialog + CRUD callbacks: the create / rename / delete /
//! mosaic-artwork commit handlers, the add-to-playlist picker, and the
//! mosaic-candidate toggling.
//!
//! Dialog *opens* (populating `Dialog.title` / `kind` / `target-id` /
//! `input-text` / `pending-track-ids` / `open`) happen inline in Slint
//! markup — the "+ New Playlist" pill and the Rename / Delete pills (both now
//! in `my-library/tab-pills.slint`, which holds every My Library pill row), and
//! the "New Playlist…" entry inside the row context menu
//! (`track-list-row.slint`). Crossing into Rust to write Dialog
//! properties *synchronously* from a click handler trips Slint's
//! "Recursion detected" property guard (`i_slint_core::properties`).
//! The exceptions are `request-add-to-playlist` and
//! `request-edit-artwork-for`, which fetch from `SQLite` first and write
//! Dialog via `upgrade_in_event_loop` (a fresh event-loop tick, safe).
//!
//! The dispatcher in `globals/dialog.slint` routes Accept to
//! `Playlists.create-playlist` / `rename-playlist` / `delete-playlist` /
//! `apply-mosaic` / `clear-artwork` — those are the commit-side
//! callbacks wired here.

use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::release_detail_hero_images;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{
    AppWindow, Dialog, PlaylistDetail, PlaylistPickRow as UiPlaylistPickRow, Playlists, TagEditor,
};

/// Wire the playlist dialog + CRUD callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // request-row-cover: row-tier lookup for the mosaic picker's
    // small candidate tiles + preview slots. Shares the row-tier
    // `cover_thumbs` LRU with Tracks / Browse / detail track-lists.
    {
        let pu = playlists_ui.clone();
        playlists.on_request_row_cover(move |path| pu.row_cover(path.as_str()));
    }

    // THE `Dialog.closed` handler — there is exactly one, and there must
    // stay exactly one. `on_closed` is `Callback::set_handler`, which has
    // a single slot: a second registration anywhere would silently clobber
    // this one (and a default `closed => { … }` body in `globals/dialog.slint`
    // would be clobbered BY it, which is precisely the leak this shape
    // replaced). A new dialog kind that pins an `image` extends this
    // handler; it does not add another.
    //
    // Two halves, fired once the close animation completes:
    //
    //   1. `invoke_closed_teardown()` — the Slint-side `public function`
    //      that resets every scalar / list / chrome property (`kind` /
    //      `target-id` / `input-text*` / `mosaic-*` / `pending-track-ids`
    //      / the two picker row models / `title` / `message` / labels /
    //      `destructive`). A `public function` has no handler slot, so it
    //      cannot be registered away the way a callback body can.
    //   2. `current-artwork` — the one `image`-typed property, which has
    //      no Slint default literal and so can only be reset from Rust.
    //      This is the ~603 KiB `SharedPixelBuffer` Arc pulled from the
    //      playlist grid-tier LRU at dialog-open; dropping it here releases
    //      it on the same tick the body branch unmounts.
    //
    // Pair the Arc drop with an off-thread `heap_trim::trim()` (parity with
    // `release_detail_artwork`) so glibc returns the freed pages instead of
    // holding them in the arena. Trim must stay off the UI thread — it
    // walks arena free lists.
    {
        let weak = weak.clone();
        let s = state.clone();
        ui.global::<Dialog>().on_closed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            // Before the teardown, which clears `kind`. Opens reach no Rust
            // seam, so this is the only trace a dialog leaves — which one, not
            // whether it was accepted, the accept dispatcher being pure Slint.
            log::debug!("dialog closed: {}", dlg.get_kind());
            dlg.invoke_closed_teardown();
            dlg.set_current_artwork(Image::default());
            // The Edit-Tags dialog pins a decoded cover in `TagEditor.cover`
            // (another `image`-typed property with no Slint default literal);
            // release it here — this is the one `on_closed`, extended not
            // duplicated.
            ui.global::<TagEditor>().set_cover(Image::default());
            s.runtime.spawn_blocking(crate::tasks::heap_trim::trim);
        });
    }

    // create-playlist: dispatcher hands us `(name, pending_track_ids)`.
    // Create the playlist; if pending ids are non-empty, add them too.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_create_playlist(move |name, description, pending| {
            let name_str = name.trim().to_owned();
            if name_str.is_empty() {
                return;
            }
            // Empty description ⇒ `None` so the DB stores NULL rather
            // than an empty string; mirrors Tauri's `description.trim()
            // || undefined` pattern.
            let desc_trimmed = description.trim();
            let description_opt = if desc_trimmed.is_empty() {
                None
            } else {
                Some(desc_trimmed.to_owned())
            };
            let pending_vec: Vec<i64> = pending.iter().map(i64::from).collect();
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                match library::playlists::create_playlist(&s, name_str.clone(), description_opt)
                    .await
                {
                    Ok(p) => {
                        if !pending_vec.is_empty()
                            && let Err(e) =
                                library::playlists::add_to_playlist(&s, p.id, pending_vec).await
                        {
                            log::warn!("playlists::create_playlist add pending: {e}");
                        }
                        if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak).await {
                            log::warn!("playlists::create_playlist refetch: {e}");
                        }
                        log::info!("playlists::create_playlist: {name_str:?}");
                    }
                    Err(e) => {
                        log::warn!("playlists::create_playlist {name_str:?}: {e}");
                    }
                }
            });
        });
    }

    // rename-playlist: dispatcher hands us `(id, new_name)`. Reuse the
    // existing `update_playlist` (description preserved by re-passing
    // it). Refresh both grid and (if it's the open detail) the header.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_rename_playlist(move |id, new_name, description| {
            let id = i64::from(id);
            let name_str = new_name.trim().to_owned();
            if name_str.is_empty() {
                return;
            }
            // Empty description from the dialog ⇒ clear (`None` → NULL
            // in the DB). The Rename dialog pre-fills `input-text-2`
            // with the current description, so an empty value at
            // commit time really does mean "user removed it".
            let desc_trimmed = description.trim();
            let description_opt = if desc_trimmed.is_empty() {
                None
            } else {
                Some(desc_trimmed.to_owned())
            };
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                match library::playlists::update_playlist(
                    &s,
                    id,
                    name_str.clone(),
                    description_opt,
                    None,
                )
                .await
                {
                    Ok(_) => {
                        if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await {
                            log::warn!("playlists::rename refetch grid: {e}");
                        }
                        if pu.detail_playlist_id() == id
                            && let Err(e) =
                                playlists_ui_mod::refresh_detail(&s, &pu, weak, id).await
                        {
                            log::warn!("playlists::rename refresh detail: {e}");
                        }
                        log::info!("playlists::rename({id}): {name_str:?}");
                    }
                    Err(e) => {
                        log::warn!("playlists::rename({id}) {name_str:?}: {e}");
                    }
                }
            });
        });
    }

    // delete-playlist: dispatcher hands us `id`. If it was the open
    // detail, swing the view back to the grid first; the cached row
    // models are emptied on the UI thread before the DB delete to
    // avoid a one-frame "deleted playlist still visible" flash.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_delete_playlist(move |id| {
            let id = i64::from(id);
            let was_open = pu.detail_playlist_id() == id;
            if was_open && let Some(ui) = weak.upgrade() {
                let d = ui.global::<PlaylistDetail>();
                d.set_playlist_id(-1);
                release_detail_hero_images!(ui, d);
                playlists_ui_mod::clear_detail(&pu);
            }
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::playlists::delete_playlist(&s, id).await {
                    log::warn!("playlists::delete({id}): {e}");
                    return;
                }
                if was_open {
                    // Clear the persisted "last detail" so a restart
                    // doesn't try to re-open the deleted playlist.
                    let s_disk = s.clone();
                    s.runtime.spawn_blocking(move || {
                        if let Err(e) = library::settings::set_last_detail_id(
                            &s_disk,
                            crate::ui::track_list_view::view_id::PLAYLIST_DETAIL,
                            None,
                        ) {
                            log::warn!("playlists::delete clear last_detail_id: {e}");
                        }
                    });
                }
                if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak).await {
                    log::warn!("playlists::delete refetch grid: {e}");
                }
                log::info!("playlists::delete({id})");
            });
        });
    }

    // apply-mosaic: dispatcher hands us `(id, paths 1..4)`. Backend
    // composes a 600x600 collage; we refresh.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_apply_mosaic(move |id, paths| {
            let id = i64::from(id);
            let path_vec: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
            if path_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::playlists::set_playlist_thumbnail(&s, id, path_vec).await {
                    log::warn!("playlists::apply_mosaic({id}): {e}");
                    return;
                }
                if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await {
                    log::warn!("playlists::apply_mosaic refetch grid: {e}");
                }
                if pu.detail_playlist_id() == id
                    && let Err(e) =
                        playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), id).await
                {
                    log::warn!("playlists::apply_mosaic refresh detail: {e}");
                }
            });
        });
    }

    // clear-artwork: empty mosaic_selection means "revert to auto"
    // (clear the custom thumbnail). Reuse `update_playlist` with
    // `clear_thumbnail = true`; description preserved.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_clear_artwork(move |id| {
            let id = i64::from(id);
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let Ok(current) = library::playlists::get_playlist_detail(&s, id).await else {
                    return;
                };
                if let Err(e) = library::playlists::update_playlist(
                    &s,
                    id,
                    current.name,
                    current.description,
                    Some(true),
                )
                .await
                {
                    log::warn!("playlists::clear_artwork: {e}");
                    return;
                }
                if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await {
                    log::warn!("playlists::clear_artwork refetch grid: {e}");
                }
                if pu.detail_playlist_id() == id
                    && let Err(e) =
                        playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), id).await
                {
                    log::warn!("playlists::clear_artwork refresh detail: {e}");
                }
            });
        });
    }

    // Add-to-Playlist commit (`add-tracks-to-selected`) + the per-row /
    // select-all toggles live in `files.rs` alongside the Export picker's
    // selection handlers — they share the model-mutation + selection-meta
    // pattern, and the commit needs the `Rc<NotificationsUi>` for its
    // completion toast (wired after the notifications stack exists).

    // request-add-to-playlist: track row's "Add to Playlist" entry
    // (single-row or multi-select). Runs two short SELECTs in parallel
    // (full playlist list + per-playlist overlap counts for the
    // selection), builds `PlaylistPickRow`s, then opens the dialog
    // already populated. `exclude-playlist-id >= 0` is the open
    // Playlist Detail id from the row, so the current playlist
    // doesn't appear as a target.
    {
        let s = state.clone();
        let weak = weak.clone();
        playlists.on_request_add_to_playlist(move |ids, exclude_id| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            if id_vec.is_empty() {
                return;
            }
            let exclude = i64::from(exclude_id);
            let s = s.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let (playlists_res, counts_res) = tokio::join!(
                    library::playlists::get_playlists(&s),
                    library::playlists::count_tracks_in_playlists_for_selection(
                        &s,
                        id_vec.clone(),
                    ),
                );
                let playlist_stats = playlists_res.unwrap_or_else(|e| {
                    log::warn!("playlists::request_add_to_playlist get_playlists: {e}");
                    Vec::new()
                });
                let counts = counts_res.unwrap_or_else(|e| {
                    log::warn!("playlists::request_add_to_playlist counts: {e}");
                    std::collections::HashMap::new()
                });
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    // Skip rows whose i64 playlist id can't fit in the
                    // Slint-side i32 (`PlaylistPickRow.id`). The picker's
                    // toggle / commit route back into Rust by that id
                    // (`Playlists.toggle-add-pick` / `add-tracks-to-selected`)
                    // — surfacing a row with a clamped id would mis-target.
                    let rows: Vec<UiPlaylistPickRow> = playlist_stats
                        .into_iter()
                        .filter(|p| p.id != exclude)
                        // Smart playlists derive membership from rules — adding
                        // tracks would write orphan `playlist_items` rows that
                        // never surface. Exclude them as targets (same gate as
                        // reorder / remove / file-drop).
                        .filter(|p| !p.is_smart)
                        .filter_map(|p| {
                            let id = i32::try_from(p.id).ok().or_else(|| {
                                log::warn!(
                                    "playlists::request_add_to_playlist: playlist id {} overflows i32 — skipping",
                                    p.id,
                                );
                                None
                            })?;
                            let contained =
                                i32::try_from(*counts.get(&p.id).unwrap_or(&0))
                                    .unwrap_or(i32::MAX);
                            Some(UiPlaylistPickRow {
                                id,
                                name: SharedString::from(p.name.as_str()),
                                artwork_path: SharedString::from(
                                    p.thumbnail_path.as_deref().unwrap_or(""),
                                ),
                                contained_count: contained,
                                // Multi-select: start with nothing ticked; the
                                // user opts in per playlist (or via "Select all").
                                selected: false,
                            })
                        })
                        .collect();
                    let dlg = ui.global::<Dialog>();
                    dlg.set_playlist_pick_rows(ModelRc::new(VecModel::from(rows)));
                    dlg.set_add_select_all(false);
                    dlg.set_add_selected_count(0);
                    dlg.set_open(true);
                });
            });
        });
    }

    // toggle-mosaic-candidate: mutate `Dialog.mosaic-selection` on the
    // UI thread — toggle path in/out, cap at 4 entries. Any toggle —
    // including toggling the last entry back off — flips
    // `mosaic-touched`, so the preview switches off the
    // "current saved artwork" branch and Apply can wipe the artwork
    // on an explicit clear.
    {
        let weak = weak.clone();
        playlists.on_toggle_mosaic_candidate(move |path| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            let cur: Vec<SharedString> = dlg.get_mosaic_selection().iter().collect();
            let path_s = path.to_string();
            let next: Vec<SharedString> =
                if let Some(pos) = cur.iter().position(|p| p.as_str() == path_s.as_str()) {
                    let mut v = cur;
                    v.remove(pos);
                    v
                } else if cur.len() < 4 {
                    let mut v = cur;
                    v.push(SharedString::from(path_s.as_str()));
                    v
                } else {
                    // Cap reached — silently no-op.
                    cur
                };
            dlg.set_mosaic_selection(ModelRc::new(VecModel::from(next)));
            dlg.set_mosaic_touched(true);
        });
    }

    // request-edit-artwork-for: grid-card variant of the detail view's
    // `request-edit-artwork`. The detail view isn't necessarily open
    // for this id, so the dialog preview's "current artwork" is
    // resolved against the saved `thumbnail_path` via the grid-tier
    // LRU (same path the card itself uses for `request-cover`, so the
    // decode is already warm). `slint::Image` isn't `Send`, so the
    // cover lookup happens INSIDE the `upgrade_in_event_loop` closure
    // — only the `Option<String>` path crosses the spawn boundary.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_request_edit_artwork_for(move |id| {
            let id = i64::from(id);
            if id < 0 {
                return;
            }
            let artwork_path =
                pu.grid_stats_by_id(id).and_then(|p| p.thumbnail_path).unwrap_or_default();
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let candidates = library::playlists::get_playlist_artwork_paths(&s, id)
                    .await
                    .unwrap_or_default();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let dlg = ui.global::<Dialog>();
                    let current_cover = if artwork_path.is_empty() {
                        Image::default()
                    } else {
                        pu.grid_cover(&artwork_path)
                    };
                    dlg.set_title(SharedString::from("Edit Artwork"));
                    dlg.set_message(SharedString::from(""));
                    dlg.set_confirm_label(SharedString::from("Apply"));
                    dlg.set_cancel_label(SharedString::from("Cancel"));
                    dlg.set_destructive(false);
                    dlg.set_kind(SharedString::from("edit-playlist-artwork"));
                    dlg.set_target_id(i32::try_from(id).unwrap_or(-1));
                    dlg.set_input_text(SharedString::from(""));
                    dlg.set_mosaic_selection(ModelRc::new(VecModel::from(
                        Vec::<SharedString>::new(),
                    )));
                    dlg.set_mosaic_touched(false);
                    dlg.set_current_artwork(current_cover);
                    let cand_rows: Vec<SharedString> =
                        candidates.into_iter().map(SharedString::from).collect();
                    dlg.set_mosaic_candidates(ModelRc::new(VecModel::from(cand_rows)));
                    dlg.set_open(true);
                });
            });
        });
    }
}
