# Aurora Backdrop

Working doc. Delete when the feature ships.

Status: **accepted, v2** · Created: 2026-08-16 · Rewritten: 2026-08-17
Phases 1–5 landed. Phases 6–11 below replace the old 6–8.

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
  hero become `Theme.text` / `Theme.subtext` / `Theme.accent` — the same tokens every other
  page uses. Polarity is the theme's, which is a constant, so one foreground is correct over
  the whole surface *by construction* rather than by measurement.
- **The backdrop follows theme polarity.** On Mocha it is dark with colour; on Latte it is
  light with colour and dark ink. Today a Latte user gets a dark island on two surfaces.
  **This is the product call the rest of v2 rests on** — see *Open questions*.
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
  today's solved tones. Aurora mounted → `Theme.text` / `Theme.subtext` / `Theme.accent`.
  **No `.slint` consumer changes at all** — not `TabBar`'s four brushes, not `MetaChip`,
  not `MosaicHeroTile`, not the `ui-patterns.md` hero contract. `backdrop::kind` is already
  the single place that asks which surface is painted, and it is already the sole reader.
- **`ui/backdrop.rs` narrows rather than splits.** `luma_p90` → `scrim_alpha` →
  `composited_tone` → the three tone solves become the *blur's* path and stop having an
  aurora arm. `BackdropKind::Aurora` stops meaning "skip the scrim solve" and starts meaning
  "there are no solved tones here" — the whole `BackdropColors` tier set is blur-only.
- **`ui/aurora.rs` owns the cap and nothing else.** `wash_cap(base, ink, accent, coverage)`
  is the one new function: it takes the live theme colours, the geometry's stated worst-case
  coverage, and returns the tone bound each wash is clamped into. Pure, closed form, fully
  unit-testable against the palette table above. **It caps against the worse of `Theme.text`
  and `Theme.accent`** — the accent is a user setting and a marginal one would otherwise slip
  under on a washed surface.
- **Deleted from `ui/aurora.rs`:** `TINT_TONE`, `PEAK_TONE` and both its const-asserts,
  `TINT_MIN_CHROMA`, `TINT_MAX_CHROMA`, `TINT_CHROMA_REFERENCE`, `chroma_band`, `FILL_HUES`,
  `FILL_WEIGHT`, `Tint::weight` and `Tint::to_color`'s alpha fold. The `Option`s in
  `BackdropSample::seeds` go with them — a population extractor fills the list. `dither_tile`
  and everything under it **stays**; see *What we keep that Amberol doesn't have*.
- **Deleted from `backdrop.rs`:** nothing yet. The tone bands, the scrim solve and
  `luma_p90` are all still the blur's. `TARGET_BACKDROP_TONE`'s doc comment stops naming the
  aurora.
- **`around_the_wheel` stays, and its justification changes.** It was there because the blob
  positions were fixed and rank decided which colour overlapped which. Under corner anchoring
  the overlaps are pairwise and adjacent, so hue-wheel ordering is what keeps each *pair*
  from compositing toward grey. Same code, better reason.
- **The extractor splits by question, not by crate.** `color_thief` answers "what is this
  image mostly made of" for the backdrop; `material-colors` keeps answering "what is the best
  UI seed" for Material You theming and the app-wide palette. `Quantized::chroma` had exactly
  one consumer — `chroma_band` — and goes with it, so the backdrop stops calling
  `QuantizerCelebi` entirely. **Material You is out of scope and does not change.**
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

### Phase 6 — The extractor · mechanical, one visible change

1. Add `color-thief = "0.2.2"` (latest as of 2026-08-17; one transitive dep, `rgb`).
   It builds under the 1.97.0 pin — verified by the benchmark above, not assumed.
2. `material_you.rs` gains `population_seeds(rgb, desired) -> Vec<u32>` beside
   `ranked_seeds`, wrapping `color_thief::get_palette` at quality 1. **Quality 1, not
   Amberol's 5** — 0.11 ms against 0.087 ms is not a trade worth taking, and sampling every
   pixel makes the result independent of stride artefacts on tiled art.
3. `BackdropSample::measure` calls it instead of `extract_seeds_from_rgb8`. `seeds` becomes
   `[u32; SEED_COUNT]`; `accent_argb` stays `seeds[0]` and stays `Option` for the empty-buffer
   case. `chroma` goes.
4. `extract_source_argb_from_rgb8` and everything Material You touches is **untouched**.

**Exit:** clippy and tests clean. New tests pin that a greyscale buffer yields four distinct
seeds (the case that returned one), and that the first seed is the population mode. The
visible change is seed *character* — expect the backdrop to shift hue on many covers.

### Phase 7 — Geometry · coverage becomes a number the cap can use

1. Move the four blob anchors to the four **corners**, and shrink `blob-reach` so each ramp
   dies before mid-element. This is what makes overlaps pairwise instead of four-way, and it
   is the property being ported from Amberol — not its angles.
2. **State worst-case and mid-element coverage as constants in `ui/aurora.rs`**, derived from
   the anchor set and the 0.7 stop rather than measured on screen. Phase 8's cap is a function
   of them, so they stop being an implementation detail of the `.slint` file and become part
   of the contract between the two halves. A test pins the Slint geometry against them.
3. **The vignette goes.** Neutral black at the periphery is the direct inverse of the light
   model being adopted — the corners are now where the colour is. `AuroraBackdrop.vignette`
   and its four-stop rect are deleted rather than defaulted to `transparent`; both bands
   already pass `transparent` and Now Playing was the only taker.
4. `blob-reach`'s `max(long, short)` form goes with it — corner anchoring makes the
   short-axis rescue unnecessary, and it was the term that produced the overlap in finding 2.

**Exit:** the backdrop is still solved to the old dark band (Phase 8 has not landed), so this
phase is judged on *structure*, not looks: four distinguishable colour regions instead of one
average. `aurora_backdrop_tests.rs` gains the geometry pin.

### Phase 8 — The composite model · the look gate

The phase that changes what the feature is. **Gate it on screen before Phase 9.**

1. `ui::aurora::wash_cap(base, ink, accent, coverage) -> (min_tone, max_tone)` — the closed
   form from finding 5, capping against the worse of ink and accent, in whichever direction
   the theme's polarity puts the danger.
2. `tints()` stops driving washes to a fixed tone and stops touching chroma. Each seed keeps
   its own tone and chroma, clamped into the band `wash_cap` returned. `Tint` loses `weight`;
   the per-blob `peak` hierarchy in the `.slint` file **stays**, being about area rather than
   about colour.
3. `AuroraBackdrop`'s base gradient stops taking solved floor stops and takes **`Theme.base`
   and `Theme.mantle`**. Both are `in` properties already, so this is a mount-site change at
   three sites, not a component change.
4. `backdrop.rs` publishes theme tokens into the hero and Now Playing tiers whenever
   `kind()` is `Aurora`, and today's solved tones whenever it is `Blur`. One branch, in the
   one function that already asks.
5. **`Theme.base` / `text` / `accent` must reach the solve.** They are already read there
   (`theme_accent` does exactly this), so this is three more reads through the same handle
   and no new layer dependency.

**Exit / gate:** side by side with Amberol on the same four records, on **Mocha and Latte
both** — Latte being the case that has never existed before and the one that can fail
outright. Confirm by measurement, not by eye, that the composite stays inside the caps in
finding 5 on a white cover, a black cover and a saturated one. If the look fails here, v2
stops and the doc records why.

### Phase 9 — The mosaic band is deleted, not ported

**Unchanged from v1's Phase 6 and still valid** — it is orthogonal to the colour model. The
two mosaic heroes compose artwork three times (`CoverMosaic` live in Slint,
`compose_mosaic_blur` at 192², `media::artwork::compose_artwork` at 600²), and Playlist
Detail already demonstrates the shape: compose **once** into a 600² collage, after which the
entity is an ordinary single-artwork hero.

1. Split `compose_artwork` into a pure `compose_cover(sources) -> canvas` plus the
   encode-hash-persist tail, which stays playlist-only — Recently Played's top four moves on
   nearly every track and would leave a ~50 KB file per distinct set with nothing to reap it.
2. The hero takes that buffer through the ordinary path. `mosaic_blur.rs` goes whole:
   `MosaicBlur`, `compose_mosaic_blur`, `blit`, `PER_TILE`, `mosaic_blur_tests.rs`.
3. `impl_mosaic_hero!` goes with it. **`last_mosaic_paths` stays and changes meaning** —
   it stops guarding a paint and starts guarding a *recompose*.
4. `MosaicHeroTile`'s `CoverMosaic` branch retires; `CoverMosaic` itself stays, the mosaic
   *picker* being its other consumer.
5. **No aurora work in this phase**, which is the point of it being a deletion.

**The cost, stated rather than buried:** a 600² compose-and-measure where there was a 192²
one. v2 makes this cheaper than v1 costed it — the measure is now `color_thief` at 0.1 ms
rather than Celebi at 45 ms on a 384² input, so the recompose is dominated by the four
decodes both shapes already pay.

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
5. Delete the scaffolding: `Player.np-aurora` and the `Ctrl+Shift+B` arm in
   `shortcut-scope.slint`.
6. **The row's copy changes with the model.** v1's subtext was "no cover blurred per track,
   no buffer held". v2 can honestly add the extraction cost — but **no numbers in shipped
   copy**; figures belong here, where they can go stale visibly.
7. Label and description are two `@tr` strings and need a `msgid` in **all six** catalogues.

### Phase 11 — Tests, docs, exit

1. **New pins v2 brings:** `wash_cap` against the palette table in finding 5, on both
   polarities; a white cover's composite staying inside the cap; the geometry constants
   agreeing between `ui/aurora.rs` and `aurora-backdrop.slint`; `backdrop.rs` publishing
   theme tokens on the aurora arm and solved tones on the blur arm.
2. **Pins that must be retired, not left passing vacuously:** everything in
   `aurora_tests.rs` covering the chroma band opening and closing with the artwork, and the
   `backdrop_tests.rs` cases asserting a solved chrome tone on an aurora surface. A test that
   still passes because its subject was deleted is worse than none.
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

- **Memory.** Strictly better than v1 on the aurora setting: the `Vec<Argb>` of every pixel
  that `extract_seeds_from_rgb8` built (≈590 KB transient at 192²) goes with the Celebi call,
  and `Quantized::chroma`'s cluster walk with it. Take one `/usr/bin/time -v` on each setting
  after Phase 10 to confirm neither regressed, and don't tune against it — we are well under
  the ceiling either way.
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

- **Does the backdrop follow theme polarity?** This is the product call the whole of v2 rests
  on. Today Now Playing and the hero bands are dark under all six palettes; under v2 they
  follow the theme, so a Latte user gets a light pastel-washed Now Playing with dark ink
  instead of a dark island. I think that is the right answer — it is what makes one fixed
  foreground correct by construction, and it is why Amberol needs no contrast machinery at
  all — but it changes two surfaces for every light-theme user and should be decided
  deliberately rather than arrived at.
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
