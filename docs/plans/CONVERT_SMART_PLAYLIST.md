# Plan: Convert a smart playlist to a regular one

Status: **in progress**, Phases 0, 1 and 2 implemented · Owner: Kenan · Created: 2026-11-03 · Issue #159, branch `feat/convert-smart-playlist`

> **Validated 2026-11-03** against `2faa2442`. The sources were:
> - three read-only code traces: the store and smart membership; the card menu, the detail page and the undo slot; the queue and what reads a playlist after a play;
> - the full Material Symbols source in `scripts/fonts-src/`, which carries a `playlist_add_check` ligature.

Working doc. Delete it when its last phase (Tests) lands.

## Context

**What you see today.** A smart playlist's tracks follow its rules. Once it holds exactly the right tracks, nothing lets you stop it changing. The closest thing is New Playlist… on its card, which makes a frozen copy beside it under a name typed again, while the smart one carries on. A random pick is worse off: the page draws a new one on every open, and New Playlist… draws one of its own, so the tracks on show can't be kept at all.

**Why.** `is_smart` and `smart_criteria` are written when the playlist is created and changed only by Edit Rules, which keeps it smart. Nothing ever writes `playlist_items` for an existing row from its rules (ADR 22: membership is a query).

**Outcome.** A **Convert to Regular Playlist** entry sits next to Edit Rules, in the card's menu and as a pill on the page. Pressing it:
- keeps the tracks the playlist holds at that moment, in its own order, and for a random pick the one on show;
- keeps the same playlist: its id, name, description, cover and place in the grid;
- makes it an ordinary playlist at once: rows drag to reorder, Remove from Playlist appears, and a file drop or Add to Playlist reaches it;
- raises "Road Trip is now a regular playlist" with **Undo**, which puts the rules back and makes it smart again. With undo turned off for playlists in Settings, it asks first instead.

**What keeps working** (the issue's three, held below):
- A smart playlist nobody converts works as today: no query, schema or read path changes.
- New Playlist… on a smart card still makes a separate copy.
- A regular playlist isn't touched: the convert refuses any row that isn't smart.

## What the issue asks, checked against the tree

| Issue says | The tree says | So |
|---|---|---|
| Keeps the tracks it shows, in the same order. | A smart page lists its membership numbered by the rules' order (`push_positioned_membership`, `smart_playlist.rs:56-61`): the cap's order, or `sort_key, id` when uncapped. The page's sort and filter run over that set. | The convert writes the whole membership in that order as positions 0 onward, so the page lists the same rows afterwards under any sort and filter. A filter is a view, not a pick. |
| A random pick keeps the pick on show. | The page's pick is `PlaylistsUi.draw` (`ui/playlists/mod.rs:83-85`), redrawn on every open and stored nowhere. The card's cover comes from the default draw (`fill_automatic_thumbnail`, `smart_playlists.rs:48-54`). The card's New Playlist… draws afresh with `ORDER BY RANDOM()` (`get_smart_playlist_track_ids`). | The draw travels with the convert: the page's own while it shows that playlist, else the default draw the card's cover came from. |
| Same name, artwork, description and place. | The grid orders by `updated_at DESC` (`queries/entity_keyset.rs`), and Edit Rules stamps it. A smart row's automatic cover is never stored: it is worked out on read as the first cover in its order (`scope_facts::first_cover`). A manual row's is stored as its first track's cover in position order (`first_track_cover!`, `playlist.rs:588-595`). | The convert keeps the row's own `updated_at` and stores the first item's cover, which is the cover shown before. One exception, decision 4. |
| Behaves like a regular playlist afterwards. | Four gates read `is_smart`: reorder and Remove in Slint, file drop and Add to Playlist in Rust (`library-data.md`). Each reads it fresh: the page from its header (`refresh_header`), the store on every query (`Membership::resolve`). | Flipping `is_smart` and refreshing the page and the grid is enough. No gate changes. |
| Undo brings the rules back, rather than asking first. | `ui::undo` holds one removal per notice. `ui-patterns.md`: a new removal offers its undo, and asks first only where undo doesn't cover it. | A new `Removal::PlaylistRules` arm holding the rules' JSON. It asks first only when Settings turns undo off for playlists, as Delete does (`Deleter::request`, `crud.rs`). |
| A track removed meanwhile shouldn't come back. | One write connection serialises every writer (`database/mod.rs`, `max_connections(1)`). An insert naming a deleted track fails its foreign key and the whole transaction (`tracks_still_there`'s doc, `queries/playlist.rs`). | The membership is read inside the convert's own write transaction. A scan's delete commits either before it, so the rules no longer match the track, or after it, so the cascade removes the item. |
| Converting one that's playing. | The queue holds bare track ids (`QueueState`, `queue.json`), and nothing reads a playlist again after a play (`library::playback::player_play_tracks`). | Nothing to do. The hand test confirms it. |

**Not in the issue, found on the way:**
- **New Playlist… and Add to Playlist fail outright when one of their tracks goes while the dialog is open.** `create_playlist_with_tracks` and `add_tracks_to_playlist` insert the ids the menu read with no check, so one track deleted meanwhile fails the foreign key and nothing is created or added. The two restores already filter through `tracks_still_there`. New Playlist… is one of the issue's "keeps working" paths, so Phase 0 fixes this on the branch, ahead of the rest.
- **A filtered list kept a pick whose track left it.** On Songs, Favorites or a detail, a reload under a filter kept every pick the filter hid, a deleted track's included, so the menu counted it and a batch action reached it unseen. Phase 2 fixes it in `paged_rows::track_rows`: a filtered reload asks the store which hidden picks the set still holds and drops the rest.
- **A test pins one smart-only entry in the card menu.** `a_card_menu_entry_is_gated_on_the_operation_and_not_the_kind` (`crates/melodia/tests/card_selection.rs:235-266`) asserts Edit Rules is the only entry gated on `card-is-smart`. Convert is the second, also offered on the page, so the test's expected set grows in Phase 2.
- **Settings' Playlists undo row lists what it covers.** `undo-section.slint:38` reads "Tracks removed from a playlist, and deleted playlists.", and the convert's undo joins that list.

## Decisions settled here

1. **One store function converts, in one write transaction:** `queries::playlist_convert::convert_to_regular(db, id, draw)`.
   1. Read `is_smart`, `smart_criteria`, `name`, `created_at` and `updated_at`. A row that is gone or already regular answers `None`, so a second press does nothing.
   2. Read the membership on the transaction in the rules' order: the cap's order and count, the natural order when uncapped, a random cap through `pick` under `draw`.
   3. `UPDATE playlists SET is_smart = FALSE, smart_criteria = NULL`.
   4. Insert the ids as positions 0 onward through `insert_items_tx`. The stats triggers count up from the smart row's stored 0.
   5. `update_playlist_thumbnail_and_timestamp_tx` with the row's own `updated_at`: an automatic cover points at the first item's, a custom one stays, and the card keeps its place.

   It answers `DroppedRules { playlist_id, created_at, name, smart_criteria }`, the JSON exactly as stored.
2. **The membership is the page's, read on the transaction.** `fetch_packed_ids` and `pick_random` take the executor they read on rather than the pool, `Membership` passing `db.read()`. A new `smart_playlist::member_ids_tx(tx, criteria, draw)` answers "these ids, in this order": a random cap through `pick_random`, anything else through `membership_query("id", …)`, whose `ORDER BY` is the fragment the page numbers by. So the page and the convert can't disagree on which tracks or in what order.
3. **The draw.** `PlaylistsUi::draw_on_show(id)` gives the page's draw while the open detail is `id` (`detail_playlist_id`), else `Draw::default()`, the draw the card's cover and the page's hero both use. One place decides, so the card entry and the pill share one callback.
4. **The cover.** Wherever the old cover was automatic, the first item's cover is the one shown before, both being the first artwork in the same order. The exception is a random cap converted from its page. The hero there shows the default draw's cover, which can belong to a track the page's pick left out. The kept pick wins, by the issue's own rule, and the cover becomes the first kept track's: an automatic cover naming a track the playlist doesn't hold would be wrong from its next edit on.
5. **Undo puts the rules back, in one transaction:** `queries::playlist_convert::restore_rules(db, &DroppedRules)`.
   - Delete the playlist's items, the triggers counting back to 0. Set `is_smart = TRUE` and `smart_criteria` to the held JSON. NULL an automatic `thumbnail_path`, which a smart row never stores.
   - The name, the description, a cover picked since and `updated_at` stay as they are now.
   - Tracks added since go with the items. A smart row never reads them, and making it smart again is what the notice offered.
   - A playlist deleted since fails with `AppError::not_found`, which raises the slot's "Couldn't undo". The row is matched on `created_at` as well as the id: the id is a bare `INTEGER PRIMARY KEY`, which SQLite reuses after the largest goes, and a notice raised while undo was on outlives turning it off, so a delete with no notice of its own can leave it up.
   - The automatic cover it clears names a track's own artwork file, so no `ArtworkHold` is owed.
6. **The undo arm.** `Removal::PlaylistRules(PlaylistConvertUndo)`, of kind `UndoKind::Playlists` and size `RemovalSize::Whole`, so the one-item switch doesn't quiet it. It never merges: a second convert replaces the notice and the first stands, as any distinct removal does. Its restore calls `restore_rules`, then `refresh_after_edit`. It holds one small JSON string, so there's no cap to check.
7. **Undo off for playlists asks first.** `DialogKind.convert-playlist` through `Dialog.open-convert-playlist(id)`, whose accept reaches `Playlists.convert-to-regular-confirmed(id)`. Settings answers synchronously, but a `Dialog` write inside the click trips Slint's recursion guard, so Rust raises it a tick later under a `DialogClaim`, as the radio removal's `confirm_first` does. It is destructive, like Delete, since the rules can't come back. The confirmed path converts with no notice.
8. **No signal bump.** The convert changes no track, so it refreshes as Rename does, through `refresh_after_edit`: the grid, plus the page when it shows the playlist converted. `library_changed` would wake every grid and the queue tasks for nothing.
9. **The entries and their strings.**
   - Card menu: **Convert to Regular Playlist**, icon `playlist_add_check`, right after Edit Rules…. Both entries gate on one named `smart-edits: playlist-edits && card-is-smart`, a single card only, as Edit Rules is today.
   - Page: a `playlist_add_check` pill, tooltip **Convert to regular playlist**, right after the Edit rules pill and in its group.
   - Notice: **{name} is now a regular playlist**, with Undo.
   - Confirmation: **Convert to regular playlist?**, "It keeps the tracks it holds now, and its rules are removed for good.", confirm **Convert**.
   - Settings, the Playlists row: "Tracks removed from a playlist, deleted playlists, and a converted smart playlist's rules."
   - `playlist_add_check` is new to `scripts/icons.txt`. No icon in use says "keep it as it is": `push_pin` already means always on top, and `auto_awesome` is the smart badge.

## Considered and not taken

- **Keeping `smart_criteria` on the row after the convert, for an undo holding nothing.** A regular row carrying rules breaks "split on `is_smart` before asking what it holds" for every later reader. The export, the rule editor and `reading_play_stats` would each have to know to ignore them. The undo holds the JSON instead.
- **Converting the ids the page lists.** The keyset is the filtered subset, read before the click. A filter would quietly drop tracks, and a track a scan removed since would fail the insert. Reading the membership inside the write answers the issue's note by construction.
- **Stamping `updated_at`, as Edit Rules does.** The issue asks for the card to keep its place.
- **`INSERT … SELECT` straight from the membership query.** It is one statement for a fixed order, but a random cap still hands its ids over from Rust, so it would be two paths for one question.
- **Converting a multi-selection.** Edit Rules is single-card, and the issue places the action beside it.

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-store/src/database/queries/playlist.rs` (639) | `tracks_still_there`, moved beside `insert_items_tx` as `pub(crate)`; `create_playlist_with_tracks` and `add_tracks_to_playlist` filter through it (P0); `contiguous` is `pub(crate)` for the convert; a removal's cut records `created_at` (P1). |
| `crates/melodia-store/src/database/queries/playlist_restore.rs` (184) | Imports `tracks_still_there` rather than owning it (P0); `restore_playlist_items` matches the row on `created_at` too (P1). |
| `crates/melodia-core/src/entities/playlist.rs` (58) | `DroppedRules`; `created_at` on it and on `PlaylistItemsCut` (P1). |
| `crates/melodia-views/src/ui/playlists/callbacks/detail.rs` | `PlaylistCutUndo::same_playlist` merges only cuts sharing `created_at` (P1); its restore refreshes through `spawn_refresh_after_edit` (P2). |
| `crates/melodia-store/src/database/queries/track/keyset.rs` (190) | `fetch_packed_ids` takes an executor (P1). |
| `crates/melodia-store/src/database/queries/smart_playlist.rs` (526) | `pick_random` takes an executor; `member_ids_tx`; `random_cap`, which `Membership::smart` asks too (P1). |
| `crates/melodia-store/src/database/queries/playlist_convert.rs` (new, P1) | `convert_to_regular` and `restore_rules`, one transaction each. |
| `crates/melodia-app/src/library/smart_playlists.rs` (112) | `convert_to_regular` and `restore_rules` (P1). |
| `crates/melodia-views/src/ui/playlists/mod.rs` (201) | `draw_on_show` (P2). |
| `crates/melodia-views/src/ui/playlists/callbacks/convert.rs` (new, P2) | `Converter`: `request` (covered: convert, refresh, offer the undo; uncovered: raise the confirmation) and `for_good` (the confirmed path); `PlaylistConvertUndo`. |
| `crates/melodia-views/src/ui/playlists/callbacks/mod.rs` (73) | Wires `convert`; `spawn_refresh_after_edit`, the one spawn of `refresh_after_edit` for a write awaited on the UI thread: the convert, its undo and the cut's undo (P2). |
| `crates/melodia-views/src/ui/paged_rows/track_rows.rs` | `forget_gone_picks`, through which a filtered reload drops the hidden picks the unfiltered set no longer holds (P2). |
| `crates/melodia-views/src/ui/undo/removal.rs` (240) | The `PlaylistRules` arm: kind, size, recipe, restore (P2). |
| `crates/melodia-ui/ui/globals/playlists.slint` (134) | `convert-to-regular(id)` and `convert-to-regular-confirmed(id)` (P2). |
| `crates/melodia-ui/ui/globals/dialog.slint` (431) | `DialogKind.convert-playlist`, `open-convert-playlist(id)` and its accept arm (P2). |
| `crates/melodia-ui/ui/globals/undo-text.slint` (86) | `playlist-converted(name)` (P2). |
| `crates/melodia-ui/ui/components/grid/card-context-menu.slint` (350), `card-cell.slint`, `playlist-grid.slint`, `views/my-library/playlists-tab.slint` | The entry, `smart-edits`, and the callback's route down to `Playlists.convert-to-regular` (P2). |
| `crates/melodia-ui/ui/views/my-library/tab-pills.slint` (257) | The pill (P2). |
| `crates/melodia-ui/ui/views/settings/undo-section.slint` | The Playlists row's description (P2). |
| `crates/melodia/tests/card_selection.rs` | The gate test follows `smart-edits` and expects Edit Rules and Convert behind it (P2). |

The counts are this session's `wc -l`. None of these files crosses 800.

## Phases

### Phase 0: An add or a new playlist skips a track the library lost meanwhile

1. Move `tracks_still_there` beside `insert_items_tx` as `pub(crate)`; `playlist_restore` imports it.
2. `create_playlist_with_tracks` and `add_tracks_to_playlist` filter `track_ids` through it inside their transaction, keeping the order, before `contiguous`.
3. `insert_items_tx`'s doc states that its items name tracks present inside `tx`, which ids read before the transaction owe `tracks_still_there`.
4. `insert_items_tx` and `add_tracks_to_playlist` return the tracks that landed. The Add to Playlist notice counts the playlists that took at least one track and the tracks that landed in at least one, rather than the tracks picked, and a drop onto a playlist counts the same way. An add that lands nothing leaves the playlist as it was.

Gate: `cargo fmt --all`, clippy and tests.

Hand test, with file watching on:
- Pick two tracks, Add to Playlist ▸ New Playlist…, delete one file while the dialog is open, then Create: the playlist holds the other. Today nothing is created.
- Add to Playlist ▸ an existing playlist, with one picked file deleted meanwhile: the other is added.

### Phase 1: Convert and restore (no UI)

1. `DroppedRules` in `melodia-core`.
2. `fetch_packed_ids` and `pick_random` take an executor; `Membership` passes `db.read()`.
3. `member_ids_tx` in `smart_playlist.rs`.
4. `queries::playlist_convert` with `convert_to_regular` and `restore_rules`.
5. `library::smart_playlists::{convert_to_regular, restore_rules}`.
6. Docs: `library-data.md`'s Smart playlists section gains "A smart playlist becomes a regular one only through `queries::playlist_convert`, which reads the membership inside the write that flips `is_smart`".

Gate: `cargo fmt --all`, clippy and tests. Nothing calls the convert until Phase 2, so there's no run.

### Phase 2: The action, its Undo and its confirmation

1. Add `playlist_add_check` to `scripts/icons.txt`, re-subset both faces with `scripts/subset-icon-fonts.py`, and run `scripts/check-icons.py` clean.
2. Slint:
   - the two `Playlists` callbacks;
   - the card entry behind `smart-edits`, routed down from the grid;
   - the page pill;
   - `UndoText.playlist-converted`;
   - `DialogKind.convert-playlist`, its opener and its accept arm;
   - the Settings description.
3. Rust: `draw_on_show`, `callbacks/convert.rs`, and the `PlaylistRules` arm.
4. `card_selection.rs`: the gate test's expected set becomes Edit Rules and Convert.
5. Run `scripts/update-translations.py`, then finish every catalog.
6. Docs: `README.md`'s Undo line (`:113`) names a converted smart playlist's rules, and its playlists line (`:115`) names converting a smart playlist to a regular one.

Gate: `cargo fmt --all`, clippy, tests, and the comment audit in `code-style.md`.

Hand test, the issue's list first:
- **A limit and an order**, say the 25 most played: the page lists the same 25 in the same order, and the card keeps its place and its cover.
- **A random pick.** From its page, the pick on show stays. From its card, the tracks are the card cover's pick, and the cover stays.
- **One that matches nothing:** an empty regular playlist with the regular empty-state copy.
- **Convert, then Undo**, by the button and by Ctrl+Z: smart again, with the same rules and badge, and an open page shows its pick again.
- **One playing in the queue:** playback and the queue carry on.

Then:
- After converting, drag reorder, Remove from Playlist, a dropped file and Add to Playlist all reach it.
- A custom cover and a description survive both the convert and the undo.
- With undo off for playlists in Settings, it asks first, and Convert converts with no notice.
- New Playlist… on a smart card still makes a separate copy, and the smart one stays smart.

### Phase 3: Tests

Tests are added where the landed change earns them, on Kenan's go.

## Cross-cutting

- **Defaults on a live app.** No migration and no new setting. The action does nothing until pressed, and every smart playlist reads as it does today.
- **Every new `@tr` follows the house rules.** The menu entry is Title Case like its neighbours, the pill tooltip sentence case like "Edit rules", and the confirmation's title reads like "Delete playlist?".
- **Memory.** The only new hold is the rules' JSON while the notice is up, so idle RSS doesn't move and no RSS check is owed.

## Open questions

None open.
