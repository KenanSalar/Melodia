# Vendored winit fork

This directory is a **vendored, trimmed copy** of
[winit](https://github.com/rust-windowing/winit) at the `v0.30.13` tag plus
three commits that add Wayland file drag-and-drop support (upstream
[winit#1881](https://github.com/rust-windowing/winit/issues/1881), unmerged):

1. `wayland: register drag & drop events` (PR #4009 by PeakKS)
2. `wayland-dnd: use platform-internal WindowId for v0.30.13`
3. `wayland-dnd: percent-decode URIs in text/uri-list`

The added code is `cfg`-gated to Linux, so this fork builds as a no-op drop-in
on every platform.

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
