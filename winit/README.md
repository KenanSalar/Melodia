# Vendored winit fork

This directory is a **vendored, trimmed copy** of
[winit](https://github.com/rust-windowing/winit) at the `v0.30.13` tag plus
three commits that add Wayland file drag-and-drop support (upstream
[winit#1881](https://github.com/rust-windowing/winit/issues/1881), unmerged),
and a fourth that reports where a file drag is:

1. `wayland: register drag & drop events` (PR #4009 by PeakKS)
2. `wayland-dnd: use platform-internal WindowId for v0.30.13`
3. `wayland-dnd: percent-decode URIs in text/uri-list`
4. `dnd: positioned drag events on Wayland, X11 and Windows`

The fourth adds `WindowEvent::DragEntered`, `DragMoved`, `DragDropped` and
`DragLeft`, in the shape `v0.31.0-beta.1` and `beta.2` shipped them (PR #4079):
one event per set of files, each carrying the pointer in physical pixels. The
Windows and X11 halves port that PR, the Windows one with its fix to the
`IDropTarget` vtable, which declared the point as a pointer where COM passes it
by value. Those three platforms send the new events in place of `HoveredFile`,
`DroppedFile` and `HoveredFileCancelled`; macOS is untouched and still sends
the old ones. On Wayland a drop that arrives before the path list waits for it,
rather than sending what has been read so far.

Upstream `master` has since replaced these events with ID-based ones whose paths
are fetched separately (PR #4571), so retiring the fork rewrites the filter's
four arms rather than deleting this directory alone.

Melodia builds it via the `[patch.crates-io]` block in the repo-root
`Cargo.toml` — see that comment for the full rationale and the upstream-bump
procedure. The tree has been trimmed to only what's needed to compile the
crate: upstream `examples/`, `tests/`, `docs/`, tooling configs and project
docs were removed. `src/` is intact — including its `changelog` module, which
is a real `pub mod` and part of the compiled crate.

winit's `dpi` sub-crate is **not** vendored — it's unmodified upstream, so
`Cargo.toml` here pulls it from crates.io (`dpi = "0.1.1"`). Vendoring it as a
path crate would create a second, un-unifiable `dpi` instance that clashes
with the `dpi` `muda` pulls from the registry on Windows.

Licensed under Apache-2.0 — see [`LICENSE`](LICENSE).
