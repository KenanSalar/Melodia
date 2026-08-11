# The band morph's offscreen layers

Working doc. Investigation owed by the `fix/library-tabbar` performance pass — the one
finding from that review deliberately left unchanged, because the fix touches a shared
component and a regression there is invisible on a Latin locale. Delete this once acted on.

Everything below is read out of the pinned crate sources (`i-slint-core-1.16.1`,
`i-slint-renderer-femtovg-1.16.1`) rather than from the review that raised it. Where I am
inferring rather than quoting, it says so.

---

## 1. The mechanism, as the source states it

**`Opacity::need_layer`** — `i-slint-core-1.16.1/items.rs:948`:

```rust
pub fn need_layer(self_rc: &ItemRc, opacity: f32) -> bool {
    if opacity == 1.0 { return false; }
    let opacity_child = match self_rc.first_child() {
        Some(first_child) => first_child,
        None => return false,          // No children? Don't need a layer then.
    };
    if opacity_child.next_sibling().is_some() { return true; }
    opacity_child.first_child().is_some()
}
```

Two bails, exactly as the tree's rules describe them: **exactly** `1.0`, and a lone child
that is itself childless. Past both, `visit_opacity` calls `render_and_blend_layer`
(`itemrenderer.rs:640`).

**And a second, independent layer source that the review did not account for** —
`visit_clip`, `itemrenderer.rs:684`:

```rust
let radius = clip_item.logical_border_radius();
if !radius.is_zero() {
    if let Some((layer_origin, layer_image)) = self.render_layer(item_rc, &|| item_rc.geometry())
```

with `combine_clip` (the cheap scissor path) carrying `debug_assert!(radius.is_zero())` and
the comment *"femtovg only supports rectangular clipping. Non-rectangular clips must be
handled via `apply_clip`, which can render children into a layer."*

So **a rounded `clip: true` over children is a layer at all times**, not only while
something fades. That is the tree's own `slint-pitfalls.md` entry, confirmed at the source.

**What a layer costs, and when it costs most** — `render_layer`, `itemrenderer.rs:1112`:
the texture is reused only when `layer_texture.size() == Some(size)`; otherwise it calls
`Texture::new_empty_on_gpu` and bumps the `layers_created` metric. The size comes from
`render_and_blend_layer`'s bounding-rect closure (`:1186`):

```rust
item_children_bounding_rect(item_rc, &window_adapter)
    .intersection(&current_clip.union(&item_rc.geometry()))
```

— the subtree's bounds **intersected with the current clip**.

## 2. The two sites

### 2a. `library-tab-band.slint:473` — the artwork tile

```slint
if root.hero-shown: VerticalLayout {
    alignment: start;
    horizontal-stretch: 0;
    opacity: root.hero-t;
    ArtworkImage { … tile-size: Theme.hero-artwork; … }
}
```

`need_layer` is true throughout: `hero-t` is only `1.0` at rest, and the lone child
(`ArtworkImage`) has children. So the whole 140 px tile is rendered to an offscreen texture
and blitted, on every frame of the 400 ms morph.

**The part that makes it more than a fixed cost:** the band's root carries `clip: true`
against its *animating* height, so `current_clip` changes every frame. The bounding rect is
intersected with it, so on the leg where the band is shorter than the tile the layer's size
moves frame to frame — and a size change is precisely the branch in `render_layer` that
allocates a **new GPU texture** rather than reusing one. On the rest of the morph the size
stabilises and the texture is reused, though still re-rendered.

### 2b. `library-tab-band.slint:463` — the back disc

```slint
opacity: clamp(root.hero-t * 2.0, 0.0, 1.0);
```

on an `IconButton`, whose root is a `Rectangle` with children. Same verdict, for the first
half of the morph (the clamp reaches 1.0 at `hero-t == 0.5`, where `need_layer` then bails).
Smaller subtree, so a smaller texture.

### 2c. The one the review missed, and it is the larger number

`ArtworkImage`'s own root (`artwork-image.slint:43-47`) is:

```slint
width: tile-size;  height: tile-size;
border-radius: tile-radius;
background: tile-bg;
clip: true;
```

with `if`-gated `Image` children. Non-zero radius over children ⇒ `visit_clip` takes the
`render_layer` branch. **Every `ArtworkImage` in the tree is already a layer, always** — not
only during a morph, and not only in the band. In the hero's circular-artist case
(`tile-radius: Theme.hero-artwork / 2`) it is a full circle, so there is no degenerate case
where the radius happens to be zero.

That reframes 2a: the morph is a layer **over** a layer, and only the outer one is the
morph's.

## 3. Why the current shape was chosen

The band argues it at `:476-480`, and the argument is about **ink**, not about layers:

> **The one fade in the band that is still an `opacity`**, and what makes it safe is that
> the tile is an image: it fills its box, so the morph's layer has no ink to crop the way
> the title block's had. The cost argument holds too — it settles at 1.0, so there is no
> layer at rest.

The first half is correct and is why the title block moved to brush alpha while this did
not: an `Opacity` layer is sized to child *geometry*, and a text run's ink leaves its line
box (the patched Vazirmatn faces reach a quarter of an em above and a third below), so
fading a text block crops Arabic marks for the duration. An image has no such overhang.

The second half — *"it settles at 1.0, so there is no layer at rest"* — is true and is not
the same claim as "the layer is cheap". It prices the steady state and says nothing about
the 400 ms in between, which is where the per-frame allocation in §2a lives. That gap is
what this document exists to name; the comment is not wrong, it answers a different
question.

## 4. What a fix would touch

**`ArtworkImage` has 15 mounts, not the six the plan estimated.** Correcting that here
because it is the whole of the risk assessment:

| file | line |
|---|---|
| `components/hero/library-tab-band.slint` | 485 |
| `components/track-list/track-list-row.slint` | 419 |
| `components/grid/entity-card.slint` | 86 |
| `components/grid/cover-mosaic.slint` | 20, 30 |
| `components/now-playing/up-next-list.slint` | 54 |
| `components/dialog/tag-editor-body.slint` | 73 |
| `components/dialog/selectable-picker.slint` | 77 |
| `components/dialog/playlist-mosaic-picker.slint` | 77, 236 |
| `layout/now-playing-bar.slint` | 138 |
| `views/mini-player.slint` | 71, 262 |
| `views/search/top-result-card.slint` | 51 |
| `views/queue-row-item.slint` | 272 |

The shape that removes the outer layer without touching any of the other fourteen:

- `ArtworkImage` gains `in property <float> fade: 1.0`.
- Its root paints `background: root.tile-bg.transparentize(1.0 - root.fade)`.
  **`transparentize`, not `with-alpha`** — the tree has paid for this once already
  (`slint-pitfalls.md`, the `TabBarCell` entry): `with-alpha` *sets* alpha where
  `transparentize` multiplies it, and the default `tile-bg` is
  `Theme.accent.with-alpha(0.15)`, so a `with-alpha(fade)` spelling would make every
  coverless tile in the app fully opaque.
- Each inner `Image` branch takes `opacity: root.fade`. Those are **childless**, so
  `need_layer` bails on the second condition and they cost nothing.
- `library-tab-band.slint` drops the wrapping `opacity` and passes `fade: root.hero-t`.

The other fourteen mounts take the default `1.0`, where `need_layer` bails on the first
condition and `transparentize(0)` is an identity — so they are unchanged in behaviour. They
do each gain one brush evaluation, which is noise on a root that already binds `background`
(no `Empty`→`Rectangle` promotion, so the "explicit background costs a discarded path"
pitfall does not apply).

**The rounded clip stays.** It is not removable — it is what makes the tile a rounded tile,
and flattening it is the HiDPI-upscale pitfall in reverse. So this buys the *outer* layer
only, and specifically the per-frame texture allocation the animating clip causes.

The back disc (§2b) needs the same treatment on `IconButton`, which is a wider change
(`hover-bg` and `icon-color` are brushes and fold cleanly, but it has many more mounts). I
would do the tile first and measure before deciding whether the disc is worth it.

## 5. Magnitude, honestly

One 140 px texture, allocated-or-reused and blitted per frame, for 400 ms, on a morph the
user triggers by drilling into or out of a My Library detail. Not a scroll path, not a
steady state, and not something anyone has reported. The disc is a second, smaller one for
half that window.

What makes it worth writing down rather than dismissing is the allocation, not the blit:
`render_layer` creating a new GPU texture on a size change is the branch that is genuinely
per-frame while the band's clip is moving, and it is the one part that does not show up in
a "we settle at 1.0" cost argument.

I have **not** measured this. There is no flamegraph or frame-time number behind it, and
per this tree's own standard that means it is a mechanism worth fixing on structural
grounds, not a hotspot with a number attached.

## 6. Verdict

**Fix 2a; leave 2b for now; leave 2c alone.**

- **2a — do it.** The change is bounded (one component, one mount that behaves differently),
  the correct spelling is already documented in the tree, and it removes the only per-frame
  GPU allocation on the morph path. It also removes the last `opacity` in the band, which
  makes the band's own rule — *fade by brush alpha, never by `opacity`* — true without an
  exception clause, and an invariant with no exceptions is the kind a later edit cannot get
  wrong by copying the wrong neighbour.
- **2b — defer.** `IconButton` has enough mounts that the risk/benefit is worse than the
  tile's, and the disc's layer is smaller and lives for half the window. Revisit after 2a.
- **2c — leave.** The rounded clip is the component's whole visual identity, and the layer
  it costs is size-stable and cached. This is a fact worth knowing (it is 15 always-on
  layers, which is a bigger number than anything else in this document) but not a defect.

Owed with the fix: a pin in `ui::library_tab_band_tests` that the band passes `fade` and
mounts no `opacity`, since the failure mode is a silent return to the layer.
