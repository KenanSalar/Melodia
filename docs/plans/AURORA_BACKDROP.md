# Aurora Backdrop

Working doc. Delete when the feature ships.

Status: **proposed** · Created: 2026-08-16

> Slint facts below were verified **2026-08-16** against the pinned `slint 1.16.1`
> sources in the registry (`i-slint-core-1.16.1/graphics/brush.rs`,
> `i-slint-compiler-1.16.1/parser/expressions.rs`,
> `i-slint-renderer-femtovg-1.16.1/itemrenderer.rs`), the quantizer facts against
> `material-colors-0.4.2/src/score.rs`, and the prior art against fresh clones of
> `World/amberol` and `neithern/g4music`.

---

## What ships

Every artwork-derived backdrop in the app — Now Playing, the six hero banners, the two
mosaic bands, Genre Detail — stops being a **blurred image** and becomes a **brush stack**:
a two-stop base gradient in the album's hue, with three soft radial blobs of album colour
floating over it.

The claim, and it has to be checkable or it is decoration:

> The backdrop is a value, not a resource. It costs ~80 bytes to hold, interpolates
> itself on every change, has no resolution, and there is nothing to release.

What that buys, in the order the reasons actually matter:

| | today | after |
|---|---|---|
| Crossfade | two `Image` slots, `use-a` bool, slot never cleared | `Brush::interpolate`, one `animate` |
| Held per cover | 72–108 KB buffer + GPU texture | `[u32; 3]` |
| Release protocol | `release_detail_hero_images!`, `release_collapsed_hero`, `forget_mosaic`, `last_mosaic_paths`, `has-blur` | none |
| On a 4K hero | a 192 px texture upscaled ~10× | vector, exact |
| Foreground tones | re-solved per cover, drift between tracks | constant, proven once |
| Mosaic band | decode 4 → blit 2×2 atlas → blur → quantize | 4 seeds, no atlas |
| Genre Detail | special-cased `apply_gradient` | the same path with hashed seeds |

**Not in scope:** any change to the sharp cover tile (`ArtworkCache` keeps producing it),
to `CoverThumbs`, to Material You theme generation, or to the accent solve outside the
backdrop surfaces. No new setting — this replaces the blur rather than sitting beside it.

---

## Prior art — the survey that decided the shape

**Amberol** (`src/utils.rs:114`, `src/window.rs:1405`, `src/gtk/style.css:118`) extracts
4 colours with `color_thief` MMCQ at quality 5 and paints three `linear-gradient`s at
127°/217°/336°, each `color-mix(… 55%, transparent)` fading to 0 at 70.71%. It writes the
colours into a `:root` CSS string and hot-loads it into a display-wide provider on every
track change. It extracts 4 and the stylesheet consumes **3**.

**G4Music** (`src/ui/window.vala:308`, `src/ui/paintables.vala:283`) does no colour
extraction at all — I grepped the tree. Its background is the cover itself, `push_blur`ed
on the GPU and `render_texture`'d **once** into a 512² texture at 25% opacity. Its
`CrossFadePaintable` is our two-slot crossfade in Vala, for the same reason: images can't
interpolate.

Three things to take, one to reject:

- **Take the blob geometry, not the linear angles.** Amberol's diagonal linear fades read
  as corner washes — fine at 55% over a theme background, banding when they *are* the
  surface. A heavy Gaussian of a cover already resolves to three or four soft colour
  blobs; that is what σ=24 on a 192 px buffer produces and why it looks good. Radial is
  the shape that matches the thing being replaced.
- **Take the fade-to-transparent.** It is what makes overlapping layers read as one
  organic field rather than as stacked shapes.
- **Take the user-facing restraint**: both apps use *few* colours. Amberol paints 3.
- **Reject both apps' contrast posture.** Theirs are *tints* over a theme-owned
  background, so contrast is inherited from Adwaita and neither has a single line of
  contrast machinery. Ours is a *replacement* — `ui/backdrop.rs`'s header already argues
  why, and it stays true. The tone-band solve is not optional for us.

**And reject G4Music's efficiency trick, because it is unreachable.** femtovg has
`ImageFilter::GaussianBlur` but Slint only calls it internally for box-shadow rendering
(`i-slint-renderer-femtovg-1.16.1/itemrenderer.rs:569`). No `.slint` element blurs its
content. Our CPU `fast_blur` at 192² already *is* the cheap version of what they do.

---

## Six findings that decide the shape

1. **The blur is not the expensive part.** We downscale to 192² *before* blurring, so
   `fast_blur` is a 3-pass box blur over 37k pixels. The cost centre is
   `QuantizerCelebi` at 128 clusters plus `Score`, and **that stays** — a mesh needs the
   same quantize. Deleting the blur is worth less CPU than intuition says. The case for
   this change is lifecycle and memory, not milliseconds; do not sell it as a speedup.

2. **Gradients are not free to paint.** Four full-bleed gradient quads is *more* fragment
   work than one bilinear texture sample. Paint cost is roughly a wash — better on large
   heroes (no upscale), slightly worse on small ones. Zero upload bandwidth either way.

3. **`Score::score` gives hue-separated colours, but not a guaranteed count.** It walks
   the required hue separation from 90° down to **15°** and takes the first that yields
   `desired` (`material-colors-0.4.2/src/score.rs:130`). At `desired=3` it typically holds
   60–90°. At `desired=6` it is forced toward the 15° floor — near-duplicates by
   construction — and returns *fewer* if it can't get there even then. **Three blobs.**

4. **A monochrome sleeve returns nothing.** `CUTOFF_CHROMA = 5.0` filters it out and the
   crate's own fallback is Google Blue (`score.rs:60`). `seed_from_pixels` already dodges
   this by passing the real dominant as `fallback_color_argb`; the multi-seed version must
   keep doing that, and must fill short lists by rotating the hue of what it has — never
   with a theme colour.

5. **The layer count and gradient type must be fixed.** `Brush::interpolate`
   (`i-slint-core-1.16.1/graphics/brush.rs:551`) animates gradient→gradient stop-for-stop
   only when both ends are the *same* type; mixed types bounce through a solid colour
   (`:647`) and look broken. A variable blob count also means mounting elements
   mid-animation, straight into the `changed`-tracker-in-an-`if` panic in
   `slint-pitfalls.md`. Same type, same stop count, same element count, always.

6. **Slint's radial gradient is a true circle centred in the element, radius = half the
   diagonal** (`itemrenderer.rs:1544`: `0.5 * (w² + h²).sqrt()`). Two consequences, both
   good: it never becomes an ellipse, so one component serves the tall Now Playing page
   and the wide hero bands unchanged; and on a **square** blob rect the circle meets the
   edge midpoints at stop `1/√2 ≈ 0.707` — Amberol's `70.71%`, arrived at independently.
   Blob rects are square and the transparent stop sits at 0.7.

---

## Structure

No new directory. This is a rewrite inside the two modules that already own the problem,
plus one new `.slint` component replacing one existing one.

```
src/ui/backdrop.rs        the solve — rewritten, ~40% smaller
src/ui/hero_backdrop.rs   the hero publisher — unchanged shape, new payload
src/ui/aurora.rs          NEW: seeds → BackdropColors; the blob-fill rule
melodia-ui/ui/components/aurora-backdrop.slint   NEW: replaces hero-blur-backdrop.slint
```

Ownership rules, so this doesn't sprawl:

- **`ui/backdrop.rs` stays the only place a foreground tone is solved.** The WCAG tiers
  (`chrome`/`text`/`muted`) do not move and do not get a second caller. What leaves is the
  *measurement* half — `luma_p90`, `scrim_alpha`, `composited_tone` — because the backdrop
  stops being something we measure and becomes something we state.
- **`ui/aurora.rs` owns seed→blob and nothing else**: the tone/chroma clamp per blob, the
  short-list rotation from finding 4, and the fixed blob geometry. It is the answer to
  "where does a fourth backdrop surface get its colours" — a call, not a copy.
- **`AuroraBackdrop` takes every colour as a defaulted `in property`.** This is the DRY
  fix the current tree is missing and the reason it is missing: `HeroBlurBackdrop` reads
  `HeroBackdrop.*` directly, so Now Playing **cannot** mount it and spells the identical
  four-layer stack inline at `views/now-playing-view.slint:59-97`. Taking the brushes as
  inputs is `MetaChip`'s idiom (`ui-patterns.md`) and is what lets one component serve both
  tiers. **One stack, two mounts** — the duplication is collapsed by this work, not after it.
- **The two globals stay separate, and that is deliberate.** `Player.np-*` and
  `HeroBackdrop.*` have the same shape but different lifetimes: a band stays mounted behind
  an open Now Playing, so merging them would let a track change repaint the hero underneath.
  Share the *component*, never the tier.
- **`ArtworkCache` keeps the sharp cover and loses the blur.** It does not become a seed
  cache — seeds are small enough that `ui::aurora` can hold them in a plain map with no
  eviction policy worth arguing about, and `BlurSpec` (the per-tier blur shape) has nothing
  left to describe.
- **Genre Detail stops being a special case.** `apply_gradient` goes; the genre publishes
  three hashed seeds through the same `apply` every cover uses.

---

## Phases

Each phase leaves the tree working. Phase 3 is a human gate — nothing is deleted before
the look is approved on screen.

### Phase 1 — Seeds beside the sample · no visual change

1. Widen `BackdropSample` to carry `seeds: [Option<u32>; 3]` alongside the existing
   `accent_argb` / `luma`. Fill it from `Score::score(&counts, Some(3), Some(dominant), None)`
   in `seed_from_pixels` — the ranked list is already computed and thrown away past
   `.first()`, so this costs nothing.
2. Add `ui::aurora::blobs(seeds, fallback_hue) -> [u32; 3]`: tone/chroma clamp each seed
   into the blob band, and fill short lists by rotating hue ±25° with a small tone step
   (finding 4). Pure function, fully unit-testable.
3. Nothing reads it yet. `solve()` and every surface behave exactly as today.

**Exit:** `cargo clippy --all-targets --locked -- -D warnings` clean, `cargo test` clean,
no pixel moves. New tests cover the short-list and monochrome paths.

### Phase 2 — `AuroraBackdrop`, mounted on Now Playing beside the blur

1. New `components/aurora-backdrop.slint`: root `Rectangle` with `clip: true`
   (rectangular and borderless, so it lowers to a scissor — `library-tab-band.slint`'s
   argument), a base `@linear-gradient(135deg, …)`, and three square blob `Rectangle`s at
   fixed fractional centres with `@radial-gradient(circle, blob 0%, transparent 70%)`.
   All six colours plus the `hero-open` gate are defaulted `in property`s.
2. Sizing: each blob is a square off the host's short side (~110%), centred at roughly
   15%/20%, 80%/10%, 50%/90%. Numbers land here as named constants with the reasoning at
   their definitions, not inline.
3. Mount it in `now-playing-view.slint` **under** the existing blur stack, gated on a
   temporary `Player.np-aurora` bool so both can be looked at against the same track.
4. Publish the three blob colours onto `Player.np-blob-{1,2,3}` from `track_change.rs`.

**Exit:** both backdrops render; toggling the bool swaps them live. Blur path untouched.

### Phase 3 — Look gate

Run a hundred covers through it — bright, near-black, monochrome, single-hue, busy
gatefolds, missing artwork. The two questions:

- Does it read as *this* record, or as generic album-coloured wallpaper?
- Does the blob geometry ever collide into a flat wash on real covers?

Tune the blob positions, radius and tone band until yes. **This phase can also end in
"no"**, in which case the plan stops here and the two-line toggle is deleted — nothing
downstream has been touched yet. That is the point of the ordering.

### Phase 4 — The solve stops measuring

1. Rewrite `ui/backdrop.rs`: `solve(seeds, …)` computes the composited backdrop tone
   analytically from the stops it owns (extend `gradient_luma_lstar` to N stops), and
   drops `luma_p90`, `scrim_alpha`, `composited_tone`, `PERCENTILE_TAIL`, the histogram
   and the `LINEARIZED` LUT.
2. The scrim becomes a **fixed** low-alpha vignette or goes entirely — decide against what
   Phase 3 actually looked like, not now.
3. `chrome_tone` / `text_tone` / `muted_tone` are unchanged but now solve against a
   constant. Their tests become exact assertions rather than range checks.

**Named trade:** `clamp_to_tone_band` currently lets a naturally bright cover keep its own
chrome tone. Against a constant backdrop, chrome becomes a pure function of hue — more
consistent between tracks, less varied within one. That is the right side of the trade and
`backdrop.rs`'s header should say so in one line.

### Phase 5 — Roll to the six heroes, collapse the duplicate stack

1. Point `HeroBackdrop` at the new payload: gain `blob-{1,2,3}`, lose `blur-img-a/b`,
   `blur-use-a`, `has-blur`.
2. Replace `HeroBlurBackdrop` with `AuroraBackdrop` at both mounts
   (`mosaic-tab-hero.slint:109`, `library-tab-band.slint:234`).
3. Replace Now Playing's inline stack with the same component and delete the toggle. The
   four-layer stack now exists **once**.
4. `hero_backdrop::apply` publishes blobs; the `hero-open` gate carries over to all four
   layers (the don't-ease-out-of-a-held-tier rule in `ui-patterns.md` is unchanged and
   still applies — a brush that eases is a brush that can ease out of a stale value).

### Phase 6 — Mosaic and Genre fold into one path

1. `compose_mosaic_blur` loses the atlas and the blur: take one seed from each of up to
   four covers, rank them, keep three. `mosaic_blur.rs` is left with the seed collection
   or disappears into `ui/aurora.rs` — whichever leaves fewer files.
2. `impl_mosaic_hero!`'s `last_mosaic_paths` guard and `forget_mosaic` go: they exist to
   answer "is this mosaic what's painted", and a brush has no such question.
3. Genre Detail publishes three hashed seeds through `apply`; `apply_gradient` and the
   `hero_color_1/2` special case go.

### Phase 7 — Deletions

Only now, and in one commit so the diff reads as the subtraction it is:

`fast_blur` call sites · `BlurSpec` and both blur-shape constants · `BLUR_TARGET` /
`BLUR_SIGMA` · `ArtworkPair.blur` · `write_crossfade_slot` and its three callers ·
the A/B `Image` slots and `use-a` on both tiers · `has-blur` · `release_detail_hero_images!` ·
`release_collapsed_hero`'s image half · `hero-blur-backdrop.slint`.

`ArtworkCache` keeps `cover` and its LRU; the caps (8 and 12) are re-argued against
covers alone, not carried over.

### Phase 8 — Tests, docs, exit

1. `ui/tests/hero_backdrop_tests.rs` (486 lines) and `backdrop_tests.rs` (550) are largely
   source walks pinning the blur-era shape. Rewrite rather than patch — most of what they
   pin will no longer exist. The walks worth keeping in new form: every hero mounts the
   shared component and grows no stack of its own; no surface spells a `Theme.*` brush on
   a backdrop; the blob count is fixed at three in Slint and in Rust.
2. New pins: `AuroraBackdrop` takes all colours as inputs (a mount reading a global
   directly is the regression); every animated layer is the same gradient type and stop
   count (finding 5).
3. `CLAUDE.md` — the `ui/` bullet's artwork-tier paragraph, and `ui-patterns.md`'s
   "Releasing what the UI pins" section, which shrinks a lot.
4. `README.md` if the backdrop is described there.
5. Delete this file.

---

## Cross-cutting

- **Memory.** The expected direction is down (~1.7 MB of resident blur buffers plus their
  GPU textures), but this is not a memory feature and should not be justified as one —
  we're well under the ceiling. Take one `/usr/bin/time -v` reading after Phase 7 to
  confirm nothing regressed, and don't tune against it.
- **No new setting.** Amberol and G4Music both gate their recolouring because theirs is an
  optional tint over a working theme background. Ours is the surface; there is nothing to
  fall back to and nothing to toggle.
- **Threading is unchanged.** Seeds come out of the same `spawn_blocking` that already
  runs the quantize; the publisher still writes on the UI thread.
- **The section-gating rules are unchanged.** A hero may still publish into a shared global
  only while it is the one on screen — `ui-patterns.md`'s hero contract survives this
  intact. What goes is only the *image release* half of it.

---

## Open questions

- **Scrim: keep as a fixed vignette, or delete?** Depends entirely on how the blobs read
  at the edges in Phase 3. Deleting it removes a layer; keeping it may be what stops the
  corners looking empty.
- **Should Now Playing get a fourth blob?** It is a full page where the bands are strips,
  so it has room. Argues against the fixed-count rule (finding 5) unless the count is fixed
  *per component instance* rather than globally — which is legal, since each mount animates
  only against itself. Decide in Phase 3, default to three.
- **Do seeds want persisting?** A `blob_seeds` column on `tracks`/`albums` would make every
  hero instant on a cold open with no decode at all. Real, and out of scope — but the shape
  chosen here should not make it harder. It doesn't: three `u32`s serialize trivially.
