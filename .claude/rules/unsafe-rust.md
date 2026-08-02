---
paths:
  - src/**/*.rs
  - melodia-ui/src/**/*.rs
  - tests/**/*.rs
  - build.rs
  - melodia-ui/build.rs
  - Cargo.toml
---

# Unsafe Rust — the posture, and what to reach for instead

`unsafe_code = "deny"` sits in `[workspace.lints.rust]` and both packages inherit it.
Two things follow, and they're the whole of this file: the one category that earns an
`#[allow]`, and the table of what to use instead of the ones that don't.

`rust-performance.md` shares these globs and answers a different question — how to make
safe code fast. Reach for it first. Nothing here is a performance technique.

## Why the bar is where it is

**`deny`, not `forbid`** — that's what makes the per-site `#[allow]` legal, and it's
deliberate. Don't "tighten" it to `forbid`; the FFI below can't be deleted, so the only
effect would be to move every allow into a `build.rs`-shaped workaround.

Three things make a soundness bug cost more here than in an average crate:

- **`panic = "abort"` in release.** UB doesn't surface as a clean crash with a
  backtrace. It surfaces as a corrupted decode, a wrong colour, or a wrong answer weeks
  later on someone else's machine.
- **The app ships with an auto-updater.** A bad release reaches installs that never
  asked for it, and the rollback path only covers a binary that fails its `--version`
  smoke test — not one that runs and is quietly wrong.
- **Miri is out of reach.** `rust-toolchain.toml` pins the compiler and that pin is the
  only toolchain installed, so new `unsafe` ships verified by review and nothing else.

## The one sanctioned category: platform FFI

Every `unsafe` in production is a call into an OS the type system can't reach. There is
no other kind, and the list is short enough to keep here:

| site | what |
|---|---|
| `src/main.rs` | `libc::mallopt` ×3 (the glibc arena / mmap / trim knobs), `env::set_var` for `PIPEWIRE_ALSA` |
| `src/tasks/heap_trim.rs` | `libc::malloc_trim` |
| `src/services/dwm_titlebar.rs` | `DwmSetWindowAttribute` ×3 |
| `src/services/settings/data.rs` | `GetUserDefaultLocaleName` |
| `src/services/updater/install/swap.rs` | `MoveFileExW` |

A new site outside that shape is a different *kind* of thing rather than one more of the
same, and owes a justification somewhere a reviewer will read — not only in the
attribute's `reason`.

Two non-FFI things that look like they'd need `unsafe` and don't: `Box::leak` (the
`'static` speakers in `state/mod.rs`, the interned column names in `entities/track.rs`)
is safe, and so is every `*mut c_void` that is only *stored* — `media_controls` holds an
`HWND` across time without a single `unsafe`, because souvlaki owns the deref.

## Writing one

- **`#[allow(unsafe_code, reason = "…")]` on the narrowest item that covers it** — the
  function, or the block. Not the module, and not the file unless every item in it is
  FFI. This is the documented exception to the root `CLAUDE.md`'s "`#[expect]`, never
  `allow`": `expect` is for a suppression that should stop being needed and fires
  `unfulfilled_lint_expectations` when it does. FFI unsafe is permanent, so an `expect`
  there would be correct on the day it was written and never fire again.
- **A `// SAFETY:` comment names the precondition being upheld**, never what the call
  does. `dwm_titlebar.rs`'s is the model: it says the pointer targets a stack local
  whose size matches the `cbAttribute` argument and that the API doesn't retain it —
  three claims a reviewer can check against the docs. "Calls `DwmSetWindowAttribute`"
  would be worth nothing.
- **Build raw pointers with `std::ptr::from_ref(&x).cast()`**, never
  `&x as *const _ as *const c_void`. The chained `as` silently accepts a reference that
  was never the type you meant.
- **Prefer `windows-sys`** — already a dependency — over a hand-rolled
  `unsafe extern "system"` block. `settings/data.rs` is the one hand-rolled declaration
  in the tree and shouldn't grow a second: a mistyped signature there is UB the compiler
  will happily agree with.
- **`cfg`-gate at the call site *and* check the manifest.** `libc` is declared under
  `[target.'cfg(target_os = "linux")']`, so a `cfg(unix)` call site compiles on Linux
  and fails to resolve on macOS. That exact mismatch shipped once in a test.

## Test env-var mutation

`std::env::set_var` / `remove_var` are unsafe in Rust 2024 because they mutate shared
process state, and `cargo test` runs in parallel by default. One shape, every time:

**lock → snapshot → mutate → `catch_unwind(body)` → restore → `resume_unwind`.**

`services/tests/settings_tests.rs::with_env_vars` and
`updater/tests/system_install_tests.rs` are the two worked examples. Three parts are
each load-bearing and each has been missing at some point:

- **One lock per *file*, not per variable.** The environment is process-global, so two
  tests touching different names still race each other's reads —
  `SettingsData::default()` reads `XDG_CURRENT_DESKTOP` through `is_kde_desktop()` and
  is exactly such a reader. Consolidating assertions into one test doesn't help; it only
  stops that test racing itself.
- **`catch_unwind` around the body.** Without it a failing assertion skips the restore
  and leaks its variable into every test that runs after.
- **`resume_unwind` on the way out**, or the failure is swallowed and the test passes.

A file-level `#![allow(unsafe_code, reason = "…")]` is fine here — the whole file is
test scaffolding. **Delete it when the last `unsafe` goes**; a stale allow silently
pre-authorises the next one, and one sat in `library/settings/tests/folders_tests.rs`
over a file with no `unsafe` in it at all.

## What to reach for instead

| temptation | reach for |
|---|---|
| `get_unchecked` on a hot slice | iterator `zip` — already the idiom in `player/equalizer.rs`, and the comment there says why. Or put the length in the *type* (`[T; N]` over `Box<[T]>`) so the compiler proves the mask itself |
| `transmute` between plain-data slices | `bytemuck::cast_slice`. It's a new dependency, so it has to earn its place against what it saves |
| uninit buffer + `set_len` | `Vec::with_capacity` + `extend`, or build with the fill you need. Every buffer in the DSP and visualizer paths is allocated once per source and reused, which is the win that actually mattered |
| `std::arch` SIMD | measure first, and read the shape of the work: ten cascaded biquads are *serially dependent*, so there is nothing to vectorise across bands, and two channels caps the other axis at 2× |
| `unsafe impl Send` / `Sync` | the `const _: fn() = \|\| { fn check<T: Send + Sync>() {} check::<FooUi>(); };` assertion — eight of them in the tree already |
| a raw pointer to dodge a lifetime | an owned `Arc` clone. `PlaybackContext` exists for precisely this and says so |

## Before reaching for unsafe on a hot path

The measured hot spots are not where intuition puts them, and none of them is a
bounds-check problem. Read these before proposing anything:

- **The per-sample DSP path has no index in it.** `EqSource::next_active` is four
  `zip`ped iterator loops; what survives is a handful of per-*frame* slice accesses. The
  bypass arms — a flat EQ at unity gain, which is the default — touch no buffer at all.
- **The visualizer's dominant per-frame cost was number *formatting*, not arithmetic.**
  At the column cap the trace writes two vertices per column into an SVG path string,
  and asking `core::fmt` for a shortest-round-tripping decimal outweighed both FFTs
  beside it. The fix was `waveform::push_fixed` — integer scale, integer print — in safe
  code.
- **The backdrop solve's cost was a transcendental with a 256-value domain.** `linearized`
  takes a `u8`; three calls per pixel became three loads from a `LazyLock<[f64; 256]>`
  in `ui/backdrop.rs`.

The pattern in all three: the win came from asking a smaller question, not from removing
a check. **No `unsafe` for performance without a flamegraph or a criterion number
showing the safe version is the bottleneck** — and when you have one, the fix is usually
still safe.
