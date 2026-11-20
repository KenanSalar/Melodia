---
paths:
  - crates/**/*.rs
---

# Rust

The prohibitions are `CLAUDE.md`'s and cost is `rust-performance.md`'s; this is how the tree's Rust
is shaped.

## Signatures

- **A newtype guards its invariant at construction** - one that carries an invariant keeps its
  field private, checks the value in its constructor, and converts from the weaker type through
  `TryFrom`, never `From`. A public field or an infallible conversion lets the invalid value in,
  and the type proves nothing.
- **Take what you keep, borrow what you read** - a parameter the function stores or consumes is
  taken owned (`String`, `Vec<T>`, `PathBuf`): a caller done with its value moves it in, one
  that isn't clones at the call site. A `&str` the body `.to_owned()`s copies on every call.
  Clippy's `needless_pass_by_value` catches an owned parameter that is only read, not this.
- **Borrow the data, not its holder** - a parameter that is only read takes `&T`, `&[T]`, `&str`,
  `&Path` or `Option<&T>`, not `&Arc<T>`, `&Rc<T>`, `&Box<T>`, `&Vec<T>`, `&String`, `&PathBuf`
  or `&Option<T>`. Naming the holder ties every caller to one way of storing the value. A
  `&Arc<T>` or `&Rc<T>` is right only where the body keeps a pointer of its own: it clones or
  downgrades it, or hands it to a callee that does. Clippy holds `&Box<T>`, `&Vec<T>`, `&String`
  and `&PathBuf` everywhere (`borrowed_box`, `ptr_arg`) and `&Option<T>` on private items only
  (`ref_option`); nothing holds `&Arc<T>` or `&Rc<T>`.
- **A free function unless it needs the type** - an inherent associated function that neither
  builds `Self` nor takes `self` belongs outside the `impl`. Parked inside, it costs every caller
  a `Type::` and claims a coupling that isn't there.

## Idioms (Rust 1.88–1.100)

- Let chains (1.88, edition 2024 only) flatten a tower of nested `if let`s:
  `if let Some(a) = x && a.ready() { … }`
- An `if let` guard on a match arm (1.95), `Some(x) if let Ok(v) = parse(x) => …`, matches inside
  the guard; guard patterns don't count toward exhaustiveness
- Std over the crate it replaced: `cfg_select!` (1.95) for `cfg-if` in platform-split code,
  `std::sync::LazyLock` / `LazyCell` (1.80) for `once_cell`, and `DropGuard` (below) for
  `scopeguard`. `LazyLock::get` / `get_mut` / `force_mut` (1.94) inspect one without forcing it
- The `Copy` range types in `core::range` (1.96) aren't what `0..1` produces yet, so there is
  nothing to adopt
- Bit-manipulation methods on every integer (1.97) replace hand-rolled shift/mask idioms, which
  clippy's `manual_isolate_lowest_one` and `manual_bit_width` flag. Note the two families return
  **different things**: `isolate_highest_one()` / `isolate_lowest_one()` return the *value* with
  only that bit set (`0b1010_0100` → `0b1000_0000` / `0b0000_0100`), while `highest_one()` /
  `lowest_one()` return the bit *index* as `Option<u32>` (`Some(7)` / `Some(2)`; `None` for zero).
  `bit_width()` gives the bits needed to represent the value (`164` → `8`). The `NonZero<{int}>`
  equivalents return a plain `u32` rather than an `Option`, since the zero case is gone: prefer
  them when the value is already `NonZero`
- `Option` / `Result::map_or_default(f)` (1.98) over `.map(f).unwrap_or_default()`, which clippy's
  `map_unwrap_or` flags
- `bool::ok_or(err)` / `ok_or_else` (1.98) turn a condition into a `Result<(), E>`, so a guard
  reads `(n > 0).ok_or(err)?`
- `str::trim_prefix` / `trim_suffix` (1.100) drop the affix when present and always return `&str`,
  where `strip_prefix(p).unwrap_or(s)` takes two steps
- `core::mem::DropGuard` (1.100) runs a closure on every exit path, `?` and a panic included, and
  `DropGuard::dismiss` disarms it: a one-off cleanup or rollback without a single-use `impl Drop`
  type
- `Vec::from_fn(n, |i| …)` (1.100) builds an `n`-long vector by index, front to back
