# Plan: Undo for removals (a notice that puts back what went)

Status: **planned** (no phase started) · Owner: Kenan · Created: 2026-10-28 · Issue #109, branch `feat/undo-removals`

> **Validated 2026-10-28** against `5fa3d5ed` (`feat/undo-removals`): three read-only traces (the
> toast stack, shortcuts and confirmations; the library removals; the queue, the equalizer and the
> prior art), direct reads of `notifications.rs`, `notification-stack.slint`, `shortcut-scope.slint`,
> the engine's `queue.rs`, the store's `playlist.rs`, `equalizer.rs` and `artwork_sweep.rs`, and the
> libadwaita and Material documentation cited under [Prior art](#prior-art).

Working doc. Delete when every phase has shipped and passed Kenan's hand test.

## Context

**What you see today.** Taking something out of Melodia is a one-way door, and the app mostly says
nothing about it:

- **Playlists.** Remove from Playlist on twenty selected rows deletes them at once: no message, no
  count, no way back except re-adding and re-dragging. Deleting a playlist asks first, then says
  nothing afterwards.
- **The queue.** Removing rows (the ✕, the menu, Delete/Backspace, Up Next's menu) is instant and
  silent. Clear asks first, and its dialog says "Playback will stop" when in fact the playing track
  runs to its end. Playing anything from a list replaces the whole queue with no message.
- **Removing the playing row from the queue misbehaves on its own, undo or not.** The "playing"
  highlight jumps to the row below, which never plays: at the track's end (and in the gapless
  preload) playback goes to the row after it. By code reading: `QueueState::remove_at`
  (`queue.rs:242-251`) leaves `current_index` on the slid-in row and `advance`/`peek_next`
  (`:338`, `:409`) step from `current_index + 1`. No test pins that path today.
- **Elsewhere.** Recent searches' Clear all and the equalizer's Reset wipe their state with no
  prompt or message. Removing a radio station asks first and says nothing after; for a hand-typed
  station that deletes the row, play history included.
- **Favorites.** "Remove {} from Favorites" (the row and card menus) and a card's heart un-heart
  whole selections or whole artists or albums silently, and on the Favorites page the rows vanish.

**Why.** There is no undo machinery anywhere, and the toast stack can't carry one:

- A toast can't be updated in place, its clock can't be restarted, and nothing pauses on hover:
  `show_auto_dismiss` arms a fire-and-forget `slint::Timer::single_shot` (`notifications.rs:114-124`)
  and the only in-place write is the locale refresh (`:177`).
- An action button routes by `NotificationKind` alone (`run_action`, `:270`), with no payload.
- Ctrl+Z is bound nowhere; `ShortcutScope` is the only `FocusScope` (`shortcut-scope.slint`).
- Most store removals destroy what a restore would need: positions are renumbered
  (`renumber_playlist_positions_tx`, `playlist.rs:402`), a deleted row's id and timestamps aren't
  recreatable (`create_playlist_with_tracks` and `save_station` mint new ones), and no store fn
  inserts at given positions (`insert_items_tx`, `:283`, writes one contiguous run stamped now).

**Outcome.** Each removal listed above raises one notice naming what went, with an Undo button.
Pressing it, or Ctrl+Z while the notice is up, puts things back as they were, positions included.

- The notice stays up for `UNDO_NOTICE_MS` and doesn't expire under the pointer.
- A run of the same removal updates the one notice, adds to its count and restarts its clock.
- Where undo covers a removal, the confirmation in front of it goes.
- A removal too large to hold says so and carries no button.
- **Unchanged:** what each removal writes, the watched-folder confirmation, tag edits (out of
  scope), and everything stored on disk. No schema change, nothing new persisted, and nothing
  survives a restart.

## Decisions settled here

1. **Write now, keep what restores it.** The issue's preferred shape. Lists refresh from the store,
   so a deferred write would leave every list describing something that hasn't happened. Strawberry
   and fooyin both apply the edit at once and keep a command that reverses it.
2. **One slot, not a stack.** A removal of the same kind on the same target (one playlist, the
   queue, one station tab) **merges** while its notice is up: counts add, the clock restarts, and
   the restore walks the merged steps newest first. Anything else **replaces** the slot, and the
   earlier removal stands. This is libadwaita's batched-undo example. It avoids Nautilus losing the
   first item of a run and Loupe stacking a toast per item (GNOME HIG #155).
3. **Restore by position, not by whole-list snapshot,** wherever rows survive the removal
   (playlist rows, queue rows). Removed rows go back in ascending index order, so an earlier insert
   never shifts a later target. That is Strawberry's algorithm, and it keeps edits made during the
   window. Only a removal that took the whole list (a deleted playlist, a cleared or replaced queue)
   holds the whole list. A row restored after other edits lands at its old index, clamped to the
   list.
4. **An undo reverses exactly what its removal did, nothing more.**
   - Removing rows or clearing the queue never touched playback, so their undo doesn't either.
   - A play that replaced the queue started a track, so its undo returns to what was playing,
     where it was, in the state it was in.
   - A deleted playlist or station comes back. The detail view its delete closed does not reopen,
     navigation not being part of what went.
5. **One cap for every hold, `UNDO_HOLD_CAP` rows** (provisionally 10,000, confirmed by a heaptrack
   of a full hold in the playlist phase). The unit that captures checks it before it writes.
   - **Over the cap a removal behaves exactly as it does today.** Its confirmation stays where it
     has one, now saying it can't be undone, and its notice names what went with no button.
   - **Under the cap a removal is never confirmed.** Every removal gets one answer, never both.
   - Strawberry's equivalent is `kUndoItemLimit = 500` plus a "too large to undo" confirm. Melodia
     holds ids and positions rather than full item records, so the same budget covers more rows.
6. **A removal that removed nothing raises nothing.** No notice for clearing an empty queue or empty
   recent searches, resetting a flat curve, or replacing a queue with the identical list.
7. **Timing and hover.**
   - `UNDO_NOTICE_MS = 10_000`. That is Material's Long snackbar duration, its value for a snackbar
     carrying an action. Melodia's existing action toasts already run 6 s, and the undo notice
     must outlast them.
   - **Every timed toast pauses while hovered**, as libadwaita's do. The pause is a property of
     the card, not of undo.
   - The issue's "while it holds the keyboard" has nothing to attach to: the card's buttons take
     no focus, and `ShortcutScope` stays the only `FocusScope`. Ctrl+Z is the keyboard's way in.
8. **Ctrl+Z means Undo only while the notice is up, and nothing else.**
   - A focused text field keeps its own Ctrl+Z: Slint's `TextInput` accepts
     `StandardShortcut::Undo` before the key bubbles (`i-slint-core` `items/text.rs:1108`,
     `input.rs:1143`).
   - A modal dialog gates it like every non-Esc key (`shortcut-scope.slint:116-118`).
   - No redo, no Ctrl+Shift+Z.
9. **A deleted row comes back under its own id.** Neither `playlists.id` nor `radio_stations.id` is
   `AUTOINCREMENT`, so the old id is free unless a row created in the window took it (SQLite reuses
   the largest). Only then does the restore take a fresh id.
10. **Artwork a held row names is held from the sweeps.**
    - A deleted playlist's custom cover and a deleted station's artwork are the files
      `library-data.md` calls unrebuildable.
    - The Radio page's leave runs the logo prune (`RADIO_GRACE`, `artwork_sweep.rs:118`), and the
      issue asks for "leave the page before the notice expires" by hand.
    - A small hold registry is unioned into `sweep_stores`' one reference set (`:149`). It is
      released when the slot drops, after which the next sweep reaps as it does today.
11. **Favorites: the explicit removals raise the notice. A lone heart that stays on screen doesn't.**
    - Raising it: the row and card menus' "Remove … from Favorites", a card's heart (a whole artist
      or album), and any heart on the Favorites page, whose row then leaves the list.
    - Not raising it: a single heart toggled on another list, Now Playing's heart, or L. Each still
      shows its new state, and pressing it again is its own undo.

## Prior art

Read for behaviour, then written to Melodia's conventions. Strawberry and fooyin are GPL-3.0 C++,
so nothing comes across verbatim.

**libadwaita `AdwToast`** (<https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/class.Toast.html>):

- `timeout` defaults to 5 s, and "Toasts cannot disappear while being hovered, pressed (on
  touchscreen), or have keyboard focus inside them" (`property.Toast.timeout`).
- Its batched-undo example keeps the toast, retitles it through `ngettext` ("%d item deleted"), and
  re-adds it with the comment "Bump the toast timeout". Decisions 2 and 7 copy this.
- `ADW_TOAST_PRIORITY_HIGH` shows a new toast at once and queues the old one.

**GNOME HIG issue #155** (<https://gitlab.gnome.org/Teams/Websites/developer.gnome.org-hig/-/issues/155>):
never resolved, but it names the two failures not to copy. Nautilus replaces the toast, so the
first item of a run can't be undone. Loupe queues one toast per item, five deletions making five
toasts.

**Material** (Compose `SnackbarDuration`: Short about 4 s, Long about 10 s, Indefinite).
Angular Material advises against a timed snackbar carrying an action, a screen reader user needing
time to reach it. The hover pause and Ctrl+Z are Melodia's answer to that.

**Strawberry** (`src/playlist/`):

- `PlaylistUndoCommandRemoveItems` stores `QList<Range { pos_, count_, items_ }>`
  (`playlistundocommandremoveitems.h:39-46`).
- Callers remove highest row first (`playlistview.cpp:794-800`, `playlist.cpp:2189-2216`), and
  `undo()` re-inserts the ranges in reverse (`.cpp:39-44`). Each insert lands in the coordinates
  of the moment it was removed. **Copied** as decision 3.
- `mergeWith` appends ranges (`.cpp:47-57`). **Copied** as decision 2's merge.
- `kUndoItemLimit = 500`, a "too large to undo" confirm (`mainwindow.cpp:2795-2806`), and
  `kUndoStackSize = 20` (`playlist.cpp:110-111`). **The cap is copied, the stack is not.**
- **Not copied:**
  - Under shuffle, `InsertItemsWithoutUndo` puts restored rows at random positions after the
    current one (`playlist.cpp:1381-1470`). Melodia restores both queue orders exactly.
  - A restored playing item is not made current again.
  - No notice. Undo is a menu item and Ctrl+Z.

**fooyin** (`src/gui/playlist/playlistcommands.cpp`):

- Every command holds the whole list before and after, and undo replaces the list
  (`:136-141`, `:156-160`). **Not copied**: no limit (`playlistworkspace.cpp:261-272`), and a
  restore clears every shuffle order (`core/playlist/playlist.cpp:1499-1523`).
- **Copied**: the current row is found again by a stable entry id. Melodia's `QueueEntry` is that
  id already.

**rox**: no queue or playlist undo. Its keymap page keeps a single-slot snapshot before a reset
(`settings/window/keymap_page.rs:43,57-61`), which is the equalizer's shape here.

## Confirmations this retires

The issue's Notes ask for this list before any confirmation goes. "Over the cap" is decision 5.

| Confirmation | Raised at | Fate |
|---|---|---|
| Delete playlist | `Dialog.open-delete-playlist` (`dialog.slint:261-271`), from `tab-pills.slint:155` and `playlists-tab.slint:28` | Goes. Kept over the cap, reworded to say it can't be undone. |
| Delete N playlists | `Dialog.open-delete-playlists` (`dialog.slint:275-285`), from `playlists-tab.slint:37` | Goes. Kept over the cap, reworded, and made a true plural. |
| Clear queue | inline, `queue-panel.slint:238-249` | Goes. Kept over the cap. "Playback will stop" was never true and is dropped. |
| Remove station (Favorites / Recently Played) | `Dialog.open-remove-station` (`dialog.slint:291-307`), from `station-context-menu.slint:186` and `kept-stations-tab.slint:62` | Goes. Kept over the cap, reworded. |
| Remove N stations | `Dialog.open-remove-stations` (`dialog.slint:311-325`) | Goes. Kept over the cap, reworded. |
| Remove watched folder | inline, `library-section.slint:90-102` | **Stays**: out of scope. |
| Restart (titlebar, tray, backdrop), exclusive output | `window-chrome-section.slint`, `appearance-section.slint`, `output-section.slint` | Untouched: not removals. |

A kept confirmation is raised from Rust once the size is known, across an await, so it takes a
`DialogClaim` (`ui-patterns.md`, held by `dialog_openers.rs`).

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-views/src/ui/undo/mod.rs` (new) | `UndoWindow`: the one slot, `offer` (merge or replace), the Undo/Ctrl+Z handler, the slot's release when its row goes, and `UNDO_NOTICE_MS` |
| `crates/melodia-views/src/ui/undo/removal.rs` (new) | `Removal`, one variant per surface wrapping that surface's token: `absorb` (merge or hand back), `count`, its wording recipe, and `restore`, which dispatches each arm to the surface module that raised it. No surface names another's. |
| `crates/melodia-views/src/ui/shell/notifications.rs` | One per-row state map (recipe, expiry, gone hook) replacing `recipes`. Adds a timed localized push, an in-place `relabel`, `restart`, the hover pause, and the `Undo` arm in `run_action`. `remove_at` stays the one removal path and now drops the row's whole state. |
| `crates/melodia-ui/ui/globals/undo-text.slint` (new) | `UndoText`: every undo wording as `pure callback`s with plural `@tr`, the `*Text` global idiom, keeping `settings.slint` (625 lines) from growing |
| `crates/melodia-ui/ui/globals/notifications.slint` | `undo-armed`, `undo()`, `hover(int, bool)` |
| `crates/melodia-ui/ui/components/dialog/notification-stack.slint` | A hover sentinel per card |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint` | The Ctrl+Z arm |
| `crates/melodia-core/src/entities/undo.rs` (new) | `UndoCapture<T>` (`Held(T)` or `OverCap { rows }`) and `UNDO_HOLD_CAP`, named by store, app and views |
| `crates/melodia-core/src/entities/{playlist,radio}.rs` | The plain tokens beside their entity: `PlaylistItemsCut`, `PlaylistSnapshot`, `StationCut` |
| `crates/melodia-store/src/database/queries/playlist.rs` | Capture inside the removal transactions. `insert_items_tx` takes explicit `(track_id, position, added_at)` rows, its two callers building contiguous ones. |
| `crates/melodia-store/src/database/queries/playlist_restore.rs` (new) | `restore_playlist_items`, `restore_playlists`, keeping `playlist.rs` (550 lines) under the 800-line cap |
| `crates/melodia-store/src/database/queries/radio.rs` | `remove_stations`: the per-id loop now in `kept.rs:94`, as one transaction. Also `restore_stations`. |
| `crates/melodia-store/src/database/queries/track/write.rs` | `unfavorite`, returning the ids it flipped |
| `crates/melodia-app/src/services/artwork_holds.rs` (new) | `ArtworkHold` (released on drop) and `held_basenames()`, unioned in `tasks::artwork_sweep::sweep_stores` |
| `crates/melodia-app/src/library/{playlists,radio/mod,queue,search,favorites}.rs` | Each removal returns its capture, and each surface gets one restore fn |
| `crates/melodia-app/src/services/search_history.rs` | `clear` hands back the list; `restore` puts it back |
| `crates/melodia-engine/src/player/engine/queue.rs` (534 lines) | `cut`/`restore_cut` replacing `remove_at` and `remove_batch` (`prune_missing` built on `cut`), `take_cleared`/`restore_cleared`, and the left-row cursor. Split into `queue/` if it passes 800. |
| `crates/melodia-views/src/ui/equalizer.rs` | `apply_curve`, shared by preset select, reset and restore |

**Duplication collapsed on the way through:**

- `wire_select_preset` and `wire_reset` each spell "set the model, push the gains, persist the
  curve" (`equalizer.rs:94-129`).
- `remove_at` and `remove_batch` are two copies of one cut (`queue.rs:231-294`), with
  `prune_missing` a third.
- The station removal loop lives in the view (`kept.rs:94`).
- The seven `player_play_tracks` callers each spell the play, and will each owe the offer. They
  converge on one views helper.

**Threading.** `UndoWindow` is installed right after `NotificationsUi` (`boot/ui_setup`) and handed
as `&Rc<UndoWindow>` to the installers that remove, the way `notifications` travels today. Some of
those installers run before the notifications exist (`playlists/mod.rs:49`); Phase 1 settles the
order.

## Phases

Each phase stops at the static gates (`cargo fmt --all`, `cargo clippy --all-targets --locked
--workspace -- -D warnings`, `cargo test --locked --workspace`) and Kenan's hand test. New tests
wait for Phase 10.

### Phase 1: the notice, with the equalizer's Reset as its first user

The shell and its simplest user land together, the shell having nothing to show by hand on its own.

1. **Slint.**
   - `NotificationKind.undo` in `models.slint`.
   - `Notifications.undo-armed`, `undo()` and `hover(id, inside)`.
   - A card-sized hover sentinel `TouchArea` declared under the card's layout. It reports
     `changed has-hover` and answers `accept` to every wheel event, `has-hover` not being
     continuous (`slint-pitfalls.md`). The gaps between cards still pass through, which is what
     the card's header comment protects; reword that comment to match.
   - Ctrl+Z in `ShortcutScope`'s Ctrl block, gated on `Notifications.undo-armed`.
   - `UndoText` added to `app-window.slint`'s export block.
2. **`NotificationsUi`.**
   - Per-row state replaces `recipes`. The expiry is an epoch-checked `single_shot`: a pause or
     restart bumps the epoch and a stale shot finds nothing, the "simply not found" idiom
     `show_auto_dismiss` already documents.
   - `show_auto_dismiss` keeps its signature and gains the hover pause.
   - A timed localized push takes a params struct (four arguments would want one) with an optional
     gone hook. Add `relabel(id, …)` and `restart(id)`.
   - `run_action`'s `Undo` arm is `invoke_undo()`. `on_action`'s dismiss after it stays harmless,
     the handler having dismissed its own row already.
3. **`UndoWindow` and `Removal::Equalizer`.**
   - The slot's row id drives `undo-armed`.
   - Undo takes the slot, dismisses the row and spawns the restore. A failed restore raises
     `show_failure` with "Couldn't undo".
4. **The equalizer.**
   - `apply_curve` is the one write of gains, preamp and preset, both the engine's and persisted.
   - Reset captures the curve first, skipping the offer when it was already flat at 0 dB.
   - Restore maps the preset name back through `preset_index`, falling to the Custom slot
     (`equalizer.rs:33-45`).
5. **Strings:** "Undo", "Equalizer reset", "Couldn't undo".

**By hand.**
- Reset raises the notice, and hovering holds it.
- Undo and Ctrl+Z restore both the card and the dialog.
- Ctrl+Z after the notice expires, or inside a text field, does nothing to the curve.
- A language switch with the notice up re-renders it.
- A Copy toast pauses under the pointer too.

### Phase 2: recent searches' Clear all

- `SearchHistoryState::clear` hands back the old list (`std::mem::take`).
- `restore` puts the list back, with any query added since kept in front, deduped and truncated to
  `MAX_ENTRIES`.
- The facade lives in `library::search`. `search/callbacks/recent.rs:42-53` offers, and the
  restore ends in `push_recent_rows_to_slint` as the clear does.
- **Not in scope:** a single chip's ✕, which the issue doesn't list. Searching again recreates a
  chip.
- **String:** "Recent searches cleared".

### Phase 3: tracks removed from a playlist

1. **Core.**
   - `UndoCapture<T>` and `UNDO_HOLD_CAP`.
   - `PlaylistItemsCut { playlist_id, playlist_name, rows: (track_id, row, added_at), updated_before, stamped_at }`.
   - `row` is the index in `ORDER BY position, id`, not the raw position, positions carrying gaps
     under a hard-deleted track (`move_playlist_tracks`' comment, `playlist.rs:383-385`).
2. **Store.**
   - `remove_tracks_from_playlist_batch` (`playlist.rs:314`) reads the cut inside its transaction
     before the delete, and only up to the cap. Over it, the delete runs as today, it never having
     confirmed.
   - `restore_playlist_items(cuts)` is one transaction walking the cuts newest first. For each cut:
     - drop rows whose track is gone (the foreign key would fail the insert) or is back in the
       playlist (the unique index; the user re-added it);
     - merge the survivors around the removed rows' indices;
     - write the survivors that moved through `batch_update_positions_pairs`;
     - insert the removed rows with their own positions and `added_at`;
     - refresh the automatic cover;
     - put `updated_at` back only where it is still the removal's own stamp.
   - Every playlist position is unique (`idx_playlist_items_unique`), so the track id names its row.
3. **Views.**
   - `wire_remove_track` (`playlists/callbacks/detail.rs:140`) offers.
   - The restore ends in the removal's own refresh (`track_rows::remove_rows`' reload,
     `refresh_detail`, `refresh_after_write`), which already handles an off-screen page.
4. **Measure.** One heaptrack of a held cap-sized cut confirms `UNDO_HOLD_CAP`, and the result is
   recorded here.
5. **Strings:** "{n} track removed from {}" (plural), and the over-cap line "Too many to undo".

### Phase 4: deleting playlists

1. **Core.** `PlaylistSnapshot`: every `playlists` column (`initial_schema.sql:178-192` plus
   `search_name`) and its items as in Phase 3.
2. **Store.**
   - `delete_playlists` (`playlist.rs:200`) is chunked with no transaction today. It becomes one
     transaction, in two forms: a held delete that reads the snapshots first, and the plain one for
     over the cap.
   - Add a count of the items a delete would take, which is what the cap is checked on.
   - `restore_playlists` inserts each row under its own id (decision 9), then its items through the
     generalized `insert_items_tx`, and the stats triggers recount. Whether those triggers touch
     `updated_at` decides whether the stamp is written last; check `:548-593`.
3. **App.** `services::artwork_holds` lands here and is unioned in `sweep_stores`. The facade holds
   a custom cover's basename for as long as the snapshot lives.
4. **Slint and views.**
   - The card trash, card menu and detail pill call `Playlists.delete-playlist(s)` directly.
   - `crud.rs` asks for the item count, then either does the held delete and offers, or raises the
     reworded opener with a `DialogClaim` and does the plain delete on accept.
   - Recheck `dialog_openers.rs`' `FOLDED_KINDS`.
5. **Strings:** "Deleted {}", "{n} playlists deleted" (plural), and the two confirmations reworded,
   the many-playlists title as a true plural.

### Phase 5: radio stations

1. **Store.**
   - `remove_stations(ids, from)` is one transaction: capture each row, unstar or clear its history,
     and delete it if it's no longer listed (today's `delete_if_unlisted` rule,
     `library/radio/mod.rs:98-116`).
   - It answers a `StationCut` per station: `Kept { id, is_favorite, last_played, play_count }`,
     or `Deleted(full row)`.
   - `restore_stations`:
     - puts a kept row's three columns back;
     - re-inserts a deleted row under its id;
     - skips a station whose `station_uuid` is back already, starred again from Browse in the
       window.
2. **Views.**
   - `kept.rs`'s loop becomes one facade call that offers. The restore ends in `kept::refresh`.
   - A deleted row's artwork is held (decision 10). A directory station's cached logo already
     survives through its `radio_logo_answers` row.
3. **Confirmations.** Both retire under the cap. Over it they stay, reworded.
4. **Strings:** "{n} station removed from Favorites", "… from Recently Played" (plurals), and a
   one-station form naming it.

### Phase 6: the playing row leaving the queue (a defect today)

**What you'll see.**
- The track keeps playing.
- No row shows as playing.
- Up Next starts at the row that took its place, and that row plays next: at the track's end, on
  Next, under gapless preload and under crossfade.
- Previous goes to the row before the gap.

This is Strawberry's behaviour (`RemoveItemsWithoutUndo`, `playlist.cpp:2218-2290`).

**Mechanism.**
- `current_index` stays "the row playing", so it is `None` once that row has left.
- A new cursor (recommended `resume_at: Option<usize>`) names the row the next transition plays.
  It is read by `advance`, `advance_skip`, `peek_next`, `previous`, `window_ids`, the view model and
  Up Next, and cleared by anything that seats a row. The current index is read at 45 sites in 6
  files today.
- `queue.json` saves `resume_at` as the current index, so a restart lands on the row that plays
  next.

This lands before Phase 7, whose queue token restores the cursor exactly and so depends on its
shape.

### Phase 7: queue rows and Clear

1. **Engine.**
   - `QueueState::cut(&[usize]) -> QueueCut` replaces `remove_at` and `remove_batch`. It records
     each row's play-order index, insertion-order index and slot, the cursor, and the `retired`
     generation; `prune_missing` is built on it.
   - `restore_cut` checks the generation, re-inserts the slots in ascending order into both
     `play_order` and `original_order`, and puts the cursor back. `tracks` keeps every removed slot
     until a clear (`queue.rs:254`), so the same `QueueEntry` values return.
   - Amend `QueueEntry`'s "never handed out again" doc (`:25-27`) to say an undo can put that row
     back.
   - `take_cleared` hands back the whole old `QueueState`, moved rather than copied, with
     `retired` advanced as `clear` does. `restore_cleared` puts it back and appends anything queued
     since after its tracks. Those rows keep their entry values, since the new `retired` was the
     old one plus the old `tracks.len()`.
2. **App.**
   - `queue_remove` and `queue_remove_batch` return their capture.
   - `queue_clear` returns its capture, or `OverCap` without clearing.
   - The two restores go through `with_state_emit`, playback untouched (decision 4).
3. **Slint and views.**
   - The Clear button calls `Queue.clear-queue()`. Rust clears and offers, or over the cap raises
     the confirmation with a `DialogClaim`.
   - The removal callbacks in `queue_panel/callbacks.rs` offer. The ✕, the menu, Delete and Up
     Next's menu all arrive there.
4. **Strings:** "{n} track removed from the queue" (plural), "Queue cleared", and the confirmation
   reworded.

### Phase 8: a play replacing the queue

1. **Capture.** Inside the same `emit_and_execute` closure as the replace (`playback.rs:67-81`),
   before `clear`, `player_play_tracks` takes the old `QueueState` and a `ResumePoint`: the old
   current entry, `position_ms`, and playing or paused, from `s.source` and `status`. It answers
   `None` when the old queue was empty or identical, and `OverCap` past the cap.
2. **Restore.**
   - Fetch the records around the old current row ahead of the lock, reusing `player_play_tracks`'
     prefetch.
   - Then, in one `emit_and_execute`: put the queue back, seat the window, and resume. Resuming is
     `start_queued` at the old position, seated paused, or stopped where nothing was playing.
   - What a station that was playing becomes is checked against `radio/playback.rs` before that arm
     is built.
3. **Views.** The seven callers (`track_list.rs:113`, `callbacks/mod.rs:114`, `card_actions.rs:34`,
   `favorites/callbacks/subviews.rs:50`, `recently_played/callbacks/subviews.rs:40`,
   `browse/callbacks/folder_actions.rs:27`, and `library/queue.rs:90`'s open-from-outside caller)
   converge on one helper that plays and offers.
4. **String:** "Queue replaced".

### Phase 9: favorites

- **Store.** `unfavorite(ids)` is `UPDATE … SET is_favorite = 0 WHERE id IN (…) AND is_favorite = 1
  RETURNING id`, chunked. It returns exactly the ids it flipped, so un-hearting an artist card
  restores only the tracks that were hearted.
- **App.** `library::favorites` wraps it in `FavoriteWrite` (the `library_changed` bump and the love
  sync). The restore is `set_favorite(flipped, true)`, which queues the matching re-love.
- **Views.** The paths in decision 11 offer: `favorites/callbacks/tracklist.rs:44-56`,
  `card_actions.rs:86-99`, and the row menu's entry.
- **String:** "{n} track removed from Favorites" (plural).

### Phase 10: tests, then docs (each on Kenan's go)

**Tests:**
- `queue_tests`:
  - a `cut`/`restore_cut` round trip with the current row before, inside and after the cut, under
    shuffle (both orders restored);
  - `take_cleared` and `restore_cleared` keeping entry values for rows queued since;
  - the left-row cursor through advance, gapless peek, Previous and persist.
- `playlist_tests`:
  - a remove and restore round trip with a gap present, a track re-added in the window, a
    hard-deleted track, and a merged run of two cuts;
  - the stamp put back only when untouched;
  - delete and restore of a regular and a smart playlist, plus the id-taken fallback.
- `radio_tests`: both `StationCut` arms, and the uuid-conflict skip.
- `search_history` tests: restore with queries added since.
- `notifications_tests`: `relabel` keeping the id, the restart, hover pause and resume, and the gone
  hook firing once on every removal path (the `:117-155` pattern).
- **Sweep:** a held basename survives `sweep_stores`.
- **Existing tests to recheck:** `dialog_openers.rs` and `translations.rs`.

**Docs:**
- `ui-patterns.md`'s "Dialogs, toasts": the slot contract (decisions 2, 5, 6 and 8).
- `library-data.md`: restores as units of work, and the artwork hold.
- `audio-stack.md`: the left-row cursor.
- Offer the `CLAUDE.md` and `README.md` edits.

## Cross-cutting

- **Translations.** Every phase with strings runs `scripts/update-translations.py`, then fills all
  15 catalogs, plurals included, with no empty `msgstr` and no `#, fuzzy` (`translations.rs`). A
  translator note is one `// Translators:` line where a `{}` is a name.
- **Memory.** Idle cost is nil: with no notice up the slot is empty. A hold is bounded by the cap
  and dropped with its notice. A cleared or replaced queue is moved, not copied, so it holds what
  the queue held a moment before.
- **No new persisted state.** No migration, no `settings.json` or `views.json` field, and nothing
  held across a restart.
- **File sizes.** Recount each touched production file at its phase against the 800-line cap.
- **Comments.** Explain why only. Amend `QueueEntry`'s doc, the notification card's header and
  `show`'s locale-rule doc as their code changes.

## Open questions

1. **Replacing a queue past the cap**, for example Play All over a 51k queue, raises "Queue replaced"
   with "Too many to undo" on every such play. That is decision 5 applied without exception;
   staying silent there is the alternative. Recommendation: keep the rule.
2. **Wording.** Every string above is a proposal for Kenan's read before its phase's translation
   pass.

## Verification

- Per phase: `cargo fmt --all`, then `cargo clippy --all-targets --locked --workspace -- -D warnings`,
  then `cargo test --locked --workspace`. The app is never launched from here.
- **Kenan's hand pass, at the end, covering the issue's list:**
  - remove the playing track from the queue, let it end, then repeat and undo;
  - clear the queue while something plays, then undo;
  - leave the page before the notice expires, undo from elsewhere, and check the Radio leave
    doesn't take a removed hand-typed station's artwork;
  - switch language with a notice up.
- **Also by hand:**
  - a run of removals updates one card and its count;
  - hover holds the notice, and Ctrl+Z is inert after expiry and inside text fields;
  - each over-cap path keeps its confirmation, or gives its button-less notice;
  - a play's undo resumes the old track where it was.
