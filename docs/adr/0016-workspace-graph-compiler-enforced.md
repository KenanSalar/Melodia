# ADR 16: Fourteen crates, and the compiler holds the direction

**Status:** Accepted, 2026-09-03

The layering was already real and nothing enforced it. Two of the rules in `CLAUDE.md` were plain
grep-able properties, checked by whoever remembered to check: the UI must not import the database,
and background tasks must not import the UI. The rest were held by tests that walk the source tree
looking for violations, because in a single crate there was nothing else that could hold them. The
tests were doing two jobs and only one of them well.

Decision: fourteen crates, flat under `crates/`, each naming in its manifest only what sits below
it. The two that must not meet cannot: the UI has no database and no socket on its dependency line,
the decoders have no mixer, the tag writer has no state machine.

Alternatives: staying one crate and adding a corpus walk per boundary; module visibility inside one
crate; vertical crates per feature rather than per layer.

Trade: tests should verify behaviour and the compiler should enforce topology. A corpus walk can
only see what it was told to look for, it passes vacuously the day its anchor moves, and it reports
a violation as a failing assertion in an unrelated test run. After the split rustc refuses the
import by name and says which crate. Module visibility cannot do it because the modules were already
mutually reachable, which is the thing being fixed.

The obvious claim for a split like this is the wrong one. **A layer split does not make a new source
kind cheaper. It makes a new source kind's misplacement impossible.** Only the second is the
argument. The audio cut is the one genuine exception: three crates rather than one, because exactly
three of those files import the network and exactly three import cpal with no overlap, so the two
dependency sets do not intersect and a new source is a new `AudioSource` and nothing else. Under one
audio crate it could reach the state machine and the mixer, and under this one it cannot.

What it costs, plainly: a single `src/` tree that any grep reached in one pass, a test corpus that
had been genuinely good at catching drift, and a packaging path that already worked. Fourteen
manifests move together on a dependency bump, and a type that wants to be seen by two crates that do
not know about each other has to move down rather than sideways. The bet is that a compiler-enforced
graph is worth more over the next two years of podcasts and streaming than those three were, and
that the cost is paid once.

Three properties no compile can answer are held by `crates/melodia/tests/workspace_shape.rs`
instead: every member carries the workspace lint set, every member sits directly under `crates/`,
and no crate re-exports another's items ([ADR 17](0017-no-cross-crate-re-exports.md)).
