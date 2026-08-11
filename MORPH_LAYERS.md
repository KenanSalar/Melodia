# The band morph's offscreen layers

Working doc. **§2a is done; §2b is the one open item.** Delete this once 2b is decided
either way — 2c's durable half has already been folded into `slint-pitfalls.md`'s
`opacity` entry, so nothing here is lost with the file.

Everything here is read out of the pinned crate sources (`i-slint-core-1.16.1`,
`i-slint-renderer-femtovg-1.16.1`).

---

## 1. The mechanism

**`Opacity::need_layer`** — `i-slint-core-1.16.1/items.rs:948`: bails at **exactly** `1.0`,
and on a lone child that is itself childless. Past both, `visit_opacity` calls
`render_and_blend_layer` (`itemrenderer.rs:640`). Below both, it calls `apply_opacity` —
one float multiply and a canvas `set_global_alpha` (`itemrenderer.rs:925`).

**A second, independent layer source** — `visit_clip` (`itemrenderer.rs:684`) takes the
`render_layer` branch whenever `!radius.is_zero()`, and `combine_clip` (the cheap scissor
path) carries `debug_assert!(radius.is_zero())` plus *"femtovg only supports rectangular
clipping. Non-rectangular clips must be handled via `apply_clip`, which can render children
into a layer."* So a rounded `clip: true` over children is a layer at all times.

**What a layer costs most** — `render_layer` (`:1099`) reuses its texture only when
`layer_texture.size() == Some(size)`; otherwise `Texture::new_empty_on_gpu`. **The size comes
from a closure its caller passes, and the two callers pass different ones**, which is the
whole of why 2a below is a per-frame allocation and 2c is not. `render_and_blend_layer`
(`:1183`) passes `item_children_bounding_rect(…).intersection(&current_clip.union(&item_rc
.geometry()))` — the subtree's bounds **intersected with the current clip**, so an animating
clip moves it every frame. `visit_clip` passes `|| item_rc.geometry()` — the clipping
element's own box, and nothing else.

The cache is a real `PropertyTracker` (`ItemCache::get_or_update_cache_entry`,
`i-slint-core/item_rendering.rs:54`, `evaluate_if_dirty`), so a layer is re-rendered when a
property read while drawing its children is dirtied — and the size closure is only
re-evaluated then too.

## 2a. The artwork tile — **fixed**

`library-tab-band.slint` wrapped its `ArtworkImage` in `opacity: root.hero-t`. `need_layer`
was true throughout (`hero-t` is only `1.0` at rest; `ArtworkImage`'s root has children), so
the 140 px tile rendered to an offscreen texture for the whole 400 ms morph — and because
the band clips against its *animating* height, the bounding rect rode a clip that moved
every frame, which is `render_layer`'s allocate-a-fresh-texture branch.

The band's own comment exempted it on **ink** — an image fills its box, so the layer had
nothing to crop the way the title block's had. That was correct and was never the whole
argument; it priced the steady state (*"it settles at 1.0, so there is no layer at rest"*)
and said nothing about the 400 ms in between.

**What landed:** `ArtworkImage` takes `in property <float> fade: 1.0`, folds it into
`background: tile-bg.transparentize(1.0 - root.fade)` and onto `opacity: root.fade` on each
of its seven childless `Image` branches; the band drops the wrapper and passes
`fade: root.hero-t`. The fourteen other mounts take the default, where `need_layer` bails on
its first condition and `transparentize(0)` is an identity.

Two things worth keeping:

- **`transparentize`, not `with-alpha`.** The first multiplies the alpha, the second sets
  it, and `tile-bg` defaults to `Theme.accent.with-alpha(0.15)` — the wrong one turns every
  coverless tile in the app opaque. (`slint-pitfalls.md`, the `TabBarCell` entry.)
- **The six fallback icons fade on `opacity`, not through `colorize`** — and that is the
  non-obvious half. `colorize` takes a brush, so folding alpha into it looks free; it isn't,
  because the renderer caches the colorized result as its own texture
  (`ItemGraphicsCacheEntry::ColorizedImage`, `itemrenderer.rs:1212`), so a brush that moves
  per frame re-renders that texture per frame. That relocates the cost rather than removing
  it. An `opacity` on a childless `Image` is the cheap path.
- **What the fade still costs, stated precisely.** The `Image`'s own `opacity` allocates no
  layer — but it is read *inside* the root's rounded clip, so it dirties **that** layer's
  tracker on every frame of the morph, where before the fix it was static across it. The
  trade is a good one and is the whole point: the layer now re-rendering is the size-stable
  one, so its texture is reused and the per-frame `Texture::new_empty_on_gpu` is gone. "A
  canvas alpha multiply rather than a layer" is true of the `Image`; it is *not* true that
  no layer work is left on the path.
- **Per-element alpha is not pixel-identical here**, unlike the title block's, because the
  fill and the image overlap: the composite gains a `(1 - fade)·a_bg·fade` term, peaking at
  `a_bg / 4` — about 0.04 on the cover arm's `chrome.with-alpha(0.15)`, 0.075 on Genre's
  opaque gradient under its 0.3 glyph. The tile reads a few percent heavier mid-fade. Worth
  knowing before quoting the title block's "paints identically" argument at a third site.

Pinned by `ui::library_tab_band_tests::the_hero_fades_on_the_morph_at_both_ends`, which
asserts no `if root.hero-shown:` half carries `opacity: root.hero-t;` *and* that the tile
still fades through `fade:`. Mutation-checked: restoring the wrapper fails it.

## 2b. The back disc — **open**

`library-tab-band.slint:463`, on an `IconButton` whose root is a `Rectangle` with children:

```slint
opacity: clamp(root.hero-t * 2.0, 0.0, 1.0);
```

Same verdict, for the first half of the morph — the clamp reaches `1.0` at `hero-t == 0.5`,
where `need_layer` then bails. Smaller subtree, so a smaller texture, and it lives for half
the window.

**The fix is not the same shape, and that is what leaves this open rather than merely
deferred.** `IconButton` carries `animate background` (`icon-button.slint:67`) and
`animate icon-color` (`:89`), and an animated *binding* restarts on dependency **dirtiness**
rather than on a value change — so a `fade` folded into `idle-bg` / `hover-bg` / `idle-fg`
re-dirties both every frame and stalls each crossfade for the length of the morph. That is
`TabBarCell`'s round-two failure verbatim, and its escape hatch fails here the same way it
did there: "no pointer is on the disc while it animates" is false precisely because you
*click the disc* to close the detail. `slint-pitfalls.md` anticipated it — *"if a band ever
routes the back disc's tiers through its mirrors, they go the same way."* The mount count
(33 against `ArtworkImage`'s 15) is the weaker half of the argument, not the blocker.

The doubled bias is the second constraint, and whatever carries the fade has to preserve it:
the disc is the one hero element whose *size* also rides `hero-t`, so a plain alpha made its
presence go as `hero-t` squared and read as a pop at the end. **Correcting this doc's earlier
claim:** fixing the tile did *not* remove the last `opacity` in the band — this is it.

## 2c. `ArtworkImage`'s own rounded clip — **left alone, and durable**

Its root is `border-radius: tile-radius; background: tile-bg; clip: true;` over `if`-gated
`Image` children, so `visit_clip` takes the `render_layer` branch **unconditionally**. Every
`ArtworkImage` in the tree is already a layer, always — 15 mounts, not only during a morph
and not only in the band. In the circular-artist case (`tile-radius: Theme.hero-artwork / 2`)
there is no degenerate zero-radius case either.

Not a defect: the rounded clip is the component's whole visual identity, flattening it is the
HiDPI-upscale pitfall in reverse, and the layer is size-stable and cached — `visit_clip` sizes
it off `item_rc.geometry()`, so a moving clip can't reach it. It is the largest number in this
document and the one most worth knowing, and **it now lives in `slint-pitfalls.md`'s `opacity`
entry** beside the tile's own fix.

## Magnitude, honestly

2a removed one 140 px texture, allocated-or-reused and blitted per frame for 400 ms, on a
morph the user triggers by drilling into or out of a My Library detail. Not a scroll path,
not a steady state, and not something anyone reported.

**Still unmeasured.** No flamegraph, no frame-time number — before and after. This was fixed
on the mechanism (a per-frame GPU allocation that a "we settle at 1.0" cost argument cannot
see), not on a profile, and per this tree's own standard that is worth saying rather than
implying otherwise.
