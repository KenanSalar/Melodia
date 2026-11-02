# Plan: Duplicate tracks (find the copies, clear out the spares)

Status: **in progress** (Phase 0 done, Phase 1 next) · Owner: Kenan · Created: 2026-11-02 · Issue #158, branch `feat/duplicate-tracks`

> **Validated 2026-11-02** against `b4c5ecff`, and re-validated the same day against `41c1f2e1`. The sources were:
> - three read-only code traces: the store and per-track data; the removal, scan, watcher and queue paths; the UI surfaces;
> - a scratch copy of the 51,012-track dev database;
> - the `trash` 5.2.9 manifest and its freedesktop and Windows sources, which argued for leaving the trash out;
> - a read of how two long-shipping desktop players handle duplicates, deletion, the trash and the history a row carries, which settled the disc-and-track check, the earliest added date and the "Considered and not taken" list;
> - the re-validation, which was three more traces (scan, watcher, import and folders; the merge, queue and engine; the dialog and every citation) and a survey of a dozen players and duplicate tools:
>   - it confirmed the matching, the kinds and the keeper order;
>   - it added exclusions that follow a moved file, the year check, Not Duplicates and Keep Copies in This Folder;
> - Kenan's answers of 2026-11-02:
>   - removal takes a copy out of the library and leaves its file alone, with no trash;
>   - the shared Dialog is the surface;
>   - Not Duplicates and Keep Copies in This Folder are in.

Working doc. Delete it when its last phase (Tests) lands.

## Context

**What you see today.** A library that grew over years holds some songs more than once: an album ripped as both FLAC and MP3, a single beside the album it came from, a folder copied in twice. Every copy shows up in Songs, in search and in shuffle. Nothing tells you how many there are, and nothing in the app clears them out. Each copy keeps its own plays, rating, heart and playlist places, so a song's history is split across copies you can't see side by side.

**Why.** Nothing groups copies. The one duplicate query, `queries::track::get_duplicate_tracks` (`full_rows.rs:83-113`), only matches byte-identical files, and only two tests call it.

**Outcome.** Settings ▸ Library ▸ Duplicates ▸ **Find…** opens a dialog listing groups of tracks that look like the same song. Each copy shows its format and quality, its folder, when it was added, and what it carries (plays, stars, heart, playlists). Each group is labelled with how sure the match is.

The suggested picks keep the best copy. Every pick can be changed, a group can never have every copy picked, and the same song on different releases is never picked for you. Right-clicking a copy offers four entries, and the dialog stays open with your picks:
- **Keep This Copy** makes it the one that stays, in one step.
- **Keep Copies in This Folder** does the same in every group with a copy in that folder, so one choice keeps a whole album folder.
- **Open Containing Folder** lets you look at the files before choosing.
- **Copy Path** puts its file path on the clipboard.

Right-clicking a group's header offers **Not Duplicates**, for copies you keep on purpose. The group leaves the list and stays hidden on later finds until a new copy joins it. **Show Dismissed** lists those groups again.

**Continue** shows what will happen. **Remove** then takes the spares out of the library and moves their history onto the copy that stays. Their files stay exactly where they are, and later scans skip them.

**Files kept out of the library**, a second row in the same card, lists every removed copy. Each one shows its path with a **Bring Back** button, which returns it to the library. Right-clicking one offers **Open Containing Folder** (for deleting the file in your file manager), **Copy Path** and Bring Back. **Copy All Paths** puts every removed file's path on the clipboard, one per line.

**What keeps working** (the issue's three, held below):
- Nothing in the library changes until you press Remove. Not Duplicates writes only the finder's own list.
- Tracks that aren't duplicates are never touched, and Melodia moves or deletes no file.
- Nothing runs while the dialog is closed, and idle memory is unchanged.

## What the issue claims, checked against the tree

| Issue says | The tree says | So |
|---|---|---|
| A byte-for-byte copy is easy to spot. | `file_hash` is a BLAKE3 hash of the whole file (`ingest/metadata.rs:18-28`), set on every row. But a retag or a rating write-back rewrites the file and its hash (`library/tags.rs:328`, `scan/retag.rs:41`). | An identical hash proves an identical file, but most copies differ by then. The hash is one tier, not the matcher. |
| Match on artist, title and length; how close is close enough? | Dev library (below): same-song re-encodes land 0.42 to 0.44 s apart. Six different songs carrying identical tags span 229.7 to 286.1 s. | **1.5 s tolerance, measured from the shortest copy in the cluster**, so near-misses can't chain. Tags alone would have merged those six. |
| A live version, a remaster or a radio edit can share a title and come close on length. | A radio edit falls outside the window. A live take sits on another album, at another position or under another recording MBID. A remaster with no MBIDs can carry its original's album, disc and track tags. `tracks.year` holds the tag's year where there is one (`initial_schema.sql:83`). | A year that differs where both copies carry one makes the group Review (`OtherRelease`), so a remaster is never picked for you. |
| Case, accents and punctuation. | `fold::push_folded` handles case and accents, not punctuation (`utils/fold.rs:15-33`). | A new `fold_words` beside it also collapses punctuation. |
| Best quality first. | `codec` holds lofty's container name (`metadata.rs:347`), and `Mp4` is either AAC or ALAC. The dev library has FLACs down to 216 kbps. | Bitrate alone ranks a 216 kbps FLAC below a 320 kbps MP3. Lossless is decided first, from codec plus bit depth. |
| Tracks that share one file, like a cue sheet's. | No CUE support at all: one row per file, `file_path` is UNIQUE (`initial_schema.sql:72`), and `.cue` isn't an audio extension. | Nothing to exclude today. A copy here is a row, which is a file. |
| Listening history moves with the copy. | No history table. History is `play_count`, `skip_count` and `last_played` (`initial_schema.sql:108-113`). Queued scrobbles hold metadata, not ids (`scrobble/queue.rs:18-23`). Nothing imports a count: a new row starts at 0, and the POPM counter is written back as 0 (`rating_tags.rs:78`). | The merge covers those three columns, summing can't count a play twice, and the scrobble queue needs nothing. |
| The system trash, or only taking the copy out of the library. | A move into a trash outside the watched root reads as a plain removal and hard-deletes the row (`watch_batch.rs:96-108`). A trash inside it re-points the row into the trash (`:269-281`). On Windows the `trash` crate panics on a thread whose COM apartment was set the other way, an abort in release. | **Out of the library only** (Kenan, 2026-11-02). Melodia moves no file. The kept-out list says where each spare is, so the file manager can delete it. |
| Library-only removal must keep the file from coming back. | There is no per-file exclusion. Four paths would re-add it: the full-scan walk (`scan/mod.rs:312`), the watcher's create and its modify fallback (`watch_batch.rs:251,299`), and an explicit import (`library/import.rs:25`). A track row also follows its file through a rename and, by `file_hash`, through a move (`ingest.rs:216-238`, `watch_batch.rs:216-240`), which a path alone can't. | A new `excluded_files` table, checked at those sites, that follows a moved file as a track row does. |
| A copy that's playing or queued. | The queue persists ids. A missing id is pruned, and if it was playing, playback jumps to the next track (`queue_prune.rs:47-92`). | Queued spares become the keeper's id in place. The playing copy is held back. |

**Not in the issue, found on the way:**
- **Trash folders get scanned.** The scan walks hidden directories (`scanner.rs:48-79`). A library folder that is a mount root holds that mount's `.Trash-$uid`, a drive-root folder on Windows holds `$RECYCLE.BIN`, and a home folder added whole holds the home trash, a plain `Trash` under `~/.local/share`. Anything a file manager trashes there comes back as a track. It doesn't depend on the duplicates feature, so Phase 0 fixes it on this branch, ahead of the rest.
- **Ratings in tags win.** A rescan takes a non-zero rating from the file's tag over the database's (`mutations.rs:328`), which shapes the rating merge below.

## Measured on the dev library (scratch copy, 2026-11-02)

- 51,012 tracks, every one hashed. None has a zero duration; 218 have no artist.
- `codec`: Mpeg 19,648 · Mp4 9,313 · Flac 8,185 · Vorbis 5,361 · Opus 4,921 · Wav 1,944 · Aiff 1,639 · Aac 1.
- `bit_depth` is set on every Flac, Wav and Aiff row, on 2,434 of the 9,313 Mp4 rows (the ALAC ones), and on no lossy row.
- FLAC bitrates run from 216 to 3,017 kbps.
- **11 byte-identical pairs**, real copies in two folders. Every one has no artist tag.
- 3 no-artist pairs sharing a title, 418 to 444 ms apart.
- 1 group of six rows with identical artist, title and album tags whose lengths span 229.7 to 286.1 s.
- Title plus artist text over the whole library is 1.57 MB.

## Decisions settled here

1. **Matching is a sort over hashed keys, in two passes, done only when the dialog opens.**
   - **Pass 1.** Read `(id, title, artist, duration_ms)` in id-keyset pages, as `backfill_folded` pages (`database/mod.rs:101-147`). Fold each artist and title through `fold_words` and keep `(key hash, duration, id)`, 24 bytes a track. Sort, and keep every run of two or more as candidates.
   - **Pass 2.** Read the candidates' facts by id (`chunked_in_query`), each with its dismissal set (decision 13). Split each run on the real folded strings, so a hash collision can't make a group. Cluster by duration: a copy joins while it is within `DURATION_TOLERANCE_MS` (1,500) of the cluster's shortest copy. Rows with a zero duration never match.
   - Hashing over sorting is the shape that stays flat at any library size, where a `HashMap<key, Vec<row>>` pays per-key vectors.
2. **Every group has a kind, and the kind decides what's picked for you.**

   | Kind | When | Suggested picks |
   |---|---|---|
   | **Identical** | Every copy shares one non-NULL `file_hash`. A row `retroactive_hash` hasn't reached yet matches nothing here. | All but the keeper. |
   | **Same release** | Same folded album; the same disc and track number wherever both copies carry them; the same `year` wherever both carry one; an artist present; and no two distinct recording MBIDs or release MBIDs. | All but the keeper. |
   | **Review**, with its reason | `OtherRelease` (albums, years or release MBIDs differ; a remaster tagged like its original differs only in year), `OtherPosition` (one album, different disc or track numbers, as when a deluxe edition holds a title twice), `OtherRecording` (`musicbrainz_track_id` holds the recording MBID, `metadata.rs:268-270`, and two differ), `NoArtist` (the title may be a file stem, `metadata.rs:203,220`). | Only identical extras (below). |

   - **Identical extras inside a Review group.** Two byte-identical files beside a compilation copy make one Review group. The group stays whole, so the compilation copy is still shown beside them. Within each set of copies sharing one `file_hash`, all but one is picked, the one kept chosen by the keeper order (decision 3). Every other copy is left alone. Removing a byte-identical file loses nothing, and the other release is never touched, which is the issue's rule.
   - Groups sort by kind, then by folded artist and title.
   - A dismissed group (decision 13) is left out of the list unless Show Dismissed is on, and nothing in it is picked for you.
3. **Keeper suggestion.**
   - `AudioQuality` orders lossless first. Lossless means the codec is Flac, Wav, Aiff, Ape or WavPack, or the codec is Mp4 with a bit depth.
   - Then bit depth and sample rate for lossless copies, bitrate for lossy ones.
   - Ties go to the earliest `date_added`, then the lowest id.
4. **Picks.**
   - Picking a group's last unpicked copy moves the keeper to the next copy instead, so every group keeps one.
   - **Keep This Copy** (the copy menu, decision 10) unpicks that copy and picks every other copy in its group, Review groups included, since the choice is the user's own.
   - **Keep Copies in This Folder** (the copy menu) applies Keep This Copy in every listed group with a copy in that copy's folder.
     - The folder is its parent directory, not the folders below it, so a disc subfolder or a library root takes in no more than it shows.
     - Where a group has more than one copy there, the best by the keeper order stays.
     - Groups with no copy there are left alone.
     - A library copied into a second folder makes one group per track, so choosing the folder once beats choosing in every group.
   - Apply takes a `GroupRemoval { keeper, spares }` per group, so "a keeper" is in the type, not only in the UI.
   - The playing copy is held back at apply and stays listed with a Playing chip. A play it finishes counts against its own id, which is gone once the delete commits.
5. **What moves to the keeper, in the delete's own transaction.**

   | Field | Rule |
   |---|---|
   | `is_favorite` | OR of the group. A keeper that gains a heart goes through a `favorites` entry point ending in `sync_love`, the one place a favorite becomes a love (`favorites.rs:1-3`), because loves match on artist and title, and a keeper's tags can differ from the loved spare's. |
   | `rating` | The keeper's own non-zero rating, else the highest among the spares. When the keeper takes one, `rating_writeback::enqueue` (`ratings.rs:30`) carries it to the file if that setting is on. A tag rating wins on rescan, so the keeper's own must not be overruled. |
   | `play_count`, `skip_count` | Summed. |
   | `last_played` | The latest. |
   | `date_added` | The earliest in the group: the song has been in the library since then, so keeping a newer copy shouldn't make it read as recently added. |
   | `last_position` | The keeper's. An offset into one encode isn't one into another. |
   | Playlist places | Each spare item becomes a keeper item at the same position, as a delete plus an insert, because the stats triggers fire on insert and delete only (`initial_schema.sql:548-593`). The insert is `insert_items_tx`'s (`playlist.rs:295`), which already skips a track the playlist holds, so where the keeper is already there the spare's item just goes (`UNIQUE (playlist_id, track_id)`, `:205`). Touched playlists are renumbered (`renumber_playlist_positions_tx`, `playlist.rs:490`, made `pub(crate)`) and their automatic covers refreshed (`:623`). |

   - Those seven columns are every one the user builds up, and `playlist_items` is the only user-built table that names a track. Every other column and credit table is rewritten from the file on rescan (`mutations.rs:310-329`).
   - A playing keeper's merged heart or rating reaches Now Playing through `sync_current_track_if_in`, as `ratings.rs:38` does.
   - Smart playlists are derived at read time, so they follow on their own.
   - The tail matches `apply_watch_batch`'s (`watch_batch.rs:150-153`): `roll_up_covers` and `prune_orphans`.
   - Pending play counts flush first. The flusher holds counts by id for 2 s (`play_count_flusher.rs:19,39-40`), and a count for a deleted id hits no row (`write.rs:45-76`).
     - The core bridge is send-only, and `PlayCountEvent` is `Copy` (`play_counts.rs:8-12`).
     - So the request rides the app's own channel instead: the apply and the flusher both live in `melodia-app`, the sender sits in `AppState`, and a oneshot answers once the write commits.
6. **The queue.**
   - Before the delete commits, every queued spare becomes its keeper's id in place, through `emit_and_execute`. `queue_prune` then never sees a missing id, and playback never jumps.
   - The queue already holds an id more than once (`window_ids` dedups for that reason, `queue/mod.rs:202-217`), so a keeper that is already queued is fine.
   - **Accepted:** a spare already staged as the next track loses its gapless join.
     - The deck holds the spare's own file (`handlers.rs:481`).
     - After the remap, the advance sees the keeper's id and starts it fresh (`transport.rs:334-339`), as any queue edit inside the preload window does.
7. **Removing a copy takes it out of the library and leaves its file alone.**
   - One write transaction does the merge, writes an `excluded_files` row per spare and deletes the spares. Then `library_changed` is bumped.
   - No file is touched, so the watcher sees nothing, and the removal is all or nothing.
   - **Checked again in the transaction.** The library can move while the dialog is open. The transaction skips a group whose keeper is gone and any spare already gone, and the result line counts what it skipped.
   - **Not during a scan.** Remove is disabled while a scan runs (`ScanControl`'s progress watch, `state/scan.rs:67`), and the footer says why.
   - Deleting a spare from disk is left to the file manager. The kept-out list (decision 9) says where each one is.
8. **`excluded_files`** (new migration):
   - **Columns.**
     - `id INTEGER PRIMARY KEY`, the kept-out list's keyset.
     - `path TEXT NOT NULL UNIQUE`, spelled exactly as `tracks.file_path`.
     - `file_hash TEXT`, the spare's, `NULL` where it had none.
     - `folder_id … REFERENCES folders (id) ON DELETE CASCADE`, the spare's.
     - `excluded_at`.
     - Indexed on `folder_id` and `file_hash`.
   - **It lives as the row it replaced did.** A track row follows its file through a rename and, by `file_hash`, through a move. An exclusion keyed by path alone would let any of these bring the spare back, so it follows the file the same two ways:
     - a rename;
     - a tagger that names files from their tags;
     - a folder reorganised with Melodia open or closed.
   - **Checked by the full scan.**
     - The first check is in `discover`'s filter (`scan/mod.rs:312`), from a set loaded once per folder by path prefix under its root. The prefix matters because a spare that came in by a drop sits under the disabled folder row an import makes for its parent (`ingest.rs:464-489`, `folder.rs:88-104`), not under the scanned folder's `folder_id`.
     - That filter only saves the parse. The check that decides runs again inside `ingest`'s write transaction, because a walk that ran before Remove committed still lists the spare.
     - A new path whose hash matches an exclusion whose own path is gone from disk is a moved spare. `resolve_move_candidates` takes exclusions beside rows, the exclusion is re-pointed and the file is skipped.
   - **Checked by the watcher.** It checks before extraction in `process_batch` (`file_event_processor/reconcile.rs:87-94`), then again inside `apply_watch_batch`'s transaction, for the same race:
     - `Created` and `Modified` events on an excluded path are dropped, which also covers the modify fallback's insert (`watch_batch.rs:299`).
     - `Renamed` from an excluded path re-points the exclusion, as a track row is re-pointed (`:269-281`). A rename to outside every folder lifts it.
     - `Renamed` onto an excluded path, a tagger's save through a temp file, is checked as a `Created` and dropped.
     - A created file whose hash matches an exclusion whose path is gone re-points it, through `moved_candidates`.
     - `Removed` lifts the exclusion only when the path is gone at processing time and no file created in the batch took it over. Today `Removed` is never existence-checked (`reconcile.rs:40`), and a save that deletes and recreates its file can land in two batches.
   - **Lifted by an explicit import.** File drop and open-with lift the exclusion on every path they're handed, because the user asked for that file. A dropped folder isn't walked (`import.rs:34-40`), so only named files lift.
   - **Cleanup.**
     - A folder's full scan forgets the exclusions under it whose file the walk didn't find.
       - It does that only where `commit_scan` purges orphans (`melodia-store/…/queries/scan/finish.rs:31-34`): on a completed scan, past the `walked == 0` return (`library/scan/mod.rs:166-169`), sparing the walk's unreadable subtrees (`scanner.rs:172`).
       - Anywhere else, an unmounted drive or a share that is only partly reachable would wipe its exclusions.
     - Removing a folder cascades.
     - Absorbing nested folders re-points their exclusions along with their tracks (`absorb_folders`, `folder.rs:60-84`), or the folders' delete would cascade them away.
   - **Accepted trades.**
     - A spare moved and changed in one go, with a new path and a new hash, comes back.
     - No scan visits a disabled import row, so an exclusion under one is never swept. It leaves through Bring Back.
9. **The kept-out list.**
   - A "Files kept out of the library" row in the Duplicates card, shown when the count is above zero, opens `DialogKind.kept-out-files`.
   - **The list.** A `ListView` over `PagedRows<KeptOutRow>` keyed by exclusion id, in path order. A library added twice leaves as many entries as tracks, so this list can hold the library too.
   - **Each row** shows the file name, its folder and when it was removed, with a **Bring Back** button, the row's main action. Bring Back imports that one file (`library::import`). The import lifts the exclusion, so the file returns at once with no rescan. A file that's gone fails the import (`import.rs:41-44`). The entry stays, because an unmounted drive looks the same, and a toast says the file wasn't found.
   - **The row menu**, raised by a right-click as the copy menu is (decision 10), holds:
     - **Open Containing Folder**, through a `KeptOutUi` callback to `library::tracks::reveal_folder` (`library/tracks.rs:41`) on the file's parent folder. A removed copy has no track id, so it can't take the track menu's call.
     - **Copy Path**, through `CopyActions.copy-text(path)`. The row holds its path, as Browse's folder card does (`card-context-menu.slint:250`).
     - **Bring Back**.
   - **Bring All Back** imports every kept-out path through Bring Back's import, a page at a time, and never rescans. `scan_one` doesn't check `is_enabled`, and a spare that came in by a drop sits under a disabled folder row, so a rescan of that row would import its whole directory.
   - **Copy All Paths** puts every kept-out path on the clipboard, one per line, in path order, for deleting the files from a terminal or a script, or for keeping a record.
     - The paths come from one `excluded_files` read and land through `ui::clipboard::write`, which shows the Copied toast and never logs the text.
     - A library added twice puts about 50k lines on the clipboard, which is fine for an action taken on purpose.
   - A returned file comes back as a new track. Its old history stays with the keeper.
10. **The surface is the shared Dialog: one new kind for the finder, two steps.**
    - `DialogKind.duplicates` takes the widest card, 760 px (`dialog.slint:63-73`). It joins `body-scrolls` (`:93-95`) and owns its scroller. `kept-out-files` does the same.
    - **Review step.** The list. Primary is **Continue**, enabled with one pick or more. The footer note shows the picked count and size (`:100-103`).
    - **Confirm step.** A plain summary: how many copies leave the library, that their files stay where they are and later scans skip them, that their history moves to the copies kept, and that they can be found under Files kept out of the library. Primary is **Remove**, destructive. Cancel reads **Back** and returns to Review. Escape and a backdrop click close the dialog, as for every kind.
    - **What the footer owes the two steps.**
      - Cancel closes unconditionally today (`dialog.slint:236-244`), so Back is a new branch there.
      - The kind joins those that stay open after accept (`:280-287`), so Remove can run and Rust closes the dialog.
      - `destructive` flips with the step.
      - The label switch (`:248-253`) and the enabled gate (`:259-278`) each gain an arm.
    - **The list.** A `ListView` over `PagedRows<DuplicateRow>` (`paged_rows/mod.rs:184`), because this list can hold the library: a library added twice. The keyset is each group's header id (negative, from the group's index) followed by its copies' track ids. A removal pick is the model's own `selected`. Header and copy rows have fixed heights, so a placeholder never jumps.
    - **Group facts and picks.** They live in a compact `DuplicateSet`, with each copy's folder as an index into an interned folder list from pass 2.
      - A pick toggles there and patches the row (`PagedRows::patch`, `:284`).
      - `patch` edits only cached rows, so the row fetch reads `selected` from `DuplicateSet`, and a row scrolled away and back keeps its pick.
    - **The copy menu.** A right-click on a copy row raises a small menu built from the shared parts (`ContextMenuPopup`, `MenuItem`, `context-menu.slint`).
      - It is mounted once in the body, not per row, because the `ListView` recycles rows.
      - It opens on the right button's release through `ContextClick.raises-menu`, so a left click keeps meaning pick or unpick.
      - **Keep This Copy** and **Keep Copies in This Folder** apply decision 4's rules through `DuplicatesUi` callbacks, the second as a walk over `DuplicateSet` with no query. They are the menu's two new strings.
      - **Open Containing Folder** calls the existing `Nav.reveal-in-folder(track_id)`, as the track menu does (`track-context-menu.slint:248-255`), ending in `library::tracks::reveal_in_file_manager` (`library/tracks.rs:23`). It reuses the track menu's `@tr("Open Containing Folder")` and `folder_open` icon, so no new string or Rust is needed.
      - **Copy Path** calls the existing `CopyActions.copy-tracks("path", [track_id])`, which resolves the path through `library::clipboard` (`TrackField::Path`, `library/clipboard.rs:186`) and lands through `ui::clipboard::write`. Its label is the already-translated `@tr("Copy Path")`.
      - **Not the track menu.** Its other entries would cut into the open dialog: Edit Tags can move a copy between groups, a heart or rating changes what the merge carries, Play fights the playing copy's hold-back, and a queue or playlist edit changes what the removal remaps.
    - **The group menu.** A right-click on a header row raises its own small menu, mounted once beside the copy menu and built from the same parts.
      - It holds **Not Duplicates** (decision 13), which reads **Count as Duplicates** on a dismissed group.
      - The toolbar's **Show Dismissed** toggle, with its count, lists dismissed groups, each with a Dismissed chip.
    - **Opening.** The open raises the dialog at once in a finding state and fills it behind a `DialogClaim` (`callbacks/mod.rs:79-103`).
    - Two steps on one kind is the Last.fm login's pattern (`dialog.slint:249-250`). One kind is needed because the Dialog can't open a second Dialog on top of itself.
11. **No undo.** An undo would have to un-sum counts and un-move playlist places on a keeper that may have moved on since. A removed copy comes back through the kept-out list instead, as a new track. `ui-patterns.md` keeps a confirmation wherever undo can't cover the removal, and the issue asks for exactly that step.
    - The apply leaves the undo slot alone. An undo held from before can name a spare, but a playlist restore already leaves out a track that's gone (`queries/playlist_restore.rs:21-24`), and a restored queue cut is pruned on the next `library_changed`.
12. **Cost.**
    - Closed, both dialogs cost nothing.
    - A find holds about 24 B a track while it sorts (about 1.2 MB at 51k), plus the candidates' facts until they are grouped.
    - An open finder holds `DuplicateSet` at a few dozen bytes a copy, its interned folder list, and `PagedRows`' row cache.
    - An open kept-out list holds 8 B an entry plus the same row cache.
    - A dismissal is 16 B a copy on disk, read only for a find's candidates.
13. **Not Duplicates.** Matching on tags groups copies the user keeps on purpose, and without a memory each find lists them again.
    - **Storage.** The table is `duplicate_dismissals (track_id INTEGER PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE, set_id INTEGER NOT NULL)`, in its own migration.
      - It's keyed by track id because a row's id survives a rename and a move.
      - A removed spare's dismissal cascades away.
    - **Writes.**
      - Dismissing is one store transaction. It takes `COALESCE(MAX(set_id), 0) + 1` and writes it to every copy with `INSERT OR REPLACE`, so a copy dismissed again moves to the new set.
      - Count as Duplicates deletes that set's rows.
    - **The rule.**
      - A group is dismissed when every copy carries one `set_id`.
      - A new copy joining shows it again, since it carries no set. That includes a file brought back from the kept-out list, which returns as a new track.
      - A copy leaving, by removal or a retag, leaves the rest dismissed.
    - **Any kind can be dismissed.** A mono and a stereo mix with identical tags reads as Same release.
    - **What it touches.**
      - The group leaves the open list at once.
      - Nothing in the library changes, and nothing waits for Remove.

## Considered and not taken

- **Moving spare files to the system trash.** It would be the first feature in which Melodia moves or deletes a user's audio, and it needs the most of anything in this plan:
  - a new dependency;
  - a dedicated thread per apply, because the crate's Windows backend panics on a thread whose COM apartment was set the other way, and a panic is an abort in release;
  - a slow copy into the home trash where a mount refuses its own trash;
  - a progress, cancel and failure UI;
  - Windows and macOS paths that can't be checked here.

  The file manager already does this well. The kept-out list's Open Containing Folder hands each spare to it, and the watcher drops the exclusion once the file is gone.
- **History keyed by song identity rather than by row.** A stats table keyed by a hash of artist, album, disc, track and title would let every copy of a release track share one history, and keep it through a removal and re-add. That would make the merge unnecessary. But it moves `play_count`, `rating`, `is_favorite` and `last_played` off `tracks`, where the sort arms, smart playlists, Most Played, five partial indexes, the stats triggers and the rating import and write-back all read them. A retag would also detach a song from its history. If wanted, that belongs in an issue of its own, not inside this one.
- **A soft-delete flag on `tracks` instead of `excluded_files`.** Every library-sized query, the keysets, the FTS, the stats triggers and the counts would need to filter on it, and a missed one shows a removed copy. A deleted row plus a path table leaves every existing query as it is.
- **Audio fingerprints.** Telling a remaster from the original by fingerprint means decoding every candidate, which is a cost the issue rules out for a big library. The recording MBID veto and the release checks cover the cases tags can answer.
- **Exact tag equality (artist, album, title) with no length check.** It would have merged the six same-tag songs measured above, and missed copies that differ only in case or accents.
- **A hash of the audio alone, tags left out.**
  - It would class a retagged copy as Identical, the tier that picks inside Review groups.
  - A retag is often deliberate, as with a compilation's copy of an album track.
  - Whole-file equality is the only kind that loses nothing.
- **Skipping every hidden entry in the walk.** It would purge, history and all, the tracks of anyone who keeps music under a dot-folder today. The trash predicate takes only what a file manager put there.
- **Hiding spares as versions of one track instead of removing them.** It needs a hidden state every library-sized query honours, which is the soft-delete flag above.
- **A dry-run report before Remove.** The Confirm step says what happens. The kept-out list and Copy All Paths keep the record of what left.
- **A keeper rule picker or protected folders, as settings.** Keep This Copy and Keep Copies in This Folder cover the same choices on the spot, without a setting to find first.

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-core/src/utils/trash_dir.rs` (new, P0) | `is_trash_dir`, which takes a folder: a volume trash by name, or the home trash at `dirs::data_dir()/Trash` by path. `in_trash_dir` asks it of every folder above a path. |
| `crates/melodia-store/src/media/ingest/scanner.rs`, `ingest/watcher.rs` | The walk skips a trash folder; the watcher's `is_library_file` rejects a path inside one (P0). |
| `crates/melodia-core/src/utils/fold.rs` | `fold_words`, beside `push_folded`: folded, each run of non-alphanumerics one space, trimmed. If nothing is left, the folded string. |
| `crates/melodia-core/src/entities/audio_quality.rs` (new, P1) | `AudioQuality`: the lossless rule and the keeper order. |
| `crates/melodia-core/src/entities/duplicates.rs` (new, P1) | `MatchKind`, `ReviewReason`, `DuplicateGroup`, `DuplicateCopy`, `GroupRemoval`; `KeptOutFile` (P4). |
| `crates/melodia-store/src/database/queries/track/duplicates.rs` (new, P1) | The keyset-paged pass-1 read and the pass-2 facts by id, each with its dismissal set. |
| `migrations/20261102000000_duplicate_dismissals.sql` (new, P1) | The dismissal table. |
| `crates/melodia-store/src/database/queries/duplicate_dismissals.rs` (new, P1) | Dismiss a group as a new set, and restore a set. |
| `crates/melodia-store/src/database/queries/track/full_rows.rs` | Loses `get_duplicate_tracks` and its two tests (`track_tests.rs:316,340`) (P1). |
| `crates/melodia-app/src/library/duplicates/mod.rs` (new, P1) | The library API: `find`, `dismiss` and `restore`; `apply` (P3); the kept-out count, ids, rows, `bring_back` and `bring_all_back` (P4). |
| `crates/melodia-app/src/library/duplicates/matcher.rs` (new, P1) | Pure grouping: key runs, collision split, duration clusters, kind, keeper, the dismissed flag. |
| `crates/melodia-ui/ui/globals/duplicates.slint` (new, P2) | `DuplicatesUi`: rows, step, busy, counts, callbacks; `KeptOutUi` (P4). |
| `crates/melodia-ui/ui/models.slint` | `DuplicateRow` (P2), `KeptOutRow` (P4). |
| `crates/melodia-ui/ui/components/dialog/duplicates-body.slint` (new, P2) | Both steps' bodies and the list's toolbar (Suggested picks, Clear picks, Show Dismissed). |
| `crates/melodia-ui/ui/components/dialog/duplicate-row.slint` (new, P2) | The header row with its Dismissed chip, and the copy row. |
| `crates/melodia-ui/ui/components/dialog/duplicates-menus.slint` (new, P2) | `DuplicateCopyMenu`, `DuplicateGroupMenu`; `KeptOutMenu` (P4). All are built from `context-menu.slint`'s parts. |
| `crates/melodia-ui/ui/components/dialog/kept-out-body.slint` (new, P4) | The kept-out list, its row with Bring Back, Bring All Back and Copy All Paths. |
| `crates/melodia-ui/ui/components/dialog/dialog.slint`, `globals/dialog.slint` | Both kinds: width, `body-scrolls`, mount, Back on Confirm, primary label and gate, the stay-open list, `destructive` by step, `accepted` arms (P2 to P4). |
| `crates/melodia-ui/ui/views/settings/duplicates-section.slint` (new, P2) | The Find row, and the kept-out row (P4). Mounted in `pages/library-page.slint` and its `has-matches`. |
| `crates/melodia-views/src/ui/duplicates/mod.rs` (new, P2) | `install`, the open behind its claim, the close teardown. |
| `crates/melodia-views/src/ui/duplicates/picks.rs` (new, P2) | `DuplicateSet`: the compact groups and their folder index, the pick rule, suggested picks with the identical extras, Keep This Copy, Keep Copies in This Folder, cleared picks, dismissed groups shown or hidden, picked count and bytes. |
| `crates/melodia-views/src/ui/duplicates/rows.rs` (new, P2) | `DuplicateRow`'s placeholder and fetch, the header's lead-copy lookup, the format label. |
| `crates/melodia-views/src/ui/duplicates/apply.rs` (new, P3) | Picks into `GroupRemoval`s, the call, the result line. |
| `crates/melodia-views/src/ui/duplicates/kept_out.rs` (new, P4) | `KeptOutRow`'s paged rows, Open Containing Folder, Bring Back, Bring All Back, Copy All Paths, the Settings row's count. |
| `crates/melodia-views/src/ui/util.rs` | `format_bitrate` and `format_bit_depth` beside `format_sample_rate` (`:97`), named from the inline closures at `now_playing/metadata.rs:14,16` and `callbacks/tags/open.rs:497,502` (P2, no behaviour change). |
| `migrations/20261102000001_excluded_files.sql` (new, P3) | The table and its folder and hash indexes. |
| `crates/melodia-store/src/database/queries/excluded_files.rs` (new, P3) | Exclude (`pub(crate)`, the merge's), excluded among paths, under a path prefix, move candidates by hash, re-point, lift, forget unseen, count; ids in path order, rows by id, every path in path order (P4). |
| `crates/melodia-store/src/database/queries/ingest.rs` (550), `queries/scan/watch_batch.rs` (316) | The exclusion check inside the inserting transaction, exclusions among the move candidates, a rename's re-point and lift (P3). |
| `crates/melodia-store/src/database/queries/folder.rs` (121) | `absorb_folders` re-points exclusions with the tracks (P3). |
| `crates/melodia-store/src/database/queries/playlist.rs` (639) | `renumber_playlist_positions_tx` becomes `pub(crate)` for the merge (P3). |
| `crates/melodia-app/src/library/favorites.rs` | The entry point the merge's keepers take to `sync_love` (P3). |
| `crates/melodia-store/src/database/queries/track/duplicate_merge.rs` (new, P3) | `remove_duplicates`: merge, playlist places, exclusions, delete, renumber, tail. One transaction. |
| `crates/melodia-engine/src/player/engine/queue/removal.rs` (321) | `replace_ids`: spare to keeper in place, then an `order_changed` bump (P3). |
| `crates/melodia-app/src/library/duplicates/apply.rs` (new, P3) | Scan guard, playing hold-back, flush, queue remap, store call, bump, covers, rating write-back, love sync, the Now Playing mirror. |
| `crates/melodia-app/src/library/scan/mod.rs`, `tasks/file_event_processor/reconcile.rs`, `library/import.rs` | The checks before parsing, the `Removed` lift, the import's lift and the scan-time cleanup (P3). |
| `crates/melodia-app/src/tasks/play_count_flusher.rs`, `crates/melodia-app/src/state/mod.rs` | A flush request on the app's own channel, its sender in `AppState`, answered once the write commits (P3). |

Size headroom, measured: `app-window.slint` 749 lines (gains two export names), `globals/dialog.slint` 419, `components/dialog/dialog.slint` 294, `models.slint` 461, `queue/removal.rs` 321, `playlist.rs` 639, `ingest.rs` 550. None crosses 800.

## Phases

### Phase 0: Trash folders are never indexed

Status: built and hand-checked (2026-11-02). The home-trash item ran on a scratch home through `XDG_DATA_HOME`, because the real home folder has more directories than inotify's per-user watch limit.

1. `utils::trash_dir`:
   - `is_trash_dir(&Path)` takes a folder. It matches `.Trash`, `.Trash-` followed by a uid, `.Trashes`, and `$RECYCLE.BIN` compared without case, by name. It matches the home trash, `dirs::data_dir()/Trash` made canonical, by path, because a plain `Trash` is a name any album folder could carry. The walk sees one folder at a time, so the home trash has to be in this predicate.
   - `in_trash_dir(&Path)` asks it of every folder above a path.
2. **The walk.** `collect_media_files` (`scanner.rs:48-79`) gives `WalkDir` a `filter_entry` that skips a trash folder below the root, so nothing under it is read.
3. **The watcher.** `classify_event` (`watcher.rs:188-239`) asks a new `is_library_file` in every arm: an audio file outside every trash folder. The rename arm's existing branches then do the rest, with no new arm:
   - a rename into a trash folder becomes a `Removed`;
   - a rename out of one becomes a `Created`;
   - events inside one are dropped.
4. **Existing installs need nothing extra.** A track already indexed from a trash folder leaves on its folder's next full scan: the walk no longer finds it, so `commit_scan`'s orphan purge removes it (`melodia-store/…/queries/scan/finish.rs:31-34`). Accepted: a folder whose only audio sits in a trash walks to zero files, which purges nothing (`library/scan/mod.rs:166-169`).
5. Docs: `library-data.md` gains "`utils::trash_dir` is the one trash-folder predicate", beside the audio-extension line.

Hand test, in a library folder:
- An audio file in a `.Trash-1000/files/` is not indexed by a rescan.
- One indexed there before this fix is gone after the rescan.
- With watching on, moving a track's file into it drops the track, and moving it back out adds it again.
- With the home folder added as a library folder and watching on, trashing a track from the file manager drops it, instead of re-pointing it into `~/.local/share/Trash`.

### Phase 1: Find (no UI)

1. `fold_words` in `utils/fold.rs`.
2. `AudioQuality` and the duplicates entities in `melodia-core`.
3. The pass-1 and pass-2 queries in `queries/track/duplicates.rs`.
4. The dismissal migration and `queries::duplicate_dismissals`, and pass 2 reading each candidate's set.
5. `library::duplicates::{find, dismiss, restore}` over `matcher`.
6. Delete `get_duplicate_tracks` and its two tests, which nothing else calls.
7. Docs: `library-data.md` gains the matching rule's anchor and "a fold for matching goes through `fold_words`".

Gate: clippy and tests. Nothing calls `find` until Phase 2, so there's no run.

### Phase 2: The review dialog (nothing removed)

1. Lift `format_bitrate` and `format_bit_depth` into `ui/util.rs`. No behaviour change.
2. `DuplicatesUi`, `DuplicateRow` and the duplicates kind on the Dialog, with the Review step only. Continue stays disabled until Phase 3.
3. `ui/duplicates/{mod,picks,rows}.rs`: find on open, the list, suggested picks (identical extras in Review groups included), toggling, the never-every-copy rule, Clear and Suggested, Show Dismissed.
4. `DuplicateCopyMenu` on copy rows:
   - Keep This Copy and Keep Copies in This Folder, through `DuplicatesUi` callbacks;
   - Open Containing Folder, through `Nav.reveal-in-folder`;
   - Copy Path, through `CopyActions.copy-tracks("path", …)`.
5. `DuplicateGroupMenu` on header rows: Not Duplicates and Count as Duplicates, through `DuplicatesUi` to `library::duplicates`.
6. `DuplicatesSection` with the Find row in Settings ▸ Library.
7. Run `scripts/update-translations.py` and finish all 15 catalogs.
8. Docs: `ui-patterns.md` names the duplicates body under Inputs/Dialog, with the two-steps-on-one-kind contract.

Hand test, on the dev library:
- The 11 identical pairs list as Identical and picked.
- The 3 no-artist pairs list as Review and unpicked.
- The six same-tag songs do not group.
- Scrolling the list stays smooth.
- The copy menu:
  - Open Containing Folder opens the copy's folder, and the dialog stays open with its picks.
  - Copy Path copies the right path, with the Copied toast.
  - Keep This Copy on a four-copy group leaves only that copy unpicked, in a Review group too.
  - Keep Copies in This Folder on one copy of an album present in two folders keeps every track of that folder, and leaves unrelated groups as they were.
  - A left click still picks.
- The group menu:
  - Not Duplicates on a Review group takes it out of the list, and after closing the dialog and finding again it stays hidden.
  - Copying a third copy of that song into a library folder shows the group again.
  - Show Dismissed lists it with its chip, and Count as Duplicates brings it back.
- A Review group holding an identical pair beside an other-release copy (two copies of one album file next to a compilation copy, made by hand): one of the pair is picked, and the compilation copy is not.
- A remaster tagged like its original but with another year lists as Review, unpicked.
- Scroll a picked row out of view and back: it keeps its pick.
- RSS (`MELODIA_RSS_SAMPLE=1`) with the dialog open, then closed.

### Phase 3: Remove from library

1. The migration and `queries::excluded_files`.
2. The checks before parsing in the scan and the watcher, and the check that decides inside `ingest`'s and `apply_watch_batch`'s transactions.
3. Exclusions that follow the file: a rename's re-point and lift, exclusions among both move-candidate reads, `absorb_folders`' re-point, and `Removed`'s existence-checked lift.
4. The import's lift, and the scan-time cleanup behind the orphan purge's guards.
5. `remove_duplicates` with the merge rules from decision 5, re-checking each group inside its transaction.
6. The flush request; `replace_ids` and the queue remap.
7. `library::duplicates::apply`, with the love sync and the Now Playing mirror.
8. The Confirm step's summary, Back, the Remove button disabled during a scan, and the result line.
9. Translations.
10. Docs:
    - `library-data.md`: "A per-track column the user builds up joins `remove_duplicates`' merge", and "a new ingest path consults `excluded_files` inside its inserting transaction, and a rename or move carries an exclusion as it carries a row".
    - `README.md`: the feature.

Hand test, the issue's list:
- An album present as FLAC and MP3: the MP3 goes, and its plays and heart land on the FLAC.
- The same song on an album and a compilation: Review, unpicked.
- A live version next to the studio one: no group, or Review through `OtherRecording`.
- A copy in a playlist carrying a rating: the keeper takes the place and the stars.
- A queued spare: still in the queue, now the keeper.
- A playing spare: held back.
- A rescan: the spare stays out, and its file is untouched.
- Dropping the file back in: it returns.

Hand test, the kept-out file staying out (watching on unless noted):
- Renaming a kept-out file in the file manager: it stays out, under its new path.
- Saving a kept-out file from a tagger that writes through a temp file: it stays out.
- Moving a kept-out file to another folder of the library while Melodia is closed, then rescanning: it stays out.
- Deleting a kept-out file: it leaves the list.
- Starting a scan with the dialog on Confirm: Remove disables, and says why.
- Removing a copy after the watcher already took it away: the result line counts it as skipped.

### Phase 4: The kept-out list

1. `excluded_files`' ids in path order, rows by id and every path in path order, plus the library API over them.
2. `KeptOutUi`, `KeptOutRow` and `DialogKind.kept-out-files`: the paged list, the row's Bring Back, Bring All Back by import a page at a time, and Copy All Paths through `ui::clipboard::write`.
3. `KeptOutMenu` on its rows:
   - Open Containing Folder, through `KeptOutUi` to `reveal_folder` on the file's parent;
   - Copy Path, through `CopyActions.copy-text`;
   - Bring Back.
4. The kept-out row in the Duplicates card, shown when the count is above zero, its count following `library_changed`.
5. Translations.
6. Docs: `README.md` names where the removed copies can be found.

Hand test:
- Right-clicking a row opens its own folder, and Copy Path copies its path.
- Copy All Paths puts one line per kept-out file on the clipboard, in path order.
- A kept-out file deleted in the file manager leaves the list.
- Bring Back returns one file at once, as a new track.
- Bring Back with the file's drive unmounted: a toast says it wasn't found, and the entry stays.
- Bring All Back returns them all at once, and a spare that came in by a drop brings back that file alone, not its folder.
- Removing a library folder takes its entries with it.

### Phase 5: Tests

Tests are added where the landed change earns them, on Kenan's go.

## Cross-cutting

- **Defaults on a live app.**
  - Nothing runs unless you open a dialog.
  - Melodia moves and deletes no file.
  - Both migrations are additive and touch no shipped one.
- **Every new `@tr` follows the house rules.** Plurals use the `"{n} copy" | "{n} copies" % n` form, and a string Rust builds goes through a Slint opener or a `*Text` global, never a `[string]` filled from Rust.
- **Memory.** Idle RSS doesn't move. The Phase 2 RSS check confirms the open dialog's cost, and heaptrack is needed only if it surprises.

## Open questions

None open. Settled 2026-11-02:
- An identical pair in a Review group stays in its group, and one of the pair is picked (decision 2).
- Keep This Copy is in the copy menu (decisions 4 and 10).
- Copy Path is in both menus, and Copy All Paths is in the kept-out list (decisions 9 and 10).
- Not Duplicates is in, as a group menu with Show Dismissed (decisions 10 and 13).
- Keep Copies in This Folder is in the copy menu (decisions 4 and 10).
