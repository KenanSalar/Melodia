# Plan: Undo for removals (a notice that puts back what went)

Status: **planned** (no phase started) · Owner: Kenan · Created: 2026-10-28 · Issue #109, branch `feat/undo-removals`

> **Validated 2026-10-28** against `5fa3d5ed` (`feat/undo-removals`): three read-only traces (the
> toast stack, shortcuts and confirmations; the library removals; the queue, the equalizer and the
> prior art), direct reads of `notifications.rs`, `notification-stack.slint`, `shortcut-scope.slint`,
> the engine's `queue.rs`, the store's `playlist.rs`, `equalizer.rs` and `artwork_sweep.rs`, and the
> libadwaita and Material documentation cited under [Prior art](#prior-art).
>
> **Revalidated 2026-10-29** against `46c7c529`: three more traces re-checked every citation, plus
> Slint 1.18.1's `timers.rs`, `items/text.rs`, `input.rs` and `typeregister.rs` in the cargo
> registry, and the GNOME HIG, WCAG 2.2.1 and toast design-system guidance under
> [Prior art](#prior-art). That pass fixed the Ctrl+Z routing, Phase 6's cursor, the favorites
> menu path, the station restore and Phase 8's scope.

Working doc. Delete when every phase has shipped and passed Kenan's hand test.

## Context

**What you see today.** Taking something out of Melodia is a one-way door, and the app mostly says
nothing about it:

- **Playlists.** Remove from Playlist on twenty selected rows deletes them at once: no message, no
  count, no way back except re-adding and re-dragging. Deleting a playlist asks first, then says
  nothing afterwards.
- **The queue.** Removing rows (the ✕, the menu, Delete/Backspace, Up Next's menu) is instant and
  silent. Clear asks first, and its dialog says "Playback will stop" when in fact the playing track
  runs to its end. Playing anything from a list replaces the whole queue with no message, a queue
  built by hand included.
- **Removing the playing row from the queue misbehaves on its own, undo or not.** The "playing"
  highlight jumps to the row below, which never plays: at the track's end (and in the gapless
  preload) playback goes to the row after it. By code reading: `QueueState::remove_at`
  (`queue.rs:242-251`) leaves `current_index` on the slid-in row and `advance`/`peek_next`
  (`:338`, `:409`) step from `current_index + 1`. Only the tail clamp is pinned
  (`queue_tests.rs:104-110`); nothing removes a playing row mid-queue and then advances.
- **Elsewhere.** Recent searches' Clear all and the equalizer's Reset wipe their state with no
  prompt or message. Removing a radio station asks first and says nothing after; for a hand-typed
  station that deletes the row, play history included.
- **Favorites.** "Remove {} from Favorites" (the row and card menus) and a card's heart un-heart
  whole selections or whole artists or albums silently, and on the Favorites page the rows vanish.

**Why.** There is no undo machinery anywhere, and the toast stack can't carry one:

- A toast can't be updated in place, its clock can't be restarted, and nothing pauses on hover:
  `show_auto_dismiss` arms a fire-and-forget `slint::Timer::single_shot` (`notifications.rs:114-124`)
  and the only in-place write is the locale refresh (`:198`).
- An action button routes by `NotificationKind` alone (`run_action`, `:270`), with no payload.
- Ctrl+Z is bound nowhere; `ShortcutScope` is the only `FocusScope` (`shortcut-scope.slint`).
- Most store removals destroy what a restore would need: positions are renumbered
  (`renumber_playlist_positions_tx`, `playlist.rs:402`), a deleted row's id and timestamps aren't
  recreatable (`create_playlist_with_tracks` and `save_station` mint new ones), and no store fn
  inserts at given positions (`insert_items_tx`, `:283`, writes one contiguous run stamped now).

**Outcome.** Each removal listed above raises one notice naming what went, with an Undo button.
Pressing it, or Ctrl+Z while the notice is up, puts things back as they were, positions included. A
play replaces the queue silently unless the queue it replaces was built by hand (decision 12).

- The notice stays up for `UNDO_NOTICE_MS`, and doesn't expire under the pointer or while the window
  is in the background.
- A screen reader announces it.
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
5. **One cap for every hold of records, `UNDO_HOLD_CAP` rows** (provisionally 10,000, confirmed by a
   heaptrack of a full hold in the playlist phase). The unit that captures checks it before it
   writes.
   - **The queue's holds are exempt.** They are ids only, a cut's copied and a clear's or a
     replace's moved, so a hold costs what the queue cost a moment before. The same heaptrack
     measures a moved full-library queue beside the capped playlist hold.
   - **Over the cap a removal behaves exactly as it does today.** Its confirmation stays where it
     has one, now saying it can't be undone, and its notice names what went with no button.
   - **Under the cap a removal is never confirmed.** Every removal gets one answer, never both.
   - Strawberry's equivalent is `kUndoItemLimit = 500` plus a "too large to undo" confirm. Melodia
     holds ids and positions rather than full item records, so the same budget covers more rows.
6. **A removal that removed nothing raises nothing.** No notice for clearing an empty queue or empty
   recent searches, resetting a flat curve, or replacing a queue with the identical list. Each
   removal returns its count, which today's queue removals drop (`library/queue.rs:147`).
7. **Timing and hover.**
   - `UNDO_NOTICE_MS = 10_000`. That is Material's Long snackbar duration, its value for a snackbar
     carrying an action. The one timed toast with a button today, the exclusive-output refusal,
     runs a bare 6 s (`subscribers.rs:219`), and the undo notice must outlast it.
   - **Every timed toast pauses while hovered**, as libadwaita's do, **and while the window is
     unfocused or hidden**, so a removal made just before switching windows keeps its whole
     window. A resume restarts the full interval. The pause is a property of the card, not of undo.
   - **The clock lives in the card**: a Slint `Timer` whose `running` reads the card's hover and
     `Theme.window-focused`, which the winit `Focused` arm already writes
     (`winit_filter.rs:298-305`). Rust holds no timer, no epoch and no hover callback.
   - **The card is a polite live region** (`accessible-live-region`, which Slint 1.18.1 declares).
     WCAG 2.2.1 excuses a message that vanishes only when the same function can be reached
     another way. Ctrl+Z is that way, and a screen reader user has to hear the notice to know it
     is there.
   - The issue's "while it holds the keyboard" has nothing to attach to: the card's buttons take
     no focus, and `ShortcutScope` stays the only `FocusScope`. Ctrl+Z is the keyboard's way in.
8. **Ctrl+Z means Undo only while the notice is up, and nothing else.**
   - A focused text field keeps its own Ctrl+Z, even with nothing to undo: Slint's `TextInput`
     accepts `StandardShortcut::Undo` before the key bubbles, whatever its undo stack holds
     (`i-slint-core` `items/text.rs:1108-1111`, `input.rs:1143`). Search's field keeps focus after
     Clear all (`search-view.slint:29-37`), so there the button is the way back.
   - **A modal dialog doesn't gate it.** The arm sits ahead of the `Dialog.open || Onboarding.open`
     gate (`shortcut-scope.slint:115-118`), matching the Undo button, which works over a dialog
     because the stack is mounted after `DialogOverlay` (`app-window.slint:701-711`). The
     equalizer is such a dialog.
   - No redo, no Ctrl+Shift+Z.
9. **A deleted row comes back under its own id.** Neither `playlists.id` nor `radio_stations.id` is
   `AUTOINCREMENT`, so the old id is free unless a row created in the window took it (SQLite reuses
   the largest). Only then does the restore take a fresh id.
10. **Artwork a held row names is held from the sweeps.**
    - A deleted playlist's custom cover and a deleted station's artwork are the files
      `library-data.md` calls unrebuildable.
    - Two sweeps reach them inside the window. The Radio page's leave runs the logo prune
      (`RADIO_GRACE`, `artwork_sweep.rs:118`), and the issue asks for "leave the page before the
      notice expires" by hand. `sweep::run`, an hour's mtime grace, follows every finished scan
      (`scan/finish.rs:38`) and boot, and takes a composite older than that.
    - Every deleted station's logo is held, hand-typed or from the directory: both live in
      `radio_logos_dir` behind a `radio_logo_answers` row that expires at 14 days or under the
      32 MiB trim (`logos.rs:50-84`).
    - A small hold registry is unioned into `sweep_stores`' one reference set (`:149`). It is
      released when the slot drops, after which the next sweep reaps as it does today.
11. **Favorites: the explicit removals raise the notice. A lone heart that stays on screen doesn't.**
    - Raising it: the row and card menus' "Remove … from Favorites", a card's heart (a whole artist
      or album), and any heart on the Favorites page, whose row then leaves the list.
    - Not raising it: a single heart toggled on another list, Now Playing's heart, or L. Each still
      shows its new state, and pressing it again is its own undo.
    - Rust can't tell a row's heart from its menu entry today: both call the list global's
      `toggle-row-favorite` (`track-list.slint:149`, `:153`). The menu's remove entry, mounted once
      (`track-context-menu.slint:190-197`), calls one new shared callback instead. The Favorites
      page's heart raises through its own handler (`favorites/callbacks/tracklist.rs:44-57`), and a
      card's heart and menu already share `CardActions.set-favorites`.
    - Stations follow the same rule. The star on Radio's Favorites tab, whose card then leaves
      (`set_kept_favorites`, `radio/callbacks/stations.rs:170-196`), raises it; Browse's star
      (`:201-254`) stays on screen and doesn't.
12. **A play raises "Queue replaced" only over a queue built by hand.**
    - A routine play over a queue an earlier play built stays silent: a notice on every
      double-click teaches the eye to skip the one that matters. GNOME's HIG asks for a toast's
      button only "if it is directly relevant … and will be generally useful".
    - `QueueState` carries one `hand_edited` flag. Play Next and append, a move, a drop-in and a
      cut set it; the replace in `player_play_tracks` clears it; a restore puts back the held value.
    - Boot restore seats it `true`. Where the queue came from isn't known and nothing new is
      persisted, so a queue that survived a restart counts as built by hand.
    - Only then does the capture run, so a play over a queue nobody built holds nothing.

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
time to reach it. The hover and focus pause, the live region and Ctrl+Z are Melodia's answer to
that.

**GNOME HIG, toasts** (<https://developer.gnome.org/hig/patterns/feedback/toasts.html>): a toast
reports an event, and carries a button only "if it is directly relevant to the message that is
being communicated, and will be generally useful". Decision 12 applies it.

**WCAG 2.2.1 Timing Adjustable** (<https://www.w3.org/WAI/WCAG22/Understanding/timing-adjustable.html>):
a message that disappears sets no time limit while the same function can be reached another way,
and owes a way to extend it otherwise. Decision 7's live region and decision 8's Ctrl+Z answer it.

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
  id already, and a queue cut names its rows by it.

**rox**: no queue or playlist undo. Its keymap page keeps a single-slot snapshot before a reset
(`settings/window/keymap_page.rs:43,57-61`), which is the equalizer's shape here.

## Confirmations this retires

The issue's Notes ask for this list before any confirmation goes. "Over the cap" is decision 5.

| Confirmation | Raised at | Fate |
|---|---|---|
| Delete playlist | `Dialog.open-delete-playlist` (`dialog.slint:261-271`), from `tab-pills.slint:155` and `playlists-tab.slint:28` | Goes. Kept over the cap, reworded to say it can't be undone. |
| Delete N playlists | `Dialog.open-delete-playlists` (`dialog.slint:275-285`), from `playlists-tab.slint:37` | Goes. Kept over the cap, reworded, and made a true plural. |
| Clear queue | inline, `queue-panel.slint:240-249` | Goes outright: the queue's holds are uncapped (decision 5), so no opener is needed. "Playback will stop" was never true. |
| Remove station (Favorites / Recently Played) | `Dialog.open-remove-station` (`dialog.slint:291-307`), from `station-context-menu.slint:186` and `kept-stations-tab.slint:62` | Goes. Kept over the cap, reworded. |
| Remove N stations | `Dialog.open-remove-stations` (`dialog.slint:311-325`), from `station-context-menu.slint:184` | Goes. Kept over the cap, reworded. |
| Remove watched folder | inline, `library-section.slint:89-102` | **Stays**: out of scope. |
| Restart (titlebar, tray, backdrop), exclusive output | `window-chrome-section.slint`, `appearance-section.slint`, `output-section.slint` | Untouched: not removals. |

A kept confirmation is raised from Rust once the size is known, across an await, so it takes a
`DialogClaim` (`ui-patterns.md`, held by `dialog_openers.rs`).

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-views/src/ui/undo/mod.rs` (new) | `UndoWindow`: the one slot, `offer` (merge, or replace and dismiss the old row), the Undo/Ctrl+Z handler acting only on the slot's own row id, the slot's release when its row goes, and `UNDO_NOTICE_MS` |
| `crates/melodia-views/src/ui/undo/removal.rs` (new) | `Removal`, one variant per surface wrapping that surface's token: `absorb` (merge or hand back), `count`, its wording recipe, and `restore`, which dispatches each arm to the surface module that raised it. No surface names another's. |
| `crates/melodia-views/src/ui/shell/notifications.rs` | `recipes` keeps its shape. Adds a timed localized push, an in-place `relabel`, `restart` (a bump of the row's `restarts`), one row-removed listener, and the `Undo` arm in `run_action`, which now takes the row id. `show_auto_dismiss` writes the row's `timeout-ms` instead of arming a Rust timer. `remove_at` stays the one removal path. |
| `crates/melodia-ui/ui/globals/undo-text.slint` (new) | `UndoText`: every undo wording as `pure callback`s with plural `@tr`, spelled as `Settings.playlist-import-title` spells them (`settings.slint:434-437`), keeping `settings.slint` (625 lines) from growing. Exported from `app-window.slint`, Rust calling it. |
| `crates/melodia-ui/ui/models.slint` | `NotificationKind.undo`; `NotificationRow` gains `timeout-ms` (0 for a sticky row) and `restarts` |
| `crates/melodia-ui/ui/globals/notifications.slint` | `undo-row` (the slot's row id, `-1` with none) and `undo(int)` |
| `crates/melodia-ui/ui/components/dialog/notification-stack.slint` | Per card: a hover sentinel wrapping the layout, the `Timer` that is its clock, and the polite live region |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint` | The Ctrl+Z arm, ahead of the modal gate |
| `crates/melodia-ui/ui/components/track-list/track-context-menu.slint` | The remove-from-Favorites entry calls one shared callback rather than the list's `toggle-row-favorite` (decision 11) |
| `crates/melodia/src/boot/ui_setup/` | `notifications::install` and `UndoWindow` move ahead of `install_views` (`install.rs:68`) |
| `crates/melodia-core/src/entities/undo.rs` (new) | `UndoCapture<T>` (`Held(T)` or `OverCap { rows }`) and `UNDO_HOLD_CAP`, named by store, app and views |
| `crates/melodia-core/src/entities/{playlist,radio}.rs` | The plain tokens beside their entity: `PlaylistItemsCut`, `PlaylistSnapshot`, `StationCut` |
| `crates/melodia-store/src/database/queries/playlist.rs` | Capture inside the removal transactions. `insert_items_tx` takes explicit `(track_id, position, added_at)` rows, its two callers building contiguous ones. `update_playlist_thumbnail_and_timestamp_tx` takes its stamp from the caller, which minted it inside (`:515`). |
| `crates/melodia-store/src/database/queries/playlist_restore.rs` (new) | `restore_playlist_items`, `restore_playlists`, keeping `playlist.rs` (550 lines) under the 800-line cap |
| `crates/melodia-store/src/database/queries/radio.rs` | `remove_stations`: the per-id loop now in `radio/callbacks/kept.rs:106-114`, as one transaction. Also `restore_stations`. |
| `crates/melodia-store/src/database/queries/track/write.rs` | `unfavorite`, returning the ids it flipped |
| `crates/melodia-app/src/services/artwork_holds.rs` (new) | `ArtworkHold` (released on drop) and `held_basenames()`, unioned in `tasks::artwork_sweep::sweep_stores` |
| `crates/melodia-app/src/library/{playlists,radio/mod,queue,search,favorites}.rs` | Each removal returns its capture, and each surface gets one restore fn |
| `crates/melodia-app/src/services/search_history.rs` | `clear` hands back the list; `restore` puts it back |
| `crates/melodia-engine/src/player/engine/queue.rs` (534 lines) | `cut`/`restore_cut` replacing `remove_at` and `remove_batch` (`prune_missing` built on `cut`), `take_cleared`/`restore_cleared`, the `playing_left` and `hand_edited` flags, and one `seat` method. Split into `queue/` if it passes 800. |
| `crates/melodia-views/src/ui/equalizer.rs` | `apply_curve`, shared by preset select, reset and restore |

**Duplication collapsed on the way through:**

- `wire_select_preset` and `wire_reset` each spell "set the model, push the gains, persist the
  curve" (`equalizer.rs:94-129`).
- `remove_at` and `remove_batch` are two copies of one cut (`queue.rs:231-294`), with
  `prune_missing` a third.
- The station removal loop lives in the view (`radio/callbacks/kept.rs:106-114`), with a second in
  `set_kept_favorites` (`radio/callbacks/stations.rs:179`).
- The six views callers of `player_play_tracks` each spell the play, and will each owe the offer.
  They converge on one views helper.

**Threading.** `NotificationsUi` and `UndoWindow` are installed ahead of `install_views`
(`install.rs:68`). `notifications::install` needs only the window (`notifications.rs:237`), while
today the playlists, favorites, recently-played, radio and search installs (`views.rs:113-117`) and
`install_equalizer` (`:189`) all run before it. Each removing installer then takes
`&Rc<UndoWindow>`, the way `notifications` travels today; `queue_panel::install` and `recent::wire`
gain the parameter.

## Phases

Each phase stops at the static gates (`cargo fmt --all`, `cargo clippy --all-targets --locked
--workspace -- -D warnings`, `cargo test --locked --workspace`) and Kenan's hand test. New tests
wait for Phase 10.

### Phase 1: the notice, with the equalizer's Reset as its first user

The shell and its simplest user land together, the shell having nothing to show by hand on its own.

1. **Slint.**
   - `NotificationKind.undo`, and `NotificationRow`'s `timeout-ms` and `restarts`, in
     `models.slint`.
   - `Notifications.undo-row`, the slot's row id or `-1`, and `undo(int)`, Ctrl+Z's way in with
     `undo-row`. The card's Undo arrives through `action` and reaches the same handler.
   - **The card's clock**: a `Timer` with
     `running: data.timeout-ms > 0 && !hovered && Theme.window-focused`, whose `triggered` calls
     the existing `Notifications.dismiss(data.id)`, and a `changed` mirror on `data.restarts`
     calling `restart()` (as `filter-throttle.slint:20` does). A resume restarts the full interval.
     A one-frame hover blink (`slint-pitfalls.md`) only restarts a clock that a leave restarts
     anyway, so the hover needs no latch.
   - **The hover sentinel** is a `TouchArea` wrapping the card's layout as its ancestor, which
     intercepts the grab and forwards the press (`input_items.rs:74-108`; the queue panel's
     sentinel is the precedent). Below the layout it would lose hover over Undo and ✕; above it,
     it would steal their presses. It answers `accept` to every wheel event, an empty one
     included. The gaps between cards still pass through, which is what the card's header comment
     protects; reword that comment to match.
   - `accessible-live-region: polite` on the card's text.
   - Ctrl+Z ahead of `ShortcutScope`'s modal gate (`:115-118`), gated on
     `Notifications.undo-row >= 0` alone (decision 8).
   - `UndoText` added to `app-window.slint`'s export block.
2. **`NotificationsUi`.**
   - `show_auto_dismiss` keeps its signature and writes `timeout-ms` in place of its
     `single_shot`, so every timed toast gains both pauses.
   - A timed localized push takes a params struct (four arguments would want one). Add
     `relabel(id, …)` and `restart(id)`, both through `set_row_data`.
   - One row-removed listener, which `UndoWindow` registers. `remove_at` runs inside `show`'s cap
     eviction (`:97-99`), so the listener is called once the borrows are released, as
     `refresh_for_locale` guards (`:175-176`).
   - `run_action` takes the row id `on_action` already has (`:256-261`), and its `Undo` arm is
     `invoke_undo(id)`. `on_action`'s dismiss after it stays harmless, the handler having
     dismissed its own row already.
3. **`UndoWindow` and `Removal::Equalizer`.**
   - The install order moves first (see Threading).
   - The slot writes its row id to `undo-row`. A replace dismisses the old row, so one slot shows
     one notice, and an Undo naming any other row does nothing.
   - Undo takes the slot, dismisses the row and spawns the restore. A failed restore raises
     `show_failure` with "Couldn't undo".
4. **The equalizer.**
   - `apply_curve` is the one write of gains, preamp and preset, both the engine's and persisted.
     Reset makes two settings writes today, gains with the preset name and the preamp on its own
     (`library/settings/equalizer.rs:15-35`); a preset pick leaves the preamp alone.
   - Reset captures the curve first: the gains from the `VecModel`, the preamp from
     `Equalizer.preamp`, the preset from `preset-idx`. It skips the offer when the curve was
     already flat at 0 dB.
   - Restore maps the preset name back to an index the way `seed` does, falling to the Custom slot
     (`equalizer.rs:33-45`).
5. **Strings:** "Undo", "Equalizer reset", "Couldn't undo".

**By hand.**
- Reset raises the notice, and hovering holds it, over Undo and ✕ too.
- Undo and Ctrl+Z restore both the card and the dialog, with the dialog still open.
- Ctrl+Z after the notice expires, or inside a text field, does nothing to the curve.
- A language switch with the notice up re-renders it.
- A Copy toast pauses under the pointer too, and a notice outlasts a switch to another window.

### Phase 2: recent searches' Clear all

- `SearchHistoryState::clear` hands back the old list (`std::mem::take`); today it answers
  `Ok(vec![])` (`search_history.rs:58-66`).
- `restore` puts the list back, with any query added since kept in front, deduped and truncated to
  `MAX_ENTRIES`, which is private, so `restore` lives beside it.
- The facade lives in `library::search`. `search/callbacks/recent.rs:44-53` offers, and the
  restore ends in `push_recent_rows_to_slint` (`search/fetch.rs:131-138`) as the clear does.
- The field keeps focus after Clear all, so Ctrl+Z edits the field here and the button is the way
  back (decision 8).
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
     - drop rows whose track is gone or is back in the playlist (the user re-added it). The first
       filter is mandatory: tracks are hard-deleted (`scan/watch_batch.rs:96-108`,
       `scan/finish.rs:31-34`, the folder cascade), and `INSERT OR IGNORE` doesn't cover a
       foreign-key failure;
     - merge the survivors around the removed rows' indices;
     - write the survivors that moved through `batch_update_positions_pairs`;
     - insert the removed rows with their own positions and `added_at`;
     - refresh the automatic cover;
     - put `updated_at` back only where it is still the removal's own stamp. The grid orders by
       `updated_at DESC` (`entity_keyset.rs:125-128`), so that puts the card back in its place.
   - A track sits in a playlist once (`idx_playlist_items_unique` is on
     `(playlist_id, track_id)`), so the track id names its row.
3. **Views.**
   - `wire_remove_track` (`playlists/callbacks/detail.rs:140`) offers.
   - The restore ends in the removal's own refresh, `refresh_detail` (which reloads the open
     detail; `track_rows::remove_rows` only drops model rows) and `refresh_after_write`, which
     already handles an off-screen page.
4. **Measure.** One heaptrack of a held cap-sized cut confirms `UNDO_HOLD_CAP`, and one of a moved
   full-library queue confirms the queue's exemption (decision 5). Both results are recorded here.
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
     generalized `insert_items_tx`. The row goes in with `track_count` and `total_duration_ms` at
     0, since the stats triggers add per item rather than recount (`initial_schema.sql:548-593`),
     and with its `search_name`, whose backfill runs only at launch. The triggers leave
     `updated_at` alone; the cover refresh stamps it, so the held stamp is written last.
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
     `library/radio/mod.rs:98-116`). That rule reads outside any transaction today, so the unit
     re-reads inside it.
   - It answers a `StationCut` per station: `Unstarred { id }`,
     `HistoryCleared { id, last_played, play_count }`, or `Deleted(full row)`.
   - `restore_stations` reverses only what its removal wrote (decision 4):
     - an unstarred row is starred again;
     - a cleared history is added back, `play_count = play_count + held` and
       `last_played = max(last_played, held)`, so a play in the window survives;
     - a deleted row is re-inserted under its id;
     - a station that is back already is skipped: by `station_uuid` when Browse starred it again,
       or by `stream_url` (`station_id_with_url`, `radio.rs:145-153`) for a hand-typed one, whose
       NULL uuid never conflicts and which a file import can re-create (`radio.rs:226-229`).
2. **Views.**
   - `radio/callbacks/kept.rs`' loop becomes one facade call that offers, and so does
     `set_kept_favorites`' (`radio/callbacks/stations.rs:170-196`) where the Favorites tab's star
     drops a card (decision 11). The restore ends in `kept::refresh` (`radio/kept.rs:204`).
   - A deleted row's logo is held (decision 10), hand-typed or from the directory.
3. **Confirmations.** Both retire under the cap. Over it they stay, reworded, raised from Rust with
   a `DialogClaim`. Neither opener is in `FOLDED_KINDS`, and their kind write is a ternary
   (`dialog.slint:304`, `:322`) the test's needle can't match.
4. **Strings:** "{n} station removed from Favorites", "… from Recently Played" (plurals), and a
   one-station form naming it.

The radio facade holds: only `library::radio` and `library::radio_files` name `queries::radio`
(`radio_facade.rs`).

### Phase 6: the playing row leaving the queue (a defect today)

**What you'll see.**
- The track keeps playing.
- No row shows as playing.
- Up Next starts at the row that took its place, and that row plays next: at the track's end, on
  Next, under gapless preload and under crossfade. Play Next lands ahead of it.
- Previous goes to the row before the gap.
- Removing the last row lets the track end the queue, or wrap under Repeat All.
- While a track is still loading, removing it starts the row that took its place, as today.

This is Strawberry's behaviour (`RemoveItemsWithoutUndo`, `playlist.cpp:2218-2290`).

**Mechanism: one flag over today's index, not a second cursor.**
- `remove_at`, `remove_batch` and `prune_missing` already land `current_index` on the row that took
  the removed row's place (`queue.rs:242-252`), and every reorder already keeps `current_index` on
  its row (`insert_at`, `move_rows`). A second index would need every one of those taught again.
  The defect is only that the next step goes past that row, and the highlight claims it plays.
- **`playing_left: bool` says the current row hasn't started.** The app sets it under the same
  lock, only when the cut took the current row while the status is Playing or Paused; at the tail
  the index becomes `None`. `Loading`, whose deck is already empty (`transport.rs:152-165`), and
  Stopped keep today's behaviour.
- Readers that change:
  - `advance`, `advance_skip` and `peek_next` return the current row rather than step past it;
    at `None` they wrap under All and end otherwise;
  - the view model's highlight;
  - Up Next starts at the current row (`up_next.rs:275-285`);
  - Play Next inserts at it rather than after it (`queue.rs:199`);
  - `previous` at `None` goes to the last row, the one before the gap;
  - `begin_shuffle` keeps `None` at the tail, where it would seat row 0 (`:436`, `:457`).
- Every seat clears the flag. `player_play_tracks`' direct write (`playback.rs:70`) moves behind
  one `seat` method.
- `queue.json` saves the current index as today, with position 0 for that row: `last_position` is
  still the removed track's.
- The prune's landing rule, which boot restore relies on (`library/queue.rs:357-358`), stays as it
  is.

This lands before Phase 7, whose queue token restores the flag with the cursor.

### Phase 7: queue rows and Clear

1. **Engine.**
   - `QueueState::cut(&[QueueEntry]) -> QueueCut` replaces `remove_at` and `remove_batch`, and
     `prune_missing` is built on it. It names rows by entry because the UI computes a play index
     the runtime applies later (`queue_panel/callbacks.rs:37-64`), so a removal racing an undo
     would cut whatever sits there by then; the drop-in already takes an entry
     (`library/queue.rs:108`). The cut holds plain ids, `(track_id, play_index, original_index)`
     per row, plus the cursor and `playing_left`, and returns its count.
   - `restore_cut` pushes the held ids as new slots, inserting in ascending index order into both
     `play_order` and `original_order`. Fresh entries keep `QueueEntry`'s "never handed out again"
     (`:25-27`) true and need no generation check.
   - `take_cleared` moves the three vectors out, clearing as `clear` does. `restore_cleared`
     re-adds the held rows under fresh entries, ahead of anything queued since.
   - **One cursor rule for both restores**: the held cursor and flag come back only if nothing was
     seated since the removal. Otherwise the live current row stands, found again after the
     insert.
   - The live shuffle and repeat flags stand, since they persist on their own
     (`library/queue.rs:292-302`), and restored rows take the order the live shuffle flag implies.
   - Each restore bumps the version, `with_state_emit` publishing only on a change
     (`state/mod.rs:195-205`).
2. **App.**
   - `queue_remove`, `queue_remove_batch` and `queue_clear` return their capture. The queue's
     holds are uncapped (decision 5), so Clear always clears.
   - The two restores go through `with_state_emit`, playback untouched (decision 4).
3. **Slint and views.**
   - The Clear button calls `Queue.clear-queue()` directly, its inline confirmation gone. Rust
     clears and offers.
   - The removal callbacks in `queue_panel/callbacks.rs` resolve their rows to entries at the click
     (`QueueRows::entry_at`, `paged_rows/queue_rows.rs:119`) and offer. The ✕, the menu, Delete
     and Up Next's menu all arrive there.
4. **Strings:** "{n} track removed from the queue" (plural) and "Queue cleared". The clear
   confirmation's strings go.

### Phase 8: a play replacing the queue

1. **Capture.**
   - Only over a queue built by hand (decision 12). With `hand_edited` down, nothing is held.
   - Inside the same `emit_and_execute` closure as the replace (`playback.rs:67-81`), before
     `clear`, `player_play_tracks` takes the old queue as `take_cleared` does, and a
     `ResumePoint`: the old current entry, `position_ms` and the status, from `s.source` and
     `status`. The closure returns actions, so the capture leaves through an outer binding, as
     `resume_station`'s does (`playback.rs:117-128`).
   - It answers `None` when the old queue was empty or held the same tracks in the same insertion
     order. That compares `original_order`, not the play ids, which a shuffled replace
     randomizes (`playback.rs:72-74`).
2. **Restore.**
   - Fetch the records around the old current row ahead of the lock, reusing `player_play_tracks`'
     prefetch.
   - Then, in one `emit_and_execute`: put the queue back, seat the window, and resume.
     `start_queued` takes no position (`transport.rs:141-148`), so a track that was playing or
     paused resumes through `play_track_inner(…, Some(pos))` (`:415-431`), a paused one paused
     again as `start_queued`'s `re_pause` does (`:189-194`). `Loading` or a stopped seated track
     comes back stopped.
   - A station that was playing comes back seated paused through `restore_station`
     (`persistence.rs:40-49`): re-tuning is network work (`playback.rs:147-194`) that can't run
     inside the lock.
   - The live shuffle flag stands. `spawn_play_then_shuffle` (`callbacks/mod.rs:104-125`) persists
     one after its play.
3. **Callers.**
   - The six views callers (`track_list.rs:113`, `callbacks/mod.rs:114`, `card_actions.rs:34`,
     `favorites/callbacks/subviews.rs:50`, `recently_played/callbacks/subviews.rs:40`,
     `browse/callbacks/folder_actions.rs:27`) converge on one helper that plays and offers.
   - `open_files` (`library/queue.rs:90`) returns its capture too. A forwarded launch
     (`boot/tasks.rs:121`) offers it; the boot open (`:98`) drops it, the notice bridge not being
     up yet.
4. **String:** "Queue replaced".

### Phase 9: favorites

- **Store.** `unfavorite(ids)` is `UPDATE … SET is_favorite = 0 WHERE id IN (…) AND is_favorite = 1
  RETURNING id`, chunked. It returns exactly the ids it flipped, so un-hearting an artist card
  restores only the tracks that were hearted.
- **App.** `library::favorites` wraps it in `FavoriteWrite` (the `library_changed` bump and the love
  sync). The love sync covers only the flipped ids, where today it covers every id passed
  (`favorites.rs:86-100`). The restore is `set_favorite(flipped, true)`, which queues the matching
  re-love.
- **Slint.** The row menu's remove entry calls the one shared callback (decision 11).
- **Views.** The paths in decision 11 offer: `favorites/callbacks/tracklist.rs:44-57`,
  `card_actions.rs:86-99`, and the shared callback's handler.
- **String:** "{n} track removed from Favorites" (plural).

### Phase 10: tests, then docs (each on Kenan's go)

**Tests:**
- `queue_tests`:
  - a `cut`/`restore_cut` round trip with the current row before, inside and after the cut, under
    shuffle (both orders restored);
  - `restore_cleared` keeping the rows queued since, and both restores minting fresh entries;
  - the cursor rule: a restore after a seat keeps the live current row;
  - `playing_left` through advance, gapless peek, Next, Previous, Play Next, shuffle and persist
    (position 0), at the tail too; a cut while `Loading` keeps today's start;
  - `hand_edited` set by each hand edit, cleared by a replace, seated by boot restore.
- `playlist_tests`:
  - a remove and restore round trip with a gap present, a track re-added in the window, a
    hard-deleted track, and a merged run of two cuts;
  - the stamp put back only when untouched;
  - delete and restore of a regular and a smart playlist, its `track_count` and
    `total_duration_ms` matching its items, plus the id-taken fallback.
- `radio_tests`: every `StationCut` arm, a history restore after a play in the window keeping the
  play, and both skips (the uuid, and a hand-typed `stream_url`).
- `search_history` tests: restore with queries added since.
- `notifications_tests`: `relabel` keeping the id, `restart` bumping `restarts`, a timed row
  carrying its `timeout-ms`, a replace dismissing the old Undo row, an Undo naming another row
  doing nothing, and the row-removed listener firing once on every removal path, the eviction
  inside `show` included (the `:117-154` pattern).
- **Slint pins:** the card's `Timer` gates on its hover and `Theme.window-focused`, and the row
  menu's remove entry calls the shared callback rather than `toggle-row-favorite`.
- **Sweep:** a held basename survives `sweep_stores`.
- **Existing tests to recheck:** `dialog_openers.rs` and `translations.rs`; the `remove_at` and
  `remove_batch` tests in `queue_tests`, `queue_entry_tests`, `state_tests` and the views crate's
  `paged_rows/tests/queue_rows_tests.rs:183,216`; and the timer-observing `show_completion` test.

**Docs:**
- `ui-patterns.md`'s "Dialogs, toasts": the slot contract (decisions 2, 5, 6, 8 and 12), and the
  card owning its clock.
- `library-data.md`: restores as units of work, and the artwork hold.
- `audio-stack.md`: `playing_left`.
- Offer the `CLAUDE.md` and `README.md` edits.

## Cross-cutting

- **Translations.** Every phase with strings runs `scripts/update-translations.py`, then fills all
  15 catalogs, plurals included, with no empty `msgstr` and no `#, fuzzy` (`translations.rs`). A
  translator note is one `// Translators:` line where a `{}` is a name.
- **Memory.** Idle cost is nil: with no notice up the slot is empty, and the toast clock is a
  `Timer` per card that goes with its row. A hold of records is bounded by the cap and dropped with
  its notice. A queue hold is ids, a cleared or replaced queue moved rather than copied, so it
  holds what the queue held a moment before; a play over a queue nobody built holds nothing.
- **No new persisted state.** No migration, no `settings.json` or `views.json` field, and nothing
  held across a restart.
- **File sizes.** Recount each touched production file at its phase against the 800-line cap.
- **Comments.** Explain why only. Amend the notification card's header and `show`'s locale-rule
  doc as their code changes. `QueueEntry`'s doc stays true, restores minting fresh entries.

## Open questions

1. **Wording.** Every string above is a proposal for Kenan's read before its phase's translation
   pass.

Settled 2026-10-29: a play past the cap no longer arises, the queue's holds being uncapped
(decision 5), and a play raises its notice only over a queue built by hand (decision 12).

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
  - hover and a switch to another window hold the notice, and Ctrl+Z is inert after expiry and
    inside text fields but works over a dialog;
  - each over-cap path keeps its confirmation, or gives its button-less notice;
  - a play over a queue you built raises the notice and its undo resumes the old track where it
    was; a play over a queue a play built raises nothing.
