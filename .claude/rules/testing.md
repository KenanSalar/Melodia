---
paths:
  - crates/*/src/**/tests/*.rs
  - crates/*/src/**/fixtures.rs
  - crates/melodia/tests/**/*.rs
  - crates/melodia-testkit/src/**/*.rs
  - crates/*/Cargo.toml
  - .cargo/config.toml
---

# Testing Best Practices

Siblings own the slices this file does not: `unsafe-rust.md` has the env-var mutation
contract, `tokio.md` the async runtime flavours, `rust-performance.md` criterion and the
profilers, `ci-packaging.md` the gate and coverage flags.

## Principles

The seven ISTQB principles, as they bear on writing a test:

- **Testing shows the presence of defects, not their absence.** A green suite is evidence,
  never proof. Report "no failures found", not "it works".
- **Exhaustive testing is impossible.** The input space always exceeds the budget, so
  choose by risk rather than by what is easy to reach.
- **Test early.** A defect costs more the later it is found. Write the test with the code.
- **Defects cluster.** A minority of modules holds the majority of defects. Put new effort
  where failures have already happened.
- **Beware the pesticide paradox.** A suite that always passes has stopped finding
  anything. Revise and extend it as the code moves.
- **Testing is context dependent.** Rigour follows risk: a path that destroys user data and
  one that misaligns a label do not earn the same coverage.
- **Absence of defects is a fallacy.** Code that passes every test can still be the wrong
  thing to have built.

## What a good test looks like

- **FIRST**: Fast, Independent, Repeatable, Self-validating, Timely.
- **One reason to fail.** A test that can fail for three reasons reports none of them
  clearly. Split it.
- **Arrange, act, assert**, in that order and visually separable. Keep the act to one call.
- **Test the contract, not the implementation.** Assert on observable behaviour and public
  API. A test that breaks on every refactor is testing the wrong surface.
- **No logic in tests.** No conditionals, loops or arithmetic around the assertion; a bug in
  the test is invisible. Table-drive instead, with the expected value written out.
- **Independent and order-free.** No shared mutable state, no reliance on another test
  having run, no reliance on the harness order.
- **Deterministic.** A test that fails once in fifty is worse than no test: it trains the
  team to re-run rather than to read.

## Designing the cases

- **Equivalence partitioning.** Split the input into classes handled the same way and take
  one value from each. Cover invalid partitions too, not just valid ones.
- **Boundary value analysis.** Defects concentrate at edges. For each boundary test the
  value on it and the step either side. Work the real constants, not round numbers.
- **Decision tables** where several conditions combine into one outcome. The interesting
  rows are the corners, where flags that are not each other's guard disagree.
- **State transition testing.** For a state machine the invalid transitions are the half
  worth writing: assert what it refuses as much as what it does.
- **Pairwise** when the full matrix explodes. Most combinatorial defects need only two
  factors to interact.
- **Error guessing** supplements the techniques above, never substitutes for them.
- **Negative and error paths deserve cases.** Empty, zero, one, maximum, absent, malformed,
  duplicated, out of order.
- **A test that pins only the value from the bug report pins the bug report.** Generalise to
  the partition the bug was one sample of.

## Test levels

- **Unit** for logic in isolation, **integration** for the seams between components,
  **system** for the assembled thing, **acceptance** for whether it is the right thing.
- **Pyramid, not ice cream cone.** Many fast unit tests, fewer integration, fewest
  end-to-end. Higher levels are slower, flakier and worse at localising a defect.
- **Push each question to the lowest level that can answer it.** Do not use an end-to-end
  test to check a boundary a unit test could pin.
- **A level that cannot answer a question honestly should not fake it.** Some behaviour is
  only observable against the real collaborator; write that test at the level it belongs.

## Rust: organizing tests

- **Unit tests** live in the crate with the code (`#[cfg(test)] mod tests`, or `#[path]` to
  a sibling file), and may reach private items.
- **Integration tests** live in `tests/` beside `src/`. Each file compiles as its own crate
  and sees only the public API, which is what makes it a genuine consumer.
- **Shared integration helpers go in `tests/common/mod.rs`**, the subdirectory form, so
  Cargo does not compile the helper as a test binary of its own.
- **`#[cfg(test)]` does not cross a crate boundary.** A dependent crate is compiled without
  it, so a fixture two crates share must be real API (`#[doc(hidden)] pub`). A fixture with
  no reader outside its crate stays `#[cfg(test)]` and never reaches the shipped surface.
- **A check that walks a corpus belongs where it can see the corpus**; a pin on one named
  file belongs beside that file, so a per-crate test run stays a question about that crate.
- **Ask for a dev-dependency per crate.** `--workspace` unifies features and
  `cargo test -p <crate>` does not, so a crate that needs a feature declares it.
- **Doc tests document; they are not the test suite.** They are disabled here
  (`[lib] doctest = false`), so an examples block explains and does not run.

## Rust: attributes and signatures

- `#[test]` applies to monomorphic free functions taking no arguments whose return type
  implements `Termination`.
- **Prefer `-> Result<(), E>` and `?`** over unwrapping fallible setup. It is the shape that
  needs no suppression and reports the cause on failure.
- **`#[should_panic]` always takes `expected`.** Bare, it passes when the code panics for a
  reason you did not mean. It cannot be combined with a `Result`-returning test; assert
  `is_err()` there instead, and match the variant rather than the message.
- **`#[ignore = "reason"]` always carries the reason**, and is a debt, not a parking space.
  Prefer filtering by name on the CI command line, so a new test runs by default rather than
  opting itself out silently and forever.
- **Platform-specific tests gate the cfg** (`#[cfg(all(test, target_os = "…"))]`) rather
  than compiling everywhere and skipping at runtime.

## Rust: assertions

- **`assert_eq!` / `assert_ne!` over `assert!(a == b)`**: they print both sides on failure.
- **The custom message argues, it does not restate.** The values are already printed; the
  string is for what broke and why it matters.
- **Pick one form of pattern assertion and use it everywhere.** `assert!(matches!(…))` and
  `assert_matches!` are both fine; mixing them within a tree is drift.
- **Compare floats with an explicit tolerance**, never `==`, and state the tolerance.
- **Do not assert on `Debug` output.** It is not a stable contract; assert on fields.
- **Avoid `unwrap`/`expect`/`panic!` in test bodies.** The workspace lints deny
  `unwrap_used` and warn `expect_used`, `panic` and the print macros with no test exemption,
  so reach for `?`, `let … else`, `matches!` or an `assert!` that reads well on failure.
  Where `expect` is genuinely right, scope `#[expect(clippy::expect_used, reason = "…")]`
  to the narrowest item and name the invariant, not the call.
- **A fixture dodges a denied call by construction, not by suppression.**

## Rust: test doubles and seams

- **Prefer the real collaborator** wherever it is fast and deterministic. A double is a cost
  paid to buy determinism or speed, not a default.
- **Prefer fakes to stubs, and stubs to mocks.** A fake has a working implementation with a
  shortcut (in-memory store, loopback server); a mock asserts interactions and so couples
  the test to how the code is written rather than to what it does.
- **Inject the seam as a parameter.** A base URL, a clock, a root directory or a pool passed
  in is testable without a framework, and the production call site spells the default.
- **Process-global state is the enemy of a test suite.** A `static`/`OnceLock` cache, a
  lazily-resolved singleton or an ambient config makes tests order-dependent and
  unresettable within one binary. Reach for a parameter or a per-instance field instead.
- **Do not add production API whose only caller is a test.** If a value cannot be observed,
  either the design hides too much or the assertion belongs somewhere else.

## Rust: determinism

- **Tests run in parallel, in one process.** No shared mutable global, no fixed port, no
  writing to a shared path, no assumption about which test ran first.
- **Filesystem work goes in a temp directory** that cleans itself up, never a path under the
  repo or the user's home.
- **Seed anything random** and print the seed on failure so it reproduces.
- **Do not depend on iteration order** of a hash map or set, or on the order a filesystem
  walk returns entries. Sort before asserting, or compare as sets.
- **Join paths, never spell separators.** A hand-written `/` seeds data the test's own code
  cannot match on Windows, and the failure looks like anything but a path bug. Where the
  separator itself is the subject, build it from `MAIN_SEPARATOR_STR`.
- **Environment variables are process-global**, and mutating them is `unsafe` in edition
  2024 because a concurrent reader races the write. Serialize every mutation and every
  deliberate read behind one lock per test binary, and restore on unwind.
- **Avoid wall-clock, timezone and locale dependence.** Pass the instant in; do not read
  "now" inside the code under test.
- **Assume an LF checkout** for anything that parses source text, and pin it in
  `.gitattributes` rather than trusting a clone's line-ending heuristics.

## Rust: async

- **`#[tokio::test]` is current-thread by default and that is the right default.** Reach for
  `flavor = "multi_thread"` only where the body must make progress on two threads at once.
- **A paused clock beats a real sleep.** `start_paused = true` (needs `test-util`)
  auto-advances to the nearest deadline once the runtime is idle, so a timeout test spends
  its budget instantly. `tokio::time::advance` steps it deliberately.
- **A paused clock and real I/O do not mix.** A request in flight looks like idle to the
  runtime, so the clock jumps to the next deadline rather than waiting for the socket, and
  any `timeout` wrapped around real work fires every time. Take a live clock there.
- **Never sleep to wait for a condition.** Await the thing itself, or a channel it signals.

## Coverage, and what it is worth

- **Coverage measures what executed, not what was verified.** A line covered by a test with
  no assertion on it is not tested.
- **Do not set a coverage number as a target.** It becomes the goal and the suite fills with
  assertions nobody reads.
- **Read it as a map of where nothing runs at all**, then decide by risk whether that
  matters. Orchestration and decision code usually deserves it more than the well-covered
  primitives underneath.
- **The check that a pin works is breaking the code deliberately** and watching that test,
  and only that test, fail. A pin that cannot fail is worse than no pin, because it reads as
  coverage.
- **Prefer an equality to a lower bound** wherever the thing under test can shrink; a floor
  keeps passing while its subject disappears.

## Anti-patterns

- **Flaky tests.** Fix or delete; never paper over with a blind retry. A quarantine needs an
  owner and a date.
- **Assertion roulette.** A long run of unlabelled assertions where the failure does not say
  which one fired.
- **Interdependent tests** that pass only in order, or that share a fixture they mutate.
- **Testing private implementation detail**, which turns every refactor into a test rewrite.
- **Over-mocking**, until the test asserts the code calls the functions it calls.
- **Ignored tests left forever**, which is a deleted test that still costs compile time.
- **Snapshot bloat**: an accepted diff nobody read is not an assertion.
- **A stray debug print** left in a passing test.
- **Tests treated as second-class code.** Name, refactor and delete them like production
  code; a suite nobody maintains is one nobody trusts.

## Naming and documenting

- **Name a test as the property it holds**, as a present-tense sentence. The name should
  say what broke without opening the file.
- **A `///` on a non-obvious test gives the why**, usually the defect that motivated it.
  It is bound by `code-style.md` like any other comment: argue the reason, do not narrate
  the body.
- **Name a prohibition test as the prohibition**, so a failure reads as the rule it enforces.

## The toolbox

Know what exists before hand-rolling, and adopt deliberately: property testing
(`proptest`, `quickcheck`), snapshot testing (`insta`), mocking (`mockall`), parameterized
cases (`rstest`), a faster runner (`nextest`), fuzzing (`cargo-fuzz`), UB detection
(`miri`), benchmarking (`criterion`), real dependencies in containers (`testcontainers`).

- **Adopting one is a decision for the whole workspace**, not for one file. A framework used
  in a single suite is drift, and the next reader cannot tell which convention is current.
- **Weigh it against what it replaces.** A dependency that saves fewer lines than its own
  matching DSL costs to learn is not a saving.
- **This workspace deliberately runs none of them**, so proposing one is an argument to
  make on its merits rather than a default to reach for.
