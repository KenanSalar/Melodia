# Plan: durable file writes (issue #175)

Status: **in progress** · Owner: Kenan · Created: 2026-11-24 · Issue #175, branch `fix/durable-file-writes`

> **Validated 2026-11-24** against `a2e45283`, rox `f95ccf4` (`rox-library/src/writer.rs`), and
> the locked sources: lofty 0.25.5, symphonia 0.6.1, tempfile 3.27.0, std 1.100.0, the SQLite in
> libsqlite3-sys 0.40.0, and `close(2)`.

Working doc. Delete when every phase has shipped and the Phase 2 costs are in the PR.

## Context

**What you see today.** Nothing: no one has reported a damaged file. A tag edit, a star rating
written into the file, lyrics saved into the file and a cue sheet edit all land through a copy
renamed over the track. Every settings and state file lands through a temp renamed over the
file. All of them survive Melodia crashing.

**What can happen.** Nothing flushes the copy before the rename. There is no `sync_all` or
`sync_data` in the tree, and `macros.rs:29` is the only hit for "fsync". The same pattern finds
rox's `writer.rs:786`, so the search does match real calls. As a result:

- A power cut or OS crash shortly after a save can bring the track back empty or cut short. It
  can also bring back an empty `settings.json`, which reads as defaults, and the next save writes
  those defaults over it.
- A library on a network share is worse: a write the share refuses (full, or over quota) can be
  reported only when the file is closed. lofty's handle is dropped and std ignores the close
  error (`fs.rs:58-60`), so the short copy replaces the track with nothing in the log. No power
  cut is needed.

**After.** The same saves, a little slower (measured in Phase 2).

- On Linux, a save that has returned survives a power cut.
- On Windows, a power cut brings back the old file whole or the new one, until Phase 3 decides
  on write-through.
- A save the disk or share refuses fails that file in the toast's count, and the original stays.
- A tag write whose copy would play differently is refused the same way.

**What keeps working:**

- editing tags on the playing track, with playback carrying on from the file it opened
- the watcher ignoring Melodia's own writes. Opens, closes and fsyncs raise no inotify event it
  subscribes to (`notify-9.0.0/src/inotify.rs:50-91`), and every flush lands on the temp before
  the rename.
- a failed write leaving the original untouched, with no temp file behind
- loudness carried through a tag edit, now checked rather than assumed
- every settings and state file reading back as it does today

## Is the issue right?

Mostly, and it goes further than rox, where the idea came from.

**What rox does.** It copies the file and writes the tags through its own handle. It then checks
the fields, the pictures and an FNV hash of the audio byte ranges. Finally it reopens the copy
for write, calls `sync_all` and renames (`writer.rs:746-789`).

**Where the issue is better than rox:**

- **It covers the settings and state files.** rox's settings writer renames without a flush
  (`rox-core/src/settings.rs:237-262`).
- **It flushes the folder after the rename.** rox doesn't.
- **It compares packets instead of hashing bytes.** rox writes MP3, FLAC and MP4 only
  (`writer.rs:1400-1424`), so hashing raw bytes works there. Melodia also writes Ogg, where lofty
  renumbers and re-checksums every page (`lofty-0.25.5/src/ogg/tag/write.rs:110-147`), so the
  bytes change when the audio hasn't.
- **It refuses the field-by-field read-back.** rox's check copies lofty's own rules, for example
  `number_form`, "Same parse it runs" (`writer.rs:1326-1330`), plus the rating at star resolution
  and a canonical genre. That is the trap the issue names.

**Corrections and additions:**

1. **The flush has to run on the handle that wrote the copy, not a reopen.** `close(2)` says
   write errors "are reported only on the final close()", and "A careful programmer ... may
   precede close() with a call to fsync(2)". lofty's `save_to_path` opens, writes and drops its
   own handle (`audio_file.rs:59-62`, `tag_ext.rs:84-91`). So when "`rename_over` flushes the
   copy" runs, the error it is meant to catch can already be gone. rox reopens the copy and has
   the same hole. lofty has a `save_to(&mut File)` for both `AudioFile` and `TagExt`, so Melodia
   can hand it the handle and flush that handle.
   - The copy's audio bytes still come from `fs::copy`, through std's own handle, which it drops
     (std `sys/fs/unix.rs:2302-2310`). On Linux the flush on the fresh handle still reports a
     writeback error no descriptor has seen (errseq, kernel 4.17). Windows is Phase 3's.
2. **The audio check must compare codec parameters too, not packets alone.**
   - The Vorbis setup header, which lofty re-packs, goes to `extra_data`, not to packets
     (`symphonia-format-ogg-0.6.1/src/mappings/vorbis.rs:244-255`).
   - So does the FLAC STREAMINFO block, and the AAC or ALAC config held inside an MP4.
3. **lofty has two bugs in 0.25.5 that the check has to catch, so it isn't hypothetical:**
   - **Ogg.** lofty truncates the copy to zero and then copies pages back in
     `while let Ok(page) = Page::read(..)` (`ogg/tag/write.rs:110-133`), so the first page it
     can't parse silently ends the file there.
   - **Fragmented MP4.** lofty's `tfhd` patch is dead code: `traf` is missing from
     `IMPORTANT_CONTAINERS`, only the first `moof` is visited, and the offset is read from the
     wrong place (`mp4/write.rs:24-32, 399, 411-415`). An absolute base offset therefore goes
     stale. Symphonia trusts that offset (`isomp4 stream.rs:232-259`), so a packet comparison
     catches it.
4. **ADTS frames like MP3 but fails differently.** A false sync is an error, not a resync, and a
   cut-short last frame is `Err(UnexpectedEof)` (`adts.rs:285-292, 308`). So the comparison ends
   where both sides end the same way, an error included, not only at end of stream.
5. **Windows rename.** std's fallback fires on any `ACCESS_DENIED`, and its comment frames it as
   handling read-only files (`sys/fs/windows.rs:1306-1367`). Whether a held-open target produces
   that error is up to Win32, so "falls back when the track is held open" stays for Phase 3 to
   confirm, as does `atomic_file.rs:130-133`, which makes the same claim.
6. **Resolved from the issue's "Not checked yet":**
   - lofty 0.25.5 no longer undoes unsync twice on ID3v2.4 (`id3/v2/read.rs:34`,
     `frame/read.rs:67-69`).
   - "FlushFileBuffers needs write access" no longer matters, because the handle lofty writes
     through is opened read and write.
   - `macros.rs:28-33`'s "rewrite and fsync a file" becomes true once Phase 1 lands, so it needs
     no edit.
7. **The same gap outside `atomic_file`:**
   - **Updater, Linux in-place swap.** `swap_in_place` (`install/swap.rs:176-199`) moves the
     binary to `.old` before renaming the staged file in. ext4's replace-by-rename cover only
     applies when the target name still exists, so it doesn't apply here, and `download.rs:246`
     only `flush()`es. A power cut just after an update can leave an empty binary.
   - **Desktop integration.** `write_if_changed` (`desktop_integration.rs:161-179`) is a
     hand-rolled copy of `write_with_sync`.
   - **Artwork store.** The store can keep a cover cut short under its hash name, because
     `persist_unless_exists` (`artwork/mod.rs:170`) never replaces an existing name. It is not
     folded in here: a flush per cover would slow every scan. It is a candidate for its own
     issue.
   - **Not a gap:** the migration backup. `VACUUM INTO` takes the source's synchronous level
     (`sqlite3.c:162417-162420`), so the backup is already flushed.
8. **Stale comment.** `rating_writeback.rs:69-73` says an overrun at shutdown "costs a track
   rewritten halfway". It actually leaves the original whole, with a hidden `.tmpXXXXXX` copy
   beside it.

## Structure

| File | Owns after this plan |
|---|---|
| `melodia-core/src/utils/atomic_file.rs` | the temp, the flush on the writing handle, the rename, the folder flush (`sync_parent`, public for the updater) |
| `melodia-audio/src/player/source/file_source.rs` (new, extracted) | the file `MediaSource`, optionally ending early |
| `melodia-audio/src/player/source/decode.rs` | `probe`, extracted from `open` |
| `melodia-audio/src/audio_match.rs` (new) | the side-by-side packet comparison and the trailing-tag bound |
| `melodia-store/src/media/ingest/tag_writer.rs` | lofty saves through the handle; the one helper that turns a verdict into an error or a warning |
| `melodia-store/src/media/cue/{embedded/mod.rs, write.rs}` | write through the handle; the FLAC sheet calls the helper |
| `melodia-app/src/services/updater/install/{download,swap}.rs` | flush the download; flush the folder after the swap |

**Duplication collapsed first:**

- `FileSource` is extracted before its second user exists.
- The probe call has one home, shared by `decode::open` and the check.
- `write_if_changed` becomes a call to `write_with_sync`.

**Files that grow (800-line cap):**

| File | Before | After |
|---|---|---|
| `atomic_file.rs` | 150 | 181 |
| `tag_writer.rs` | 492 | 516 |
| `decode.rs` | 415 | 425 |
| `audio_match.rs` | new | 192 |
| `file_source.rs` | new | 87 |
| `file_decode.rs` | 510 | 464 |
| `desktop_integration.rs` | | shrinks |

## Phases

### Phase 1: flush before the swap (done, hand-checked on Linux)

1. **`write_with_sync`.** Call `sync_all` on the temp's own handle after the `BufWriter` flush and
   before `into_temp_path`. A failure drops the `NamedTempFile`, which removes the temp.
2. **`rewrite_with_sync`.**
   - Hand the closure the copy opened for read and write (`&mut File`) and its path.
   - After the closure returns, `sync_all` that same handle, then `rename_over`.
   - Its doc names the reason: errors reported only at the writer's close, which `Drop` discards.
3. **Callers write through the handle.**
   - `tag_writer::save_tags` (`tag_writer.rs:423-431`, both arms) and `EmbeddedWrite::save`
     (`embedded/mod.rs:149-155`) move from `save_to_path` to `save_to`.
   - `replace_sheet` (`cue/write.rs:160-167`) truncates and writes through the handle.
4. **`rename_over` flushes the folder.** After the rename, on unix, it calls `sync_parent`:
   - It opens the folder and calls `sync_all`.
   - A failure is logged, not returned, since the rename has already happened.
   - `Unsupported` and `InvalidInput` log at debug. That is a filesystem that can't flush a
     folder, and `tags.rs:86-89` already guards a log that rotates at a size against a batch of
     thousands.
   - Anything else warns, through `describe`.
5. **Docs this phase makes wrong:**
   - `atomic_file.rs:1-3`'s header says what holds for a crash against a power cut, per platform.
   - `rating_writeback.rs:69-73`.
   - `.claude/rules/lofty.md`'s "Writing Metadata" bullet, which names `save_to_path`, becomes a
     pointer to `rewrite_with_sync`'s handle.
6. **Beyond the issue.** These are small and the same fix:
   - `download.rs:246` calls `sync_all` on the handle it wrote, off the runtime's workers.
   - `swap_in_place` calls `sync_parent` after its swap.
   - `write_if_changed` becomes `write_with_sync` under its blake3 skip.
   - Read `.claude/rules/updater.md` before touching the updater files.
7. **Gates.** fmt, then clippy `--workspace`. The `atomic_file_tests.rs` rewrite tests break on
   the closure's new signature and are fixed here.

### Phase 2: check the audio before the swap (done)

1. **Extract, inside `melodia-audio`:**
   - `FileSource` moves out of `file_decode.rs` into `file_source.rs`, with an end-of-audio
     bound, beside the extension `Hint` both of its users build.
   - `decode::probe` is split out of `decode::open`.
2. **`audio_match::compare(original, copy) -> AudioMatch`.** It returns `Same`,
   `Unchecked(reason)` or `Changed(AudioChange)`, where `AudioChange` is an `Error`.
   - **The bound.** Each file is bounded where a trailing ID3v1 (`TAG`, 128 bytes) and an APEv2
     footer before it start. That applies to every file: lofty writes those tags only on MP3 and
     AAC, and a file without them is bounded at its own length.
   - **The original won't open,** or has no audio track to compare. Return `Unchecked`, so its
     tags can still be fixed. No loudness row is at stake, since the loudness pass decodes
     through the same Symphonia (`loudness/mod.rs:119`).
   - **The copy won't open.** Return `Changed`.
   - **The tracks.** Same count, and per track the same id, time base, codec, sample rate,
     channels and `extra_data`.
   - **The packets.** Read in lockstep. Each pair must match in every field, and both sides must
     end at the same packet the same way.
   - **A chained Ogg** raises `ResetRequired` on both sides at each new link
     (`symphonia-format-ogg demuxer.rs:92-96`). The comparison compares the tracks again and
     carries on, so a later link is checked too.
   - **Memory.** Two packets at a time, nothing buffered per file.
3. **The caller.**
   - One `tag_writer` helper runs inside both lofty closures, after the save and before the
     flush, so a refused copy costs no flush.
   - `Changed` becomes `AppError::metadata(.., change)` and joins `TagEditReport.failures` like
     any failed file. No new UI string.
   - `Unchecked` logs a warning.
4. **Docs this phase makes wrong:**
   - `database/queries/loudness/mod.rs`: `carry_over` now names the check that makes its premise
     hold.
   - `melodia-store/Cargo.toml`'s note on the `melodia-audio` edge, and the crate doc's line on
     it. Both called the duration probe the one use, which the loudness types had already made
     wrong.
   - `melodia-audio/src/lib.rs`'s crate doc names the check.
   - `rating_writeback.rs`: the uncancellable write now includes the check.
   - `.claude/rules/lofty.md`: a new write path calls the check.
5. **Confirm by hand on real files (done).** The shapes below were run through `apply_to_file`
   from a scratch binary, then again in the app.
   - **The app run:** one Album edit over all 15 tracks of a test library folder, in the dev data
     folder. The folder held the shapes below, a FLAC carrying its sheet, and a copy of a real
     file in each of the eight formats, the MP3 playing.
   - **The result:** the toast read "13 tracks updated, 2 files failed", the two being the shapes
     refused below.
   - **The written files:** every one decodes in ffmpeg identically to its pristine copy.
   - **The playing track:** the deck played on from the replaced inode.
   - **The watcher:** it suppressed all 12 events and re-ingested nothing.
   - An Ogg file with an unparsable page and a fragmented M4A with an absolute `tfhd` offset are
     both refused. Each file is left byte for byte as it was, with no temp beside it.
   - A fragmented M4A carrying a `sidx`: Symphonia reads only its duration (`isomp4
     demuxer.rs:189-209`), so the check can't see it. Its references are relative and move with
     the fragments. The written copy decodes identically to the original in ffmpeg, whole and
     from six seek points, so it needs no refusal.
   - A multiplexed Ogg: lofty numbers every page with one counter across streams
     (`write.rs:123-133`). lofty refuses the file at the read ("file missing magic signature"),
     taking the second stream's first page for the comment header. The Ogg spec puts every
     stream's first page ahead of any header, so it is never written.
   - A chained Ogg keeps each link's pages consecutive. It is written, checked across both links,
     and decodes identically in ffmpeg.
   - Every `test-assets/silence.*` lofty can write reads as `Same` after a real write, and as
     `Changed` once its last tenth is cut.
6. **Measure in a release build, before and after each phase (done 2026-11-24).**
   - **How:** release builds of `0c2de974` (before), `c8df08f5` (Phase 1) and `dfe983f2` (Phase
     2), each in a detached worktree with its own `target/`. A throwaway hook ran the scenarios
     through `apply_tag_edit` and `ratings::set_rating`, the star-click path, after the boot scan
     settled, and `Instant` logs timed `run_write_pass` and the exit flush. Dev data folder, backed
     up and restored.
   - **Data:** a 12-track MP3 album (7.0-17.0 MB, median 12.1) and its 16-bit FLAC transcode
     (17.9-48.3 MB, median 31.7).
   - **Local disk:** btrfs on NVMe, where `fs::copy` reflinks. Files evicted from the page cache
     before each launch, so the first edit is cold and the next two warm. Two launches per build,
     interleaved; each cell gives both, or their range.

     | | Before | Phase 1 | Phase 2 |
     |---|---|---|---|
     | FLAC album edit, cold | 232, 236 ms | 227, 214 ms | 362, 376 ms |
     | FLAC album edit, warm | 110 ms | 115 ms | 173-182 ms |
     | FLAC per file, cold / warm (median) | 58-62 / 1-2 ms | 60-64 / 4-7 ms | 98-108 / 44-50 ms |
     | MP3 album edit, cold | 75, 83 ms | 90, 93 ms | 119, 123 ms |
     | MP3 album edit, warm | 43-44 ms | 47-48 ms | 52-54 ms |
     | One rating (FLAC write) | 51-97 ms | 53-100 ms | 78-157 ms |
     | 8 ratings at quit | 287, 300 ms | 287, 279 ms | 410, 417 ms |
     | Slowest file | 163 ms | 167 ms | 255 ms |

   - **Slow disk:** an ext4 image through `fuse2fs`, so a copy writes every byte, its process held
     by cgroup to 8 MB/s write and 25 MB/s read, a USB 2.0 stick's rates. One launch per build.

     | | Before | Phase 1 | Phase 2 |
     |---|---|---|---|
     | One rating, 17.9 / 33.2 / 27.9 MB FLAC | 1.0 / 1.5 / 1.1 s | 1.0 / 1.4 / 4.6 s | 3.3 / 5.7 / 4.6 s |
     | 8 ratings at quit | 0 of 8 written | 0 of 8 written | 0 of 8 written |
     | Left beside the tracks | 4 partial copies | 4 partial copies | 4 partial copies |

   - **Phase 1's flush costs almost nothing on btrfs:** the copy is a reflink, so the flush writes
     only the blocks the tag changed.
   - **Phase 2's check is the cost that shows:** it reads the original and the copy whole, about
     45 ms more per warm FLAC and 12 ms per warm MP3. A 12-track FLAC album edit goes from about
     110 to 175 ms warm.
   - **The budgets hold on a fast disk:** 8 ratings at quit take under 0.42 s against 3 s, and the
     slowest file under 0.26 s locally and 5.7 s on the slow disk, against the 30 s TTL.
   - **The exit flush overruns a slow disk in every build, before the branch too.** Reading 8 cold
     files whole at USB 2.0 rates already outlasts 3 s. The 8 tags go unwritten, the rows keep
     their stars, and each quit left 4 hidden partial `.tmp` copies of about 15 MB beside the
     tracks, one per write thread, which nothing removed. The tracks stay intact. Both phases
     lengthen each write, Phase 2 most, since it reads each file twice. Item 7 removes the copies.
   - **Not resolved:** Phase 1's flush cost on the slow disk, which showed on one file of three.
     `fuse2fs` passes a flush down by syncing its whole image, so what one flush waits for depends
     on what else is dirty.
7. **The exit removes the copies of writes still in flight (done).** No count cap fits 3 s on a
   slow disk, one file alone taking up to 5.7 s, and `process::exit` skips the temp's `Drop`.
   - `atomic_file` lists each temp while its write runs, and `main` calls `remove_unfinished`
     after the task budget, ahead of both ways out. It removes the listed temps and refuses any
     write that starts after. A slow disk costs those tags, as the cap already accepted, and no
     longer leaves copies behind.
   - Each temp is created outside the list's lock, so a slow create stalls no other write.
   - On the slow disk: 0 files left beside the tracks where every earlier build left 4, the log
     reading "Removed 4 unfinished write(s) at exit".
   - The cost: the first unlink waits behind the copies' queued writes, 8.14 s in a direct test
     on the slow disk, so the exit ran 7 and 11 s in two runs instead of 4. It falls after the
     window has gone, and only when the budget was overrun.
8. **Found on the way, and fixed: a scan could delete the rows of tracks a tag edit was writing.**
   - A directory read promises nothing about an entry renamed while it runs. On btrfs a
     rename-over during a read hid the file 5 times in 5, and a scan took the rows it missed for
     orphans and purged them, ratings and play counts with them.
   - It happened in one measuring launch: an album edit ran under the boot scan, which logged
     "Removed 2 orphaned tracks" for two files still there.
   - `WalkIndex` now confirms each file the walk missed with a `stat` before calling it gone. That
     covers tracks, kept-out files and cue sheets, and runs off the async workers.
   - Through the real scanner, with renames running under the walk: across 10 rounds the walk
     missed 274 of 3,000 files, and none came back as orphans.

### Phase 3: Windows (open, on a real machine)

- **Tag edit on the playing track.** Does `MoveFileExW` answer `ACCESS_DENIED`, so that std's
  POSIX fallback replaces the held-open file? Also with a third-party handle on the track (an
  antivirus scan, the Search indexer), where std's `FILE_SHARE_DELETE` doesn't apply: the edit
  should either land or fail cleanly, the original whole and no temp left.
- **The flush.** `FlushFileBuffers` on the read and write handle succeeds, and a refused write
  fails the save.
- **The copy.** `fs::copy` is `CopyFileExW`, whose handle Melodia never sees. Check that a write
  it can't land fails the save, rather than leaving a short copy the flush can't see.
- **`MOVEFILE_WRITE_THROUGH` for targets nobody holds open.** It needs a `MoveFileExW` of our own
  and the `Win32_Storage_FileSystem` feature, which the workspace doesn't enable
  (`Cargo.toml:302-311`). It can't cover the playing track. Decide whether it is worth having.
- **Attributes.** tempfile's Windows `keep` sets `FILE_ATTRIBUTE_NORMAL` (`tempfile
  windows.rs:81-90`). Check whether a hidden or read-only track keeps its attributes through a
  tag edit. This one is not new.

### Phase 4: Tests (open)

## Verification

- **Phase 1.** fmt, clippy `--workspace`, `cargo test --locked --workspace` green.
- **Phase 1, by hand.** In the dev data folder:
  - Edit tags on the playing track; playback carries on.
  - The watcher logs no re-ingest of it.
  - Settings survive a restart.
- **Phase 2.** The hand checks above, a tag edit on each of the eight formats, and the costs from
  item 6.
- **Phase 3.** The checklist above, run on Windows.
