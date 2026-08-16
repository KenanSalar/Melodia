# Aurora Backdrop

Working doc. Delete when the feature ships.

Status: **accepted** · Created: 2026-08-16 · Phases 0–2 landed, Phase 3 in progress

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
> where it bites rather than listed here.
>
> **Phase 3 rewrote what the stack is made of.** Six rounds on screen retired the layer
> shape this doc proposed and several of its numbers; "What the look gate settled" below
> records what replaced them and why, and the phases past it are stated against that.

---

## What ships

Every artwork-derived backdrop in the app — Now Playing, the six hero banners, the two
mosaic bands, Genre Detail — stops being a **blurred image** and becomes a **brush stack**:
a two-stop base gradient in the album's hue, three broad radial washes of album colour over
it, a vignette, and a tile of noise to keep the whole thing off the 8-bit grid.

The claim, and it has to be checkable or it is decoration:

> The backdrop is a value, not a resource. It costs ~80 bytes to hold, interpolates
> itself on every change, has no resolution, and there is nothing to release.

**One asterisk, added in Phase 3 and not to be quietly dropped:** the stack also draws a
64×64 noise tile, because FemtoVG does not dither its gradients and a ramp this wide bands
without one. Per *process*, not per cover, and nothing releases it — so the lifecycle claim
holds and the "no resource at all" one doesn't.

What that buys, in the order the reasons actually matter:

| | today | after |
|---|---|---|
| Crossfade | two `Image` slots, `use-a` bool, slot never cleared | `Brush::interpolate`, one `animate` |
| Held per cover | 72–108 KB buffer + GPU texture | `[u32; 3]` |
| Release protocol | the blur half of `release_hero_slots!`, `forget_mosaic`, `last_mosaic_paths`, `has-blur` | none |
| On a 4K hero | a 192 px texture upscaled ~10× | vector, exact |
| Foreground tones | re-solved per cover, drift between tracks | constant, proven once |
| Mosaic band | decode 4 → blit 2×2 atlas → blur → quantize | atlas kept, blur gone, one quantize |
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
  four-layer stack inline at `views/now-playing-view.slint:59-99`. Taking the brushes as
  inputs is `MetaChip`'s idiom (`ui-patterns.md`) and is what lets one component serve both
  tiers. **One stack, two mounts** — the duplication is collapsed by this work, not after it.
- **The two globals stay separate, and that is deliberate.** `Player.np-*` and
  `HeroBackdrop.*` have different lifetimes: a band stays mounted behind an open Now
  Playing, so merging them would let a track change repaint the hero underneath. Share the
  *component*, never the tier. They are not the same *shape* either — `Player` carries a
  four-tier accent family, `HeroBackdrop` carries `chip-fill-at()`/`disc-hover`/`tile-edge`
  — so the sharing has to be by parameter rather than by moving one onto the other.
- **The blobs belong on `HeroBackdrop`, beside `chrome` and `floor-*`.** They are a solved
  colour set, and that global already *is* the solved colour set for all six heroes. The
  blur slots sat on the six per-view globals instead because an image belongs to the view
  that decoded it; a brush doesn't, so the per-view quartets go rather than gaining a
  fourth member — and `my-library-view.slint`'s three-way fan-in over them goes with them.
- **`ArtworkCache` keeps the sharp cover and loses the blur.** It does not need to become a
  seed cache either: seeds already ride `ArtworkPair` through the same path-keyed LRU, at
  twelve bytes beside a cover buffer, so a second map would be a second eviction policy for
  nothing. `BlurSpec` (the per-tier blur shape) has nothing left to describe.
- **The quantize stays in one place.** `seed_from_pixels` also feeds the app-wide Material
  You palette, which is out of scope — so it delegates to a ranked-list function rather than
  being widened, and the theme path keeps asking for one seed.
- **Genre Detail stops being a special case.** `apply_gradient` goes; the genre publishes
  three hashed seeds through the same `apply` every cover uses.

---

## Phases

Each phase leaves the tree working. Phase 3 is a human gate — nothing is deleted before
the look is approved on screen.

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
   is a line to delete rather than a setting to migrate.
4. Publish the three colours onto `Player.np-tint-{1,2,3}` from `track_change.rs`, each carrying
   its weight in the alpha channel.
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
downstream has been touched yet. That is the point of the ordering.

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
- **The three are unequal, but only just** — 0.5 / 0.46 / 0.42. At one strength the eye has no
  reason to prefer any and reads the boundaries between them instead; at the usual 60/30/10 the
  dominant shows through the other two wherever they overlap and the second and third colours never
  get a region. That advice is about *area*, which the geometry now supplies.
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

Still open at the end of the gate: whether this is *better* than the blur rather than merely
defensible. The blur carries the cover's own spatial structure, and three gradients contain
strictly less information than the image does.

### Phase 4 — The solve stops measuring

1. Rewrite `ui/backdrop.rs`: the backdrop tone is now **stated, not averaged**. Since
   `ui::aurora` puts every tint on one tone and no wash is opaque, the brightest point the
   foreground can land on is a constant — measured against real seeds it peaks around 31, under
   the 32 the tiers are solved for, so the existing target holds. **Not an N-stop
   `gradient_luma_lstar`**,
   which is a mean, and understating bright regions is precisely the failure `luma_p90`
   exists to avoid; a blob centre is exactly the smeared wordmark that argument was about.
   Drop `luma_p90`, `scrim_alpha`, `composited_tone`, `PERCENTILE_TAIL` and the histogram.
   **The `LINEARIZED` LUT stays until Phase 6** — `pixel_lstar` has a second caller in
   `rgb_lstar` → `gradient_luma`, which is Genre Detail's, and Genre retires there.
2. The scrim goes: Phase 3 settled it as a fixed neutral vignette, which `AuroraBackdrop` now
   owns and which needs no per-artwork solve.
3. `chrome_tone` / `text_tone` / `muted_tone` are unchanged but now solve against a
   constant. Their tests become exact assertions rather than range checks.

**Named trade:** `clamp_to_tone_band` currently lets a naturally bright cover keep its own
chrome tone. Against a constant backdrop, chrome becomes a pure function of hue — more
consistent between tracks, less varied within one. That is the right side of the trade and
`backdrop.rs`'s header should say so in one line.

### Phase 5 — Roll to the six heroes, collapse the duplicate stack

1. `HeroBackdrop` gains `tint-{1,2,3}` and `dither` beside `chrome` and `floor-*`;
   `hero_backdrop::write` grows the setters, and `boot::ui_setup` writes the one tile to both
   globals. **The blur quartet is not there to lose** — it sits on six per-view globals
   (`AlbumDetail`, `ArtistDetail`, `PlaylistDetail`, `Favorites`, `RecentlyPlayed`, `Player`),
   which is Phase 7's subtraction rather than this phase's edit.
2. Replace `HeroBlurBackdrop` with `AuroraBackdrop` at both mounts
   (`mosaic-tab-hero.slint:109`, `library-tab-band.slint:234`).
3. Replace Now Playing's inline stack (`now-playing-view.slint:59-99`) with the same
   component and delete the toggle. The four-layer stack now exists **once**.
4. `hero_backdrop::apply` publishes tints; the `hero-open` gate carries over to every gated
   layer (the don't-ease-out-of-a-held-tier rule in `ui-patterns.md` is unchanged and
   still applies — a brush that eases is a brush that can ease out of a stale value).
   **It has to stay a discrete bool input**, as it is today: the band's instinct is a
   `hero-t` ternary, and a leaf cannot tell an eased input from a stepped one — it would
   restart its own `animate` every frame and arrive in one late rush.

### Phase 6 — Mosaic and Genre fold into one path

1. `compose_mosaic_blur` loses the **blur** and **keeps the atlas**. A seed per cover would
   be four quantizes where there is one today, and finding 1 says the quantize is the cost
   centre — the 2×2 blit is trivial beside the four decodes both shapes pay anyway. So:
   compose as now, skip `fast_blur`, take three seeds from the one quantize. The mixed
   distribution is also the better answer, the band being about the *set*.
2. `impl_mosaic_hero!`'s paint guard goes — it answers "is this mosaic what's painted", and
   a brush has no such question; an identical `set_*` is value-compared and restarts
   nothing. **`last_mosaic_paths` itself stays**, its two readers outside the macro
   (`favorites/hero.rs`, `recently_played/songs.rs`) skipping the whole *compose* — four
   decodes — which is a question that survives. `forget_mosaic` stays with it.
3. Genre Detail publishes three hashed seeds through `apply`; `apply_gradient` goes, and
   `gradient_luma` / `rgb_lstar` / `LINEARIZED` retire behind it as its only callers.
   **`GenreRow.hero_color_1/2` are what go — not `tile_color_1/2`**, which has three live
   Slint readers (the genre grid, the top-result card, My Library's band tile) and is not
   this feature's business.

### Phase 7 — Deletions

Only now, and in one commit so the diff reads as the subtraction it is:

`fast_blur` call sites · `BlurSpec`, `BLUR_SIGMA`, and the detail tier's inline `128`/`20.0`
(the shape was never fully hoisted) · `ArtworkPair.blur` · the A/B `Image` slots and `use-a`
on **six** globals · `has-blur` and `my-library-view.slint`'s three-way fan-in over it ·
`hero-blur-backdrop.slint`.

Three things that look like they belong on that list and don't:

- **`write_crossfade_slot` survives.** Four of its five call sites are the blur; the fifth
  drives the sharp `np-cover-a/b` pair, which this feature doesn't touch. An image still
  can't interpolate.
- **`release_hero_slots!` and everything above it survives**, minus three lines.
  `release_detail_hero_images!` and `release_collapsed_hero` also hand back `cover` and
  re-solve the two shared globals — none of which this removes. The subtraction is the
  *blur half* of the release protocol, not the protocol.
- **`BLUR_TARGET` is renamed, not deleted.** 192 is also the quantize downscale and the
  mosaic atlas dimension, so it outlives the thing it is named after.

The quantize already moved onto the unblurred downscale in Phase 3, so what is left here is the
transient copy that move introduced: with `fast_blur` gone, `BackdropSample::measure` takes the
one buffer that remains and `buffer_from_rgb` is called once rather than twice.

`ArtworkCache` keeps `cover` and its LRU; the caps (8 and 12) are re-argued against
covers alone, not carried over.

### Phase 8 — Tests, docs, exit

1. **Six test files, and the two obvious ones are the ones that mostly survive.**
   `backdrop_tests.rs` (550 lines) is numerical with a single source walk — only its
   `luma_p90` / `scrim_alpha` / `composited_tone` / LUT sections die, and the tone solvers
   become exact assertions rather than range checks. `hero_backdrop_tests.rs` (486) is the
   source-walk file, but what it walks is the **section-gating** contract, which this work
   leaves alone. What actually dies whole is `hero_blur_backdrop_tests.rs`, every assertion
   in it pinning the replaced component — its `dur-med` count, its transparent-pair idle arm,
   its `HeroBackdrop.scrim` read. `mosaic_blur_tests.rs`, `mosaic_tab_hero_tests.rs`,
   `artwork_cache_tests.rs` and `library_tab_band_tests.rs`'s `HeroBlurBackdrop {` split all
   need edits.
2. Walks worth keeping in new form: every hero mounts the shared component and grows no
   stack of its own; no surface spells a `Theme.*` brush on a backdrop.
3. **`aurora_tests.rs` and `aurora_backdrop_tests.rs` already exist** and were written as Phase 3
   settled each rule — the tint count agreeing between Slint and Rust, no ramp ending on
   `transparent`, the dither's `image-fit`/tiling quartet, the component naming no global, the
   hue-arc pull, the chroma floor lifting a washed-out seed, and the tile's flat histogram, blue
   spectrum and one-level alpha. What Phase 8 owes is what only exists once the heroes are on it:
   that every hero mounts the shared component rather than growing a stack.
4. `CLAUDE.md` — the `ui/` bullet's artwork-tier paragraph. `ui-patterns.md` — the hero-bands
   bullets, and "Releasing what the UI pins", which loses two of its three (the Dialog
   teardown is a different subject and is untouched).
5. `README.md:23` calls the artist detail screenshot "a hero-blur backdrop".
6. `docs/plans/ARTWORK_STORE.md` — its Phase 2 item 3 exists to stop
   `thumbnail_exact(BLUR_TARGET, blur_spec.height)` upscaling, and this work deletes that
   call. Say so there rather than letting it be built twice.
7. Delete this file.

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
- **The section-gating rules are unchanged, and the blobs join the gated side.** A hero may
  still publish into a shared global only while it is the one on screen — `ui-patterns.md`'s
  hero contract survives intact, and what goes is only the *image release* half of it. But
  the blur slots were the ungated half, being the view's own, where blobs on `HeroBackdrop`
  are gated like every other tier there. That is a consolidation rather than a regression:
  an off-screen pre-fetch already couldn't fill the floor or the chrome it sat under, and
  the enter re-publishes both.

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
