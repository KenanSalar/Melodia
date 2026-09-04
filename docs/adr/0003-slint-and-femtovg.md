# ADR 3: Slint for the UI, and FemtoVG under it

**Status:** Accepted, 2026-05-25

Dropping the web stack ([ADR 2](0002-native-rust-desktop-app.md)) meant picking a Rust toolkit
that could carry what this app actually does: library grids and track lists long enough to need
virtualizing, animation throughout rather than as decoration, a theme system swapped at runtime down
to every brush, and six locales. A toolkit that makes any one of those hard shows up in every
screen.

Decision: Slint, with the markup compiled by `slint-build` into Rust at build time, and FemtoVG
selected directly on the `slint` dependency rather than sitting behind a Cargo feature anyone can
flip.

Alternatives: iced, egui, gtk4-rs, cxx-qt.

Trade: Slint's declarative markup treats the things this app leans on as first-class. Animations,
`states` and `transitions`, and property bindings are language constructs rather than something
reimplemented per widget, which is most of what the app's feel rests on; the markup is compiled
rather than interpreted, so the UI is a build artifact and a typo is a build failure; and `@tr()`
bundles the catalogues at codegen. An immediate-mode toolkit would have made the animation and
theming work fight the paradigm, and the two mature retained-mode options mean binding to a C or C++
library, which reintroduces the shape [ADR 2](0002-native-rust-desktop-app.md) exists to
remove.

The cost is that Slint is younger than the alternatives and its gaps are ours. Several things this
app wants do not exist in it: no backdrop blur, no RTL or bidi-aware layout, no Rust-callable
`tr()`, and tooltips clip at the window edge. `docs/plans/SLINT_NATIVE_ADOPTION.md` tracks ten such
workarounds against upstream. Version 1.17 regressed both enter-transitions and per-frame cost, so
the tree is pinned to 1.16.1 and a toolkit upgrade is a budgeted task rather than a version bump.
Wayland file drag-and-drop needs a vendored winit fork. And Slint's own test API is unusable here,
so the UI is pinned from the Rust side by tests that read the markup as text.

FemtoVG is not a preference to revisit: the software renderer cannot clip to `border-radius`
(slint#4176), and rounded surfaces are the whole visual language. It is set on the dependency rather
than behind a feature so there is no build that quietly loses them.

This ADR was written in September 2026 and reconstructed from the dependency comments in
`Cargo.toml`, `CLAUDE.md` and the maintainer's account.
