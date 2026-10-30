# Plan: Undo for removals (a notice that puts back what went)

Status: **Phases 1 to 5 done** (hand-tested 2026-10-29 and 2026-10-30; Phase 3's measure waits for Kenan's go), **Phase 6 next** · Owner: Kenan · Created: 2026-10-28 · Issue #109, branch `feat/undo-removals`

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

- The notice stays up for 6 s by default, and doesn't expire under the pointer or while the window
  is in the background.
- A screen reader announces it.
- A run of the same removal updates the one notice, adds to its count and restarts its clock.
- Where undo covers a removal, the confirmation in front of it goes.
- A removal too large to hold says so and carries no button.
- Settings' new Accessibility category turns undo off, per kind, or for single items, and sets the
  notice's duration (decision 13).
- **Unchanged:** what each removal writes, the watched-folder confirmation, tag edits (out of
  scope), and everything in the library. No schema change, and no hold survives a restart.

## Decisions settled here

1. **Write now, keep what restores it.** The issue's preferred shape. Lists refresh from the store,
   so a deferred write would leave every list describing something that hasn't happened. Strawberry
   and fooyin both apply the edit at once and keep a command that reverses it.
2. **One slot, not a stack.** A removal of the same kind on the same target (one playlist, the
   queue, one station tab) **merges** while its notice is up: counts add, the clock restarts, and
   the restore walks the merged steps newest first. Anything else **replaces** the slot, and the
   earlier removal stands. This is libadwaita's batched-undo example. It avoids Nautilus losing the
   first item of a run and Loupe stacking a toast per item (GNOME HIG #155). A whole-state hold
   (the equalizer's curve) merges by keeping the older hold: walking snapshots newest first ends
   there anyway.
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
   writes, and `Removal::absorb` checks a merged run's total: a run past the cap can't be undone
   at all, its notice going button-less with the run's count.
   - **The queue's holds are exempt.** They are ids only, a cut's copied and a clear's or a
     replace's moved, so a hold costs what the queue cost a moment before. The same heaptrack
     measures a moved full-library queue beside the capped playlist hold.
   - **Over the cap a removal behaves exactly as it does today.** Its confirmation stays where it
     has one, now saying it can't be undone, and its notice names what went with no button.
   - **A removal undo covers is never confirmed.** Every removal gets one answer, never both. One
     that undo doesn't cover, over the cap or turned off in Settings (decision 13), keeps its
     confirmation.
   - Strawberry's equivalent is `kUndoItemLimit = 500` plus a "too large to undo" confirm. Melodia
     holds ids and positions rather than full item records, so the same budget covers more rows.
6. **A removal that removed nothing raises nothing.** No notice for clearing an empty queue or empty
   recent searches, resetting a flat curve, or replacing a queue with the identical list. Each
   removal returns its count, which today's queue removals drop (`library/queue.rs:147`).
7. **Timing and hover.**
   - `UNDO_NOTICE_MS = 6_000` (Kenan, 2026-10-29: 10 s, Material's Long snackbar, read as too
     long). That sits between libadwaita's 5 s default and Material's Long, and matches the other
     timed toast with a button, the exclusive-output refusal (`subscribers.rs:219`). The pauses
     below cover anyone reading it.
   - **Every timed toast pauses while hovered**, as libadwaita's do, **and while the window is
     unfocused or hidden**, so a removal made just before switching windows keeps its whole
     window. A resume restarts the full interval. The pause is a property of the card, not of undo.
   - **The clock lives in the card**: a Slint `Timer` whose `running` reads the card's hover and
     `Theme.window-focused`, which the winit `Focused` arm already writes
     (`winit_filter.rs:298-305`). Rust holds no timer, no epoch and no hover callback.
   - **A `restarts` bump restarts only a running clock.** Slint 1.18.1's `Timer::restart()`
     re-arms a stopped timer (`i-slint-core/timers.rs:171`), and the generated `update_timers`
     runs only when `running` or `interval` changes (`passes/lower_timers.rs:110-111`), so an
     unguarded bump under the pointer would dismiss the card being read.
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
    - Stations follow the same rule (Kenan, 2026-10-30). On Radio's Favorites tab the heart takes
      the card off the tab, so it is the removal: it goes through the menu's `remove-stations`,
      raising the notice, and asks first with undo off (Phase 10). A directory card there loses its
      trash, which did the same thing, and Edit takes the trash's slot; a hand-typed card has no
      heart and keeps its trash.
    - The heart on Recently Played and Browse, the menu's heart entry there included, leaves the
      card where it is and raises nothing.
12. **A play raises "Queue replaced" only over a queue built by hand.**
    - A routine play over a queue an earlier play built stays silent: a notice on every
      double-click teaches the eye to skip the one that matters. GNOME's HIG asks for a toast's
      button only "if it is directly relevant … and will be generally useful".
    - `QueueState` carries one `hand_edited` flag. Play Next and append, a move, a drop-in and a
      cut set it; the replace in `player_play_tracks` clears it; a restore puts back the held value.
    - Boot restore seats it `true`. Where the queue came from isn't known and nothing new is
      persisted, so a queue that survived a restart counts as built by hand.
    - Only then does the capture run, so a play over a queue nobody built holds nothing.
13. **Undo is configurable, in a new Accessibility category** (Kenan, 2026-10-29).
    - **A master switch, on by default.** That is a deliberate exception to new visible behaviour
      defaulting off.
    - **One switch per kind of removal**, all on: Playlists (tracks removed, playlists deleted),
      Queue (rows, Clear, a replace), Favorites, Radio stations, Equalizer, Recent searches.
    - **One "Also for a single item" switch** for every kind that counts items, on by default.
      With it off, removing one track, row, playlist, station or favorite raises nothing, so small
      adjustments go quiet while larger removals keep their undo. A removal of a whole state (Clear
      all, Reset, Clear, a replace) isn't counted. One shared switch rather than a pair per kind:
      "one or several" is the same question everywhere.
    - **The notice's duration, 4 to 10 s in whole seconds, 6 s by default** (decision 7). That is
      WCAG 2.2.1's "adjust" technique, and Windows (Accessibility → "Dismiss notifications after
      this amount of time") and Android (Accessibility → "Time to take action") put the same
      control there.
    - **A removal undo doesn't cover behaves exactly as it does today** (decision 5). It captures
      nothing and raises no notice, and its confirmation comes back where it had one: deleting
      playlists, removing stations, Clear queue.
    - A change applies from the next removal. A notice already up stays.

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
| Delete playlist | `Dialog.open-delete-playlist`, raised from Rust (`crud.rs`' `confirm_first`) | Gone under the cap (Phase 4). Kept over it, saying it can't be undone because the playlist has too many tracks; with undo off from Phase 10, in its own wording. |
| Delete N playlists | `Dialog.open-delete-playlists`, raised from Rust (`crud.rs`' `confirm_first`) | Gone under the cap (Phase 4). Kept over it, naming the same reason, and a true plural; with undo off from Phase 10, in its own wording. |
| Clear queue | inline, `queue-panel.slint:240-249` | Hidden while queue undo is on. The queue's holds are uncapped (decision 5), so only Settings brings it back (decision 13). "Playback will stop" was never true and is reworded. |
| Remove station (Favorites / Recently Played) | `Dialog.open-remove-station`, from the card trash and the menu | Deleted (Phase 5): one station is never past the cap. Phase 10 brings back the one-station form undo off needs, the Favorites heart's included. |
| Remove N stations | `Dialog.open-remove-stations`, raised from Rust (`radio/callbacks/removal.rs`' `confirm_first`) | Gone under the cap (Phase 5). Kept over it, saying there are too many stations to undo; with undo off from Phase 10, in its own wording. |
| Remove watched folder | inline, `library-section.slint:89-102` | **Stays**: out of scope. |
| Restart (titlebar, tray, backdrop), exclusive output | `window-chrome-section.slint`, `appearance-section.slint`, `output-section.slint` | Untouched: not removals. |

A kept confirmation is raised from Rust once the size is known, across an await, so it takes a
`DialogClaim` (`ui-patterns.md`, held by `dialog_openers.rs`).

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-views/src/ui/undo/mod.rs` (new) | `UndoWindow`: the one slot, `offer` (merge, or replace and dismiss the old row), `notify_over_cap` (replaces the slot with a button-less notice), the Undo/Ctrl+Z handler acting only on the slot's own row id and running the restore under `spawn_local`, the one "Couldn't undo" failure branch, the slot's release when its row goes, `UNDO_NOTICE_MS`, and the app state and window it hands each restore, so a hold carries only what it took |
| `crates/melodia-views/src/ui/undo/removal.rs` (new) | `Removal`, one variant per surface wrapping that surface's token: `absorb` (merge or hand back), its wording recipe, the title builders an over-cap notice shares, and the async `restore`, which dispatches each arm to the surface module that raised it and answers a failed write. No surface names another's. |
| `crates/melodia-views/src/ui/shell/notifications.rs` | `recipes` keeps its shape. Adds a timed localized push, an in-place `relabel`, `restart` (a bump of the row's `restarts`), one row-removed listener, and the `Undo` arm in `run_action`, which now takes the row id. `show_auto_dismiss` writes the row's `timeout-ms` instead of arming a Rust timer. `remove_at` stays the one removal path. |
| `crates/melodia-ui/ui/globals/undo-text.slint` (new) | `UndoText`: every undo wording as `pure callback`s with plural `@tr`, spelled as `Settings.playlist-import-title` spells them (`settings.slint:434-437`), keeping `settings.slint` (625 lines) from growing. Exported from `app-window.slint`, Rust calling it. |
| `crates/melodia-ui/ui/models.slint` | `NotificationKind.undo`; `NotificationRow` gains `timeout-ms` (0 for a sticky row) and `restarts` |
| `crates/melodia-ui/ui/globals/notifications.slint` | `undo-row` (the slot's row id, `-1` with none) and `undo(int)` |
| `crates/melodia-ui/ui/components/dialog/notification-stack.slint` | Per card: a hover sentinel wrapping the layout, the `Timer` that is its clock, and the polite live region |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint` | The Ctrl+Z arm, ahead of the modal gate |
| `crates/melodia-ui/ui/components/track-list/track-context-menu.slint` | The remove-from-Favorites entry calls one shared callback rather than the list's `toggle-row-favorite` (decision 11) |
| `crates/melodia/src/boot/ui_setup/` | `notifications::install` and `UndoWindow` move ahead of `install_views` (`install.rs:68`) |
| `crates/melodia-core/src/entities/undo.rs` (new) | `UndoCapture<T>` (`Nothing`, `Held(T)` or `OverCap { rows }`) and `UNDO_HOLD_CAP`, named by store, app and views |
| `crates/melodia-core/src/entities/{playlist,radio}.rs` | The plain tokens beside their entity: `PlaylistItemsCut`, `PlaylistSnapshot` (a `Playlist` and its `CutItem`s), `KeptList` and `StationCut`, plus `RadioStation::is_listed`, which the store's removal reads |
| `crates/melodia-core/src/utils/reinsert.rs` (new) | `reinsert`: held items back at their indices in one linear walk, the playlist restore's and the queue's `restore_cut`'s |
| `crates/melodia-store/src/database/queries/playlist.rs` | Capture inside the removal transactions. `insert_items_tx` takes explicit `(track_id, position, added_at)` rows, its two callers building contiguous ones. `update_playlist_thumbnail_and_timestamp_tx` takes its stamp from the caller, which minted it inside (`:515`). |
| `crates/melodia-store/src/database/queries/playlist_restore.rs` (new) | `restore_playlist_items`, `restore_playlists`, keeping `playlist.rs` (550 lines) under the 800-line cap |
| `crates/melodia-store/src/database/queries/playlist_delete.rs` (new) | Both deletes, each one transaction: `delete_playlists` (for good, answering the names) and `delete_playlists_held` (answering `PlaylistSnapshot`s), plus `item_count` and `custom_covers` |
| `crates/melodia-store/src/database/queries/radio_removal.rs` (new) | `remove_stations` and `restore_stations`, each one transaction, `delete_if_unlisted` (Browse's un-heart) and `station_artwork`; `radio.rs` loses `delete_station` and `clear_play_history` |
| `crates/melodia-store/src/database/queries/track/write.rs` | `unfavorite`, returning the ids it flipped |
| `crates/melodia-app/src/tasks/artwork_sweep.rs` | `ArtworkHold` (refcounted per basename, released on drop) beside its one reader, unioned into `sweep_stores`' reference set |
| `crates/melodia-app/src/library/{playlists,radio/removal,queue,search,favorites}.rs` | Each removal returns its capture, and each surface gets one restore fn |
| `crates/melodia-views/src/ui/radio/callbacks/removal.rs` (new) | `Remover` (the heart, the trash and the menu, and the confirmation past the cap) and `StationRemovalUndo` |
| `crates/melodia-app/src/services/search_history.rs` | `clear` hands back the list; `restore` puts it back |
| `crates/melodia-engine/src/player/engine/queue.rs` (534 lines) | `cut`/`restore_cut` replacing `remove_at` and `remove_batch` (`prune_missing` built on `cut`), `take_cleared`/`restore_cleared`, the `playing_left` and `hand_edited` flags, and one `seat` method. Split into `queue/` if it passes 800. |
| `crates/melodia-views/src/ui/equalizer.rs` | `apply_curve`, shared by preset select, reset and restore |
| `crates/melodia-app/src/services/settings/undo.rs` (new) | `UndoFlags`, flattened into `SettingsData`; `library::settings::undo` holds its setters |
| `crates/melodia-views/src/ui/settings/undo_settings.rs` (new) | The Accessibility page's undo section: seeds it, and each callback sets the slot's shadow, then persists |
| `crates/melodia-ui/ui/views/settings/{pages/accessibility-page,undo-section}.slint` (new) | The category's page and its first section |

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
`install_equalizer` (`:189`) all run before it. The view slices reach the slot through
`ViewCtx.undo`, a shared service like the row tier rather than a peer, so their `install`
signatures don't change. `install_equalizer` and `queue_panel::install`, outside `install_views`,
take `&Rc<UndoWindow>`.

## Phases

Each phase stops at the static gates (`cargo fmt --all`, `cargo clippy --all-targets --locked
--workspace -- -D warnings`, `cargo test --locked --workspace`) and Kenan's hand test. New tests
wait for Phase 11.

### Phase 1: the notice, with the equalizer's Reset as its first user (done 2026-10-29)

The shell and its simplest user land together, the shell having nothing to show by hand on its own.

1. **Slint.**
   - `NotificationKind.undo`, and `NotificationRow`'s `timeout-ms` and `restarts`, in
     `models.slint`.
   - `Notifications.undo-row`, the slot's row id or `-1`, and `undo(int)`, Ctrl+Z's way in with
     `undo-row`. The card's Undo arrives through `action` and reaches the same handler.
   - **The card's clock**: a `Timer` with
     `running: data.timeout-ms > 0 && !hovered && Theme.window-focused`, whose `triggered` calls
     the existing `Notifications.dismiss(data.id)`, and a `changed` mirror on `data.restarts`
     calling `restart()` only while the clock runs (decision 7). A resume restarts the full
     interval.
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
   - Undo takes the slot, dismisses the row and runs the restore. The equalizer's can't fail, so
     the failure branch lands with Phase 3's.
4. **The equalizer.**
   - `apply_curve` is the one write of gains, preamp and preset, both the engine's and persisted.
     Reset makes two settings writes today, gains with the preset name and the preamp on its own
     (`library/settings/equalizer.rs:15-35`); a preset pick leaves the preamp alone.
   - Reset captures the curve first: the gains from the `VecModel`, the preamp from
     `Equalizer.preamp`, the preset from `preset-idx`. It skips the offer when the curve was
     already flat at 0 dB.
   - Restore maps the preset name back to an index the way `seed` does, falling to the Custom slot
     (`equalizer.rs:33-45`).
5. **Strings:** "Undo", "Equalizer reset".

**By hand.**
- Reset raises the notice, and hovering holds it, over Undo and ✕ too.
- Undo and Ctrl+Z restore both the card and the dialog, with the dialog still open.
- Reset, nudge a band, Reset again: one card, its clock restarted, and Undo returns the curve from
  before the first Reset. Reset on a flat curve raises nothing.
- Ctrl+Z after the notice expires, or inside a text field, does nothing to the curve.
- A language switch with the notice up re-renders it.
- A Copy toast pauses under the pointer too, and a notice outlasts a switch to another window.

### Phase 2: recent searches' Clear all (done 2026-10-29)

- `SearchHistoryState::clear` hands back the old list (`std::mem::take`). `add`, `remove` and
  `restore` share one `change`, which edits under the lock and then flushes. The write takes its
  snapshot under a writer lock, so the last write to run saves the newest list.
- `restore` puts the list back behind any query added since, deduped and truncated to
  `MAX_ENTRIES` (`restore_queries`, beside `add_query`, since `MAX_ENTRIES` is private).
- The facade is `library::search::{clear,restore}_search_history`. `on_recent_clear` runs on the
  UI thread through `spawn_local`, because the slot is an `Rc`. It offers `Removal::RecentSearches`
  only when something was cleared, and the restore ends in `push_recent_rows_to_slint`. Phase 3's
  offers take the same shape.
- A run of Clear alls merges with the newer list in front, since a second clear could only take
  what was searched after the first. Keeping the older hold, as the equalizer does, would lose
  those queries.
- The field keeps focus after Clear all, so Ctrl+Z edits the field here and the button is the way
  back (decision 8).
- **Not in scope:** a single chip's ✕, which the issue doesn't list. Searching again recreates a
  chip.
- **String:** "Recent searches cleared".

**By hand.**
- Clear all raises the notice, and Undo brings the chips back in their order.
- Clear all, search something new and let it join (about 2 s), then Undo: the new query comes
  first, then the old ones, ten at most.
- Clear all, search, empty the box, Clear all again inside the window: one card with its clock
  restarted, and Undo brings back both sets.
- With the field focused, Ctrl+Z edits the field. With focus elsewhere, it restores.
- An undone clear is still there after a restart.
- A language switch with the notice up re-renders it.

### Phase 3: tracks removed from a playlist (done 2026-10-29, its measure pending)

1. **Core.**
   - `UndoCapture<T>` and `UNDO_HOLD_CAP`. A third arm, `Nothing`, puts decision 6 in the type: a
     removal that removed nothing writes nothing (no renumber, no stamp) and raises nothing.
   - `PlaylistItemsCut { playlist_id, rows: Vec<CutItem { track_id, row, added_at }>, updated_before, stamped_at }`.
     No name: the views read it off the detail header at the click, the over-cap answer carrying
     only a count.
   - `row` is the index in `ORDER BY position, id`, not the raw position, positions carrying gaps
     under a hard-deleted track.
   - `utils::reinsert`, the merge as one linear walk. An insert per row is quadratic on a
     library-sized playlist. Phase 7's `restore_cut` reuses it.
2. **Store.**
   - `remove_tracks_from_playlist_batch` checks the cap on the ids asked for, then reads the cut
     inside its transaction before the delete (a chunked `IN` over a `ROW_NUMBER()` subquery).
     Over the cap, the delete runs as today and answers `OverCap { rows }`.
   - `update_playlist_thumbnail_and_timestamp_tx` takes its stamp from the caller, so the removal
     records the one it wrote. `insert_items_tx` takes explicit `NewItem` rows, its two old
     callers building a contiguous run.
   - `queries::playlist_restore::restore_playlist_items(cuts)` is one transaction:
     - a gone playlist fails it, the rows having nowhere to go;
     - every cut merges newest first into the survivors, each in the coordinates of the moment it
       was made;
     - **only then** are the rows dropped whose track is gone or is back in the playlist (the user
       re-added it). A dropped row holds its slot through the merge, or every later row of its cut
       lands one early. The track filter is mandatory: tracks are hard-deleted, and
       `INSERT OR IGNORE` doesn't cover a foreign-key failure;
     - the survivors that moved are rewritten, and the restored rows go in with their own
       positions and `added_at`;
     - the stamp steps back through the cuts while `updated_at` still reads as each one left it,
       written through the cover refresh. The grid orders by `updated_at DESC`, so that puts the
       card back in its place; a playlist touched since is stamped now.
   - A track sits in a playlist once (`idx_playlist_items_unique` is on
     `(playlist_id, track_id)`), so the track id names its row.
3. **Views.**
   - `wire_remove_track` runs on the UI thread through `spawn_local`, as Clear all does. It keeps
     `remove_rows` and the runtime refresh, then offers `Removal::PlaylistTracks`, or calls
     `notify_over_cap`.
   - **Over the cap goes through the slot too.** `notify_over_cap` replaces the held notice with a
     button-less one, so the card up is always the latest removal and Ctrl+Z never reaches past it.
   - `PlaylistCutUndo` merges only on the same playlist. Its restore ends in `refresh_after_edit`,
     which reloads the grid, and the detail only while it still shows that playlist.
   - `callbacks::wire` takes `ViewCtx` whole, as Search's does.
4. **Measure.** One heaptrack of a held cap-sized cut confirms `UNDO_HOLD_CAP`, and one of a moved
   full-library queue confirms the queue's exemption (decision 5). Both are taken by attaching to
   Kenan's running build after the hand test, with his go between them, and recorded here.
5. **The first restore that reports its failure.** `Removal::restore` is async and returns
   `Result`, and `UndoWindow::undo` owns the one failure branch: `show_failure`, "Couldn't undo"
   over "See the log for details.". Phase 2's search restore is awaited there too, so a failed
   history write raises the same notice. Only the equalizer's restore can't fail.
6. **Strings** (Kenan's read, 2026-10-29): "{n} track removed from {}" (plural), "Too many to
   undo", "Couldn't undo", in all 15 catalogs.

**By hand.**
- Remove one row, then a selection: the notice names the count and the playlist. Undo and Ctrl+Z
  (the filter box unfocused) put the rows back in place with their dates added, and the card goes
  back to its grid slot.
- Remove twice inside the window: one card, the count adds, the clock restarts, and Undo restores
  both.
- Remove from playlist A, then from B: B's notice replaces A's, and A's removal stands.
- Remove, re-add one removed track, Undo: it stays where it was re-added and the rest return.
  Remove, drag a row, Undo: the rows land at their old indices.
- Remove the track whose cover is the playlist's automatic cover, then Undo: the cover returns.
- Remove, leave the page, Undo from elsewhere: reopening shows the rows.
- With the filter box focused, Ctrl+Z edits the field.
- Remove, then delete that playlist: the delete's notice replaces the removal's. "Couldn't undo"
  is left to Phase 11's store test of a restore into a deleted playlist, no hand path reaching it
  once a delete holds the slot.
- More than 10,000 rows: a notice with no button saying it's too many to undo.
- A language switch with the notice up re-renders it.

### Phase 4: deleting playlists (done 2026-10-30)

1. **Core.** `PlaylistSnapshot { playlist: Playlist, items: Vec<CutItem> }`, `row` being each
   item's index. `search_name` isn't held: the restore packs it from the name, as create does, a
   card's name and its `search_name` being written together.
2. **Store** (`queries/playlist_delete.rs`, keeping `playlist.rs` well under the cap).
   - `delete_playlists` is one transaction now, where it was chunked with none, and answers the
     names it deleted (`RETURNING name`), so the confirmed path knows what went.
   - `delete_playlists_held` reads the snapshots, then deletes, in one transaction, and returns
     before any write when no id matched.
   - `item_count` is what the cap is checked on, read at the click.
   - `restore_playlists` (`playlist_restore.rs`), one transaction: each row under its own id, or a
     fresh one where a playlist created since took it (decision 9). The stats go in at 0, the
     per-item triggers adding back only the items whose track is still there. The cover refresh
     writes the held `updated_at` last, so the card returns to its grid slot.
3. **App.**
   - `ArtworkHold` lives in `tasks::artwork_sweep`, its one reader, refcounted per basename since
     two playlists can share a content-addressed composite.
   - `library::playlists::delete_playlists_held` takes it off `custom_covers` **before** the delete
     commits: a sweep reads the columns and then the holds, so one of the two always names the
     file.
   - `DeletedPlaylists` carries the snapshots and the hold, and its restore drops the hold only
     once the rows are back.
4. **Slint and views.**
   - The trash, the card menu and the detail pill all call `Playlists.delete-playlists([int])`,
     and the single-id callback is gone.
   - In `crud.rs`, a `Deleter` takes a `DialogClaim` at the click and counts the items:
     - past the cap, `confirm_first` raises the reworded opener by count, and its accept,
       `delete-playlists-confirmed`, deletes for good and raises the button-less notice;
     - otherwise it closes the detail, does the held delete, and offers.
   - The count can't come after the write: the confirmation must precede it, and the detail closes
     before it only once the delete is decided.
   - **Deletes merge**, the target being the playlist collection, so deleting B doesn't make A's
     delete permanent. `Removal::absorb` sums the run's items against the cap.
   - Both kinds stay in `dialog_openers.rs`' `FOLDED_KINDS`, now as deferred raises. The batch kind
     gains the delete icon the single one had.
   - **The card's hover trash hides while two or more cards are picked** (`CardCell.multi-selection`),
     as does a station card's remove. Beside a selection a one-card delete reads as the set's, and
     with no confirmation one stray click took the wrong scope. The right-click menu deletes the
     set and names its count first. Rename and Edit artwork stay, meaning one card either way.
   - **No confirmation under the cap** (Kenan, 2026-10-29). The notice comes up as the user acts,
     so 6 s is enough. Phase 10's slider lengthens it, and turning a kind's undo off brings its
     confirmation back.
5. **Strings:** "{} deleted", "{n} playlist deleted" (plural), "Delete {n} playlist?" (plural,
   replacing "Delete {} playlists?"), and both messages saying why the delete can't be undone:
   too many tracks (Kenan, 2026-10-30, so the user learns it before deciding rather than from the
   notice after). No number, the cap being Rust's and free to move.

**By hand.**
- Delete from the card trash, the card menu and the detail pill. Each deletes at once with "Road
  Trip deleted". Undo and Ctrl+Z bring it back in its grid slot with its tracks in order, dates
  added, description and cover, and the detail stays closed.
- Select three and delete them: "3 playlists deleted", and Undo restores all three.
- With two or more cards picked, no card shows its trash, and a Radio card on a kept tab shows no
  remove. With one picked, both are back.
- Delete A, then B inside the window: one card, "2 playlists deleted", its clock restarted, and
  Undo brings back both.
- Delete a smart playlist, then Undo: its rules come back.
- Delete the newest playlist, create one inside the window, then Undo: both exist.
- Delete a playlist with a custom cover more than an hour old, rescan a folder while hovering the
  notice, then Undo: the cover is still there.
- Remove tracks from a playlist, then delete it: the delete's notice replaces the removal's.
- Over 10,000 tracks: the reworded confirmation. Cancel keeps everything, the detail included;
  Delete gives a notice with no button.
- A language switch with the notice up re-renders it.

### Phase 5: radio stations (done 2026-10-30)

1. **Core.** `KeptList { Favorites, Recent }`, which decides what a removal takes, and
   `StationCut`: `Unfavorited { id, name }`, `HistoryCleared { id, name, last_played,
   play_count }`, or `Deleted(Box<RadioStation>)`, the row as it stood. `is_listed` moved onto
   `RadioStation`, the store's transaction reading it.
2. **Store** (`queries/radio_removal.rs`).
   - `remove_stations(ids, from)` is one transaction: it reads the rows, takes the heart or the
     plays, and deletes a row `is_listed` no longer holds, all as chunked statements. A station
     the tab doesn't list yields no cut (decision 6).
   - `restore_stations` undoes the cuts newest first, reversing only what each wrote (decision 4):
     - the heart goes back on;
     - the plays are added back, `play_count + held` and the later `last_played`, a NULL losing,
       so a play in the window survives;
     - a deleted row goes back whole under its id, or a fresh one where another took it, unless
       `kept_station_matching` finds it back already (by uuid, then `stream_url`);
     - a row gone since stays gone.
   - Browse's un-heart keeps `delete_if_unlisted`, now read and delete in one transaction.
3. **App** (`library/radio/removal.rs`). `remove_stations` holds every station's logo in an
   `ArtworkHold` before the transaction, which alone knows which rows go (decision 10). Past
   `UNDO_HOLD_CAP` stations it holds nothing and answers `OverCap`.
4. **Views** (`radio/callbacks/removal.rs`).
   - `Remover` takes the heart on Favorites, the trash and the menu, all through
     `Radio.remove-stations(list, ids)`. Under the cap it removes and offers; past it,
     `confirm_first` takes a `DialogClaim` and raises `open-remove-stations` a tick later on a
     model of its own. The accept is `remove-stations-confirmed`.
   - `StationRemovalUndo` merges on the same tab, and its restore ends in `kept::refresh`. A
     station page the removal closed stays closed.
   - Recently Played's heart still un-hearts through `remove_stations`, dropping the capture.
5. **Cards** (decision 11). On Favorites a directory card has no trash and its heart hides beside
   a multi-selection, as the trash does; a hand-typed card keeps its trash. Edit takes the
   trash's slot wherever no trash shows.
6. **Confirmations.** The one-station opener is deleted. The plural one is raised from Rust past
   the cap only, saying there are too many stations to undo. Both kinds joined `FOLDED_KINDS`,
   written through an `if` the walk can match.
7. **Strings:** "{} removed from Favorites", "{} removed from Recently Played", "{n} station
   removed from Favorites", "{n} station removed from Recently Played" (plurals), the menu's
   remove labels without "…", and the two over-cap messages.

The radio facade holds: only `library::radio` and `library::radio_files` name `queries::radio`
(`radio_facade.rs`), `queries::radio_removal` included by prefix.

**By hand.**
- Favorites, a directory station: no trash, and Edit sits bottom left. The heart removes it with
  "Jazz FM removed from Favorites", and Undo or Ctrl+Z brings it back with its logo and play
  count. An unplayed one comes back under its own id, and the station page it closed stays closed.
- Favorites, a hand-typed station: the trash removes it with its history at once, and Undo
  restores both.
- Recently Played: the trash clears the plays and Undo restores them. Play it inside the window,
  then Undo: the count adds and the newer stamp stands.
- The heart on Recently Played or Browse raises nothing.
- Two removals from one tab inside the window: one card, the count added, its clock restarted, and
  Undo restores both. A removal from the other tab replaces the card.
- Pick three on Favorites: no card shows its heart or trash, the menu's "Remove 3 from Favorites"
  raises "3 stations removed from Favorites", and Undo brings all three back.
- Remove a station with a logo, leave the Radio page (the logo prune) while hovering the notice,
  then Undo: the logo is still there.
- Remove an unplayed station, heart it again from Browse, then Undo: nothing is duplicated.
- A language switch with the notice up re-renders it.

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
     `play_order` and `original_order` through `melodia_core::utils::reinsert`. Fresh entries keep `QueueEntry`'s "never handed out again"
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
   - The Clear button calls `Queue.clear-queue()` directly. Rust clears and offers.
   - The inline confirmation stays, shown only while `Queue.clear-confirms` is up. Rust holds it
     down until Phase 10 writes it from the undo preferences (decision 13). Its "Playback will
     stop" line is reworded, since the playing track runs to its end.
   - The removal callbacks in `queue_panel/callbacks.rs` resolve their rows to entries at the click
     (`QueueRows::entry_at`, `paged_rows/queue_rows.rs:119`) and offer. The ✕, the menu, Delete
     and Up Next's menu all arrive there.
4. **Strings:** "{n} track removed from the queue" (plural), "Queue cleared", and the clear
   confirmation's reworded line.

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

### Phase 10: undo in Settings, in a new Accessibility category

**What you'll see.** Settings gains an Accessibility category, whose first section is Undo:

```
Undo
  [x] Offer undo after a removal
      [x] Also for a single item
      [x] Playlists
      [x] Queue
      [x] Favorites
      [x] Radio stations
      [x] Equalizer
      [x] Recent searches
  Keep the notice up   ────●────────  6 s
```

Turning a kind off makes its removals behave as they did before this plan. Deleting playlists,
removing stations and Clear queue ask first again (decision 13).

1. **Preferences.**
   - `UndoFlags`, flattened into `SettingsData` under `#[serde(default)]`, so an older
     `settings.json` opens with undo on at 6 s. Its fields:
     - `undo_enabled` and `undo_single_items`;
     - `undo_kinds_off`, the keys of the kinds turned off. A list of keys rather than a bool per
       kind: a key never repoints, an unknown one is ignored, a kind added later starts on, and the
       struct stays under clippy's `struct_excessive_bools`;
     - `undo_notice_ms`, clamped to 4,000..=10,000 on read.
   - Setters in `library::settings::undo` over `mutate_settings`. They kick nothing, since only the
     slot reads these, through its shadow.
2. **The slot.**
   - `UndoKind` (Playlists, Queue, Favorites, Radio, Equalizer, RecentSearches) with its persisted
     `key()`. Each `Removal` arm names its kind.
   - `UndoWindow` holds an `UndoPrefs` shadow, seeded at install from the settings and set by the
     Settings callbacks before their write is spawned (the shared-shadow rule).
   - `covers(kind, count)`, with the count `Items(n)` or `Whole`: undo on, the kind on, and
     `Items(1)` only with single items on. `UNDO_NOTICE_MS` becomes the shadow's default, and
     `offer` and `notify_over_cap` read the shadow's duration.
3. **Every removal asks first, before it captures**, so an uncovered one holds nothing:
   - Equalizer Reset, Clear all, queue rows, a replace and favorites: uncovered is the plain write
     and no notice.
   - Playlist tracks: the store fn splits into a plain form and a held one, the held form's
     over-cap branch already being the plain write.
   - Deleting playlists and removing stations: uncovered takes the over-cap path, its confirmation
     raised from Rust with a `DialogClaim` (Phases 4 and 5 build it).
   - **The playlist delete confirmations need a second wording for undo off.** Phase 4's messages
     give the over-cap reason, "too many tracks to undo", which is wrong when the user turned undo
     off. The undo-off form says only that the delete can't be undone, so the openers take which
     reason applies. Phase 5's station confirmation owes the same, naming the cap too.
   - **Stations need a one-station confirmation back.** Phase 5 deleted it, one station never
     being past the cap. With radio undo off, `Remover::request` asks first for every removal,
     the Favorites heart's included (Kenan, 2026-10-30), and one card's removal names the station
     and says what a hand-typed one loses.
   - Clear queue: Rust writes `Queue.clear-confirms` from the shadow, which shows Phase 7's kept
     inline confirmation.
4. **The category** (`ui-patterns.md`'s "A new category" list):
   - `pages/accessibility-page.slint`, a `category-accessibility` constant, a router branch and a
     search mount, both inline arrays in `settings-nav.slint`, `SettingsCategory::Accessibility` in
     `ALL`, `category-count` at 13, and `category-name:` on its section.
   - The icon (`accessibility_new`) owes `scripts/icons.txt`, a re-subset by glyph id and a clean
     `scripts/check-icons.py`.
   - **Placed above About**, near the end, where GNOME and Windows put theirs. Inserting it
     renumbers every later index, so **the persisted category moves to a string key** in
     `views.json` (`settings_category_key`). The old int is left unread, as `settings_tab` was
     before it. The first launch after the update opens Settings on Library, and no later
     category, #36's included, renumbers anything.
   - `animations-section` stays under Appearance. Whether it moves is #36's call.
5. **The section** (`undo-section.slint`):
   - A `SectionCard` holding the master `SettingRow` toggle and its sub-rows: single items, then
     the six kinds, disabled while the master is off.
   - The duration is a `SettingSlider` snapping to whole seconds, its readout reusing crossfade's
     `@tr("{} s", …)`. It is the Settings page's numeric control already; `FlyoutStepper` is built
     around a flyout row and hold-to-repeat, which seven values don't need.
   - Each row's label and description are declared once, read by the row and by `row-visible`, and
     each sub-row owes its pair.
6. **Views.** `ui/settings/undo_settings.rs` seeds the section from the settings, and each callback
   sets the shadow, then persists.
7. **Strings:**
   - "Accessibility" and "Undo";
   - "Offer undo after a removal" and "Also for a single item", each with a description;
   - the six kind names;
   - "Keep the notice up";
   - the reworded confirmations, if Phases 4, 5 and 7 haven't already settled them;
   - the undo-off wording of the two playlist delete confirmations and of the station
     confirmation, one message each, and the one-station confirmation's strings.

**By hand.**
- Undo off: no notice anywhere and Ctrl+Z does nothing. Deleting a playlist, removing a station and
  Clear queue ask first.
- Single items off: removing one playlist track raises nothing, and two raise the notice. Clear all
  and Reset still raise theirs.
- One kind off: only that kind goes quiet.
- The slider at 4 s and at 10 s: the notice lasts that long, and hover still holds it.
- A restart keeps every choice. An older `settings.json` opens with undo on at 6 s.
- Searching Settings for "undo" finds the section from any category.
- The category survives a restart, and the first launch after the update opens Settings on Library.

### Phase 11: tests, then docs (each on Kenan's go)

**Tests:**
- `queue_tests`:
  - a `cut`/`restore_cut` round trip with the current row before, inside and after the cut, under
    shuffle (both orders restored);
  - `restore_cleared` keeping the rows queued since, and both restores minting fresh entries;
  - the cursor rule: a restore after a seat keeps the live current row;
  - `playing_left` through advance, gapless peek, Next, Previous, Play Next, shuffle and persist
    (position 0), at the tail too; a cut while `Loading` keeps today's start;
  - `hand_edited` set by each hand edit, cleared by a replace, seated by boot restore.
- `reinsert_tests` (core): ascending indices, an index past the end, an empty side, and equality
  with inserting one by one.
- `playlist_tests`:
  - a remove and restore round trip with a gap present, a track re-added in the window, a
    hard-deleted track ahead of a kept row in the same cut (merge first, then drop), and a merged
    run of two cuts;
  - a removal of ids the playlist doesn't hold answering `Nothing` and leaving `updated_at` alone,
    and one over the cap answering `OverCap`;
  - a restore into a deleted playlist failing;
  - the stamp put back only when untouched, and an edit between two cuts keeping its own;
  - delete and restore of a regular and a smart playlist, its `track_count` and
    `total_duration_ms` matching its items, plus the id-taken fallback.
- `radio_tests`: every `StationCut` arm (`Unfavorited`, `HistoryCleared`, `Deleted`), a station
  the tab doesn't list answering no cut, a history restore after a play in the window keeping the
  play and the later stamp, a deleted row back under its own id and the id-taken fallback, and
  both skips (the uuid, and a hand-typed `stream_url`).
- `search_history` tests: restore with queries added since, and a merged run of two clears
  restoring the newer list in front.
- `notifications_tests`: `relabel` keeping the id, `restart` bumping `restarts`, a timed row
  carrying its `timeout-ms`, a replace dismissing the old Undo row, an Undo naming another row
  doing nothing, and the row-removed listener firing once on every removal path, the eviction
  inside `show` included (the `:117-154` pattern).
- **Slint pins:** the card's `Timer` gates on its hover and `Theme.window-focused`, the row menu's
  remove entry calls the shared callback rather than `toggle-row-favorite`, the playlist card's
  trash and the station card's remove hide on `CardCell.multi-selection`, and on Favorites a
  station card's heart hides there too while its trash shows only on a hand-typed card.
- **Sweep:** a held basename survives `sweep_stores`, and so does one whose hold drops while the
  column read is in flight.
- **Settings:**
  - `UndoFlags` defaulting from an older `settings.json`, the duration clamped at both ends, and an
    unknown kind key ignored;
  - `covers` over every kind, both counts and both switches;
  - the category's key round-tripping through `views.json`.
- **Existing tests to recheck:** `settings_page_tests` (the category count and arrays),
  `dialog_openers.rs` and `translations.rs`; the `remove_at` and
  `remove_batch` tests in `queue_tests`, `queue_entry_tests`, `state_tests` and the views crate's
  `paged_rows/tests/queue_rows_tests.rs:183,216`; and the timer-observing `show_completion` test.

**Docs:**
- `ui-patterns.md`'s "Dialogs, toasts": the slot contract (decisions 2, 5, 6, 8 and 12), and the
  card owning its clock.
- `ui-patterns.md`'s "Selection": a card's hover control that deletes it hides under a
  multi-selection (`CardCell.multi-selection`).
- `library-data.md`: restores as units of work, and the artwork hold.
- `audio-stack.md`: `playing_left`.
- `ui-patterns.md`'s "The Settings page": the category persisted by key, and Accessibility as #36's
  home.
- Offer the `CLAUDE.md` and `README.md` edits.

## Cross-cutting

- **Translations.** Every phase with strings runs `scripts/update-translations.py`, then fills all
  15 catalogs, plurals included, with no empty `msgstr` and no `#, fuzzy` (`translations.rs`). A
  translator note is one `// Translators:` line where a `{}` is a name.
- **Memory.** Idle cost is nil: with no notice up the slot is empty, and the toast clock is a
  `Timer` per card that goes with its row. A hold of records is bounded by the cap and dropped with
  its notice. A queue hold is ids, a cleared or replaced queue moved rather than copied, so it
  holds what the queue held a moment before; a play over a queue nobody built holds nothing.
- **Persisted state.** No migration, and no hold survives a restart. Only Phase 10 writes anything
  new: the undo preferences in `settings.json` under `#[serde(default)]`, and the Settings category
  as a string key in `views.json`.
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
