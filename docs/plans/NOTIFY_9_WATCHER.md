# Plan: filter the watcher's open events in the kernel with notify 9 (#129)

Created: 2026-11-23 · Branch: `chore/bump-notify-9`

Status: Phase 1 **done** · Phase 2 **done** · Phase 3 **pending** · Phase 4 **pending**

Crate sources cited below are the extracted copies under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Line numbers into `watcher.rs` and
`watcher_tests.rs` are against `d2e0ee8c`, before Phase 1.

## Context

**What you see today.** On the 6,318-folder test library you see nothing: according to the issue,
no launch has logged an overflow. The case that could show up is a much bigger library, which the
issue estimates (from notify 8's source, without measuring it) at somewhere between roughly 8,000
and 16,000 folders. There the launch's own folder walk could fill the kernel's queue of 16,384
inotify events. The overflow becomes one redundant full reconcile and the "Library re-syncing"
toast (`settings.slint:321`) on every launch.

The half you can't see: notify 8 adds `IN_OPEN` and `IN_CLOSE_WRITE` to every watch
(`notify-8.2.2/src/inotify.rs:427-434`). So every open by a walk, a scan, a hash or playback queues
a kernel event, which `ChangeWatcher` (`watcher.rs:40-84`) then drops in the app. On top of that,
the 0.7 debouncer's thread wakes every tick (the 2 s timeout / 4) for the life of the process, even
when idle (`notify-debouncer-full-0.7.0/src/lib.rs:670-674`).

**Why.** notify 9 builds the inotify mask from an `EventKindMask`
(`notify-9.0.0/src/inotify.rs:50-92`). Debouncer 0.8's `new_debouncer` asks for
`EventKindMask::CORE` (`notify-debouncer-full-0.8.0/src/lib.rs:207,1353-1371`), and CORE leaves
`OPEN`, `CLOSE_WRITE` and `CLOSE_NOWRITE` out of the kernel mask (the crate's own test at
`inotify.rs:5400-5420`). The wrapper can't survive the bump anyway, because it implements
`Watcher::paths_mut`, which notify 9 removes (#705).

**What you'll see after.** Nothing on screen changes. Creates, edits, removals and renames reach
the library exactly as before, and an overflow still becomes one reconcile with the toast. The
changes are all measurable:
- no `inotify_add_watch` mask carries `IN_OPEN`;
- the launch walk no longer fills the queue with opens;
- the debouncer thread sleeps while idle (`lib.rs:1313`).

Two costs, both measured in Phase 2. notify 9's inotify loop keeps each watched path four times
where notify 8 kept it twice, the `watch_paths` set (#1094) and the watch's `reported_path` (#901)
joining the `watches` key and the descriptor's `paths` entry, now a one-path `Vec` (#1072). The
watch's metadata also doubles each `watches` entry (`notify-9.0.0/src/inotify.rs:110-134`,
`walk.rs:33-39`; `notify-8.2.2/src/inotify.rs:40-41`). The heap grows with the number of watched
folders, and neither those PRs nor the changelog mention memory. 0.8's file-ID cache adds a
per-directory children index (`file_id_map.rs:14-19`), which only Windows and macOS build: on
Linux, `RecommendedCache` is `NoCache` (`notify-debouncer-full-0.8.0/src/cache.rs:117-122`, and
the same in 0.7).

## Is the issue correct?

Mostly yes. Every mechanism it relies on checks out against the shipped sources. One reason is
wrong, three stale comments are missing from its list, and a few side effects aren't mentioned.

**Confirmed**
- notify 9.0.0 and debouncer-full 0.8.0 were both published on 2026-11-21 and are the latest
  versions (crates.io, checked 2026-11-23). Both need Rust 1.91, and the pin is 1.100
  (`Cargo.toml:26`).
- An overflow still arrives. `Q_OVERFLOW` becomes `Other` with the rescan flag and is handed on
  before the mask filter runs (`inotify.rs:590-593`). The debouncer then emits it ahead of every
  other event (`lib.rs:467-473`).
- notify-rs/notify#727 recovers directories only, not files (`inotify.rs:1122-1125`), so the
  `Create(Folder)` walk has to stay.
- A file created and removed inside one window now comes through as a `Remove`.
  `handle_removed` does nothing for a path with no row (`watch_batch.rs:393-397`).
- On Windows, a created entry is classed by a stat as `Folder` or `File`, and as `Any` only when
  the stat fails (`windows.rs:810-824`). FSEvents sends `Create(Any)` for an entry of no known
  type (`fsevent.rs:334`).

**Wrong or incomplete**
1. **`retarget`'s order is still load-bearing, for a new reason.** notify 9's `unwatch` does keep
   an ancestor's OS watches (#1097). But debouncer 0.8's `remove_root` (`lib.rs:1114-1134`) drops
   the cached file IDs under the path being unwatched, and re-adds only the roots nested *under*
   that path, never an ancestor that still covers it. #1028 fixes unwatching a parent, not a
   child.

   So if a child were unwatched after the parent replacing it had been added, the parent would
   lose its cached IDs for that subtree. Those IDs are what pair a Windows or macOS move between
   folders into a rename. Linux keeps no such cache, so the order matters only on Windows and
   macOS. The order stays as it is, and the comment is rewritten to this reason rather than
   deleted.
2. **Three more comments go stale.**
   - The comment on the `Create(Folder)` arm (`watcher.rs:215-216`) gives only inotify's reason,
     but notify 9 on Windows also reports a folder moved in as `Create(Folder)`, with nothing for
     the files inside it.
   - `watcher_tests.rs:253-254` says `Create(Any)` is how Windows reports a move between parents.
   - `watcher_tests.rs:289-291` says `Create(Folder)` is inotify's.
3. The issue says Windows now takes the per-file walk for a folder moved in "not from this
   change". It does come from this change: from the bump (notify 9's Windows create kinds), not
   from deleting the wrapper. The behaviour is the same, and Phase 3 checks it.
4. **Lockfile:** `inotify` will resolve to the newest 0.11.x, which is 0.11.5 on crates.io today,
   not the 0.11.4 the issue names. That version requires `inotify-sys` ^0.1.8
   (`inotify-0.11.5/Cargo.toml:76-77`), so `inotify-sys` also moves from 0.1.5 to 0.1.8; `inotify`
   is its only dependent. `xxhash-rust` is new and resolves to 0.8.21. `rustc-hash` 2.1.2, the
   debouncer's new dependency, is already locked.
5. **Not mentioned; checked, and no code change needed:**
   - A Linux unmount arrives as `Remove(Other)` with info "unmount" (`inotify.rs:380-384`), which
     `classify_event`'s `_ => {}` drops.
   - Windows now reports a watched root that disappears as `Remove(Folder)`
     (`windows.rs:951-957`), and macOS reports a root change as a removal or `From`
     (`fsevent.rs:288-307`). Either becomes `DirectoryGone`, which costs one reconcile, and
     `scan_one` refuses a folder that doesn't exist instead of pruning its rows
     (`scan/mod.rs:189`). Linux's `DELETE_SELF` already works this way.
   - A subfolder a recursive watch can't enter is now reported through the handler while the rest
     of the tree stays watched (#970, `walk.rs:15-22`). Melodia logs one `Watch error` per such
     folder. Running out of watches still fails `watch()` as it does today.
   - The new create-then-remove `Remove` for a non-library temp file on Windows becomes a
     `DirectoryGone`. The `holds_tracks_under` read drops it, at the cost of one read.

## Structure

- **`crates/melodia-store/src/media/ingest/watcher.rs`** still owns the watcher and
  `classify_event`. It loses `ChangeWatcher`, `SkipAccess` and the `Watcher` impl.
  `FolderWatcher.debouncer` becomes `Debouncer<RecommendedWatcher, RecommendedCache>`, built by
  `new_debouncer`.
- **Duplication collapsed in the blast radius.** The callback writes the `blocking_send` and the
  same "File event channel closed" warning twice (`watcher.rs:115-120, 127-132`). One small free
  function, `forward(tx, event)`, takes both. The overflow log line loses its em dash in the same
  edit.
- **`tests/watcher_tests.rs`** loses the "Access events, dropped ahead of the debouncer" section
  (`:348-426`: `passed_on` and its four tests), along with `SkipAccess`. These stay:
  - `classify_access_skipped`, since `classify_event` still ignores access events;
  - the end-to-end `a_file_appearing_under_a_watched_folder_reaches_the_channel`, which now runs
    against the CORE mask.
- **Root `Cargo.toml:142-143`** gets the new versions. `melodia-store/Cargo.toml` inherits them
  through `workspace = true` and doesn't change.
- **Nothing else names the wrapper**, `IN_OPEN` or the debouncer constructors (`rg --hidden` from
  the repo root, with `watcher.rs` as the known positive). No doc, rule or ADR goes stale. Both
  touched files shrink.

## Phase 1: bump and delete the wrapper (one commit) [done]

1. In `Cargo.toml`, set `notify = "9.0.0"` and `notify-debouncer-full = "0.8.0"`, then run
   `cargo update -p notify -p notify-debouncer-full`. Read the lockfile diff and expect exactly
   these moves:
   - notify 9.0.0, notify-debouncer-full 0.8.0;
   - notify-types 2.3.1, inotify 0.11.5, inotify-sys 0.1.8;
   - xxhash-rust added.
2. In `watcher.rs`:
   - delete the wrapper and trim the imports;
   - build with `new_debouncer(Duration::from_secs(2), None, …)`;
   - add `forward`;
   - rewrite three comments:
     - `retarget`'s, to the cache reason in finding 1;
     - `relocates_an_audio_directory`'s Windows sentence: Windows reports `Create(Folder)` after a
       stat and `Create(Any)` only when that fails, and FSEvents sends `Create(Any)` for an entry
       of no known type;
     - the `Create(Folder)` arm's, widened to both reasons: a folder moved in on Windows, and
       files written before inotify's watch.
3. In `watcher_tests.rs`, delete `:348-426` and fix the doc comments at `:253-254` and
   `:289-291`.
4. Gates:
   - `cargo fmt --all`
   - `cargo clippy --all-targets --locked --workspace -- -D warnings`
   - `cargo test --locked --workspace`
   - the comment audit from `code-style.md` / `/doc-style`.

## Phase 2: Linux checks on the test library [done]

You run these by hand, or Claude runs them unattended once you say go. They use the `Melodia-dev`
data folder, with its usual backup and restore.

1. Build release binaries of `d2e0ee8c` (notify 8) and `1dba0b77` (notify 9), each in its own
   detached worktree, and `cmp` them before measuring. They differ by Phase 1 alone, where `main`
   and the branch tip carry other bumps too. Wrap every launch in `systemd-run --user --scope`.
2. Read each launch's watch masks from `/proc/<pid>/fdinfo/<fd>` for its `anon_inode:inotify`
   fd (strace isn't installed here). `main` should show `IN_OPEN` (0x20) and `IN_CLOSE_WRITE`
   (0x8), and the branch should show neither. Phase 1 already read the branch's mask this way from
   the end-to-end watcher test's binary: `0xfc6`, which is `MODIFY ATTRIB MOVED_FROM MOVED_TO
   CREATE DELETE DELETE_SELF MOVE_SELF`.
3. Create, modify, rename and remove a probe audio file in a library folder. Each change should
   reach the library ("Processing batch of N file events" in the log, and the row in the UI).
4. Memory: run `heaptrack --record-only` over a launch with an incremental scan only (never a full
   scan) for both builds. Compare the peak heap and `/usr/bin/time -v` peak RSS. Linux builds no
   `FileIdMap`, so its cost needs a scratchpad harness instead. The harness calls
   `FileIdMap::new()` and then `add_path(root, RecursiveMode::Recursive)` over the test library. It
   is built once against 0.7 and once against 0.8, each under heaptrack. Both types are `pub`, and
   `file-id` reads inodes on Linux.
5. Idle: read `voluntary_ctxt_switches` for the debouncer thread from `/proc/<pid>/task/*/status`
   60 s apart on both builds. The thread's 15-byte name is `notify-rs debou`.

**Results** (2026-11-23, one plain and one heaptrack launch per build):
- **Masks:** 6,320 watches in each build. notify 8 sets `0x3ee` on folders and `0xfee` on the three
  roots, notify 9 `0x3c6` and `0xfc6`: the difference is exactly `OPEN | CLOSE_WRITE`. No launch
  logged an overflow.
- **Probe:** create, modify, rename and remove each arrived as one batch of one event. The row, read
  from `melodia.db` rather than the UI, took the new title, kept its id and `date_added` through the
  modify and the rename, and was gone after the remove.
- **Idle:** the debouncer thread made 120 voluntary switches in 60 s on notify 8 and 0 on notify 9,
  in both pairs. The inotify thread made 0 in every launch.
- **Heap:** peak 36.61M against 38.43M. Frames under `notify::` hold 2,083,800 B against
  3,969,608 B, all still held at exit. A harness running the bare watcher over the same roots
  reproduces both totals within 0.3%. notify 9's extra 1,887,110 B, about 300 B per watched
  folder, is two more path copies 1,083,742 B, the wider `watches` table 393,216 B, the
  `watch_paths` tree nodes 256,384 B and the per-descriptor `Vec` 151,680 B. The harness's
  `RssAnon` grows by that live heap alone (+2,304 kB against +4,364 kB), malloc's free chunks
  not growing. In the app, idle `RssAnon` rose 2.3 MiB in the plain pair and 2.0 MiB in the
  heaptrack pair. Peak RSS (502,556 kB against 510,440 kB) can't separate the builds: it follows
  the database's file-backed pages, and only notify 9's run carried the probe.
- **FileIdMap harness** over the three roots: peak heap 23.12M on 0.7 against 19.48M on 0.8.

## Phase 3: Windows checks [pending]

You run these on a Windows machine.

1. A folder of audio moved in from outside the library adds its tracks.
2. A folder moved between two parents inside the library lands at the new path with its tracks'
   history (play counts, ratings).

## Phase 4: Tests [pending]

This waits for your hand test and a go that names the tests.

## Verification

Phase 1 is done when the four gates pass. Phases 2 and 3 are the behavioural evidence. When Phase
4 lands, this doc is deleted, and the issue is linked in the PR's Development sidebar (no closing
keyword).
