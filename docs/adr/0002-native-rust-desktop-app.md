# ADR 2: A native Rust desktop app, after the Tauri version

**Status:** Accepted, 2026-05-25

The previous Melodia was Tauri and SolidJS. It shipped an embedded WebKitGTK browser engine, and the
combined real-world footprint of the two processes ran to roughly 900 MB. The number was not the
worst of it: memory drifted upward over a session in ways that were hard to attribute from the Rust
side, because the leak could be in the web engine, in the view layer, or in the bridge between them,
and only one of those three was ours to read. This project exists because of that.

Decision: one native process. The UI runs in-process on a Rust toolkit, backend work is ordinary
function calls and channels, and there is no WebView, no web runtime and no IPC boundary. Memory is
a product requirement rather than a nice-to-have, and the figures in `README.md` are measured and
published as such.

Alternatives: staying on Tauri and attacking the footprint from inside it, another web shell, and a
hybrid keeping the SolidJS front end over a Rust core.

Trade: what this buys is a process whose whole memory profile is legible to one language, and a
click that reaches a database row without crossing a serialization boundary. Idle RSS landed around
a sixth of what it replaced. The cost is that every widget is ours. The web ecosystem's component
libraries, its CSS, its hot reload and its enormous pool of people who already know it are all gone,
and the toolkit's gaps become our gaps rather than something we can paper over with a stylesheet. It
also narrows who can contribute a UI change, which for a project that wants contributors is a real
price and not a rounding error. The optimisation route was the honest alternative and it loses on
ceiling rather than on effort: the browser engine is most of the footprint and it is not ours to
shrink.

One precision, because the README's "pure Rust" wording is load-bearing and not literally true. The
binary statically links SQLite and aws-lc-sys, so there is C in it. What the claim actually means,
and what is worth defending, is that there is no WebView, no IPC and no FFmpeg or GStreamer media
stack: the parts that decode a file, draw a frame and hold the library are Rust. A future C media
dependency would make the current wording misleading rather than merely loose.

This ADR was written in September 2026 and reconstructed from `README.md`, `CLAUDE.md` and the
maintainer's account. The commit that carries the migration has an empty body.
