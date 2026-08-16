# Aurora Backdrop

Working doc. Delete when the feature ships.

Status: **accepted** · Created: 2026-08-16 · Phases 0–5 landed; 6–8 remaining

> Slint facts below were verified **2026-08-16** against the pinned `slint 1.16.1`
> sources in the registry (`i-slint-core-1.16.1/graphics/brush.rs`,
> `i-slint-compiler-1.16.1/parser/expressions.rs`,
> `i-slint-renderer-femtovg-1.16.1/itemrenderer.rs`), the quantizer facts against
> `material-colors-0.4.2/src/score.rs`, and the prior art against fresh clones of
> `World/amberol` and `neithern/g4music`.
>
> The **tree** facts were verified a second time before implementation, and that pass
> moved three things: the blur slots are not on `HeroBackdrop`, `write_crossfade_slot`
> also drives the sharp cover, and the mosaic wants to keep its atlas. Each is argued
> where it bites rather than listed here. **The third was later reversed** — Phase 6 now
> deletes the atlas rather than keeping it, and says why at its own heading.
>
> **Phase 3 rewrote what the stack is made of, and what the feature is.** Six rounds on
> screen retired the layer shape this doc proposed and several of its numbers; "What the look
> gate settled" below records what replaced them. It also reversed the premise: the blur is
> **kept** and the two become a user setting, so no phase deletes anything. Everything past
> Phase 3 is stated against that, and the sentences it contradicts are marked where they sit
> rather than quietly rewritten.

---

## What ships

Every artwork-derived backdrop in the app — Now Playing, the six hero banners, the two
mosaic bands — gains a second way of being drawn: a **brush stack** of a two-stop base
gradient in the album's hue, four broad radial washes of album colour over it, a vignette,
and a tile of noise to keep the whole thing off the 8-bit grid.

**Both stay, the blur keeps its position, and the user may trade it away.** The blurred cover
remains the default and the aurora is the opt-in, in Settings → Interface, persisted like every
other preference.

That is a reversal of what this doc originally proposed — it argued the aurora should *replace*
the blur, and the lifecycle case below was the argument. Two things changed. The blur turned out
to be worth keeping on its own merits: it carries the cover's own spatial structure, which no
synthesis of four colours can, so it will probably always look better. And the aurora turned out
to be good enough that trading a little of that for the lifecycle is a reasonable thing to want
— which makes this a preference rather than a migration.

**Which way round the default sits is the whole shape of the feature.** Blur-by-default means an
existing install looks identical after the upgrade and nobody's backdrop changes under them; it
also means the aurora is only ever seen by someone who goes looking. That is the trade taken.

The aurora's claim, which has to be checkable or it is decoration:

> The backdrop is a value, not a resource. It costs ~80 bytes to hold, interpolates
> itself on every change, has no resolution, and there is nothing to release.

**One asterisk, added in Phase 3 and not to be quietly dropped:** the stack also draws a
64×64 noise tile, because FemtoVG does not dither its gradients and a ramp this wide bands
without one. Per *process*, not per cover, and nothing releases it — so the lifecycle claim
holds and the "no resource at all" one doesn't.

What the aurora buys over the blur, which is also what the setting's subtext has to be true to:

| | blur | aurora |
|---|---|---|
| Crossfade | two `Image` slots, `use-a` bool, slot never cleared | `Brush::interpolate`, one `animate` |
| Held per cover | 72–108 KB buffer + GPU texture | `[u32; 4]` |
| Release protocol | the blur half of `release_hero_slots!`, `forget_mosaic`, `last_mosaic_paths`, `has-blur` | none |
| Per track change | decode → downscale → `fast_blur` → texture upload | the quantize both already pay |
| On a 4K hero | a 192 px texture upscaled ~10× | vector, exact |
| Mosaic band | decode 4 → blit 2×2 atlas → blur → quantize | neither — the band is **deleted** in Phase 6 and its heroes become ordinary artwork ones |

**Not in scope:** any change to the sharp cover tile (`ArtworkCache` keeps producing it),
to `CoverThumbs`, to Material You theme generation, or to the accent solve outside the
backdrop surfaces.

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
   same quantize, so both settings pay it. Skipping the blur is worth less CPU than intuition
   says; what the setting saves is mostly the buffer, its GPU texture and the upload. The case
   is lifecycle and memory, not milliseconds — and the subtext must not oversell it either.

2. **Gradients are not free to paint.** Four full-bleed gradient quads is *more* fragment
   work than one bilinear texture sample. Paint cost is roughly a wash — better on large
   heroes (no upscale), slightly worse on small ones. Zero upload bandwidth either way.

3. **`Score::score` gives hue-separated colours, but not a guaranteed count.** It walks
   the required hue separation from 90° down to **15°** and takes the first that yields
   `desired` (`material-colors-0.4.2/src/score.rs:131`). At `desired=3` it typically holds
   60–90°. At `desired=6` it is forced toward the 15° floor — near-duplicates by
   construction — and returns *fewer* if it can't get there even then. **Three blobs.**

   **The first element is invariant across `desired`**, which is what makes widening the
   call a no-op rather than a change to be re-eyeballed: `chosen_colors` is cleared at the
   top of every degree pass and its first push is unconditional, so entry 0 is the
   top-scored survivor whichever separation the walk settles on.

4. **A monochrome sleeve returns nothing.** `CUTOFF_CHROMA = 5.0` filters it out and the
   crate's own fallback is Google Blue (`score.rs:60`). `seed_from_pixels` already dodges
   this by passing the real dominant as `fallback_color_argb`; the multi-seed version must
   keep doing that, and must fill short lists by rotating the hue of what it has — never
   with a theme colour. **The crate will not do the filling**: the fallback is pushed only
   when the list is *entirely* empty (`score.rs:157`) and never tops up a short one, so
   rotation is the caller's job or a two-seed cover silently paints two blobs.

5. **The layer count and gradient type must be fixed.** `Brush::interpolate`
   (`i-slint-core-1.16.1/graphics/brush.rs:551`) animates gradient→gradient stop-for-stop
   only when both ends are the *same* type; mixed types flatten to a solid at the midpoint
   and expand back out of it (`:648-661`), so the geometry is hidden rather than blended.
   Mismatched *counts* within a type don't panic, they degrade — and the linear arm is the
   worse one, collapsing the surplus stops' positions to 1.0 while leaving their colours
   untouched (`:581-583`), where the radial arm at least converges them to the target's
   last stop. A variable blob count also means mounting elements mid-animation, straight
   into the `changed`-tracker-in-an-`if` panic in `slint-pitfalls.md`. Same type, same stop
   count, same element count, always.

6. **Slint's radial gradient is a true circle centred in the element, radius = half the
   diagonal** (`itemrenderer.rs:1544`: `0.5 * (w² + h²).sqrt()`). There is exactly one
   form — the compiler rejects `ellipse` and rejects `at`, so the centre cannot be moved
   and the shape cannot be squashed (`i-slint-compiler-1.16.1/passes/resolving.rs:574`,
   `:580`). Position therefore comes from the *rect*, never from the gradient.

   **The circle never adapts to aspect ratio, so square blob rects are load-bearing rather
   than tidy.** A 1000×200 element would expose only stops 0…0.19 on its short axis and
   read as a horizontal band. Kept square, the circle meets the edge midpoints at
   `1/√2 ≈ 0.707` — Amberol's `70.71%`, arrived at independently — which is why one
   component serves the tall Now Playing page and the wide hero bands unchanged. Blob
   rects are square and the transparent stop sits at 0.7.

---

## Structure

No new directory. This is a rewrite inside the two modules that already own the problem,
plus one new `.slint` component replacing one existing one.

```
src/ui/backdrop.rs        the solve — rewritten, ~40% smaller
src/ui/hero_backdrop.rs   the hero publisher — unchanged shape, new payload
src/ui/aurora.rs          NEW: seeds → BackdropColors; the blob-fill rule
src/services/material_you.rs   the one quantize path, widened to a ranked list
melodia-ui/ui/components/aurora-backdrop.slint   NEW: sits beside hero-blur-backdrop.slint
```

Ownership rules, so this doesn't sprawl:

- **The setting reaches the *decode*, not just the mount.** Gating only which component paints
  leaves every cover still decoded, blurred, uploaded and released — the whole cost the aurora
  exists to avoid, paid by a user who switched it off. `fast_blur` and the buffer it produces
  are what the flag skips; the quantize is not, both paths seeding their foreground tiers from
  it. That makes `ArtworkPair.blur` an `Option`, which is also what makes the subtext honest.
- **`ui/backdrop.rs` stays the only place a foreground tone is solved**, and now solves against
  two surfaces. The WCAG tiers (`chrome`/`text`/`muted`) do not move and do not get a second
  caller, but the tone they are solved against does: a measured `luma_p90` → `scrim_alpha` →
  `composited_tone` for the blur, a constant for the aurora, whose brightest point `ui::aurora`
  states. The measurement half stays — it is the blur's, and the blur is staying.
- **`ui/aurora.rs` owns seed→blob and nothing else**: the tone/chroma clamp per blob, the
  short-list rotation from finding 4, and the fixed blob geometry. It is the answer to
  "where does a fourth backdrop surface get its colours" — a call, not a copy.
- **`AuroraBackdrop` takes every colour as a defaulted `in property`.** This is the DRY
  fix the current tree is missing and the reason it is missing: `HeroBlurBackdrop` reads
  `HeroBackdrop.*` directly, so Now Playing **cannot** mount it and spells the identical
  four-layer stack inline at `views/now-playing-view.slint:59-99`. Taking the brushes as
  inputs is `MetaChip`'s idiom (`ui-patterns.md`) and is what lets one component serve both
  tiers. **One stack, two mounts** — the duplication is collapsed by this work, not after it.
- **The choice is one `if`/`else` at each mount, never a branch inside either component.**
  Both are full-bleed stacks that own their own layering, and a component asked to be either
  would carry both. Three sites end up with the pair: Now Playing, `MosaicTabHero`,
  `LibraryTabBand`. Whichever is unmounted costs nothing, Slint dropping the branch outright.
- ~~**The blur keeps its inline copy on Now Playing, and that stays a known wart.**~~ **Reversed
  in Phase 5.** The argument for deferring was that bundling it would put the riskiest edit in the
  least-related phase; Phase 5 turned out to be the phase that rewrites all three mounts, so it was
  the most related one. `HeroBlurBackdrop` took the same defaulted inputs `AuroraBackdrop` has and
  the inline copy went. **Both stacks name no global** — that is now the rule, not a property of
  one of them.
- **The two globals stay separate, and that is deliberate.** `Player.np-*` and
  `HeroBackdrop.*` have different lifetimes: a band stays mounted behind an open Now
  Playing, so merging them would let a track change repaint the hero underneath. Share the
  *component*, never the tier. They are not the same *shape* either — `Player` carries a
  four-tier accent family, `HeroBackdrop` carries `chip-fill-at()`/`disc-hover`/`tile-edge`
  — so the sharing has to be by parameter rather than by moving one onto the other.
- **The tints belong on `HeroBackdrop`, beside `chrome` and `floor-*`.** They are a solved
  colour set, and that global already *is* the solved colour set for all six heroes. The blur
  slots sit on the six per-view globals instead because an image belongs to the view that
  decoded it; a brush doesn't. Both sets now coexist, and `my-library-view.slint`'s three-way
  fan-in over the blur quartet stays exactly as it is — the aurora needs no equivalent,
  reading one shared global rather than whichever detail is open.
- **`ArtworkCache` keeps the sharp cover and makes the blur optional.** It does not need to
  become a seed cache: seeds already ride `ArtworkPair` through the same path-keyed LRU, at
  sixteen bytes beside a cover buffer, so a second map would be a second eviction policy for
  nothing. `BlurSpec` still describes the per-tier blur shape, since there is still a blur.
- **The quantize stays in one place.** `seed_from_pixels` also feeds the app-wide Material
  You palette, which is out of scope — so it delegates to a ranked-list function rather than
  being widened, and the theme path keeps asking for one seed.
- **Genre Detail is out of scope, and not because it's awkward.** It has no artwork at all, so
  neither backdrop has anything to derive from — its name-hashed two-stop gradient *is* the
  genre's identity, and it already reads smooth. `apply_gradient`, `gradient_luma`, `rgb_lstar`
  and `GenreRow.hero_color_1/2` all stay untouched, and the setting doesn't reach it: there is
  no third rendering of a thing that was never artwork-derived.
- **The setting itself is `settings.json` + the `Settings` global**, wired like every other
  toggle: `#[serde(default)]` on a shipped file, `ui::settings_bind::toggle_binding` for the
  apply-then-persist shape. It is a *preference*, not view state, so it does not go near
  `views.json`.
- **It is restart-gated, and that is what keeps the rest of this simple.** The two artwork
  caches are constructed once at boot, so a flag read there decides whether a `BlurSpec` exists
  at all — the decode question is answered in one place instead of threaded through every call,
  and no cache can hold a pair produced under the other setting. Live switching would need both,
  plus a generation counter: a decode already in flight when the flag flips lands *after* the
  cache clear and re-poisons it, which no amount of clearing fixes. The dialog already exists
  and takes a third case cleanly — `restart-titlebar` and `restart-tray` are the two worked
  examples, both ending at `window_chrome::request_respawn_and_quit`, which can decline and
  raise a sticky toast rather than vanishing.

---

## Phases

Each phase leaves the tree working. Phase 3 is a human gate, and its verdict was to keep both
backdrops — so no phase deletes anything, and the ordering that protected the blur until the
look was approved now protects it permanently.

**Phases 1 and 2 land together**, the tree having no way to hold the first on its own:
`dead_code` is a workspace `warn` and CI runs `-D warnings`, so a `seeds` field nothing reads
fails the gate, and there is no `#[allow]` to reach for. They stay separate below because they
are separate arguments; they are one commit.

### Phase 1 — Seeds beside the sample · no visual change

1. Lift the quantize out of `seed_from_pixels` into `ranked_seeds(pixels, desired)`, and
   leave `seed_from_pixels` asking it for one. **The ranked list is not free today** — the
   call is `desired: 1`, so the hue walk exits on its first pass and there is no discarded
   tail; `desired: 3` makes it walk. Still noise beside `QuantizerCelebi`, but the saving
   is not the reason to do it, and `accent_argb` staying put (finding 3) is.
2. Widen `BackdropSample` to carry `seeds: [Option<u32>; 3]` alongside `accent_argb` /
   `luma`, taking `accent_argb = seeds[0]` off one call. It must stay `Copy` and `Default`
   — `mosaic_hero.rs` copies it out of an `Option<MosaicBlur>` by reference.
3. Add `ui::aurora::blobs(seeds, fallback_hue) -> [u32; 3]`: tone/chroma clamp each seed
   into the blob band, and fill short lists by rotating hue ±25° with a small tone step
   (finding 4). Pure function, fully unit-testable.
4. Nothing reads it yet. `solve()` and every surface behave exactly as today.

**Exit:** `cargo clippy -p Melodia --all-targets --locked -- -D warnings` clean,
`cargo test` clean, no pixel moves. New tests cover the short-list and monochrome paths,
and pin `seeds[0]` against the single-seed answer — the invariant the no-op rests on.

### Phase 2 — `AuroraBackdrop`, mounted on Now Playing beside the blur

1. New `components/aurora-backdrop.slint`: root `Rectangle` with `clip: true`
   (rectangular and borderless, so it lowers to a scissor rather than the offscreen layer a
   rounded clip would cost — `slint-pitfalls.md`'s argument), a base
   `@linear-gradient(135deg, …)`, and three square blob `Rectangle`s at
   fixed fractional centres with `@radial-gradient(circle, blob 0%, transparent 70%)`.
   Every colour plus the `hero-open` gate is a defaulted `in property` — the defaults exist
   so the file imports no global, `MetaChip`'s reason, not because they are a fallback
   anyone should reach. The gate swaps each layer's *stops* for a transparent pair rather
   than the whole brush, which is what keeps both arms one type at one stop count
   (finding 5) and keeps the idle floor's existing idiom intact.
2. Sizing: each blob is a square off the host's short side (~110%), centred at roughly
   15%/20%, 80%/10%, 50%/90%. Numbers land here as named constants with the reasoning at
   their definitions, not inline. **Square is a constraint, not a choice** — finding 6.
3. Mount it in `now-playing-view.slint` **under** the existing blur stack, gated on a
   temporary `Player.np-aurora` bool. The gate has to reach the *whole* old stack — floor,
   both slots and the scrim — or the opaque blur simply covers the thing being judged.
   One `ShortcutScope` binding flips it, so the A/B is on the same track and the scaffolding
   is a line to delete rather than a setting to migrate. (It ended up becoming one — Phase 7.)
4. Publish the colours onto `Player.np-tint-{1..}` from `track_change.rs`, each carrying its
   weight in the alpha channel.
5. The seed source starts on the blurred buffer, so the A/B moves one variable. Phase 3 moved
   it — see below.

**Exit:** both backdrops render; the shortcut swaps them live. Blur path untouched.

### Phase 3 — Look gate

Run a hundred covers through it — bright, near-black, monochrome, single-hue, busy
gatefolds, missing artwork. The two questions:

- Does it read as *this* record, or as generic album-coloured wallpaper?
- Does the blob geometry ever collide into a flat wash on real covers?

Tune the blob positions, radius and tone band until yes. **This phase can also end in
"no"**, in which case the plan stops here and the two-line toggle is deleted — nothing
downstream has been touched yet. That is the point of the ordering, and it is what let the
answer come back as "yes, and keep the other one too" without costing anything.

**Judge the bands separately from Now Playing.** On My Library the backdrop is only ever
seen through `library-tab-band.slint`'s idle pane fading over it, so a blob arrangement
that reads well full-screen can be invisible there and vice versa.

#### What the look gate settled

Six rounds on screen, each fixing something the previous one exposed. What follows replaces the
layer shape and several numbers proposed above; the code carries the full argument at each
constant, so this is the index rather than the reasoning.

- **The blobs are radial after all, and the doc's "take the blob geometry, not the linear angles"
  was right for a reason it didn't give.** Copying Amberol's three *linear* washes verbatim looked
  worse, not better, and the cause is structural: alpha-blending linear ramps composites to
  something still linear in position, so any stack of them keeps straight parallel contours. That
  is the most visible form banding can take and it cannot read as blurred. Curvature needs a
  primitive that isn't linear in position.
- **Two stops, never six.** An intermediate Gaussian-ish falloff put a slope break at every
  junction and banded the surface into ribbons. Finding 5's rule turns out to bind harder than
  stated: not just a fixed *count*, but a small one.
- **A ramp never ends on the `transparent` keyword.** FemtoVG interpolates stops in straight RGBA,
  so fading to rgba(0,0,0,0) drags the colour toward black and a layer darkens across its own
  length instead of thinning. Amberol's odd-looking `color-mix(in srgb, <colour> 0%, transparent)`
  exists precisely for this, which the prior-art survey read past.
- **Alpha reaches zero at 70% of the radius, and Amberol's stop was right after all.** Run the ramp
  to the corner and the rectangle's own edge carries ~0.3 of peak, which draws as a straight seam;
  reaching zero just inside frees the rect from covering the host and is what allows blobs small
  and far apart enough to each own a region. It reads as a slope break only when paired with the
  `transparent` bug above — the two were removed together and only one deserved it.
- **Four blobs, not three, and the count comes with its geometry.** `Score` still answers four with
  its nearest pair 25–44° apart, where five drops to 19–31° and six is forced toward its 15° floor.
  But a fourth blob at the three-blob span and offset *cost* a third of the hue variation on a
  blue-and-red cover — more layers over the same area is more mixing — so span went 1.5 → 1.3
  diagonals and offsets 0.3 → 0.35, on the diagonals a quarter turn apart. That recovers the
  three-blob spread at the same coverage, chroma and tone headroom, with a fourth colour on top.
  **The *diagonal* half of that was wrong and is reversed after Phase 5** — it holds only on the
  shape it was tuned on. See "The band that painted itself grey" below.
- **They are unequal, but only just** — 0.5 / 0.46 / 0.42 / 0.38. At one strength the eye has no
  reason to prefer any and reads the boundaries between them instead; at the usual 60/30/10 the
  dominant shows through the rest wherever they overlap and the later colours never get a region.
  That advice is about *area*, which the geometry now supplies.
- **Seeds come off the sharp downscale, not the blur.** Measured on a real cover: two seeds against
  three, and the two nearly a shared hue. Blur averages away exactly the separation `Score` looks
  for. This was deferred to Phase 7 above to keep the A/B honest; the A/B had outlived that by the
  time it mattered.
- **Chroma needs a floor, not just a ceiling, and the tone must be set first.** `Score` ranks by
  usability rather than saturation, so a cover's second and third seeds are routinely a near-white
  and a near-black; taken as they came they dilute the dominant and the surface converges on grey.
  Chroma is bounded by tone, so asking a near-black seed for saturation *at its own tone* discards
  the request before the tone change can make room — the ordering bug that had a third tint stuck
  at chroma 15 against 36.
- **The whole chroma band scales with how colourful the artwork is**, or the backdrop is more of a
  colour than the record. A black-and-white sleeve still quantizes to seeds carrying a few points
  of chroma: lifted to the floor they painted it red and violet, and left at their own 9 they still
  washed it mauve, since a tint covering the whole surface needs very little chroma to read as one.
  The seeds can't answer which case it is — a greyscale cover's 9.4 sits below a colourful one's
  12.6 — so `BackdropSample` carries the image's own mean chroma, taken population-weighted over
  the quantizer's clusters, which tracks the per-pixel mean to under a point at a hundredth of the
  conversions. Scaled by its **square**, so colour falls away faster than the artwork does; the
  greyscale cover lands at chroma 2–3 and the colourful ones are untouched.
- **Tint tone sits above `TARGET_BACKDROP_TONE`, not on it.** That ceiling belongs to the
  composite, not to any single layer under it. Holding each tint down to it spent the whole margin
  for nothing and, chroma being bounded by tone, cost most of the colour.
- **Seeds keep their own hues, and separation is geometry's job.** An arc clamp was tried first —
  the reasoning being that overlapping washes composite in sRGB, whose midpoint between distant
  hues is grey. It answered the wrong question: a blue-and-red cover came out three violets, its
  most vivid colour discarded. Blobs spread far enough to hold regions of their own is what keeps
  the overlaps honest, so span, offset and the weight hierarchy are one decision — measured on that
  cover, 6° of hue variation across the panel before, 85° after.
- **A blue-noise dither tile, at one 8-bit level.** FemtoVG has no dithering pass, and the blur
  escaped banding only because photographs carry grain. Amplitude is the whole game: six levels
  read as dust, one is invisible. `image-fit: preserve` is load-bearing — the default `fill` scales
  the tile before tiling it and the noise draws as mottling.
- **A vignette**, neutral black, eased so the middle two thirds stay clear. The one place extra
  stops earn their keep: two give a constant slope, which dims the artwork it should be framing.

**The gate's verdict: keep it, and keep the blur too.** The aurora is good enough to be the
default and the blur is good enough not to delete — it carries the cover's own spatial
structure, which four synthesized colours cannot, and on some records that wins. Phases 4
onward are stated against that, and it is why this doc no longer removes anything.

### Phase 4 — The solve answers for two surfaces

The measurement half **stays**. It is the blur's — `luma_p90` sizes a scrim against how bright
that particular cover came out — and the blur is staying, so the earlier plan to delete
`luma_p90`, `scrim_alpha`, `composited_tone`, `PERCENTILE_TAIL`, the histogram and the
`LINEARIZED` LUT is off. What the phase does instead is make the *target* depend on which
backdrop is drawn.

1. `BackdropSample::solve` takes which surface it is solving for. The blur path is exactly what
   it does today. The aurora path skips the scrim solve and targets a constant: every tint sits
   on one tone and no wash is opaque, so the brightest point is stated rather than measured —
   measured against real seeds it peaks around 31, under the 32 the tiers already use.
2. **Not an N-stop `gradient_luma_lstar`** for that constant. It is a mean, and understating
   bright regions is precisely the failure `luma_p90` exists to avoid; a blob centre is exactly
   the smeared wordmark that argument was about.
3. `chrome_tone` / `text_tone` / `muted_tone` are unchanged and keep their range-check tests,
   since one of the two inputs is still a measurement.

**Named trade, now confined to the aurora:** `clamp_to_tone_band` lets a naturally bright cover
keep its own chrome tone. Against the aurora's constant, chrome becomes a pure function of hue
— more consistent between tracks, less varied within one. Under the blur it behaves as it
always has, so the two backdrops differ slightly in their chrome and that is honest rather than
a bug: they are different surfaces.

#### What landed, and the one thing this phase turned out not to be

**It moved no pixel, and that was worth finding out.** `chrome_tone` and `muted_tone` saturate at
their band floor of 70 and `text_tone` at 78 across the *entire* achievable range — the worst
permitted backdrop only asks chrome for 63 — so substituting a constant for the composite changes
no output at any tone the solve can be handed. The named trade above describes a difference that
does not exist numerically today. What the phase actually bought is the coupling removed (the
aurora's foreground no longer answers to a cover it never draws) and the invariant pinned, which
is the half that has teeth: nothing previously caught `TINT_TONE` or a blob peak rising until the
surface was brighter than the tiers were solved for.

- `PEAK_TONE` is **argued, not measured** — the note above overstated it. Every wash sits at
  `TINT_TONE`, so 36 bounds it absolutely; the geometry (1.3-diagonal blob rects whose ramps die
  0.643 diagonals out, centres 0.35 out and 0.495 apart) caps coverage near 0.6, which over a base
  at `FLOOR_TONE_START` lands near 29. 31 is that with headroom.
- **Both bounds are `const _: () = assert!(…)`**, not tests — clippy's `assertions_on_constants`
  pointed at the const block and it is the better answer: a peak past `TINT_TONE` or
  `TARGET_BACKDROP_TONE` now fails the build rather than a test run.
- `backdrop::theme_accent` was hoisted out of `hero_backdrop.rs` beside the new `backdrop::kind`,
  since `track_change.rs` had open-coded the same accent read.
- Three doc comments and one `ui-patterns.md` bullet still said the measurement came off the blur;
  Phase 3 had moved it to the sharp downscale. Fixed in passing.

### Phase 5 — Roll to the six heroes

**The flag moved in Phase 4, ahead of its schedule here.** `backdrop::kind` has to read it and so
do all three mounts, so `Player.np-aurora` became **`Theme.aurora-backdrop`** — the same temporary
role, but reachable by every site instead of only Now Playing, and `Ctrl+Shift+B` writes it. Phase 7
hydrates it from `settings.json` and deletes the shortcut, exactly as written there.

1. `HeroBackdrop` gains `tint-{1..}` and `dither` beside `chrome` and `floor-*`;
   `hero_backdrop::write` grows the setters, and `boot::ui_setup` writes the one tile to both
   globals. The blur quartet on the six per-view globals is untouched — both sets coexist.
2. It also gains **`has-tints`**, false only from `apply_gradient`. Genre Detail shares the band
   with three artwork details, so mounting the aurora there would mount it for a hero whose
   name-hashed stops sit near L\*50 and are only legible under the scrim the aurora doesn't
   paint. Genre keeps the blur stack under both settings and is byte-identical to today.
3. Mount `AuroraBackdrop` beside `HeroBlurBackdrop` at both sites —
   `components/hero/mosaic-tab-hero.slint:109` and `components/hero/library-tab-band.slint:234`
   (the doc had them under `views/`). **Not one `if`/`else`**: Slint has no element-level `else`
   and there is not one in the tree, so it is a local `aurora-shown` property plus two `if`s over
   it and its negation — which is also what keeps the condition single-sourced. Only the band
   folds `has-tints` in; the mosaic hero gates on the setting alone, never being able to show a
   genre and a stale `false` there only costing it a frame of blur stack.
4. Same at Now Playing, on the setting alone. The aurora stops mounting *over* the blur and
   becomes one of the two arms. **The inline stack was collapsed rather than kept** — see below.
5. `hero_backdrop::apply` publishes tints; the `hero-open` gate carries over to every gated
   layer (the don't-ease-out-of-a-held-tier rule in `ui-patterns.md` is unchanged and
   still applies — a brush that eases is a brush that can ease out of a stale value).
   **It has to stay a discrete bool input**, as it is today: the band's instinct is a
   `hero-t` ternary, and a leaf cannot tell an eased input from a stepped one — it would
   restart its own `animate` every frame and arrive in one late rush.

#### What landed, and the two departures from the above

- **Now Playing's inline blur stack is gone, and the Open Question with it.** Structure argued the
  collapse was "the riskiest edit in the least-related phase" — but this is the phase that rewrites
  all three mounts, which makes it the *most* related one. `HeroBlurBackdrop` now takes
  `floor-start` / `floor-end` / `scrim` as defaulted `in property`s, the same `MetaChip` idiom
  `AuroraBackdrop` already carried, so Now Playing mounts it off `Player.np-*` and the four
  duplicated layers went. Both stacks now name no global, which is the property that lets either
  serve either tier and is pinned in both directions.
- **The bands pass `vignette: transparent`.** The vignette is a full-page device: Slint's circle is
  centred with radius = half the diagonal, so on a wide short band it puts ~0.37 black on the left
  and right ends and nothing top or bottom — an end-darkening over the back button, tile and search
  bar rather than a frame.
- **`write`'s two arguments became one.** It carried `floor_override: Option<(u32, u32)>` and would
  have gained `Option<[Tint; 4]>` — two options that can never disagree, since a genre's floor
  override *is* the absence of washes. One `HeroFill` enum, and the impossible state stops being
  spellable.
- `aurora::Tint::to_color()` is now the single spelling of "the weight rides in the alpha, which
  the component's falloff multiplies"; both publishers call it.
- The dither tile left `install_app_chrome` for `install_backdrop_dither`, now that it writes two
  globals and never had anything to do with window chrome.
- `hero_blur_backdrop_tests` lost its Now Playing arm — that file no longer has a floor of its own,
  and the shrink is the result rather than a cost.

#### The band that painted itself grey

The first mosaic banner on screen came out a flat grey-mauve, and the cause was neither the seeds
nor the chroma band — the Favorites mosaic solves to violet `c48`, brick red `c48`, teal `c32` and
olive `c36`, with the band fully open at artwork chroma 35.6. Two things were destroying them, both
invisible on the shape the look gate ran against. Diagnosed by simulating the composite and matching
it to a screenshot within 1–2 per channel, so the numbers below are measured rather than argued.

- **Paint order was `Score`'s ranking, which has nothing to do with hue.** Blob *positions* are
  fixed, so rank decides which colour overlaps which — and here it seated red 29° against teal 203°
  and violet 291° against olive 132°. Two complementary pairs, and overlapping complementary washes
  composite in sRGB toward grey. `aurora::tints` now walks the hue wheel from the dominant, which
  lifts mean chroma across that band 11.4 → 17.5 and gains as much on a square host, so it is a
  root-cause fix rather than a band tweak. Anchored at the dominant because the base gradient under
  it is solved from the same seed.
- **The layout was a fraction of the *diagonal*, which collapses as a host widens.** On an 887×231
  banner the four centres sat 227 px either side of centre with their vertical pair 111 px *outside*
  the element, so only two blobs reached any pixel and each covered the whole strip — no colour had
  a region, and what showed was their average. Positions are now fractions of each axis, spread
  evenly along the long one and alternating across the short one, with the reach sized off both.
  A square host is where the spread is tightest, so it becomes the worst case for coverage: 0.643,
  peaking at tone 29.8, still under `PEAK_TONE`. Today's geometry measured 0.597 → 29.0 at every
  aspect, which reproduces the derivation already in `aurora.rs` exactly.

Discarded on measurement: weighting each wash by its seed's population share. `Score`'s four seeds
carry 0.22% / 0.20% / 4.61% / 0.18% of a 128-cluster quantize, which splits a photograph far too
finely for share to mean prominence — it would have promoted a pale green over the dominant violet.
`Score`'s own ranking is the prominence signal, and the weights already follow it.

### Phase 6 — The mosaic band is deleted, not ported

**This phase reverses what the doc said above**, and the reason is that the two mosaic heroes
compose their artwork **three times**. `CoverMosaic` lays out a live 1/2/3/4 tile in Slint,
`compose_mosaic_blur` blits a *second* composition at 192² purely so there is something to blur
behind it, and `media::artwork::compose_artwork` already holds a third — the one Playlist Detail
uses. Teaching the middle one about the aurora entrenches a duplicate; deleting it is the
smaller tree and the unified look.

Playlist Detail is the shape to copy, and it already works: a playlist's mosaic is composed
**once** into a 600² collage, and from that point the playlist is an ordinary single-artwork
entity — one image feeding the tile, the backdrop, the seeds and the chips. Favorites and
Recently Played adopt exactly that. Neither gets an aurora of its own; they get whatever a
detail hero gets, because they *become* one.

1. **Lift the composition, not the file.** `compose_artwork` composes 1–4 covers onto a 600²
   canvas and then persists a content-addressed JPEG. The layouts are the reusable half; the
   persist is not — a playlist's collage is written on a rare user action, where Recently
   Played's top four moves on nearly every track and would leave a ~50 KB file per distinct set
   in `artwork_dir` with nothing to reap them. Split it: a pure `compose_cover(sources)`
   returning the canvas, shared by both, with `compose_artwork` keeping the
   encode-hash-persist tail for playlists.
2. **The hero takes that buffer through the ordinary path**, so `BackdropSample::measure` and
   the conditional `fast_blur` are the ones every cover already runs. `mosaic_blur.rs` goes
   whole — `MosaicBlur`, `compose_mosaic_blur`, `blit`, `PER_TILE`, `mosaic_blur_tests.rs`.
3. **`impl_mosaic_hero!` goes with it.** `apply_hero_blur` / `clear_hero_blur` are the mosaic's
   own copy of what `apply_detail_artwork` does, and two views calling the shared helper is the
   point of deleting them. **`last_mosaic_paths` stays and changes meaning**: it stops guarding
   a *paint* and starts guarding a *recompose*, which is what keeps an unchanged top-four off
   the composer.
4. **The hero tile becomes one `ArtworkImage`**, as Playlist Detail's is, so `MosaicHeroTile`'s
   `CoverMosaic` branch retires along with the second composition it was the visible half of.
   `CoverMosaic` itself **stays** — the mosaic *picker* is its other consumer and the surface it
   was written for.
5. **No aurora work in this phase at all**, which is the whole point of it being a deletion:
   once these two are ordinary artwork heroes, Phase 5's `MosaicTabHero` mount already covers
   them and there is no third rendering to teach.
6. **Genre Detail is still not part of this.** It has no artwork, so it has no aurora and no
   mosaic — see Structure.

**The cost, stated rather than buried:** a 600² compose-and-measure where there was a 192² one.
The four decodes both shapes pay dominate it and the recompose guard means it runs when the set
changes rather than per visit, but it is more work per change — bought with one composition path
instead of three, and a band that agrees with its own tile.

### Phase 7 — The setting

**Nothing is deleted.** This phase turns the temporary toggle into a real preference, restart-
gated, and deletes the scaffolding that stood in for it.

1. `SettingsData` gains the flag under `#[serde(default)]` — a shipped JSON file, so an install
   that predates it reads as the default rather than failing. Default is **the blur**, which is
   also what makes the upgrade invisible to everyone who never opens Settings.
2. **The row takes the tray toggle's shape exactly**, that being the established one for a
   restart-gated preference: a `ToggleSwitch` with `manual: true` — load-bearing, since the
   switch must not write itself when the user cancels — whose `toggled` populates
   `Dialog.kind = "restart-backdrop"` plus `target-id`, title, message and labels. Then one
   `else if` in `globals/dialog.slint`'s `accepted` dispatcher, one callback on `WindowChrome`
   in `globals/shell.slint`, one icon-map entry beside `restart-titlebar`'s in
   `components/dialog/dialog.slint`, and one handler in `window_chrome/controls.rs` that
   persists and then calls `request_respawn_and_quit` — which may decline, and says so.
3. **The restart is what keeps the decode simple.** Both artwork tiers are built once at boot,
   so the flag is read there and decides whether a `BlurSpec` exists at all: `ArtworkCache`
   takes `Option<BlurSpec>`, and `ArtworkPair.blur` / `DetailPair.blur` become `Option`. One
   place answers the question, no flag is threaded through any decode, and no cache can hold a
   pair made under the other setting. **There is no second decode path to teach** once Phase 6
   has retired `compose_mosaic_blur` — the mosaic heroes come through `ArtworkCache` like
   everything else, which is most of what that deletion buys. Gating only the *mount* would leave every cover still decoded, blurred, uploaded
   and released — the whole cost the setting exists to let a user avoid.
4. **Delete the scaffolding**: `Player.np-aurora`, and the `Ctrl+Shift+B` arm in
   `shortcut-scope.slint` that flips it. It existed to A/B the two on one track and is replaced
   by the thing it was standing in for; leaving an undocumented shortcut writing a property the
   settings row also owns is two writers for one piece of state.
5. **The row's copy, and it has to stay true to the table in *What ships*.** The blur is the
   default, so the description belongs to what turning it off buys: the album's colours drawn
   as a gradient, no cover blurred per track, and no buffer or GPU texture held. In that order,
   **no numbers** — figures belong in this doc, not in shipped copy that goes stale silently —
   and no claim of a large CPU saving, since the quantize dominates and both settings pay it.
   It should also not oversell the look: the blur is the default because it is the better
   picture.
6. Label and description are two `@tr` strings, so both need a `msgid` in **all six**
   catalogues — `every_translated_literal_has_a_msgid_in_every_catalogue` fails otherwise, and
   a miss ships as English inside another language.

### Phase 8 — Tests, docs, exit

1. **Phase 5 took most of this.** `backdrop_tests.rs` survived whole and gained the tint-default
   agreement pin; `hero_backdrop_tests.rs`'s section-gating walks were untouched;
   `hero_blur_backdrop_tests.rs` lost its Now Playing arm to the collapse and gained the pairing
   walks, and `library_tab_band_tests.rs`'s `hero-open` window now covers both arms. What is left
   for this phase is only what the *setting* brings: `artwork_cache_tests.rs` for the optional
   buffer. `mosaic_blur_tests.rs` is not edited but **deleted**, with the module it pins, by
   Phase 6.
2. Walks that landed in Phase 5: **each of the three sites mounts exactly one of the two stacks**
   under one condition and its negation, nowhere else mounts either, and **neither stack names a
   global** — the regression being a site that grows a third mount, one that forgets an arm, or a
   stack that reaches for a tier and so pins itself to one host.
3. **`aurora_tests.rs` and `aurora_backdrop_tests.rs` already exist** and were written as Phase 3
   settled each rule — the tint count agreeing between Slint and Rust, no ramp ending on
   `transparent`, the dither's `image-fit`/tiling quartet, the component naming no global, the
   seeds keeping their hues, the chroma band opening and closing with the artwork, and the tile's
   flat histogram, blue spectrum and one-level alpha.
4. New pins the setting brings: the flag reaches the **decode** and not only the mount (the
   regression is a blur nobody can see still being computed), and flipping it invalidates both
   artwork tiers.
5. `CLAUDE.md` — the `ui/` bullet's artwork-tier paragraph gains the second backdrop.
   `ui-patterns.md`'s hero-bands bullets landed with Phase 5, the mount contract being what that
   file catalogues; "Releasing what the UI pins" is unchanged, the blur still being there to
   release.
6. `README.md:23` calls the artist detail screenshot "a hero-blur backdrop" — still true, now
   as one of two.
7. `docs/plans/ARTWORK_STORE.md` — its Phase 2 item 3 stops
   `thumbnail_exact(BLUR_TARGET, blur_spec.height)` upscaling. That call **survives** this work,
   so the item stands as written; the earlier note here said the opposite and was wrong.
8. Delete this file.

---

## Cross-cutting

- **Memory.** On the default — the blur — it is exactly where it is today, which is the point of
  that default. Opting into the aurora takes ~1.7 MB of resident blur buffers plus their GPU
  textures off. This is not a memory feature and should not be justified as one; we're well under
  the ceiling either way. Take one `/usr/bin/time -v` reading **on each setting** after Phase 7 to
  confirm neither regressed, and don't tune against it.
- **One new setting, which reverses what this doc first said.** The original argument was that
  the aurora *is* the surface, so there is nothing to fall back to and nothing to toggle —
  Amberol and G4Music gate theirs because theirs is an optional tint over a working theme
  background. That reasoning was sound and its premise no longer holds: there are now two
  surfaces and both are wanted. Note the shape it landed in is the *opposite* of theirs —
  ours gates the cheaper synthesized backdrop and defaults to the photograph, where they
  default to the theme and gate the recolouring.
- **Threading is unchanged.** Seeds come out of the same `spawn_blocking` that already
  runs the quantize; the publisher still writes on the UI thread.
- **The section-gating rules are unchanged, and the tints join the gated side.** A hero may
  still publish into a shared global only while it is the one on screen — `ui-patterns.md`'s
  hero contract survives intact, image release included. The blur slots are the ungated half,
  being the view's own, where tints on `HeroBackdrop` are gated like every other tier there.
  Not a regression: an off-screen pre-fetch already couldn't fill the floor or the chrome it
  sat under, and the enter re-publishes both.

---

## Open questions

Phase 3 answered the first two: the scrim became a fixed neutral vignette that `AuroraBackdrop`
owns, and the blob count went to four everywhere rather than per mount — `Score` still separates
four by 25–44°, and a per-instance count would have made the Slint/Rust contract a range instead
of a number.

Phase 5 answered the third: the inline copy is gone, `HeroBlurBackdrop` taking its colours as
defaulted inputs like its opposite number.

- **Do seeds want persisting?** A `tint_seeds` column on `tracks`/`albums` would make every
  aurora hero instant on a cold open with no decode at all — and with the blur optional, a user
  on the default would rarely decode a cover for the backdrop at all. Real, and out of scope —
  but the shape chosen here should not make it harder. It doesn't: four `u32`s and a mean chroma
  serialize trivially.
- **Should the setting be per-surface?** Now Playing is a full page one lingers on, where a band
  is a strip glanced at. Nothing in the wiring forbids two flags. Not worth it unless someone
  actually wants the blur in one place and not the other.
