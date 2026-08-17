# Aurora Backdrop

Working doc. Delete when the feature ships.

Status: **accepted, v2** · Created: 2026-08-16 · Rewritten: 2026-08-17
Phases 1–9 landed; **Phase 8's look gate passed on 2026-08-17**. 10–11 remain. Phases 6–11
replaced the old 6–8.

> **v2 reverses this doc's central premise.** The aurora shipped as a *self-contained
> surface* — every wash driven to one tone, the composite pinned into a dark band, and the
> foreground solved per cover against it. On screen that reads flat and muddy, and the
> measurements below say why: it sits ~16 L\* under its own legal ceiling, its four washes
> overlap so heavily that every pixel is a four-way average, and its extractor answers a
> question a backdrop wasn't asking. The fix is the model this doc originally surveyed and
> rejected — **bounded washes of the album's own colours over the theme's own base** — and
> the reason for rejecting it does not hold. That argument is at *The rejection that was
> wrong* below. Sentences elsewhere that contradict v2 are marked where they sit.
>
> Measurements taken **2026-08-17** on this machine: quantizer timings from a standalone
> release build against `material-colors 0.4.2` and `color-thief 0.2.2` under the pinned
> 1.97.0 toolchain; contrast figures computed from the WCAG relative-luminance formula the
> code already uses, against the real `catppuccin.rs` palettes. Slint and quantizer source
> facts from the 2026-08-16 pass stand and are not re-derived.

---

## The observed symptom

Side by side with Amberol on the same record, our Now Playing and hero bands look dull.
Two halves, both visible without measuring anything:

- **The surface has no bright region.** Amberol's corners carry the album's actual colours
  at their actual lightness; ours never leaves a narrow dark band, and the Now Playing
  vignette darkens the periphery on top of that.
- **The colours are muddier than the record.** A cover of blue *and* red reads as two
  similar mid-tones, and a black-and-white sleeve reads as one flat grey.

Everything below is why, and what to do instead.

---

## What v2 ships

The aurora stops being a surface solved in isolation and becomes **the album's colours
washed over `Theme.base`**, capped so the composite cannot leave the band where the theme's
own ink stays legible. Consequences, in the order they bite:

- **The foreground stops adapting to the cover.** Titles, secondary lines and chrome on a
  hero become `Theme.text` / `Theme.subtext1` / `Theme.accent` — the same tokens every other
  page uses. Polarity is the theme's, which is a constant, so one foreground is correct over
  the whole surface *by construction* rather than by measurement.
- **The backdrop follows theme polarity.** On Mocha it is dark with colour; on Latte it is
  light with colour and dark ink, where a Latte user used to get a dark island on two
  surfaces. **This is the product call the rest of v2 rests on**, taken deliberately on
  2026-08-17 — see *Open questions*.
- **Washes keep their own tone and chroma**, clamped only where the cap binds. The album's
  value structure survives: a bright ochre stays brighter than a deep navy.
- **Seeds come from population, not from a usability score**, so the backdrop is what the
  cover mostly *is*.

**The blur stays, unchanged, and stays a setting.** It is a photograph and can be any
brightness, so it keeps `ui/backdrop.rs`'s measure-and-scrim solve. The two models coexist
by *publishing into the same tier names* — see Structure. Nothing about the blur path,
`HeroBlurBackdrop`, or the release protocol moves in v2.

**Not a 1:1 port of Amberol.** Four things are ours and are argued below rather than
copied: the geometry is aspect-independent because our heroes are 5:1 strips and its window
is not; the cap is *computed per theme* because we ship six palettes and two of them are
generated at runtime; we keep the dither because FemtoVG has no dithering pass and GTK's
renderer does; and we keep an accent, because ours is a user setting and Amberol has none to
keep.

---

## Prior art, re-measured

The 2026-08-16 survey read both apps correctly and drew one wrong conclusion from it. What
follows corrects the record with numbers.

### Amberol — `src/utils.rs:114`, `src/window.rs:1409`, `src/gtk/style.css:106-124`

`color_thief::get_palette(pixels, format, quality: 5, max_colors: 4)` over a 384² nearest
downscale — MMCQ median-cut, ranked by **population**. Palette and texture are cached
together and an entry is stored only if both succeeded. Rust builds a `:root` CSS string and
hot-loads it into a display-wide provider per track change.

The paint is three linear gradients at 127° / 217° / 336°, each
`color-mix(<colour> 55%, transparent)` → `0%` at `70.71%`, over `--window-bg-color`. Three
properties do the work:

1. **Each wash dies at 70.71 % of its own axis**, so it covers part of the surface. Total
   coverage is **~41 % mid-element and ~68 % worst-case corner** — colours meet pairwise,
   never four-way, and the far side of each is clean theme background.
2. **Colours are used at their natural tone and chroma.** No normalisation whatsoever.
3. **The chrome is neutralised while recolouring is on** (`style.css:112`):
   `--accent-bg-color` becomes `rgb(0 0 0 / 75%)` (white in dark) and the sidebar tokens
   become translucent black/white so they composite *over* the wash. One coloured thing in
   the window.

It extracts 4 and paints 3. No vignette, no dither, no blur — `.blurred` is drag-overlay
only. 250 ms `transition` on `background`.

### G4Music — `src/ui/paintables.vala:283`, `src/ui/window.vala:310`

No colour extraction at all. `create_blur_paintable(widget, paintable, size: 512,
blur: size * 0.2, opacity: 0.25)` pushes an opacity node over a blur node, snapshots the
cover into it and `render_texture`s the result **once** into a 512² texture. A `blur-mode`
GSetting picks ALWAYS / ART_ONLY / off.

The number that matters is **`opacity: 0.25`**. G4Music is the same model as Amberol with a
photograph in place of the washes: bounded coverage over the theme background, foreground
inherited from the theme, no contrast machinery anywhere. Discussed as a follow-on under
*Open questions*, not adopted here.

### The measurement the survey didn't take

Standalone release build, same synthetic covers, 1.97.0:

| input | ours (`QuantizerCelebi`@128 + `Score`) | `color_thief` q5/max4 | ratio |
|---|---:|---:|---:|
| photo-like 192² (20.5k unique colours) | **30.6 ms** | **0.087 ms** | 350× |
| photo-like 384² (34.4k unique) | 45.5 ms | 0.102 ms | 446× |
| flat poster art (5 unique) | 0.39 ms | 0.137 ms | 3× |
| greyscale ramp (254 unique) | 1.23 ms | 0.251 ms | 5× |

**Finding 1 above — "the cost centre is `QuantizerCelebi`… and that stays" — was right about
the cost and wrong that it has to stay.** The gap is driven by *unique colour count*, not
pixel count: Celebi runs Wu (33³ moment histogram, 128 box cuts) and then ten k-means
iterations over every unique Lab point with a 128×128 distance matrix re-sorted per
iteration. Album art is photographic, so 30–45 ms is the ordinary case, not the tail. It is
in `spawn_blocking` and cached per cover, so it is not a frame cost — it is 30–45 ms of
latency between a skip and the backdrop landing, and Material You pays a second Celebi pass
on the same track change. `color_thief` at quality 1 (every pixel) is still 0.11 ms.

---

## Five findings that decide v2's shape

Measured against the tree as it stood on 2026-08-17 and left as taken. **All five are now
fixed** — 2 and 3 by Phase 7's corner anchoring and Phase 6's extractor, 1, 4 and 5 by Phase 8.
Kept because they are the argument the gate is judged against, not a to-do list. Two figures to
read carefully: 1's ratios moved by a tenth of a point once `PEAK_TONE` reached 32, and 5's
wash-tone column was computed at 68 % coverage where the shipped geometry states **0.73**, which
puts Mocha's real ceiling at L\*≈49 and Latte's floor at L\*≈72. `backdrop_tests` pins those.

1. **The aurora sits ~16 L\* below its own legal ceiling.** `PEAK_TONE = 31` puts the solved
   chrome tier at **3.93:1** against a 3:1 target and the text tier at **5.0:1** against
   4.5:1 — both over target, both saturated at their band floors. Working the solve
   backwards: nothing in the foreground moves at all until the backdrop reaches L\*33.9, the
   chrome tier holds its floor to L\*38.3, and the declared bands stay legal to **L\*47.1**.
   The contrast machinery is not what makes it dark. The constants are.

2. **The four washes overlap so heavily that every pixel is a four-way average.**
   `blob-reach: max(long * 0.315, short * 0.8)` picks the **short-axis** term whenever the
   host is narrower than 2.54:1 — which is every Now Playing panel. On a 1280×800 panel that
   is a 640 px reach against 298 px of spacing: all four cover the whole surface. Composite
   at the centre works out to L\*25.5; at a blob's own centre, L\*25.5. It is essentially
   *uniform*, and averaging four hues spread around the wheel drags chroma toward neutral as
   well. This is the "flat mauve" failure `around_the_wheel` was written to prevent, arriving
   through geometry instead of through ordering.

   Coverage is not the problem — ours is **53 %** at the panel centre against Amberol's
   41 %. The colours are. Ours are pre-darkened to tone 36 and pre-desaturated by
   `chroma_band` before they are ever composited.

3. **`Score` answers the wrong question for a backdrop, and on greyscale it answers once.**
   On the photo case ours picks the reddest thing in the image (`#af3a2e`) where MMCQ picks
   what the image mostly is (`#372b3c`). Defensible for an accent seed; wrong for a surface
   meant to feel like the record. On the greyscale ramp `Score` returned **one** seed
   (`#939393`) where `color_thief` returned four real greys — so three of four washes fall to
   `rotate_hue(origin, ±25/50)` at `FILL_WEIGHT` 0.3, `chroma_band` scales them by
   `(≈2.5/20)² ≈ 0.02`, and the surface is one flat grey plus three near-identical ghosts.
   **That is a seed-count failure, not a chroma failure**, and no amount of tuning the chroma
   band reaches it.

4. **Amberol's alpha schedule *is* its contrast machinery, and it is not quite enough for
   us.** Bounded coverage over a known base bounds the composite by construction. But a white
   cover at 41 % coverage over Mocha lands at L\*51.3 — **2.96:1** against `Theme.text`.
   Amberol survives that case only because Adwaita's dark foreground is pure white (4.28:1)
   *and* its title is 15 pt weight 800, i.e. large text at a 3:1 bar. Our Now Playing carries
   ordinary-weight secondary lines and a whole Up Next list, so we cannot inherit that
   escape. **One clamp fixes it**, and it replaces the entire measure-scrim-solve chain
   rather than joining it.

5. **The cap is a closed form over `Theme.base` and `Theme.text`, and both are already
   known.** Coverage is fixed by geometry, so given a base, an ink and a worst-case
   coverage the maximum admissible wash tone is algebra:

   | theme | base | ink | composite cap | wash-tone cap @68 % coverage |
   |---|---|---|---|---|
   | Mocha | L\*12.0 | L\*85.8 | L\*39.8 (4.5:1) / 50.9 (3:1) | **L\*51.7** / 67.2 |
   | Latte | L\*95.1 | L\*34.3 | L\*78.6 floor / 65.3 floor | **L\*70.5** floor / 50.3 |

   So on Mocha a wash may run to **L\*≈52** at its natural chroma, against today's
   `TINT_TONE` of 36 applied to every wash regardless. Most album colours live at L\*30–70
   and pass through untouched. On Latte they are floored into pastel — which is what Amberol
   looks like on a light theme, and it looks good.

   **This must be computed, not tabled.** Two of the six palettes are generated at runtime
   (Material You, KDE-from-`kdeglobals`) and have no compile-time values, which is the same
   reason `Theme.is-light` exists rather than a per-variant flag.

### The rejection that was wrong

The 2026-08-16 survey said:

> **Reject both apps' contrast posture.** Theirs are *tints* over a theme-owned background,
> so contrast is inherited from Adwaita and neither has a single line of contrast machinery.
> Ours is a *replacement* — `ui/backdrop.rs`'s header already argues why, and it stays true.

The tint-versus-replacement framing is not what makes Amberol safe. Its window is also fully
covered by the recoloured surface; its song title sits directly on the gradient. The real
distinction is **bounded alpha over a known base** versus **an arbitrary surface, measured
then scrimmed** — and the first gives the *stronger* guarantee, because it holds by
construction:

- `backdrop.rs`'s header objects that adapting the foreground answers a bright cover with a
  polarity flip past the black/white crossover. Under v2 the foreground does not adapt to the
  cover at all — it is the theme's, and the theme is a constant. The objection has no subject.
- It objects that a blurred cover is not uniform, so no global foreground decision serves a
  backdrop bright in one corner and dark in another. Under v2 the composite is bounded within
  a known band of the base *everywhere*, so one fixed foreground is correct across the whole
  surface. **That half of the header stays true for the blur**, which is why the blur keeps
  the solve.

---

## Structure

No new directory, no new module. v2 is a rewrite inside the three files that already own the
problem, plus one dependency.

```
Cargo.toml                     + color-thief = "0.2.2"   (one dep: rgb)
src/services/material_you.rs   ranked_seeds gains a population sibling; theming untouched
src/ui/aurora.rs               tone flattening and the chroma band out; the cap in
src/ui/backdrop.rs             the solve narrows to the blur; publishes theme tokens for the aurora
melodia-ui/ui/components/aurora-backdrop.slint   anchors to corners; vignette out
```

Ownership rules, so this doesn't sprawl:

- **The two backdrops publish into the *same tier names*, and that is what keeps the blur
  free.** `HeroBackdrop.on-backdrop` / `on-backdrop-muted` / `chrome` and `Player.np-*` stay
  exactly as they are; what changes is only what Rust writes into them. Blur mounted →
  today's solved tones. Aurora mounted → `Theme.text` / `Theme.subtext1` / `Theme.accent`, and
  `Theme.base` / `Theme.mantle` into `floor-start` / `floor-end`.
  **No `.slint` consumer changes at all** — not `TabBar`'s four brushes, not `MetaChip`,
  not `MosaicHeroTile`, not the `ui-patterns.md` hero contract. `backdrop::kind` is already
  the single place that asks which surface is painted, and it is already the sole reader. This
  is also why Phase 8's item 3 went through the tier rather than the mounts: `MosaicHeroTile`
  reads `floor-start` too, so a mount-site `Theme.base` would have left the square behind.
- **`ui/backdrop.rs` narrowed rather than split.** `luma_p90` → `scrim_alpha` →
  `composited_tone` → the three tone solves are the *blur's* path, and `solve` no longer takes a
  `kind` at all. `BackdropKind::Aurora` stopped meaning "skip the scrim solve" and now means
  "there are no solved tones here": `BackdropSample::solve` picks between `solve` and
  `theme_backdrop`, and the tier set on that arm is the theme's own.
- **`ui/aurora.rs` owns the geometry the cap is a function of; `backdrop.rs` owns the cap** —
  reasoning in Phase 8. `BLOB_PEAKS` / `REACH_FRACTION` / the two coverage functions stay, being
  what `wash_cap` takes, and `peak_coverage()` gained a `> 0.0` const-assert now the cap divides
  by it; `mid_coverage`'s only consumer is still a const-assert. `dither_tile` and everything
  under it **stays**; see *What we keep that Amberol doesn't have*.
- **Deleted from `ui/aurora.rs`:** `TINT_TONE`, `TINT_MAX_CHROMA`, `PEAK_TONE` and its two tone
  const-asserts. `FILL_HUES` / `FILL_WEIGHT` were to go with them on the reasoning that a
  population extractor fills the list — **it does not**: a near-white sleeve returns one colour,
  so the `Option`s and the fill rule stay, and `Tint::weight` and `to_color`'s alpha fold stay
  with them.
- **Deleted from `backdrop.rs`:** nothing. The tone bands, the scrim solve and `luma_p90` are all
  still the blur's; `theme_accent` widened into `ThemeTokens` rather than being replaced, and
  `TARGET_BACKDROP_TONE`'s doc comment stopped naming the aurora.
- **`around_the_wheel` stays, and its justification changes.** It was there because the blob
  positions were fixed and rank decided which colour overlapped which. Under corner anchoring
  the overlaps are pairwise and adjacent, so hue-wheel ordering is what keeps each *pair*
  from compositing toward grey. Same code, better reason.
- **The extractor splits by question, not by crate.** `color_thief` answers "what is this
  image mostly made of" for the backdrop; `material-colors` keeps answering "what is the best
  UI seed" for Material You theming and the app-wide palette. Both live in
  `services/material_you.rs`, which is where a cover is turned into colours. `Quantized::chroma`
  had exactly one consumer — `chroma_band` — and went with it, so the backdrop no longer calls
  `QuantizerCelebi` at all. **Material You is out of scope and does not change.**
- **Genre Detail is still out of scope**, and still for the original reason: it has no
  artwork, so neither backdrop has anything to derive from, and its name-hashed gradient *is*
  the genre's identity. `apply_gradient`, `gradient_luma` and `rgb_lstar` are untouched.
  `has-tints` keeps its meaning and its one `false`.
- **The setting stays restart-gated**, and for the unchanged reason: both artwork tiers are
  built once at boot, so the flag decides whether a `BlurSpec` exists at all. v2 makes that
  cheaper to justify, not different.

### What we keep that Amberol doesn't have

Four departures, each forced by our stack rather than chosen:

- **Aspect-independent geometry.** Amberol's window is near-square; a 127° linear ramp on our
  5:1 `LibraryTabBand` is nearly vertical and three of them read as horizontal stripes. We
  keep the radial `AuroraBlob` primitive — proven, and already aspect-safe because Slint's
  radial gradient is a true circle whose radius is half the element diagonal, so square blob
  rects meet the edge midpoints at exactly Amberol's `1/√2`. Only the *anchors* move.
- **A computed cap.** Amberol hard-codes 55 % and inherits Adwaita's two foreground colours.
  We ship six palettes with a 12 L\* base at one end and a 95 L\* base at the other, two of
  them generated at runtime. The cap has to come off the live theme.
- **The dither.** FemtoVG has no dithering pass; GTK's renderer does. One 64×64 tile per
  process at one 8-bit level, unchanged.
- **An accent.** Amberol neutralises its accent because it has no user-chosen one to protect.
  Ours is a setting that every other page honours, and Now Playing becoming the one page
  where it disappears would read as a bug. We neutralise nothing; we cap against it instead.

---

## Phases

Each phase leaves the tree working. Phase 8 is a human look gate, as Phase 3 was.

### Phase 6 — The extractor · **landed**

1. `color-thief = "0.2.2"`, beside `material-colors` rather than instead of it.
2. `material_you.rs` gains `population_seeds(buf, desired) -> Vec<u32>` over
   `color_thief::get_palette` at quality 1, reading the buffer in place — no `Vec<Argb>`.
   **Quality 1 is not "every pixel"**, as this doc claimed: the crate advances its cursor by
   `bytes-per-pixel × quality` pixels, so 1 on RGB samples every third. It is still right,
   being the finest stride on offer, but the reason is cost rather than exhaustiveness.
3. `BackdropSample::measure` calls it; `chroma` goes. **`seeds` stays
   `[Option<u32>; SEED_COUNT]`** — median cut returns at most `max_colors` and a near-white
   sleeve returns *one*, the crate dropping every pixel over 250 on all three channels. The
   `FILL_HUES` / `FILL_WEIGHT` rule stays with it.
4. **An empty buffer is guarded in the wrapper**, `get_palette` answering `Ok([white])` on
   zero pixels where every caller here spells "no artwork" as an empty list.
5. **`chroma_band` went in this phase, not in 8** — `Quantized::chroma` was a walk over
   Celebi's clusters and could not outlive the Celebi call. Deleted: `Quantized`,
   `ranked_seeds`, `extract_seeds_from_rgb8` (its `desired` was always 1, so it folded into
   `seed_from_pixels`), `to_tone_with_chroma`, `TINT_MIN_CHROMA`, `TINT_CHROMA_REFERENCE`.
   **`TINT_MAX_CHROMA` stays** as the pathological-seed ceiling. The floor's whole argument was
   about `Score` handing over a near-white and a near-black; a population extractor answers
   with what the cover is made of, so lifting it would make the backdrop more of a colour than
   the record is. Phase 8 removes the ceiling and the tone with it.
6. `extract_source_argb` / `extract_source_argb_from_rgb8` keep their signatures, and Material
   You is untouched.

**Landed:** four new pins — a greyscale ramp yielding a full set of seeds (the case that
returned one), the first seed being the bulk colour rather than the vivid stripe, and the
empty/white pair being told apart. `a_washed_out_seed_is_lifted_to_carry_colour` and
`the_first_seed_is_the_same_however_many_are_asked_for` were retired with their subjects.

### Phase 7 — Geometry · **landed**

1. The four blobs anchor at the four **corners**, walking round the rectangle (TL, TR, BR, BL)
   so consecutive washes are edge neighbours — the same adjacency `around_the_wheel` gives
   them, four corners being a cycle exactly as the hue wheel is. `wide`, `long-side`,
   `short-side`, `blob-x` and `blob-y` are gone; corners are corners whatever the aspect.
2. `blob-reach` is **1/√2 of the host diagonal**, not a shrunken axis fraction. **"Dies before
   mid-element", as this doc asked for, would leave the centre bare `Theme.base`** — under the
   Now Playing cover, of all places. Amberol's own washes reach its centre at ~29 % strength
   for the ~41 % coverage cited above, and that is what is matched.
3. `ui/aurora.rs` states the geometry as the contract Phase 8's cap is a function of:
   `BLOB_PEAKS`, `REACH_FRACTION`, and `const fn` `mid_coverage()` (**0.424**, all four washes
   at the centre but each at 29 % of its peak) and `peak_coverage()` (**0.73**, the supremum
   over aspect of a corner pair sharing a short edge). Real hosts sit at 0.50 on a square and
   ~0.64–0.67 on the bands; over-stating the peak only costs brightness.
4. **`PEAK_TONE` 31 → 32, and it is now a derivation.** A `TINT_TONE` wash at `peak_coverage()`
   over `FLOOR_TONE_START` composites to L\*31.3, which `backdrop_tests` computes rather than
   restates. Nothing on screen moves — every tier saturates at its band floor until the
   backdrop reaches L\*33.9.
5. **The vignette is gone**, property and rect both, at all three mounts. A whole-tree pin says
   so: neutral black at the periphery is the direct inverse of a model whose colour now lives
   in the corners.

**Landed:** `the_washes_are_laid_out_against_the_axes_rather_than_the_diagonal` retired (it
asserted the string `diagonal` was *absent*), replaced by `the_washes_are_anchored_at_the_
corners`; the peak-tone pin now reads `blob-reach` numerically and compares the four `peak`
bindings against `BLOB_PEAKS`.

### Phase 8 — The composite model · **landed, gate open**

The phase that changes what the feature is. **Gate it on screen before Phase 9.**

1. `ui::backdrop::wash_cap(theme, coverage) -> (min_tone, max_tone)` — the closed form from
   finding 5. **In `backdrop.rs`, not `aurora.rs`**: the geometry it takes is aurora's and stays
   there (`peak_coverage()` is the argument), but `grey_byte`, `byte_tone`, `contrast::darker` and
   the two ratios are all private to `backdrop.rs`, so siting it there would have promoted four of
   them to move one function away from its own test helpers. Leaves a one-way `aurora → backdrop`
   edge now `PEAK_TONE` is gone. Polarity comes off `base` vs `text` — the *relationship*, not a
   threshold, so the two generated palettes are covered. It caps against **three** tiers rather
   than two (`text` @4.5, `subtext1` @3, `accent` @3), which costs one array entry and removes a
   "the third is dominated" claim nobody could check; an unreachable tier is **skipped**, a theme
   whose own ink fails everywhere not being the backdrop's to rescue.
2. `tints()` stops driving washes to `TINT_TONE` and stops capping chroma at all. Each seed keeps
   its own tone and chroma, clamped into the band through `clamp_to_tone_band`, which returns a
   colour inside the band **untouched** — that pass-through is what makes this a bound rather than
   a second flattening, and it has its own pin. **`Tint::weight` stays**, against this doc's own
   line: Phase 6 kept `FILL_WEIGHT`, `weight` is how it reaches the paint, and a weighted wash is
   strictly safe against the cap in both directions.
3. **No `.slint` change beyond a comment.** The three mounts already read `floor-start`/`floor-end`
   off their tier, so publishing `Theme.base`/`Theme.mantle` *into* the tier reaches all three plus
   `MosaicHeroTile`'s square — which reads `floor-start` too and would otherwise disagree with the
   surface under it. That is what this doc's own Structure section asks for, and it keeps
   `the_backdrop_names_no_global` passing where a mount-site `Theme.base` would have broken it.
4. `BackdropSample::solve` grew the branch and `solve` lost its `kind`, so `solve` *is* the blur's
   path and `theme_backdrop` is the aurora's. `BackdropColors` is unchanged, so both publishers and
   `write` are one token wider and nothing else.
5. `theme_accent` widened into `ThemeTokens` + `theme_tokens(ui)`, keeping its one-read property.
   **`subtext1`, not `subtext`** — there is no such getter, and `Theme.text-muted` carries its
   dimming in alpha, which `brush_to_rgb` drops.

**Landed:** `the_cap_matches_the_derivation_on_both_polarities` (Mocha ≈ L\*49, Latte ≈ L\*72 at
the shipped 0.73 coverage — finding 5's table was computed at 0.68),
`every_theme_tier_survives_the_washes_on_both_polarities` (five covers × two polarities × the four
edge pairs, composited per channel the way FemtoVG does rather than through the grey proxy the cap
reasons with), the two arms not leaking into each other, and the `clamp_to_tone_band`
pass-through. Retired with their subjects: `the_stated_peak_bounds_the_composite_the_geometry_
produces`, `every_tier_clears_its_target_on_the_aurora`, `every_tint_lands_on_one_tone`.
The cap has real headroom — mutating the ceiling by +2 L\* still passes, +10 fails on a white
cover — the conservatism coming from `contrast::darker` returning a verified rather than an exact
answer.

**Exit / gate — passed 2026-08-17**, side by side with Amberol on Mocha and Latte both. Latte was
the case that had never existed and the one that could have failed outright; it holds. The three
things worth the specific look — the mosaic heroes' square now on `Theme.base` against a brighter
surround, `LibraryTabBand`'s morph crossfading `idle-pane` over a backdrop that is now the same
colour at `hero-t` 0, and a greyscale sleeve reading tonal rather than flat — all read correctly.
v2 continues; the blur was never touched, so nothing here was ever at risk.

### Phase 9 — The mosaic band is deleted, not ported · **landed**

Orthogonal to the colour model, and a deletion. The two curated heroes composed the same four
covers twice — `CoverMosaic` live in Slint over a third cache tier, and `compose_mosaic_blur` into
a 192² atlas — where Playlist Detail already demonstrated the shape: compose **once** into a 600²
collage, after which the entity is an ordinary single-artwork hero.

1. `media::artwork::compose_cover(sources) -> RgbImage` split out of `compose_artwork`, whose
   encode-hash-persist tail stays playlist-only — Recently Played's top four moves on nearly every
   track and would leave a ~50 KB file per distinct set with nothing to reap it. **Two corrections
   came with the extraction**: it decodes through `decode_capped`, this being the one composition
   path outside the tree's bounded-decode preamble; and it decodes one source at a time against its
   own destination rect, where the original held four full-size `DynamicImage`s at once. Composed
   pixels are unchanged — `resize_to_cover` only ever read its own image.
2. `ui::artwork_cache::pair_from_image` split out of `decode_artwork` the same way, so the collage
   takes the ordinary path from an image already in hand. `mosaic_blur.rs` went whole:
   `MosaicBlur`, `compose_mosaic_blur`, `blit`, `PER_TILE`, `decode_tile`, `mosaic_blur_tests.rs`.
3. **`impl_mosaic_hero!` went, and its replacement is `impl_detail_view_helpers!`** — a new
   `artwork_only` arm, the curated pages wanting the header helper without the detail `tracks`
   swap. `ui::mosaic_hero` survives as what the two pages genuinely share: `compose_off_thread`
   and **`MosaicGuard`**, the `last_mosaic_paths` field given a type. It stops guarding a paint and
   starts guarding a recompose, and the invariant that made it correct — claim past the section
   check, never beside the compose — now has a doc home instead of being restated at four sites.
   `claim` is check-and-set under one lock, so the second of two in-flight composes is a no-op.
4. `MosaicHeroTile` draws an `Image`; `CoverMosaic` stays for the playlist *picker*, which wants
   the live form because its tiles follow a selection nothing has composed yet. A source walk pins
   that it is the picker's alone. **The arm moved from `count` to the cover**: a set whose every
   entry lacks artwork is populated and has nothing to paint, so `count` now picks between the
   page's glyph and the placeholder note rather than between the two arms. `tile-count` went with
   the hero, its whole reason having been the hero.
5. Also deleted: the two `mosaic_thumbs` `CoverThumbs` tiers (128 px, cap 16) with
   `MOSAIC_THUMB_SIZE`/`MOSAIC_THUMB_CAP`, `mosaic_cover`, `request-mosaic-cover` on both globals,
   the two `mosaic-paths` `VecModel`s and their clears. Both leaves now route through
   `release_hero_slots!`, which hands back the new `cover` slot beside the blur pair.
6. **No aurora work in this phase**, which was the point of it being a deletion.

**The cost, as costed:** a 600² compose-and-measure where there was a 192² one, dominated by the
four decodes both shapes already paid — the measure has been `color_thief` at ~0.1 ms since
Phase 6. Against it: one cache tier per page gone, and the four-full-size-decodes spike removed
from the playlist path too.

**Landed:** `compose_cover` had *no* test at all, so its four layouts are now pinned against
composed pixels (`each_layout_puts_every_source_in_its_own_rect`) along with the empty/5-source,
unreadable-source and past-the-decode-cap refusals; `mosaic_hero_tests` covers the composed pair
and the guard's claim-once contract; `the_cover_mosaic_is_the_pickers_alone` walks the tree.
Retired with their subject: the five `mosaic_blur_tests`. Two stale comments fixed in passing —
`BLUR_SIGMA`'s doc naming the mosaic composition, and a `pad-to-four` property named in
`track.rs` and `track_tests.rs` that `CoverMosaic` had already replaced with `tile-count`.

### Phase 10 — The setting

1. `SettingsData` gains the flag under `#[serde(default)]`.
2. **The default is revisited, not assumed.** v1 chose blur-by-default so the upgrade would be
   invisible. v2 weakens that: the aurora now costs 0.1 ms instead of 45, holds no buffer and
   no GPU texture, and — if Phase 8's gate passes — looks better. Recommend **aurora by
   default**, and record the decision here either way. It is a visible change to every
   existing install, which is the whole argument against.
3. The row takes the tray toggle's shape: `ToggleSwitch` with `manual: true`, a
   `"restart-backdrop"` dialog kind, one `else if` in the `accepted` dispatcher, one callback
   on `WindowChrome`, one icon-map entry, one handler ending at
   `window_chrome::request_respawn_and_quit` — which may decline, and says so.
4. The restart is what keeps the decode simple: `ArtworkCache` takes `Option<BlurSpec>` and
   `ArtworkPair.blur` becomes `Option`, so one place answers whether a blur exists at all.
   **Gating only the mount would leave every cover still decoded, blurred and uploaded** —
   the cost the setting exists to let a user avoid.
   **The flag must be raised before the first publish, not merely before `app.show()`.** Since
   Phase 8 the two arms write genuinely different tiers, so a mount and a publish that disagree
   are now visible — flipping `Theme.aurora-backdrop` mid-session leaves the blur stack painting
   `Theme.base` and `Theme.text`, which is why `Ctrl+Shift+B` misreports the blur today and why
   the restart gate is the fix rather than a convenience. `install_views` seeds all four detail
   views, so hydration has to land ahead of `wire_all`, beside the persisted nav index.
5. Delete the scaffolding: `Player.np-aurora` and the `Ctrl+Shift+B` arm in
   `shortcut-scope.slint`.
6. **The row's copy changes with the model.** v1's subtext was "no cover blurred per track,
   no buffer held". v2 can honestly add the extraction cost — but **no numbers in shipped
   copy**; figures belong here, where they can go stale visibly.
7. Label and description are two `@tr` strings and need a `msgid` in **all six** catalogues.

### Phase 11 — Tests, docs, exit

1. **New pins: all landed in Phase 8**, which is where they belong — `wash_cap` against the
   palette table on both polarities, the composite staying legible over five covers × two
   polarities, and the two arms not leaking into each other. The geometry constants agreeing
   between `ui/aurora.rs` and `aurora-backdrop.slint` landed in Phase 7, and Phase 9's own —
   the collage layouts, the guard, the `CoverMosaic` walk — with it. Nothing further is owed here
   unless Phase 10 adds a surface.
2. **Retirements: done.** The two `backdrop_tests.rs` cases asserting a solved chrome tone on an
   aurora surface and the peak-tone derivation went with `PEAK_TONE` in Phase 8, and
   `every_tint_lands_on_one_tone` with `TINT_TONE`. The chroma-band tests went with the band in
   Phase 6, and the five `mosaic_blur_tests` with their module in Phase 9.
3. **Pins that survive whole:** the dither tile's flat histogram, blue spectrum and one-level
   alpha; no ramp ending on `transparent`; the `image-fit`/tiling quartet; each of the three
   sites mounting exactly one of the two stacks; neither stack naming a global.
4. `CLAUDE.md` — the `ui/` artwork-tier paragraph. `ui-patterns.md` — the hero-bands bullet
   asserting that anything painting on a hero reads `HeroBackdrop` and never a `Theme.*`
   brush **is still true and gets more so**: consumers keep reading the tier, and the tier is
   what changed source.
5. `README.md:23` calls the artist detail screenshot "a hero-blur backdrop" — still true as
   one of two, but re-shoot it if the default flips in Phase 10.
6. Delete this file.

---

## Cross-cutting

- **Memory.** Strictly better than v1 on the aurora setting: `population_seeds` reads the
  buffer in place, so the `Vec<Argb>` of every pixel the backdrop used to build (≈590 KB
  transient at 192²) went with the Celebi call, and `Quantized::chroma`'s cluster walk with it.
  **Material You still builds its own** on the same cover when a Color Style is picked, so the
  saving is the backdrop's alone. Take one `/usr/bin/time -v` on each setting after Phase 10 to
  confirm neither regressed, and don't tune against it — we are well under the ceiling either
  way.
- **Latency.** The user-visible one: 30–45 ms between a track change and the backdrop landing
  becomes ~0.1 ms. Material You's own Celebi pass is untouched and still runs on the same
  change, so the *page* does not become instant — only the backdrop.
- **Threading is unchanged.** The extraction stays in the `spawn_blocking` that already
  decodes; the publisher still writes on the UI thread.
- **Section gating is unchanged.** A hero may still publish into a shared global only while it
  is the one on screen. Tints stay on the gated side.
- **The blur is untouched.** No phase here edits `HeroBlurBackdrop`, `write_crossfade_slot`,
  the release protocol or `BlurSpec`. If v2 fails its gate, reverting is reverting the aurora.

---

## Open questions

- ~~**Does the backdrop follow theme polarity?**~~ **Decided 2026-08-17: yes.** Mocha stays
  dark-with-colour, Latte becomes light-with-colour with dark ink. It is what makes one fixed
  foreground correct by construction, and why Amberol needs no contrast machinery at all. It
  changes two surfaces for every light-theme user, which is what Phase 8's gate is for.
- **Should the blur move to the same model?** G4Music draws its blurred cover at a flat
  `opacity: 0.25` over the theme background and has no contrast machinery either. If the blur
  went there, `backdrop.rs` would collapse to `wash_cap` alone — `luma_p90`, `scrim_alpha`,
  `composited_tone`, all three tone solves and every band constant would go, and the two
  backdrops would share one contrast argument instead of two. That is the largest deletion
  available here and it is deliberately **not** in v2's phases: it changes the blur's look,
  which is the thing v2 promises not to touch. Worth its own gate afterwards.
- **Do seeds want persisting?** Weaker than it was. A `tint_seeds` column would have avoided
  a 45 ms quantize; it now avoids 0.1 ms plus a decode. Still real for a cold open with no
  decode at all, still out of scope.
- **Should the setting be per-surface?** Unchanged: nothing forbids two flags, not worth it
  unless someone wants the blur in one place and not the other.
