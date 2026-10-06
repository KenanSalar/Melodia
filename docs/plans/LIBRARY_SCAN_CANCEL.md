# Cancel a library scan

Working doc. Delete when the feature ships.

Status: **implemented, awaiting the manual pass** · Created: 2026-10-06

Checked on 2026-10-06 against the tree at `0c2748e6` and the prior-art checkouts beside it
(`../fooyin`, `../rox`, `../strawberry`).

## What we see

- Adding a large folder in Settings ▸ Library, or letting the first-run card add `~/Music`, starts a
  scan that can run for minutes, and nothing on screen stops it. Add Folder, Rescan and Remove are
  disabled for the whole scan, so the folder the user regrets adding cannot even be taken back.
  The onboarding music step shows the same bar with the same lack of a way out.
- The first part of a big scan shows nothing at all. The directory walk publishes no progress, so
  a large tree looks as if nothing happened, and the buttons stay live through it, which lets a
  second scan start on top of the first.
- Quitting mid-scan waits out shutdown's 3 s budget and abandons the scan wherever it is.

## What ships

- A **Cancel** button beside the progress bar in both places, there from the first frame of the
  scan. The walk shows "Looking for music… N files found".
- Pressing it shows "Stopping…" for no longer than writing what was already read takes, then the
  bar goes away.
- Every track already read stays in the library, playable, ratings and play counts intact. Nothing
  is deleted. The folder stays listed without a "scanned" stamp, and the next scan of it (a Rescan,
  or the reconcile at the next launch with folder watching on) reads only the rest, the size and
  mtime gate skipping what is stored.
- Quitting mid-scan takes the same path, so it stops promptly instead of being abandoned.

## Prior art

| | committed work on cancel | prune after cancel | "scanned" stamp | confirm |
|---|---|---|---|---|
| fooyin | kept (flushed 250-row batches) | skipped | n/a (in-memory status) | no |
| rox | kept (stops at a 512-file batch) | skipped, pinned by a test | not updated | no |
| Strawberry | the whole folder discarded | skipped | n/a | no |

Melodia follows fooyin and rox. Strawberry's choice throws away minutes of work on exactly the
folder a cancel is most likely to be pressed on.

## Findings

- `scan_folder_internal` checks no flag. The walk, the incremental filter and the parse run inside
  `spawn_blocking` plus a rayon `ScanPool`, neither of which can be aborted from outside. The only
  check is `reconcile_watched_folders` reading the shutdown token between folders.
- Add Folder awaits the whole scan inside `slint::spawn_local` on the UI thread; Rescan uses an
  untracked `runtime.spawn`. Neither keeps a handle.
- The progress guard and first publish come after the walk, which is the silent phase.
- **The orphan purge compares against the walk**, so a purge after a cut-short walk deletes every
  row the walk didn't reach. A cancel must never purge.
- **On the bulk path every chunk commits with the stats triggers dropped**, so stage 2's recalc is
  what makes the committed rows' artist, album and genre counts right. A cancel still runs it.
- Walk errors are dropped (`filter_map(Result::ok)`), so a subtree that fails to list is purged as
  orphans by the next completed scan. Pre-existing; fixed in Phase 5.
- `add_folder` never registers the folder with the watcher. Pre-existing; not in this plan.

## Structure

| file | owns |
|---|---|
| `melodia-app` `state/scan.rs` | `ScanControl`: the progress `watch` and the cancel epoch (a child of the shutdown token). `cancel()` stops every scan running at that moment; a scan started afterwards starts clean. |
| `melodia-app` `library/scan/mod.rs` | The door: `scan_folder`, `start`, `cancel`, `reconcile_watched_folders`, `ScanOutcome`, and the orchestration. |
| `library/scan/run.rs` | `ScanRun`: one folder scan's token and reporting, the store's `ScanObserver`, clearing the bar on drop. |
| `library/scan/finish.rs` | Stage 2, and `orphans_to_purge`, the one place deciding whether rows may be deleted. |
| `library/settings/folders.rs` | Validation, add, remove, list, watching. No scan. |
| `melodia-store` `media/ingest/scanner.rs` | `ScanObserver` + `Unobserved`; a walk returning `None` when stopped; a parse skipping the rest once cancelled. |
| `components/settings/scan-progress.slint` | Bar, status line and Cancel, mounted by the Library card and the onboarding music step. |

Collapsed on the way: the duplicated progress block, `scan_folder` beside `scan_folder_internal`,
the two error paths for Add and Rescan, Rust's `progress_fraction`, the unused
`scanner::ScanProgress`, the never-read `ScanProgressTick.folder_id`, and two stale comments.

## Phases

- [x] **1. Store seam.** `ScanObserver`, cancellable walk and parse, callers and existing tests on
  the new signatures.
- [x] **2. `ScanControl` and `library::scan`.** The move, the checkpoints (walk, filter, chunk head,
  per file in the parse), "what was read is written", stage 2 on both arms with no purge after a
  cancel, no stamp or post-scan spawns on a stop. First launch skips its reconcile after a stopped
  auto-scan.
- [x] **3. UI.** `ScanPhase`, derived `scanning` and `scan-progress`, `cancel-scan`, the shared
  component, three msgids in six catalogues.
- [x] **4. Rust UI wiring.** The subscriber, Add and Rescan through `start`, Cancel through
  `cancel`.
- [x] **5. Walk errors.** The walk returns what it could not list; the purge skips rows under it.

Gates green on 2026-10-06: `cargo fmt --all --check`, the clippy gate, `cargo test --locked
--workspace`, `scripts/check-icons.py`.

## Cross-cutting

- Gates: `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings`,
  `cargo test --locked --workspace`.
- Manual pass: cancel during the walk (nothing lands, no stamp); cancel mid-read (partial tracks
  play, Artists and Albums counts right, Rescan finishes the rest); cancel during onboarding (no
  rescan on the same boot); quit mid-scan (prompt exit, next launch resumes).
- After the manual pass, on the go: new tests (a cancelled scan purges nothing, a partial chunk is
  ingested, a cancel during the walk writes nothing, first launch skips its reconcile after a stop)
  and the `library-data.md` scan bullet.

## Open questions

- None blocking. A newly added folder going unwatched until the next launch wants its own fix.
