# ADR 18: What is deliberately not split, and why there are zero Cargo features

**Status:** Accepted, 2026-09-03

Every boundary drawn in [ADR 16](0016-workspace-graph-compiler-enforced.md) invites the next
one, and a split that keeps going stops paying for itself: past some point the crates exist to
satisfy the rule rather than to hold a real edge. Three further splits were proposed and refused,
and each will be proposed again by someone reading the graph and noticing the asymmetry, so the
refusals belong here beside the boundaries.

Decision: the UI stays one crate, the library command layer stays inside the app crate, there are no
per-feature vertical crates, and the workspace has zero Cargo features.

Alternatives, in the order they come up: cutting the UI into per-view crates; `library/` as its own
crate; `melodia-podcast` and `melodia-radio` as vertical slices; a `self-update` feature gate.

Trade: the UI is twenty view slices in a dense mesh with a shared component library importing
fourteen of them, so cutting it needs a view registry, and inventing an abstraction whose only
purpose is to satisfy the split is the stopping rule the whole exercise set for itself. The library
layer cannot be a leaf: forty-one of its forty-three files take the application state, so extracting
it would mean extracting that too. It is a command layer wearing a query layer's name, and the
honest fix there is the rename rather than the crate. Vertical feature crates are the shape that
would make a source kind a one-crate change, and they need three inversions the tree does not have:
the artwork reference ledger becoming a registry features contribute to rather than a constant
naming their tables, the playback source becoming open rather than a closed enum, and the navigation
index becoming data rather than a constant. That is a real design and it is not this one.

Zero features is the one that will be argued with, so here is the mechanism rather than the taste.
Workspace-wide feature unification is nightly-only and the toolchain is pinned to stable, so a
per-crate feature reselects and rebuilds shared dependencies under any scoped invocation, which
makes `cargo clippy -p <member>` a different build from the gate. And a real feature matrix is
combinations that a single `--workspace` run cannot cover, so the gate would stop meaning what it
says the day the second feature lands.

The condition that would justify the first one is a genuine external requirement rather than a
preference, so it belongs here rather than in a future argument: a distribution channel that
requires the network fetch and self-replace code to be **provably absent** from the binary rather
than merely disabled at runtime. Repository and store reviewers ask for exactly that, and no runtime
toggle satisfies it. If that channel is pursued, this ADR is superseded rather than quietly bent,
and the cost above is what gets paid.

This ADR settles a contradiction that had been live in two working docs at once:
`docs/plans/SELF_UPDATE_FEATURE_GATE.md` proposes that feature and predates the split, and neither
doc cited the other.
