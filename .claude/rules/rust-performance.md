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

## Collections

- Use Entry API for HashMap insert-or-update: `map.entry(key).or_insert_with(|| value)`
- Accept `&[T]` over `&Vec<T>` in function signatures — more flexible, same performance
- Consider `SmallVec<[T; N]>` for collections that are usually small but occasionally grow
- Avoid `Box<dyn Trait>` in hot paths — use enums or generics for static dispatch

## Iterator Patterns

- Iterator chains are zero-cost abstractions — same performance as manual loops
- Avoid intermediate `.collect()` calls — chain operations instead
- Use `.filter_map()` instead of `.filter().map()` for combined operations
- `iter().enumerate()` over manual index tracking
- Prefer `iter().any()` / `iter().find()` over collecting then checking

## Compile-Time Optimizations (release profile)

```toml
[profile.release]
lto = "fat"           # Full link-time optimization
codegen-units = 1     # Single codegen unit for better optimization
panic = "abort"       # Smaller binary, no unwind overhead
strip = true          # Strip debug symbols from release binary
```

- Set `target-cpu=native` via `RUSTFLAGS` for platform-specific SIMD optimizations
- Use `mold` linker for faster dev builds: `RUSTFLAGS="-C link-arg=-fuse-ld=mold"`

## Profiling

- Always profile before optimizing — `cargo flamegraph` for CPU profiling
- Use `criterion` for microbenchmarks with statistical significance
- `#[bench]` is unstable — prefer criterion for stable Rust
- Use `perf` or `samply` for system-level profiling on Linux

## Miscellaneous

- Prefer `to_owned()` over `to_string()` when converting `&str` — avoids `Display` trait overhead
- Use `std::mem::take()` or `std::mem::replace()` instead of clone-then-clear patterns
- Consider `mimalloc` or `jemalloc` as global allocator for 5-10% improvement with no code changes
- Avoid unnecessary `Arc` — single-threaded code doesn't need atomic reference counting
- Use `#[inline]` sparingly — the compiler usually makes good inlining decisions; only hint for small hot functions crossing crate boundaries

## Type-Level Optimizations

- Prefer enums over `Box<dyn Trait>` in data structures — avoids heap allocation and vtable dispatch
- Use `#[repr(C)]` or `#[repr(packed)]` only when ABI compatibility or minimal padding is required; measure first
- Newtype wrappers (`struct Foo(u32)`) are zero-cost — use freely for type safety without runtime penalty
- `NonZero*` types (e.g. `NonZeroU32`) allow the compiler to use 0 as a niche for `Option<NonZeroU32>` — same size as `u32`

## Concurrency Patterns

- `AtomicUsize` / `AtomicBool` for lock-free counters and flags — cheaper than `Mutex<usize>` for simple state
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
