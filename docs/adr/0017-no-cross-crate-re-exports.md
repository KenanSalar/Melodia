# ADR 17: No crate re-exports another member's items

**Status:** Accepted, 2026-09-03

Splitting into a workspace (ADR 16) makes a facade tempting immediately. `crate::error::AppError`
reads better than `melodia_core::error::AppError`, and one `pub use` at a crate root gets it back
everywhere. It also hands every dependent of that crate a path into whatever it re-exported,
including the crates its own manifest was drawn to keep out of reach.

Decision: zero re-exports across members. Every `melodia_*` path is a plain import naming the
crate it comes from, and that includes the generated Slint types: `melodia_ui::AppWindow`, never
`crate::AppWindow`.

Alternatives: a facade crate re-exporting the common vocabulary; `pub(crate) use` at each crate
root to get the short path without widening the public surface; a prelude module.

Trade: the split exists so the import site shows the layering, and a re-export hides exactly that.
`melodia_core::error::AppError` names its layer where it is read; `crate::error::AppError` does
not, which is the whole difference between a graph you can see and one you have to reconstruct.
The `pub(crate)` middle course is the interesting loser, because it looks like it gets both. It
does not, and the reason is the failure mode rather than the visibility: with a `pub(crate)`
re-export, naming a forbidden crate fails as "this item is private", which reads as something to
work around. With no re-export at all it fails as an unlinked crate, which reads as "this crate is
not yours to use". The second error is the one that teaches the rule.

There is a trap here worth writing down, because it pushes the wrong way. Clippy's
`wildcard_imports` returns early when a glob's visibility is not restricted to the parent module,
so `pub use x::*` is skipped while `pub(crate) use x::*` at a crate root is linted, and pedantic is
denied in this workspace. The lint therefore permits the spelling this ADR forbids and rejects the
narrower one, which is why the rule cannot be a lint and is held by
`crates/melodia/tests/workspace_shape.rs` walking every crate root instead.

What it costs is longer paths at every site and import lists that name four or five crates in one
file. That is the price and it is also the product: the file says what it depends on.
