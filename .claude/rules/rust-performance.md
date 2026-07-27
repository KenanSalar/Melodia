---
paths:
  - src/**/*.rs
  - tests/**/*.rs
  - build.rs
  - Cargo.toml
---

# Rust Performance Best Practices

## Allocation Reduction

- Pre-allocate: `Vec::with_capacity(n)`, `String::with_capacity(n)`, `HashMap::with_capacity(n)`
- Reuse buffers: `vec.clear()` then refill — retains allocated capacity
- `clone_from(&source)` reuses the target's allocation when possible (unlike `clone()`)
- Use `String::new()` or `Vec::new()` for empty containers — they don't allocate until first push

## String Handling

- Prefer `&str` in function parameters over `String` or `&String`
- Use `Cow<'_, str>` for functions that usually return input unchanged but sometimes modify
- Avoid `format!()` for simple conversions — use `.to_string()`, `.into()`, or string concatenation
- `push_str()` is faster than `format!` for building strings incrementally
- `std::fmt::from_fn` (1.93) builds an ad-hoc `Display` from a closure — formatted output without a newtype or an intermediate `String`

## Collections

- Use Entry API for HashMap insert-or-update: `map.entry(key).or_insert_with(|| value)`
- Accept `&[T]` over `&Vec<T>` in function signatures — more flexible, same performance
- Consider `SmallVec<[T; N]>` for collections that are usually small but occasionally grow
- Avoid `Box<dyn Trait>` in hot paths — use enums or generics for static dispatch
- `Vec::extract_if` (1.87) / `HashMap::extract_if` (1.88) drain matching elements with a predicate in one pass — no retain-plus-collect double walk
- `VecDeque::pop_front_if` / `pop_back_if` (1.93) for conditional queue draining (e.g. a draining action queue)
- `Vec::push_mut` / `VecDeque::push_back_mut` (1.95) push and hand back `&mut` to the new element — no re-index after the push

## Iterator Patterns

- Iterator chains are zero-cost abstractions — same performance as manual loops
- Avoid intermediate `.collect()` calls — chain operations instead
- Use `.filter_map()` instead of `.filter().map()` for combined operations
- `iter().enumerate()` over manual index tracking
- Prefer `iter().any()` / `iter().find()` over collecting then checking
- `Peekable::next_if_map` (1.94) — conditional consume-and-transform without a separate peek/next pair
- `<[T]>::array_windows::<N>()` (1.94) yields const-length `&[T; N]` windows — the compiler knows the width (good for frame-based DSP); `slice::as_array::<N>()` (1.93) is the safe `&[T]` → `Option<&[T; N]>` conversion

## Language Idioms (Rust 1.88–1.97)

- Let chains (1.88, edition 2024 only): `if let Some(a) = x && a.ready() { … }` — flatten nested `if let` towers instead of stacking indentation
- `if let` guards on match arms (1.95): `Some(x) if let Ok(v) = parse(x) => …` — pattern-match inside a guard; guard patterns don't count toward exhaustiveness
- `cfg_select!` (1.95) — compile-time `match` over cfgs, in std; replaces the `cfg-if` crate for platform-split code paths
- `assert_matches!` / `debug_assert_matches!` (1.96) — prefer over `assert!(matches!(…))` in tests: supports `if` guards and prints the non-matching value on failure
- New `Copy` range types exist in `core::range` (1.96), but `0..1` syntax still produces the legacy `Iterator` ranges — nothing to adopt yet
- Bit-manipulation methods on every integer (1.97) — replace hand-rolled shift/mask idioms. Note the two families return **different things**: `isolate_highest_one()` / `isolate_lowest_one()` return the *value* with only that bit set (`0b1010_0100` → `0b1000_0000` / `0b0000_0100`), while `highest_one()` / `lowest_one()` return the bit *index* as `Option<u32>` (`Some(7)` / `Some(2)`; `None` for zero). `bit_width()` gives the bits needed to represent the value (`164` → `8`). The `NonZero<{int}>` equivalents return a plain `u32` rather than an `Option`, since the zero case is gone — prefer them when the value is already `NonZero`
- `clippy::manual_assert_eq` (1.97) — `assert!(a == b)` is now a lint; use `assert_eq!(a, b)`. If the type has no `PartialEq` (so the `assert!` was comparing `mem::discriminant`s or similar), don't just wrap the discriminants in `assert_eq!` — their `Debug` prints an opaque `Discriminant(..)` on both sides of a failure. Compare a cheap injective projection instead (a token/`as_str`), which prints something readable

## Compile-Time Optimizations (release profile)

```toml
[profile.release]
lto = "fat"           # Full link-time optimization
codegen-units = 1     # Single codegen unit for better optimization
panic = "abort"       # Smaller binary, no unwind overhead
strip = "debuginfo"   # Drop DWARF but keep symbols — end-user panic backtraces stay readable ("symbols"/true strips both)
```

- Set `target-cpu=native` via `RUSTFLAGS` for platform-specific SIMD optimizations
- Since Rust 1.90, `rust-lld` is the default linker on `x86_64-unknown-linux-gnu` — dev links are fast out of the box (opt out with `-C linker-features=-lld`); `mold` is at best a marginal further gain, not a recommendation
- `--remap-path-scope` (stable since 1.95) controls which local paths get remapped out of the binary; Cargo's `trim-paths` profile key is **still nightly-only as of 1.97** (verified — a `profile.*.trim-paths` key fails with ``feature `trim-paths` is required``). Don't recommend it on stable
- **v0 symbol mangling is the default since 1.97** — nothing to configure. It pairs with the `strip = "debuginfo"` choice above (kept so end-user panic backtraces stay readable): v0 encodes generic parameters, so those frames are now unambiguous instead of collapsing distinct monomorphizations onto one legacy-mangled name
- Cargo's `build.warnings` config (stable 1.97) can deny warnings globally. **Don't** — it applies to *every* cargo command, so a stray unused variable during a `cargo run` iteration becomes a hard error. Keep `-D warnings` on the CI clippy invocation, where the gate belongs

## Profiling

- Always profile before optimizing — `cargo flamegraph` for CPU profiling
- Use `criterion` for microbenchmarks with statistical significance
- `#[bench]` is unstable — prefer criterion for stable Rust
- Use `perf` or `samply` for system-level profiling on Linux

## Miscellaneous

- Prefer `to_owned()` over `to_string()` when converting `&str` — avoids `Display` trait overhead
- Use `std::mem::take()` or `std::mem::replace()` instead of clone-then-clear patterns
- Global-allocator swaps (`mimalloc`/`jemalloc`) trade RSS for speed — measure peak RSS before adopting. **Melodia caveat:** mimalloc's per-thread segments across the app's many long-lived threads caused a large idle-RSS regression and was reverted; don't propose allocator swaps here
- Avoid unnecessary `Arc` — single-threaded code doesn't need atomic reference counting
- Use `#[inline]` sparingly — the compiler usually makes good inlining decisions; only hint for small hot functions crossing crate boundaries
- `core::hint::cold_path()` (1.95) marks a branch as unlikely — use in hot loops where the error/edge arm is rare
- `std::sync::LazyLock` / `LazyCell` (1.80) for lazy statics — no `once_cell` dependency; `get`/`get_mut`/`force_mut` (1.94) inspect without forcing initialization. Pairs with "construct heavy clients lazily"

## Type-Level Optimizations

- Prefer enums over `Box<dyn Trait>` in data structures — avoids heap allocation and vtable dispatch
- Use `#[repr(C)]` or `#[repr(packed)]` only when ABI compatibility or minimal padding is required; measure first
- Newtype wrappers (`struct Foo(u32)`) are zero-cost — use freely for type safety without runtime penalty
- `NonZero*` types (e.g. `NonZeroU32`) allow the compiler to use 0 as a niche for `Option<NonZeroU32>` — same size as `u32`

## Concurrency Patterns

- `AtomicUsize` / `AtomicBool` for lock-free counters and flags — cheaper than `Mutex<usize>` for simple state
- `Atomic*::update` / `try_update` (1.95, on all atomic types) — closure-based read-modify-write, cleaner than a hand-rolled `fetch_update` loop for lock-free shared state
- `RwLock<T>` instead of `Mutex<T>` when reads dominate — allows concurrent reads
- Avoid contention on a single `Mutex` in parallel code — partition state or use thread-local storage

## Build & Debug Profile

- Add a `[profile.dev.package."*"]` section to compile dependencies with optimizations even in debug builds — huge speedup for codec/image crates:
  ```toml
  [profile.dev.package."*"]
  opt-level = 3
  ```
- Use `cargo build --timings` to identify slow-to-compile crates
- `cargo bloat --release` identifies the largest contributors to binary size

## Benchmarking Discipline

- Never benchmark in debug mode — results are meaningless
- Use `criterion`'s `black_box()` to prevent the compiler from optimizing away benchmark workloads
- Profile with `cargo flamegraph` or `samply` before optimizing — the hotspot is rarely where intuition suggests
- Measure allocation with `dhat` or `heaptrack` when optimizing memory usage
