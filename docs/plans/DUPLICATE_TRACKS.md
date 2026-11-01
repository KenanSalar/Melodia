# Plan: Duplicate tracks (find the copies, clear out the spares)

Status: **proposed** · Owner: Kenan · Created: 2026-11-02 · Issue #158, branch `feat/duplicate-tracks`

> **Validated 2026-11-02** against `b4c5ecff`. The sources were:
> - three read-only code traces: the store and per-track data; the removal, scan, watcher and queue paths; the UI surfaces;
> - a scratch copy of the 51,012-track dev database;
> - the `trash` 5.2.9 manifest and its freedesktop and Windows sources, which argued for leaving the trash out;
> - a read of how two long-shipping desktop players handle duplicates, deletion, the trash and the history a row carries, which settled the disc-and-track check, the earliest added date and the "Considered and not taken" list;
> - Kenan's answers of 2026-11-02: removal takes a copy out of the library and leaves its file alone, with no trash; the shared Dialog is the surface.

Working doc. Delete it when its last phase (Tests) lands.

## Context

**What you see today.** A library that grew over years holds some songs more than once: an album ripped as both FLAC and MP3, a single beside the album it came from, a folder copied in twice. Every copy shows up in Songs, in search and in shuffle. Nothing tells you how many there are, and nothing in the app clears them out. Each copy keeps its own plays, rating, heart and playlist places, so a song's history is split across copies you can't see side by side.

**Why.** Nothing groups copies. The one duplicate query, `queries::track::get_duplicate_tracks` (`full_rows.rs:83-113`), only matches byte-identical files, and only two tests call it.

**Outcome.** Settings ▸ Library ▸ Duplicates ▸ **Find…** opens a dialog listing groups of tracks that look like the same song. Each copy shows its format and quality, its folder, when it was added, and what it carries (plays, stars, heart, playlists). Each group is labelled with how sure the match is.

The suggested picks keep the best copy. Every pick can be changed, a group can never have every copy picked, and the same song on different releases is never picked for you. Right-clicking a copy offers three entries, and the dialog stays open with your picks:
- **Keep This Copy** makes it the one that stays, in one step.
- **Open Containing Folder** lets you look at the files before choosing.
- **Copy Path** puts its file path on the clipboard.

**Continue** shows what will happen. **Remove** then takes the spares out of the library and moves their history onto the copy that stays. Their files stay exactly where they are, and later scans skip them.

**Files kept out of the library**, a second row in the same card, lists every removed copy. Each one shows its path with a **Bring Back** button, which returns it to the library. Right-clicking one offers **Open Containing Folder** (for deleting the file in your file manager), **Copy Path** and Bring Back. **Copy All Paths** puts every removed file's path on the clipboard, one per line.

**What keeps working** (the issue's three, held below):
- Nothing changes until you press Remove.
- Tracks that aren't duplicates are never touched, and Melodia moves or deletes no file.
- Nothing runs while the dialog is closed, and idle memory is unchanged.

## What the issue claims, checked against the tree

| Issue says | The tree says | So |
|---|---|---|
| A byte-for-byte copy is easy to spot. | `file_hash` is a BLAKE3 hash of the whole file (`ingest/metadata.rs:18-28`), set on every row. But a retag or a rating write-back rewrites the file and its hash (`library/tags.rs:328`, `scan/retag.rs:41`). | An identical hash proves an identical file, but most copies differ by then. The hash is one tier, not the matcher. |
| Match on artist, title and length; how close is close enough? | Dev library (below): same-song re-encodes land 0.42 to 0.44 s apart. Six different songs carrying identical tags span 229.7 to 286.1 s. | **1.5 s tolerance, measured from the shortest copy in the cluster**, so near-misses can't chain. Tags alone would have merged those six. |
| Case, accents and punctuation. | `fold::push_folded` handles case and accents, not punctuation (`utils/fold.rs:15-33`). | A new `fold_words` beside it also collapses punctuation. |
| Best quality first. | `codec` holds lofty's container name (`metadata.rs:347`), and `Mp4` is either AAC or ALAC. The dev library has FLACs down to 216 kbps. | Bitrate alone ranks a 216 kbps FLAC below a 320 kbps MP3. Lossless is decided first, from codec plus bit depth. |
| Tracks that share one file, like a cue sheet's. | No CUE support at all: one row per file, `file_path` is UNIQUE (`initial_schema.sql:72`), and `.cue` isn't an audio extension. | Nothing to exclude today. A copy here is a row, which is a file. |
| Listening history moves with the copy. | No history table. History is `play_count`, `skip_count` and `last_played` (`initial_schema.sql:108-113`). Queued scrobbles hold metadata, not ids (`scrobble/queue.rs:18-23`). | The merge covers those three columns, and the scrobble queue needs nothing. |
| The system trash, or only taking the copy out of the library. | The watcher reads a move into the trash as a plain removal and hard-deletes the row (`watch_batch.rs:95-107`). On Windows the `trash` crate panics on a thread whose COM apartment was set the other way, an abort in release. | **Out of the library only** (Kenan, 2026-11-02). Melodia moves no file. The kept-out list says where each spare is, so the file manager can delete it. |
| Library-only removal must keep the file from coming back. | There is no per-file exclusion. Four paths would re-add it: the full-scan walk (`scan/mod.rs:308-311`), the watcher's create and its modify fallback (`watch_batch.rs:205,298-299`), and an explicit import (`library/import.rs:25`). | A new `excluded_files` table, checked at those sites. |
| A copy that's playing or queued. | The queue persists ids. A missing id is pruned, and if it was playing, playback jumps to the next track (`queue_prune.rs:47-92`). | Queued spares become the keeper's id in place. The playing copy is held back. |

**Not in the issue, found on the way:**
- **Trash folders get scanned.** The scan walks hidden directories (`scanner.rs:47-77`). A library folder that is a mount root holds that mount's `.Trash-$uid`, and a drive-root folder on Windows holds `$RECYCLE.BIN`, so anything a file manager trashes there comes back as a track. It doesn't depend on the duplicates feature, so Phase 0 fixes it on this branch, ahead of the rest.
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
   - **Pass 1.** Read `(id, title, artist, duration_ms)` in id-keyset pages, as `backfill_folded` pages (`database/mod.rs:101-145`). Fold each artist and title through `fold_words` and keep `(key hash, duration, id)`, 24 bytes a track. Sort, and keep every run of two or more as candidates.
   - **Pass 2.** Read the candidates' facts by id (`chunked_in_query`). Split each run on the real folded strings, so a hash collision can't make a group. Cluster by duration: a copy joins while it is within `DURATION_TOLERANCE_MS` (1,500) of the cluster's shortest copy. Rows with a zero duration never match.
   - Hashing over sorting is the shape that stays flat at any library size, where a `HashMap<key, Vec<row>>` pays per-key vectors.
2. **Every group has a kind, and the kind decides what's picked for you.**

   | Kind | When | Suggested picks |
   |---|---|---|
   | **Identical** | Every copy shares one `file_hash`. | All but the keeper. |
   | **Same release** | Same folded album; the same disc and track number wherever both copies carry them; an artist present; and no two distinct recording MBIDs or release MBIDs. | All but the keeper. |
   | **Review**, with its reason | `OtherRelease` (albums or release MBIDs differ), `OtherPosition` (one album, different disc or track numbers, as when a deluxe edition holds a title twice), `OtherRecording` (`musicbrainz_track_id` holds the recording MBID, `metadata.rs:268-270`, and two differ), `NoArtist` (the title may be a file stem, `metadata.rs:203,220`). | Only identical extras (below). |

   - **Identical extras inside a Review group.** Two byte-identical files beside a compilation copy make one Review group. The group stays whole, so the compilation copy is still shown beside them. Within each set of copies sharing one `file_hash`, all but one is picked, the one kept chosen by the keeper order (decision 3). Every other copy is left alone. Removing a byte-identical file loses nothing, and the other release is never touched, which is the issue's rule.
   - Groups sort by kind, then by folded artist and title.
3. **Keeper suggestion.**
   - `AudioQuality` orders lossless first. Lossless means the codec is Flac, Wav, Aiff, Ape or WavPack, or the codec is Mp4 with a bit depth.
   - Then bit depth and sample rate for lossless copies, bitrate for lossy ones.
   - Ties go to the earliest `date_added`, then the lowest id.
4. **Picks.**
   - Picking a group's last unpicked copy moves the keeper to the next copy instead, so every group keeps one.
   - **Keep This Copy** (the copy menu, decision 10) unpicks that copy and picks every other copy in its group, Review groups included, since the choice is the user's own.
   - Apply takes a `GroupRemoval { keeper, spares }` per group, so "a keeper" is in the type, not only in the UI.
   - The playing copy is held back at apply and stays listed with a Playing chip. A play it finishes counts against its own id, which is gone once the delete commits.
5. **What moves to the keeper, in the delete's own transaction.**

   | Field | Rule |
   |---|---|
   | `is_favorite` | OR of the group. The column is written directly, with no love sync (`favorites.rs:1-3`), since the spare's love already went out. |
   | `rating` | The keeper's own non-zero rating, else the highest among the spares. When the keeper takes one, `rating_writeback::enqueue` (`ratings.rs:30`) carries it to the file if that setting is on. A tag rating wins on rescan, so the keeper's own must not be overruled. |
   | `play_count`, `skip_count` | Summed. |
   | `last_played` | The latest. |
   | `date_added` | The earliest in the group: the song has been in the library since then, so keeping a newer copy shouldn't make it read as recently added. |
   | `last_position` | The keeper's. An offset into one encode isn't one into another. |
   | Playlist places | Each spare item becomes a keeper item at the same position, as a delete plus an insert, because the stats triggers fire on insert and delete only (`initial_schema.sql:548-593`). Where the keeper is already in that playlist, the spare's item goes (`UNIQUE (playlist_id, track_id)`, `:205`). Touched playlists are renumbered (`playlist.rs:490`) and their automatic covers refreshed (`:623`). |

   - Smart playlists are derived at read time, so they follow on their own.
   - The tail matches `apply_watch_batch`'s (`watch_batch.rs:150-153`): `roll_up_covers` and `prune_orphans`.
   - Pending play counts flush first. The flusher holds counts by id for 2 s (`play_count_flusher.rs:19,39-40`), and a count for a deleted id hits no row (`write.rs:45-76`), so the apply sends a flush request over the bridge and waits for it.
6. **The queue.**
   - Before the delete commits, every queued spare becomes its keeper's id in place, through `emit_and_execute`. `queue_prune` then never sees a missing id, and playback never jumps.
   - The queue already holds an id more than once (`window_ids` dedups for that reason, `queue/mod.rs:202-217`), so a keeper that is already queued is fine.
7. **Removing a copy takes it out of the library and leaves its file alone.**
   - One write transaction does the merge, writes an `excluded_files` row per spare and deletes the spares. Then `library_changed` is bumped.
   - No file is touched, so the watcher sees nothing, and the removal is all or nothing.
   - Deleting a spare from disk is left to the file manager. The kept-out list (decision 9) says where each one is.
8. **`excluded_files`** (new migration):
   - Columns: `id INTEGER PRIMARY KEY` (the kept-out list's keyset), `path TEXT NOT NULL UNIQUE`, `folder_id … REFERENCES folders (id) ON DELETE CASCADE`, `excluded_at`. Indexed on `folder_id`. `path` is spelled exactly as `tracks.file_path`.
   - **Checked by the full scan.** In `discover`'s filter, from a set loaded once per folder.
   - **Checked by the watcher.** Before extraction in `process_batch` (`file_event_processor/reconcile.rs:87-94`):
     - `Created` and `Modified` events on an excluded path are dropped, which also covers the modify fallback's insert.
     - `Removed` lifts the exclusion, which is how a spare deleted in the file manager leaves the list.
     - `Renamed` lifts it for the old path and treats the new one as any new file.
   - **Lifted by an explicit import.** File drop and open-with lift the exclusion on every path they're handed, because the user asked for that file.
   - **Cleanup.**
     - A folder's full scan forgets exclusions whose file its walk didn't find.
     - Removing a folder cascades.
   - **Accepted trade.** A move made while Melodia is closed, or one across filesystems, brings the file back, because the exclusion is keyed by path.
9. **The kept-out list.**
   - A "Files kept out of the library" row in the Duplicates card, shown when the count is above zero, opens `DialogKind.kept-out-files`.
   - **The list.** A `ListView` over `PagedRows<KeptOutRow>` keyed by exclusion id, in path order. A library added twice leaves as many entries as tracks, so this list can hold the library too.
   - **Each row** shows the file name, its folder and when it was removed, with a **Bring Back** button, the row's main action. Bring Back imports that one file (`library::import`). The import lifts the exclusion, so the file returns at once with no rescan.
   - **The row menu**, raised by a right-click as the copy menu is (decision 10), holds:
     - **Open Containing Folder**, through a `KeptOutUi` callback to `library::tracks::reveal_folder` (`library/tracks.rs:41`) on the file's parent folder. A removed copy has no track id, so it can't take the track menu's call.
     - **Copy Path**, through `CopyActions.copy-text(path)`. The row holds its path, as Browse's folder card does (`card-context-menu.slint:250`).
     - **Bring Back**.
   - **Bring All Back** lifts every exclusion and rescans the folders they sat in.
   - **Copy All Paths** puts every kept-out path on the clipboard, one per line, in path order, for deleting the files from a terminal or a script, or for keeping a record.
     - The paths come from one `excluded_files` read and land through `ui::clipboard::write`, which shows the Copied toast and never logs the text.
     - A library added twice puts about 50k lines on the clipboard, which is fine for an action taken on purpose.
   - A returned file comes back as a new track. Its old history stays with the keeper.
10. **The surface is the shared Dialog: one new kind for the finder, two steps.**
    - `DialogKind.duplicates` takes the widest card, 760 px (`dialog.slint:63-73`). It joins `body-scrolls` (`:93-98`) and owns its scroller. `kept-out-files` does the same.
    - **Review step.** The list. Primary is **Continue**, enabled with one pick or more. The footer note shows the picked count and size (`:100-103`).
    - **Confirm step.** A plain summary: how many copies leave the library, that their files stay where they are and later scans skip them, that their history moves to the copies kept, and that they can be found under Files kept out of the library. Primary is **Remove**, destructive. Cancel reads **Back** and returns to Review. Escape and a backdrop click close the dialog, as for every kind.
    - **The list.** A `ListView` over `PagedRows<DuplicateRow>` (`paged_rows/mod.rs:184`), because this list can hold the library: a library added twice. The keyset is each group's header id (negative, from the group's index) followed by its copies' track ids. A removal pick is the model's own `selected`. Header and copy rows have fixed heights, so a placeholder never jumps.
    - **Group facts and picks.** They live in a compact `DuplicateSet`. A pick toggles there and patches the row (`PagedRows::patch`, `:284`).
    - **The copy menu.** A right-click on a copy row raises a small menu built from the shared parts (`ContextMenuPopup`, `MenuItem`, `context-menu.slint`).
      - It is mounted once in the body, not per row, because the `ListView` recycles rows.
      - It opens on the right button's release through `ContextClick.raises-menu`, so a left click keeps meaning pick or unpick.
      - Header rows raise nothing.
      - **Keep This Copy** applies decision 4's rule through a `DuplicatesUi` callback. It is the menu's one new string.
      - **Open Containing Folder** calls the existing `Nav.reveal-in-folder(track_id)`, as the track menu does (`track-context-menu.slint:248-255`), ending in `library::tracks::reveal_in_file_manager` (`library/tracks.rs:23`). It reuses the track menu's `@tr("Open Containing Folder")` and `folder_open` icon, so no new string or Rust is needed.
      - **Copy Path** calls the existing `CopyActions.copy-tracks("path", [track_id])`, which resolves the path through `library::clipboard` (`TrackField::Path`, `library/clipboard.rs:186`) and lands through `ui::clipboard::write`. Its label is the already-translated `@tr("Copy Path")`.
      - **Not the track menu.** Its other entries would cut into the open dialog: Edit Tags can move a copy between groups, a heart or rating changes what the merge carries, Play fights the playing copy's hold-back, and a queue or playlist edit changes what the removal remaps.
    - **Opening.** The open raises the dialog at once in a finding state and fills it behind a `DialogClaim` (`callbacks/mod.rs:79-103`).
    - Two steps on one kind is the Last.fm login's pattern (`dialog.slint:249-250`). One kind is needed because the Dialog can't open a second Dialog on top of itself.
11. **No undo.** An undo would have to un-sum counts and un-move playlist places on a keeper that may have moved on since. A removed copy comes back through the kept-out list instead, as a new track. `ui-patterns.md` keeps a confirmation wherever undo can't cover the removal, and the issue asks for exactly that step.
12. **Cost.**
    - Closed, both dialogs cost nothing.
    - A find holds about 24 B a track while it sorts (about 1.2 MB at 51k), plus the candidates' facts until they are grouped.
    - An open finder holds `DuplicateSet` at a few dozen bytes a copy, plus `PagedRows`' row cache.
    - An open kept-out list holds 8 B an entry plus the same row cache.

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

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-core/src/utils/trash_dir.rs` (new, P0) | `is_trash_dir`, which names a trash folder, and `in_trash_dir`, which finds one in a path. |
| `crates/melodia-store/src/media/ingest/scanner.rs`, `ingest/watcher.rs` | The walk skips a trash folder; the watcher's audio predicate rejects a path inside one (P0). |
| `crates/melodia-core/src/utils/fold.rs` | `fold_words`, beside `push_folded`: folded, each run of non-alphanumerics one space, trimmed. If nothing is left, the folded string. |
| `crates/melodia-core/src/entities/audio_quality.rs` (new, P1) | `AudioQuality`: the lossless rule and the keeper order. |
| `crates/melodia-core/src/entities/duplicates.rs` (new, P1) | `MatchKind`, `ReviewReason`, `DuplicateGroup`, `DuplicateCopy`, `GroupRemoval`; `KeptOutFile` (P4). |
| `crates/melodia-store/src/database/queries/track/duplicates.rs` (new, P1) | The keyset-paged pass-1 read and the pass-2 facts by id. |
| `crates/melodia-store/src/database/queries/track/full_rows.rs` | Loses `get_duplicate_tracks` and its two tests (`track_tests.rs:316,340`) (P1). |
| `crates/melodia-app/src/library/duplicates/mod.rs` (new, P1) | The library API: `find`; `apply` (P3); the kept-out count, ids, rows, `bring_back` and `bring_all_back` (P4). |
| `crates/melodia-app/src/library/duplicates/matcher.rs` (new, P1) | Pure grouping: key runs, collision split, duration clusters, kind, keeper. |
| `crates/melodia-ui/ui/globals/duplicates.slint` (new, P2) | `DuplicatesUi`: rows, step, busy, counts, callbacks; `KeptOutUi` (P4). |
| `crates/melodia-ui/ui/models.slint` | `DuplicateRow` (P2), `KeptOutRow` (P4). |
| `crates/melodia-ui/ui/components/dialog/duplicates-body.slint` (new, P2) | Both steps' bodies and the list's toolbar (Suggested picks, Clear picks). |
| `crates/melodia-ui/ui/components/dialog/duplicate-row.slint` (new, P2) | The header row and the copy row. |
| `crates/melodia-ui/ui/components/dialog/duplicates-menus.slint` (new, P2) | `DuplicateCopyMenu`; `KeptOutMenu` (P4). Both are built from `context-menu.slint`'s parts. |
| `crates/melodia-ui/ui/components/dialog/kept-out-body.slint` (new, P4) | The kept-out list, its row with Bring Back, Bring All Back and Copy All Paths. |
| `crates/melodia-ui/ui/components/dialog/dialog.slint`, `globals/dialog.slint` | Both kinds: width, `body-scrolls`, mount, Back on Confirm, primary label and gate, `accepted` arms (P2 to P4). |
| `crates/melodia-ui/ui/views/settings/duplicates-section.slint` (new, P2) | The Find row, and the kept-out row (P4). Mounted in `pages/library-page.slint` and its `has-matches`. |
| `crates/melodia-views/src/ui/duplicates/mod.rs` (new, P2) | `install`, the open behind its claim, the close teardown. |
| `crates/melodia-views/src/ui/duplicates/picks.rs` (new, P2) | `DuplicateSet`: the compact groups, the pick rule, suggested picks with the identical extras, Keep This Copy, cleared picks, picked count and bytes. |
| `crates/melodia-views/src/ui/duplicates/rows.rs` (new, P2) | `DuplicateRow`'s placeholder and fetch, the header's lead-copy lookup, the format label. |
| `crates/melodia-views/src/ui/duplicates/apply.rs` (new, P3) | Picks into `GroupRemoval`s, the call, the result line. |
| `crates/melodia-views/src/ui/duplicates/kept_out.rs` (new, P4) | `KeptOutRow`'s paged rows, Open Containing Folder, Bring Back, Bring All Back, Copy All Paths, the Settings row's count. |
| `crates/melodia-views/src/ui/util.rs` | `format_bitrate` and `format_bit_depth` beside `format_sample_rate`, replacing the copies in `now_playing/metadata.rs:14,16` and `callbacks/tags/open.rs:497,502` (P2, no behaviour change). |
| `migrations/20261102000000_excluded_files.sql` (new, P3) | The table and its folder index. |
| `crates/melodia-store/src/database/queries/excluded_files.rs` (new, P3) | Exclude (`pub(crate)`, the merge's), excluded among paths, under a folder, lift, forget unseen, count; ids in path order, rows by id, every path in path order, lift all (P4). |
| `crates/melodia-store/src/database/queries/track/duplicate_merge.rs` (new, P3) | `remove_duplicates`: merge, playlist places, exclusions, delete, renumber, tail. One transaction. |
| `crates/melodia-engine/src/player/engine/queue/removal.rs` (321) | `replace_ids`: spare to keeper in place, then an `order_changed` bump (P3). |
| `crates/melodia-app/src/library/duplicates/apply.rs` (new, P3) | Scan guard, playing hold-back, flush, queue remap, store call, bump, covers, rating write-back. |
| `crates/melodia-app/src/library/scan/mod.rs`, `tasks/file_event_processor/reconcile.rs`, `library/import.rs` | The exclusion checks, lifts and cleanup (P3). |
| `crates/melodia-core/src/utils/play_counts.rs`, `crates/melodia-app/src/tasks/play_count_flusher.rs` | A flush request and its answer (P3). |

Size headroom, measured: `app-window.slint` 749 lines (gains two export names), `globals/dialog.slint` 419, `components/dialog/dialog.slint` 294, `models.slint` 461, `queue/removal.rs` 321. None crosses 800.

## Phases

### Phase 0: Trash folders are never indexed

1. `utils::trash_dir`:
   - `is_trash_dir(&OsStr)` matches `.Trash`, `.Trash-` followed by a uid, `.Trashes`, and `$RECYCLE.BIN` compared without case.
   - `in_trash_dir(&Path)` checks every component of a path.
2. **The walk.** `collect_media_files` (`scanner.rs:47-77`) gives `WalkDir` a `filter_entry` that skips a trash folder below the root, so nothing under it is read.
3. **The watcher.** `classify_event` (`watcher.rs:187-238`) reads `is_audio_file` in every arm, so that predicate also rejects a path inside a trash folder. The rename arm's existing branches then do the rest, with no new arm:
   - a rename into a trash folder becomes a `Removed`;
   - a rename out of one becomes a `Created`;
   - events inside one are dropped.
4. **Existing installs need nothing extra.** A track already indexed from a trash folder leaves on its folder's next full scan: the walk no longer finds it, so `commit_scan`'s orphan purge removes it (`finish.rs:31-34`).
5. Docs: `library-data.md` gains "`utils::trash_dir` is the one trash-folder predicate", beside the audio-extension line.

Hand test, in a library folder:
- An audio file in a `.Trash-1000/files/` is not indexed by a rescan.
- One indexed there before this fix is gone after the rescan.
- With watching on, moving a track's file into it drops the track, and moving it back out adds it again.

### Phase 1: Find (no UI)

1. `fold_words` in `utils/fold.rs`.
2. `AudioQuality` and the duplicates entities in `melodia-core`.
3. The pass-1 and pass-2 queries in `queries/track/duplicates.rs`.
4. `library::duplicates::find` over `matcher`.
5. Delete `get_duplicate_tracks` and its two tests, which nothing else calls.
6. Docs: `library-data.md` gains the matching rule's anchor and "a fold for matching goes through `fold_words`".

Gate: clippy and tests. Nothing calls `find` until Phase 2, so there's no run.

### Phase 2: The review dialog (read only)

1. Lift `format_bitrate` and `format_bit_depth` into `ui/util.rs`. No behaviour change.
2. `DuplicatesUi`, `DuplicateRow` and the duplicates kind on the Dialog, with the Review step only. Continue stays disabled until Phase 3.
3. `ui/duplicates/{mod,picks,rows}.rs`: find on open, the list, suggested picks (identical extras in Review groups included), toggling, the never-every-copy rule, Clear and Suggested.
4. `DuplicateCopyMenu` on copy rows:
   - Keep This Copy, through a `DuplicatesUi` callback;
   - Open Containing Folder, through `Nav.reveal-in-folder`;
   - Copy Path, through `CopyActions.copy-tracks("path", …)`.
5. `DuplicatesSection` with the Find row in Settings ▸ Library.
6. Run `scripts/update-translations.py` and finish all 15 catalogs.
7. Docs: `ui-patterns.md` names the duplicates body under Inputs/Dialog, with the two-steps-on-one-kind contract.

Hand test, on the dev library:
- The 11 identical pairs list as Identical and picked.
- The 3 no-artist pairs list as Review and unpicked.
- The six same-tag songs do not group.
- Scrolling the list stays smooth.
- The copy menu:
  - Open Containing Folder opens the copy's folder, and the dialog stays open with its picks.
  - Copy Path copies the right path, with the Copied toast.
  - Keep This Copy on a four-copy group leaves only that copy unpicked, in a Review group too.
  - A left click still picks, and a header row raises no menu.
- A Review group holding an identical pair beside an other-release copy (two copies of one album file next to a compilation copy, made by hand): one of the pair is picked, and the compilation copy is not.
- RSS (`MELODIA_RSS_SAMPLE=1`) with the dialog open, then closed.

### Phase 3: Remove from library

1. The migration and `queries::excluded_files`.
2. The checks in the scan, the watcher and import, and the scan-time cleanup.
3. `remove_duplicates` with the merge rules from decision 5.
4. The flush request; `replace_ids` and the queue remap.
5. `library::duplicates::apply`.
6. The Confirm step's summary, the Remove button and the result line.
7. Translations.
8. Docs:
   - `library-data.md`: "A per-track column the user builds up joins `remove_duplicates`' merge", and "a new ingest path consults `excluded_files`".
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

### Phase 4: The kept-out list

1. `excluded_files`' ids in path order, rows by id, every path in path order, and lift all, plus the library API over them.
2. `KeptOutUi`, `KeptOutRow` and `DialogKind.kept-out-files`: the paged list, the row's Bring Back, Bring All Back, and Copy All Paths through `ui::clipboard::write`.
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
- Bring All Back returns them all after the rescan.
- Removing a library folder takes its entries with it.

### Phase 5: Tests

Tests are added where the landed change earns them, on Kenan's go.

## Cross-cutting

- **Defaults on a live app.**
  - Nothing runs unless you open a dialog.
  - Melodia moves and deletes no file.
  - The migration is additive and touches no shipped one.
- **Every new `@tr` follows the house rules.** Plurals use the `"{n} copy" | "{n} copies" % n` form, and a string Rust builds goes through a Slint opener or a `*Text` global, never a `[string]` filled from Rust.
- **Memory.** Idle RSS doesn't move. The Phase 2 RSS check confirms the open dialog's cost, and heaptrack is needed only if it surprises.

## Open questions

None open. Settled 2026-11-02:
- An identical pair in a Review group stays in its group, and one of the pair is picked (decision 2).
- Keep This Copy is in the copy menu (decisions 4 and 10).
- Copy Path is in both menus, and Copy All Paths is in the kept-out list (decisions 9 and 10).
