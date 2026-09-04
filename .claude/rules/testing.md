---
paths:
  - crates/*/src/**/tests/*.rs
  - crates/melodia/tests/**/*.rs
  - crates/melodia-testkit/src/**/*.rs
  - crates/melodia-store/src/database/queries/fixtures.rs
  - crates/melodia-engine/src/player/engine/fixtures.rs
  - crates/*/Cargo.toml
  - .cargo/config.toml
---

# Testing: where a test goes, and what it may say

How the suite *runs* is not here. `.claude/rules/ci-packaging.md` owns the gate, the
headless-audio shim, the memory caps and every `llvm-cov` flag, and it is scoped to
`.github/**`, so it never loads while a test file is open. Three siblings own a slice each
and none is re-argued below: `unsafe-rust.md` has the whole env-var contract
(`with_env_set`, `reading_env`, one lock per test binary), `tokio.md` has the
`#[tokio::test]` flavours and the paused clock, `rust-performance.md` has criterion and the
profilers. What is left, and what this file is, is the half you need with the test in front
of you: which of the three homes it belongs in, which helper already exists, what a walk
over the corpus owes, and what an assertion may contain under a lint set that denies
`unwrap`.

## Where a test goes

**A unit test is a `tests/` subdir beside the module, declared as the source file's last
item.** Never an inline `#[cfg(test)] mod tests { … }`.

```rust
#[cfg(test)]
#[path = "tests/<name>_tests.rs"]
mod tests;
```

The cfg can carry a platform (`#[cfg(all(test, target_os = "linux"))]` on
`themes/kde.rs`). Two files spell an inline `mod tests` legitimately, and both hold a
shared *helper* rather than a test body: `player/source/mod.rs` and
`player/playback/mod.rs`, each a `#[cfg(test)] pub(crate) mod tests { pub(crate) mod
helpers; }` declaring the one file its crate's suites share. `tasks/updater_daily.rs` is an
un-migrated holdout and not a third precedent.

**A check that enumerates a corpus lives in `crates/melodia/tests/`. A pin on one named
file lives beside that file.** This is what keeps `cargo test -p <crate>` a question about
that crate: a walk left inside one compiles the whole tree to answer a question about
neither, which is what `melodia-net`'s did. The binary crate's `tests/` is the only one
outside `src/` and every walk in the tree is in it.

The workspace split made the boundary mechanical rather than stylistic. A pin needing to
`include_str!` two crates' sources cannot be a unit test at all, because no crate can reach
both, and `services/tests/view_state_tests.rs` carries the worked case: the nav-index bound
has a write clamp in one crate and a read guard in another, so both halves went to
`crates/melodia/tests/cross_tier.rs` and only the round trip stayed behind.

**A fixture two crates share is `#[doc(hidden)] pub`, never `#[cfg(test)]`.** A `cfg(test)`
item cannot cross a crate boundary, and `lto = "fat"` is what makes the shipped binary pay
nothing for the visibility. There are three: `queries::fixtures`,
`player::engine::fixtures` and `DbPool::test_pool`. `crates/melodia/tests/common/mod.rs`
exists for the mirror-image reason, an integration binary being unable to reach a
`cfg(test)` module in the other direction, and each binary picks it up with a plain
`mod common;`.

## Reach for what exists

| you need | it is already |
|---|---|
| the repo root, the Slint tree, the Rust wiring tree, the rules dir, the shared assets | `melodia_testkit::{REPO_ROOT, UI_DIR, UI_SRC_DIR, RULES_DIR, ASSETS_DIR, FONTS_DIR}` |
| every Rust source, comment-stripped and path-tagged | `rust_sources()` |
| the same over one directory | `stripped_sources(root, ext, floor)` |
| every view slice's wiring | `callback_sources()`, checked against `CALLBACK_HOMES` |
| who spells a needle, minus an exemption list | `spellings_outside(needle, exempt)` |
| whether two offsets share a brace scope | `depth_between`, `block_body`, `blocks_named` |
| a throwaway image on disk | `write_test_png` / `write_test_jpeg` / `write_test_jpeg_sized` |
| an in-memory database, migrations applied | `DbPool::test_pool()` |
| a library with rows in it | `setup_seeded_db()`, `insert_test_track`, `make_test_metadata` |
| an `AudioSource` with known samples | `player::playback::tests::helpers::TestSource` |
| float comparison that will not trip `float_cmp` | the same module's `approx_eq`, `assert_approx`, `bits` |
| a `Shape` from two integers | `helpers::shape`, or `tests/common/mod.rs`'s for an integration binary |
| a real audio file in a format you do not have | `test-assets/` at the repo root, reached through `ASSETS_DIR` |

`crossfade_tests` deliberately takes none of the DSP helpers: it is pure predicates at a
tighter tolerance than `approx_eq` allows, and reaching for the shared one would loosen it.

**`melodia-testkit` is a leaf and may name no workspace member.** Building a member's test
target compiles that member a second time under `cfg(test)`, so a value handed back by a
testkit linked against the plain rlib is not the type the test names. Two helpers that did
name workspace types were deleted rather than carried. A helper that needs a workspace type
belongs in that crate as a `#[doc(hidden)] pub` fixture instead.

A dev-dependency is asked for per crate, not inherited. `--workspace` unifies features and
`cargo test -p <crate>` does not, so `melodia-integrations` declares its own
`tokio/test-util` for one paused-clock test rather than reaching another member's.

## What a corpus walk owes

**A vacuity floor.** A walk that finds nothing passes, and every pin standing on it passes
with it, which is the exact failure the floor exists to catch. `MIN_SOURCES`,
`MIN_UI_SOURCES` and `MIN_SLINT_SOURCES` are shared so ten pins cannot disagree about how
much of a corpus has to be there; a walk over a narrower tree declares its own
(`MIN_FACADE_FILES`, `MIN_MEMBERS`). Keep them loose. One tight enough to be interesting
trips on an ordinary file deletion.

**An equality wherever a vanishing subtree is the failure.** A floor cannot see one slice
stop existing, because every count-based pin over the corpus quietly loses that slice's
coverage and still passes. `CALLBACK_HOMES` is the worked example and says so at its
definition.

**Unreadable paths collected and asserted empty, never skipped.** A path that would not
read is indistinguishable from one that holds nothing.

**Comments stripped before the needle, and brace-matched scope where a substring would
lie.** `crates/melodia/tests/scrollbars.rs` lifts each scroller's body before reading its
policy, so a nested `TrackList`'s opt-out cannot be borrowed by the scroller around it.

**Exemptions held to an exact count.** `spellings_outside` asserts the number rather than
forgiving the file, since a second call written into a sanctioned site is itself the
regression, and it asserts every exemption still matches something so a stale entry cannot
sit there.

**The walk lives outside the tree it walks**, which is what retires the self-exemption it
would otherwise owe as its own first hit. Two pins had bent their needle around that before
the walks moved, one splitting it as `concat!("Result", "<")`.

## Assertions under this lint set

`unwrap_used` is denied and `expect_used`, `panic`, `print_stdout` and `print_stderr` are
warned, all crate-wide with no test exemption, and CI's `-D warnings` promotes the four
warnings. There is no `.unwrap()` in the tree; the single grep hit is the word inside a
comment.

- **Default to `-> Result<(), AppError>` and `?`.** It is the only shape that needs no
  suppression at all, and it is what every test doing fallible setup already uses. An
  integration binary with no `AppError` in scope returns `std::io::Result<()>` instead.
- **`expect()` needs `#[expect(clippy::expect_used, reason = "…")]` on the narrowest item
  that covers it.** Per function where one setup step can fail; file-level `#![expect(…)]`
  only where the whole file is one argument, as in `ui/search/tests/top_result_tests.rs`,
  whose reason is that a missing Top Result *is* the assertion. The message names the
  invariant, never the call.
- **`clippy::panic` is denied, so `let … else { panic!() }` is not available.** Two legal
  shapes replace it: `unreachable!("<why the fixture guarantees it>")` where the fixture
  really does, and an `assert!(x.is_some())` followed by `unwrap_or_default()` where it is
  the assertion you want to read on failure.
  `hero_backdrop_tests.rs`'s `the_detail_gate_is_the_live_tab_read_after_the_drill_lands`
  is the second, and says why in one line.
- **`assert!(matches!(…))` is the tree's form.** `assert_matches!` appears nowhere despite
  1.96 stabilising it and `rust-performance.md` preferring it; adopting it in one file is
  drift, so it moves everywhere or nowhere.
- **A fixture dodges a denied call by construction, not by suppression.**
  `NonZero::new(v).unwrap_or(NonZero::MIN)` in `tests/common/mod.rs`, whose floors are
  unreachable because every caller passes a literal;
  `read_to_string(…).unwrap_or_default()` plus an emptiness assertion in `packaging.rs`,
  which also gets to name the file that moved.
- **A `println!` left in from debugging fails the gate**, which is the point.
- **The message argues, it does not restate.** `assert_eq!` already prints both sides, so
  the string is for the sentence the next reader needs: what broke and why it matters.

## Naming

A test is named as the property it holds, as a sentence in the present tense:
`no_thread_name_outgrows_what_the_kernel_keeps`,
`a_seek_lands_on_the_frame_it_asked_for`,
`every_bundled_font_is_named_in_the_attribution`. A walk is named as the prohibition it
enforces (`nothing_tests_a_url_scheme_by_prefix`).

Two older habits survive and neither is a second convention: the terse names in the query
suites, and the `test_` prefix in the settings, player-engine and view-state ones. The prefix
says nothing the attribute above it did not. On a *fixture* it is the convention and stays:
`test_pool`, `test_track`, `test_station`, `make_test_metadata`, `TestSource`.

Anything non-obvious carries a `///` giving the why, usually the bug that motivated it.
That doc comment is bound by `.claude/rules/code-style.md` like any other: argue the
reason, do not narrate the body.

## What to test

**Partition the input and test both sides of every boundary.** A threshold, clamp or cap
is the commonest defect site in this tree and the reported case is one sample of it. Work
the real constants at both ends and at the step either side: `Theme.is-light`'s luminance
split, `MAX_NAV_INDEX`, `STORE_MAX_DIM`, `SEEK_END_MARGIN`, the star-rating clamp. A test
that pins only the value in the bug report pins the bug report.

**Cross the flags that are not each other's guard.** Where two independent settings decide
one outcome, the table has four rows and the interesting ones are the corners: backdrop
style against theme polarity, the radio off switch against a station already seated in the
queue.

**For a state machine, the invalid transitions are the half worth writing.** `PlayerState`
answers what it refuses as much as what it does.

**Prefer an equality to a floor wherever the corpus can shrink.** A suite that keeps
passing while its subject gets smaller has stopped testing, and a walk is the shape most
prone to it.

**A pin that cannot fail is worse than no pin**, because it is read as coverage. That is
the whole argument for the vacuity floors above, and the reason `rules_globs.rs` resolves
every glob rather than skipping the ones it cannot: a skipped entry and a rotted one look
identical.

Where the defects actually cluster here: path handling across platforms, teardown ordering
in the view slices, and anything crossing the Rust/Slint boundary. That is why the pins
concentrate there, and where a new one usually earns its place.

## Keeping it deterministic

- **`#[tokio::test]` is current-thread by default and that is the right default.**
  `flavor = "multi_thread", worker_threads = 2` only where the body must make progress on
  two threads at once. Today that is `crossfade.rs`, where pulling the mixer *is* the audio
  thread and a control op blocks until that thread services it, and `headless.rs`. Nothing
  else needs it.
- **A paused clock over a real sleep.** `start_paused = true` auto-advances to the nearest
  deadline once everything is idle, so a timeout test spends its budget instantly instead
  of sleeping through it.
- `RUST_TEST_THREADS = "8"` in `.cargo/config.toml`, non-forced, so a dev can override it.
- **A path in a fixture is joined, never spelled.** `format!("{dir}/x.mp3")` seeds a row
  Windows can never match, and the failure looks like anything but a path bug. Where the
  separator itself is the subject, build it from `MAIN_SEPARATOR_STR`.
- **The suite assumes an LF checkout** and `.gitattributes` is what guarantees one. Every
  walk that splits on `"\n}\n"` and every signed updater fixture depends on it.
- **No `#[ignore]`.** There are none. A test that cannot run on a platform is skipped by
  *name* on the CI command line, so a new integration test runs there by default; an
  `#[ignore]` would opt it out silently and forever.

## Not in this tree, and why

So none of it gets proposed as an improvement:

- **No doctests.** Every library carries `[lib] doctest = false`, so a `# Examples` block
  documents and does not run.
- **No proptest, insta, mockall, rstest, serial_test, assert_cmd, trybuild or nextest.**
  Serialising the env-touching tests is hand-rolled behind testkit's private lock for the
  reasons `unsafe-rust.md` gives, and nothing else has needed a framework yet. Adding one
  is a real argument to make, not a default to reach for.
- **No `benches/`, no criterion, no fuzzing.** Performance work here is measured with
  flamegraph, heaptrack and peak RSS instead; `rust-performance.md` has the discipline and
  `docs/adr/0034-performance-is-profiled-not-benchmarked.md` argues why there is no gate.
- **No coverage threshold.** Coverage is a manual workflow publishing HTML, and it never
  blocks a merge.
- **No `slint::testing` call anywhere**, despite the API existing. The UI is pinned from
  the Rust side instead, by `include_str!`ing the `.slint` source and asserting on it, as
  `ui/tests/grid_prewarm_tests.rs` does with the card constants. That catches the drift
  that actually happens, which is a Rust constant and a Slint property disagreeing.

## Above unit and integration

The system tier is `crates/melodia/tests/headless.rs`: it boots the real backend in a
temp directory, ingests the checked-in fixture and asserts the row lands. It is the only
test that opens an audio device, which is why CI needs a null PCM to run it and Windows
skips it by name.

The acceptance gate on a *shipped* binary is the updater's `--version` smoke test, which
spawns the new executable and rolls back unless it exits 0 with the right prefix. That is
why the branch in `main()` is a forward-compat contract rather than a convenience.

Above that it is a manual run, and `CONTRIBUTING.md` asks for one by name on UI changes.
There is no browser-style end-to-end tier because there is no browser.
