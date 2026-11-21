---
paths:
  - crates/**/*.rs
  - Cargo.toml
  - crates/*/Cargo.toml
---

# Rust Performance Best Practices

Language idioms are `rust.md`'s; this file is what changes the cost of the code.

## Allocation Reduction

- Pre-allocate: `Vec::with_capacity(n)`, `String::with_capacity(n)`, `HashMap::with_capacity(n)`
- Reuse buffers: `vec.clear()` then refill — retains allocated capacity
- `clone_from(&source)` reuses the target's allocation when possible (unlike `clone()`)
- Use `String::new()` or `Vec::new()` for empty containers: they don't allocate until the first
  push, and clippy's `with_capacity_zero` flags a `with_capacity(0)`
- `Box::map` (1.100) reuses the box's allocation when the new value has the same size and
  alignment; `Arc::map` / `Rc::map` do so only while the pointer is unique, allocating afresh
  otherwise

## String Handling

- Use `Cow<'_, str>` for functions that usually return input unchanged but sometimes modify
- Avoid `format!()` for simple conversions — use `.to_owned()` for a `&str`, `.to_string()`
  otherwise, `.into()`, or string concatenation
- `push_str()` is faster than `format!` for building strings incrementally
- `std::fmt::from_fn` (1.93) builds an ad-hoc `Display` from a closure — formatted output without
  a newtype or an intermediate `String`
- `{integer}::format_into(&mut NumBuffer::new())` (1.98) prints an integer into a stack buffer and
  hands back the `&str`, bypassing `core::fmt`'s dynamic dispatch, for a hot path writing many
  numbers
- `String::from_utf8_lossy_owned` (1.99) moves valid UTF-8 into the `String`;
  `String::from_utf8_lossy(&bytes).into_owned()` copies it

## Collections

- Use Entry API for HashMap insert-or-update: `map.entry(key).or_insert_with(|| value)`
- Consider `SmallVec<[T; N]>` for collections that are usually small but occasionally grow
- Avoid `Box<dyn Trait>` in hot paths — use enums or generics for static dispatch
- `Vec::extract_if` (1.87) / `HashMap::extract_if` (1.88) drain matching elements with a predicate
  in one pass — no retain-plus-collect double walk
- `VecDeque::pop_front_if` / `pop_back_if` (1.93) for conditional queue draining (e.g. a draining
  action queue)
- `Vec::push_mut` / `VecDeque::push_back_mut` (1.95) push and hand back `&mut` to the new element —
  no re-index after the push

## Iterator Patterns

- Iterator chains are zero-cost abstractions — same performance as manual loops
- Avoid intermediate `.collect()` calls — chain operations instead
- Use `.filter_map()` instead of `.filter().map()` for combined operations
- `iter().enumerate()` over manual index tracking
- Prefer `iter().any()` / `iter().find()` over collecting then checking
- `Peekable::next_if_map` (1.94) — conditional consume-and-transform without a separate peek/next
  pair
- `<[T]>::array_windows::<N>()` (1.94) yields const-length `&[T; N]` windows — the compiler knows
  the width (good for frame-based DSP); `slice::as_array::<N>()` (1.93) is the safe `&[T]` →
  `Option<&[T; N]>` conversion
- `<[T]>::as_chunks::<N>()` (1.88) over `chunks_exact(N)` for a constant `N`: the chunks are
  `&[T; N]`, so they destructure (`for &[r, g, b] in rgb.as_chunks::<3>().0`) and a constant
  index needs no runtime check. `.0` drops the remainder as `chunks_exact` does; clippy's
  `chunks_exact_to_as_chunks` holds it
- `f32::algebraic_add` / `algebraic_mul` and siblings (1.98) let LLVM reorder a float reduction (a
  sum, a dot product) so it can vectorize. The result is non-deterministic, even between two calls
  in one run, so never where a test pins bits (`equalizer_tests.rs`' mono/stereo `to_bits` check)
  or two channels must come out identical; measure first

## Compile-Time Optimizations (release profile)

```toml
[profile.release]
lto = "fat"           # Full link-time optimization
codegen-units = 1     # Single codegen unit for better optimization
panic = "abort"       # Smaller binary, no unwind overhead
strip = "debuginfo"   # Drop DWARF but keep symbols — end-user panic backtraces stay readable
                      # ("symbols"/true strips both)
```

- No `target-cpu=native`, or any raised `target-cpu`, on a build that ships: it runs on CPUs we
  never see, and one instruction the user's CPU lacks is an illegal-instruction crash. A local
  benchmark build may use it
- Since Rust 1.90, `rust-lld` is the default linker on `x86_64-unknown-linux-gnu` — dev links are
  fast out of the box (opt out with `-C linker-features=-lld`); `mold` is at best a marginal
  further gain, not a recommendation
- `--remap-path-scope` (stable since 1.95) controls which local paths get remapped out of the
  binary; Cargo's `trim-paths` profile key is **still nightly-only as of 1.100**: a
  `profile.*.trim-paths` key fails with ``feature `trim-paths` is required``. Don't recommend it
  on stable
- **v0 symbol mangling is the default since 1.97** — nothing to configure. It pairs with the
  `strip = "debuginfo"` choice above (kept so end-user panic backtraces stay readable): v0 encodes
  generic parameters, so those frames are now unambiguous instead of collapsing distinct
  monomorphizations onto one legacy-mangled name
- Cargo's `build.warnings` config (stable 1.97) can deny warnings globally. **Don't** — it applies
  to *every* cargo command, so a stray unused variable during a `cargo run` iteration becomes a
  hard error. Keep `-D warnings` on the CI clippy invocation, where the gate belongs

## Profiling

- Always profile before optimizing — `cargo flamegraph` for CPU profiling
- Use `criterion` for microbenchmarks with statistical significance
- `#[bench]` is unstable — prefer criterion for stable Rust
- Use `perf` or `samply` for system-level profiling on Linux

## Miscellaneous

- `to_owned()` and `to_string()` on a `&str` are the same copy: std specializes `ToString` for
  `str` to `String::from`, which is `to_owned()`, so neither is faster. Spell it `to_owned()`, the
  tree's convention
- Use `std::mem::take()` or `std::mem::replace()` instead of clone-then-clear patterns
- Global-allocator swaps (`mimalloc`/`jemalloc`) trade RSS for speed — measure peak RSS before
  adopting. **Melodia caveat:** mimalloc's per-thread segments across the app's many long-lived
  threads caused a large idle-RSS regression and was reverted; don't propose allocator swaps here
- Custom allocators are stable for `Vec` and `Box` (1.100: `Allocator`, `Vec::new_in`,
  `Box::new_in`). A per-collection arena is an RSS decision like the global swap above, measured
  the same way
- Avoid unnecessary `Arc` — single-threaded code doesn't need atomic reference counting
- Use `#[inline]` sparingly — the compiler usually makes good inlining decisions; only hint for
  small hot functions crossing crate boundaries
- `core::hint::cold_path()` (1.95) marks a branch as unlikely — use in hot loops where the
  error/edge arm is rare

## Type-Level Optimizations

- Prefer enums over `Box<dyn Trait>` in data structures — avoids heap allocation and vtable dispatch
- Use `#[repr(C)]` or `#[repr(packed)]` only when ABI compatibility or minimal padding is
  required; measure first
- Newtype wrappers (`struct Foo(u32)`) are zero-cost — use freely for type safety without runtime
  penalty
- `NonZero*` types (e.g. `NonZeroU32`) allow the compiler to use 0 as a niche for
  `Option<NonZeroU32>`, the same size as `u32`. An unsigned value divides or takes a remainder by
  one directly (`x % n`), with no `.get()` and no zero check (clippy's `needless_nonzero_get`)
- `Box<str>` / `Box<[T]>` (`Arc<str>` when shared) for owned data that never changes once built
  and exists in many copies: a two-word handle against `String`'s or `Vec`'s three, and no spare
  capacity. `into_boxed_str()` / `into_boxed_slice()` shrink as they convert; a long-lived
  collection that stays growable, built without an exact reservation, takes `shrink_to_fit()`
  before it is stored

## Concurrency Patterns

- `AtomicUsize` / `AtomicBool` for lock-free counters and flags — cheaper than `Mutex<usize>` for
  simple state
- `Atomic*::update` / `try_update` (1.95, on all atomic types) — closure-based read-modify-write,
  cleaner than a hand-rolled `fetch_update` loop for lock-free shared state
- `RwLock<T>` instead of `Mutex<T>` when reads dominate — allows concurrent reads
- Avoid contention on a single `Mutex` in parallel code — partition state or use thread-local
  storage

## Build & Debug Profile

- **A dev build optimizes only the dependencies `Cargo.toml`'s `[profile.dev.package]` names**, the
  rest building at 0. A new dependency whose concrete code runs per sample, pixel, frame or track
  joins that list, or `cargo run` runs it unoptimized
- Use `cargo build --timings` to identify slow-to-compile crates
- `cargo bloat --release` identifies the largest contributors to binary size

## Benchmarking Discipline

- Never benchmark in debug mode — results are meaningless
- Use `criterion`'s `black_box()` to prevent the compiler from optimizing away benchmark workloads
- Profile with `cargo flamegraph` or `samply` before optimizing — the hotspot is rarely where
  intuition suggests
- Measure allocation with `dhat` or `heaptrack` when optimizing memory usage
