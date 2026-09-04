# Architecture decisions

Why Melodia is built the way it is. One decision per file, numbered, and the title states what
was decided rather than what it is about, so this index reads without opening anything.

These are for people. `CLAUDE.md` and the files under `.claude/rules/` are written for an agent,
loaded on demand by a path glob and phrased as prohibitions because that is what works on one.
`README.md` is written for someone installing the app. This directory is for a contributor, or
for whoever comes back to the repository in two years and asks why.

## The index

| ADR | Status |
| --- | --- |
| [1. Record architecture decisions here, and argue everything else at its anchor](0001-record-architecture-decisions.md) | Accepted, 2026-09-04 |
| [2. A native Rust desktop app, after the Tauri version](0002-native-rust-desktop-app.md) | Accepted, 2026-05-25 |
| [3. Slint for the UI, and FemtoVG under it](0003-slint-and-femtovg.md) | Accepted, 2026-05-25 |
| [4. SQLite through sqlx, one writer and a pool of readers](0004-sqlite-through-sqlx.md) | Accepted, 2026-05-25 |
| [5. One tokio runtime, direct calls, and no IPC](0005-one-runtime-no-ipc.md) | Accepted, 2026-05-25 |
| [6. AGPL-3.0-or-later, and what every artifact then owes](0006-agpl-and-what-artifacts-owe.md) | Accepted, 2026-05-25 |
| [7. Decode through Symphonia and cpal directly, not rodio](0007-symphonia-and-cpal-not-rodio.md) | Accepted, 2026-08-20 |
| [8. Playback sources are capability-shaped, not a source-kind enum](0008-capability-shaped-playback-sources.md) | Accepted, 2026-08-20 |
| [9. Crossfade on two decks, with the ramp innermost and the curve linear](0009-two-decks-and-a-linear-ramp.md) | Accepted, 2026-07-11 |
| [10. The DSP chain is one wrapper in a fixed order, with lock-free state](0010-one-dsp-wrapper-lock-free-state.md) | Accepted, 2026-06-16 |
| [11. AAC encoder delay is read out of the file, not switched on](0011-aac-trim-read-from-the-file.md) | Accepted, 2026-09-02 |
| [12. The station directory is radio-browser.info](0012-radio-browser-as-the-station-directory.md) | Accepted, 2026-08-20 |
| [13. Stations are one table in the library database, not a JSON file](0013-stations-in-the-library-database.md) | Accepted, 2026-08-20 |
| [14. The network never touches the audio callback thread](0014-no-network-on-the-audio-callback.md) | Accepted, 2026-08-20 |
| [15. Radio ships off, and the guard is at the facade rather than the UI](0015-radio-ships-off-guarded-at-the-facade.md) | Accepted, 2026-08-20 |
| [16. Fourteen crates, and the compiler holds the direction](0016-workspace-graph-compiler-enforced.md) | Accepted, 2026-09-03 |
| [17. No crate re-exports another member's items](0017-no-cross-crate-re-exports.md) | Accepted, 2026-09-03 |
| [18. What is deliberately not split, and why there are zero Cargo features](0018-what-is-not-split-and-zero-features.md) | Accepted, 2026-09-03 |
| [19. The updater's trust boundary is the GitHub repo](0019-updater-trust-boundary-is-the-repo.md) | Accepted, 2026-05-25 |
| [20. Artwork is content-addressed and swept, never reference-counted](0020-artwork-swept-not-refcounted.md) | Accepted, 2026-08-19 |
| [21. Lyrics live in the file, not in a column](0021-lyrics-live-in-the-file.md) | Accepted, 2026-07-20 |
| [22. Smart playlist membership is derived at read time](0022-smart-playlists-derived-at-read-time.md) | Accepted, 2026-07-08 |
| [23. Scrobbling is a read-only tap on the player, decided by a pure function](0023-scrobbling-is-a-read-only-tap.md) | Accepted, 2026-07-22 |
| [24. Discord presence speaks its IPC directly, with no new dependency](0024-discord-ipc-is-hand-rolled.md) | Accepted, 2026-07-24 |
| [25. A rotating log file and a crash hook that survives `panic = "abort"`](0025-flexi-logger-and-a-crash-hook-under-abort.md) | Accepted, 2026-08-06 |
| [26. The backdrop is bounded washes of the cover's colours over the theme's own base](0026-aurora-bounded-washes-over-the-theme-base.md) | Accepted, 2026-08-17 |
| [27. One instance per data directory, and binding the socket is the claim](0027-one-instance-per-data-directory.md) | Accepted, 2026-08-15 |
| [28. No `unsafe` outside platform FFI, and no `unwrap` anywhere](0028-no-unsafe-outside-ffi-no-unwrap-anywhere.md) | Accepted, 2026-05-25 |
| [29. A vendored winit fork, checked in, with a stated retirement](0029-vendored-winit-fork.md) | Accepted, 2026-05-25 |
| [30. Memory is a product requirement, and three knobs hold it](0030-memory-is-a-product-requirement.md) | Accepted, 2026-05-25 |
| [31. Melodia ships its own packages, in five formats](0031-five-package-formats.md) | Accepted, 2026-08-12 |
| [32. A release is a pushed tag, not a merge](0032-release-from-a-pushed-tag.md) | Accepted, 2026-08-28 |
| [33. The artwork crate depends on the UI toolkit, and stores its buffers](0033-artwork-depends-on-slint.md) | Accepted, 2026-09-03 |

## The shape

```markdown
# ADR 7: Decode through Symphonia and cpal directly, not rodio

**Status:** Accepted, 2026-08-31

One or two sentences of what was happening: the problem in the reader's terms,
before any mechanism.

Decision: what we are doing, in a sentence or two.

Alternatives: what we are not doing, as a comma list.

Trade: one paragraph carrying both halves, why it won and what it costs.
```

That is Nygard's four parts compressed until they fit on a screen. The opening paragraph is his
Context and it is not optional: half the decisions recorded here were forced by something that
broke, and without the symptom the decision reads as arbitrary.

## What keeps it from becoming a form

- **The title states the call.** "Decode through Symphonia and cpal directly, not rodio", never
  "Audio stack".
- **`NNNN-slug.md`, sequential, and numbers are never reused**, including for one that ends up
  rejected. A rejected ADR keeps its number and its status.
- **The status carries the date the decision was made**, not the date the file was written. A
  bare `Accepted` cannot be told apart from a current one.
- **The statuses are `Proposed`, `Accepted`, `Superseded by ADR N`.**
- **An accepted ADR is never edited to reverse itself.** A fact that moved while the decision
  still stands gets a dated `**Amendment:**` paragraph at the end. A decision that reverses gets
  a new ADR, and the old one gets `Superseded by ADR N` and is otherwise left exactly as written.
  This is the property no other tier here has: `CLAUDE.md` is rewritten in place, so a paragraph
  that used to say something else leaves no trace, and a deleted plan doc leaves less than that.
- **Every ADR names what it costs.** One with only upside in it is an advertisement, and that is
  the most common way a collection like this goes bad.
- **The alternatives were really on the table.** A loser invented so the winner looks better is
  worse than no alternatives at all.
- **One page.** A decision that genuinely needs a survey or a diagram is a working doc under
  `docs/plans/`, and the ADR links to it and stays short. Nothing else holds the length down.
- **Where a test holds the decision, name it in a clause.** It is the only evidence an ADR can
  carry that its decision is still in force rather than merely still written down.
- **No other application is named.** Not as a comparison, not as precedent, not as reassurance.
  What another program does is not a reason, and an ADR leaning on one has not stated its own.
  Prior-art surveys stay in the working doc where they were done.
- **An ADR written after the fact says so.** Most of the low numbers here were reconstructed from
  the code, from plan docs already deleted, and from commit messages. A reconstructed context
  read as a contemporaneous one is the failure this collection is most exposed to.

## When not to write one

An ADR is for a decision that is hard to reverse, whose consequences are spread across the tree
rather than concentrated in one file, and that has a live alternative someone will eventually
re-propose. Library and toolkit choices, threading and data-flow shapes, trust boundaries, the
licence, what the crate graph may not reach.

Not a tuning constant, which is argued in the doc comment on the constant. Not a contract
spanning two trees, which is a `.claude/rules/` entry. Not a prohibition violable from anywhere,
which stays in `CLAUDE.md`. Not a feature's implementation shape, which is a working doc under
`docs/plans/`. And never a record of what changed, because an ADR is not a changelog.

The bar matters in both directions. A collection that documents the formatter choice and skips
the datastore is worse than no collection, because it teaches people not to look.

And one caution learned here rather than read anywhere. **A decision resting on a claim about
what is possible is not ready to be recorded.** Radio carried segmented stations as unplayable on
the strength of "there is no demuxer for this", which turned out never to have been the real
constraint, and the decision was superseded before it was ever written up. Had it been an ADR the
claim would have travelled onward under the record's own authority, which is worse than it
travelling in a working doc that everyone knows is provisional. Wait until the constraint you are
deciding against is the one that actually binds.
