# Audio Crossfade

Design notes for the crossfade feature. Mirrors the working plan; the
authoritative summary lives as three bullets in `CLAUDE.md`.

## Context

Melodia had **gapless** playback but no **audio crossfade** — the only
`crossfade` in the tree was `write_crossfade_slot`, which cross-fades cover
*artwork*. Strawberry, Clementine, mpd, and Tauon all overlap the audio itself.
This adds that: the tail of one track fades out while the head of the next fades
in, both audible at once.

Ships **off by default**, in Settings → Playback, with a duration slider
(0.5–12 s, default 2 s) and three sub-options — skip same-album transitions,
crossfade on manual track change, and fade out on pause/stop.

## The constraint that shapes everything

rodio 0.22's `Player` **sequences** its sources: `append` schedules a source to
start when the previous one ends. Two tracks can never overlap on one `Player`.
Gapless works today *because* of this — `preload_gapless` appends the next track
behind the current one on the same `Player`.

Two mechanisms could overlap audio.

**rodio's built-in `source::crossfade(a, b, dur)` — rejected.** It expands to
`Mix<TakeDuration<A>, FadeIn<TakeDuration<B>>>`: it truncates *both* inputs to
`dur` and yields **only the overlap window**, then ends. Driving an end-of-track
crossfade with it would mean re-decoding the outgoing track's tail seeked to
`end − dur` (you cannot retroactively splice the copy already playing) and
separately appending `b.skip_duration(dur)` for the remainder. And
`Mix::try_seek` returns `SeekError::NotSupported`, so seek and per-track pause
die inside the window.

**Two `Player`s on the same mixer — the answer.** `Player::connect_new(&Mixer)`
creates an independent voice, and `Player::new()` builds its queue with
`keep_alive_if_empty = true`, so an idle deck emits silence and stays attached
to the mixer rather than detaching. Melodia already `Box::leak`s the
`MixerDeviceSink`, so both decks are created once at startup and alternate roles.

## Where the fade gain goes

`Player::append` wraps every source as:

```
EqSource → speed(1.0) → track_position() → pausable(false) → amplify(volume) → skippable → stoppable → periodic_access(5ms)
```

`EqSource` is **innermost**. Putting the ramp there — rather than driving
`Player::set_volume` from a ticker, which is the obvious approach — buys four
things:

- **Sample-accurate.** No 5 ms `periodic_access` quantization, no tokio-ticker
  jitter.
- **Speed-independent.** The ramp counts *media* samples, the same clock
  `remaining_ms` is measured in, so the fade still lands on the track end at any
  playback speed. A `set_volume` ramp is wall-clock and desyncs at speed ≠ 1.0.
- **No fight with master volume.** `Player::set_volume` stays purely the user's
  volume instead of racing a ramp against live volume changes.
- **No clipping.** `Mixer::sum_current_sources` sums its voices with **no
  clamping**. The ramp lands *after* `EqSource`'s existing `clamp(-1.0, 1.0)`,
  and the curve is complementary linear (`g_out + g_in ≡ 1`), so two overlapping
  decks sum to at most 1.0 — the same never-exceed-unity invariant as
  `MAX_VOLUME`.

Equal-power (`sin`/`cos`) is **rejected**: it holds perceived loudness across the
overlap but its amplitude sum peaks at √2. The cost of linear is the familiar dip
at the midpoint for uncorrelated material — the same trade-off GStreamer's linear
volume ramps make in Strawberry and Clementine.

## Architecture

### Dual decks — `src/player/rodio_backend.rs`

```rust
struct Deck  { player: Player, fade: Arc<FadeShared> }
struct Decks { decks: [Deck; 2], active: usize }
```

`active` holds the currently-playing track. `preload_gapless`, `query_position`
and `check_playback_state` read **only** the active deck, so their behaviour is
unchanged. `stop` / `pause` / `resume` / `set_volume` / `set_speed` apply to both;
`set_speed` re-anchors (seeks) only the active one.

`play_media` with `fade_ms == 0` clears **both** decks and reuses the live active
one. That last part is load-bearing: rodio only zeroes a `Player`'s tracked
position when `clear()` actually removes a source, so starting on a long-idle
deck reports the *previous* track's position for a few milliseconds.

`is_crossfading()` is `crossfade_armed && !idle_deck.empty()`. A fade-out armed
with `end_on_complete` returns `None` when its ramp lands, ending the source and
draining its deck — that *is* the completion signal. `sound_count` is incremented
synchronously inside `append`, so a deck that was just handed a source never
reports empty. No `Drop` impl and no extra atomic needed.

### `FadeShared` — `src/player/crossfade.rs`

A lock-free ramp cell mirroring `EqShared`'s generation-poll pattern. It is
**deck-scoped**: exactly two exist for the app's lifetime, and each is cloned
into every `EqSource` appended to its deck, so a ramp armed on a deck moves
whatever that deck is currently playing (including a gapless-appended successor).

It stores `ramp_ms` rather than a sample count, because each source converts it
with its **own** sample rate × channels — the two decks may hold tracks at
different rates, and the controller doesn't know the outgoing source's.

The same file holds `CrossfadeShared` (the master settings cell; read by the
control layer, never the audio thread, so no generation counter) and the two
decision predicates below.

### `EqSource`'s fade stage — `src/player/equalizer.rs`

`next()` polls a third generation alongside the EQ and ReplayGain ones. The
bit-identical bypass fast path survives when the cell is idle. While fading in
bypass, the sample is **clamped before scaling** — raw decoder output can exceed
full scale, and two unclamped decks summing would clip. The ramp advances once
per **frame**, not per sample, in both paths: a frame is one sample per channel,
i.e. one time step, and advancing per sample shears the stereo image.

`try_seek` deliberately does **not** touch the fade fields. `set_speed`
re-anchors the active deck with a `try_seek` to its own position, and a crossfade
abort arms a ramp and *then* seeks; resetting `fade_pos` would restart a fade-in
from silence in both cases.

### The two decision predicates

```rust
// Timing-independent: is this transition a crossfade transition at all?
crossfade_eligible(xf, pause_at_end, has_next, same_album) -> bool

// Adds the timing + liveness terms. Returns the fade length in media ms.
should_crossfade(eligible, gapless_pending, is_crossfading,
                 position_ms, duration_ms, duration_cap_ms) -> Option<u64>
```

**The split is load-bearing.** The playback monitor gates its late gapless
preload on `!crossfade_eligible` — the *timing-independent* one. If it were gated
on `should_crossfade` instead, then for any crossfade shorter than
`PRELOAD_LEAD_MS` (1.5 s) the preload would fire first (while `should_crossfade`
is still `None`), set `gapless_pending`, and then permanently block the crossfade
via its own `!gapless_pending` gate — or, worse, stage the next track twice.
`a_crossfade_shorter_than_the_gapless_preload_lead_still_fires` pins this.

`should_crossfade` returns the **real remaining** media, never clamped up to the
configured duration, so the ramp lands exactly on the declared track end and
self-corrects for the monitor's 500 ms poll granularity. Its
`remaining ∈ [MIN_FADE_MS, cap]` window doubles as a stale-position filter: too
high saturates `remaining` to zero, too low pushes it past the cap.

`same_album(a, b)` requires `a.album.is_some()`. Without that, two untagged
tracks compare equal (`None == None`) and an untagged library would never
crossfade.

### The gapless / fade-cell hazard

A staged gapless source sits *behind* the current one on the active deck and
**shares its fade cell**. A self-ending fade-out armed there would be inherited
by the staged source the moment the current one ends — it would start at full
volume and audibly fade out. Three defences:

1. The automatic path can't reach it: `crossfade_eligible` suppresses the preload
   for a crossfade transition, and `should_crossfade` refuses to fire while one
   is staged.
2. `crossfade::manual_fade_ms` refuses to fade while `gapless_pending`, so a
   manual next in the last 1.5 s of a track hard-cuts instead.
3. `play_media` re-checks the flag **under the deck lock** (the monitor's preload
   runs off the `exec_lock` that serializes actions), and `begin_crossfade`
   `debug_assert!`s the invariant.

Only `end_on_complete: true` ramps are dangerous. The pause/stop fades hold at
silence instead, so the staged source never starts.

### State machine — `src/player/state.rs`

```rust
BeginCrossfade { file_path, replaygain, fade_ms, volume, speed },
Stop { fade_ms },   // was a unit variant
```

`build_crossfade_actions(fade_ms)` mirrors `build_end_of_stream_actions`:
`UpdatePlayCount(outgoing)` → `queue.advance()` → reset position/duration/
current_track → `BeginCrossfade`. State advances at fade *start*, so Now-Playing
switches as the overlap begins — the behaviour Strawberry and mpd have. It
re-reads the queue under the emit lock rather than trusting the monitor's
`peek_next` (a skip may have landed in between), and pushes the play count only
once `advance()` has confirmed somewhere to go.

`execute_actions`' `BeginCrossfade` arm mirrors `PlayMedia`'s `Path::exists()`
pre-flight, toast, and `enqueue_auto_skip`, with one difference: it does **not**
call `rodio.stop()`. The outgoing track is still audible on the other deck, and
the `play_media` that the auto-skip produces clears both decks anyway — stopping
would only insert a gap of silence.

`Stop` carries `fade_ms`: `stop_end_of_queue` always passes `0` (the track has
already run out of audio), while `player_stop` passes `PAUSE_FADE_MS` when the
setting is on.

### Fade on pause / stop

Arms a ramp to silence with `end_on_complete: false` — holding at zero rather
than ending the source, so a staged gapless successor can't start — and defers
the real `Player::pause()` / `clear()` by that duration on a `runtime.spawn`
task. A `fade_epoch: AtomicU64` guards it: the task re-reads the epoch **while
holding the decks lock**, so a concurrent `play_media` can't be clobbered by a
deferred clear that had already passed a lock-free check.

`seek()` deliberately does **not** bump the epoch. A seek doesn't replace deck
contents, and cancelling a pending deferred pause there would leave the decks
running — silently, at the pause ramp's zero gain — while the UI reads Paused.

Mid-crossfade, pause and stop are immediate: the outgoing deck's in-flight ramp
has no start gain that could be restored on resume.

`RodioPlayer` takes a `tokio::runtime::Handle` for this, and only this.
`AppState::init(paths, runtime)` already has one before it constructs the player.

## Settings and UI

`CrossfadeFlags` is a `#[serde(default)]` substruct `#[serde(flatten)]`ed into
`SettingsData` beside `EqualizerFlags` / `ReplayGainFlags`:

| field | default |
|---|---|
| `crossfade_enabled` | `false` |
| `crossfade_duration_ms` | `2000` |
| `crossfade_manual` | `false` |
| `crossfade_skip_same_album` | **`true`** |
| `crossfade_fade_on_pause` | `false` |

Same-album defaults on so continuous-mix albums stay gapless, matching
Strawberry's and Clementine's `NoCrossfadeSameAlbum`.

Crossfade is the one audio feature that lives in **Settings → Playback** rather
than a Now-Playing overflow dialog: a `swap_horiz` toggle plus four sub-rows
mounted under `if Settings.crossfade-enabled` (never `visible: false` — a hidden
child still claims layout space, slint#7377). Wired in
`src/ui/playback_settings.rs::install_crossfade_callbacks`, which already owns
that section.

The duration slider carries **seconds** across the Slint boundary
(`crossfade-min-secs` / `-max-secs` are `in` properties seeded from
`crossfade::{MIN,MAX}_CROSSFADE_MS`, so the range lives in one place) and splits
`changed` (live apply, no disk) from `committed` (drag release, persists), the
same convention as `set-volume` / `commit-volume`.
`crossfade::{secs_to_crossfade_ms, crossfade_ms_to_secs}` own the conversion.

## How the other players do it

| | Strawberry / Clementine | Audacious | Tauon | mpd |
|---|---|---|---|---|
| Mechanism | 2nd GStreamer pipeline overlaps the old one | buffer overlap in the output plugin | fade buffers in the C backend | overlaps last N s with first N s |
| Manual vs auto | two independent toggles | separate *durations* (5 s auto, 0.2 s manual) | manual/seek only; auto stays gapless | no distinction |
| Same-album exception | `NoCrossfadeSameAlbum`, default **on** | — | auto transitions stay gapless | crossfade and gapless are mutually exclusive |
| Fade on pause/stop | separate toggles, 250 ms default | — | — | — |
| Default duration | 2000 ms (range 0–10 s) | 5 s auto / 0.2 s manual | 700 ms | integer seconds |
| Default state | everything **off** | — | — | off |

Melodia follows the Strawberry model: independent toggles, same-album exception
on, everything else off, 2 s default.

## Testing

- `src/player/tests/crossfade_tests.rs` — `ramp_gain` endpoints/midpoint/
  saturation; **complementarity** (`ramp_gain(1,0,p,n) + ramp_gain(0,1,p,n) ≡ 1`)
  including from a partial start gain; `FadeShared` arm/reset/`None`-start NaN
  round-trip; `CrossfadeShared` defaults and clamping; the `crossfade_eligible` ×
  `should_crossfade` truth tables (untagged albums, short tracks, stale positions,
  the sub-`PRELOAD_LEAD_MS` case); `manual_fade_ms`'s gapless gate.
- `src/player/tests/equalizer_tests.rs` — a fade-out ends the source when its
  ramp lands; a fade-in restores unity *and* the bypass fast path; an idle cell
  is bit-identical passthrough; the bypass path clamps a hot source before
  scaling; the ramp advances per frame (both stereo channels share a gain);
  `try_seek` leaves the ramp position alone.
- `src/player/tests/state_tests.rs` — `build_crossfade_actions` advances the
  queue and emits `[UpdatePlayCount, BeginCrossfade]`; emits nothing (and counts
  no play) when there is no next track.
- `src/player/tests/actions_tests.rs` — `MockBackend` records `begin_crossfade` /
  `stop_with_fade`; a vanished or undecodable file auto-skips **without** a stop.
- `tests/crossfade.rs` — **end-to-end against the real audio chain, no audio
  device.** rodio's `mixer()` builds a device-less `Mixer` + `MixerSource`, so
  two real `Player` decks are connected to it and the summed output is pulled by
  hand: Symphonia decoder, `EqSource` fade stage, rodio's volume/pause wrappers,
  and the unclamped mixer sum. The fixtures are constant-amplitude DC WAVs, so
  two correlated signals under a complementary linear crossfade must sum to a
  *constant* — the mixed output holds at the source amplitude across the whole
  overlap and never exceeds it. Covers the auto path, the manual path, and the
  seek-abort path.

  ⚠ Pulling `MixerSource` *is* the audio thread there. `Player::clear()` on a
  live deck and `Player::try_seek()` block until that thread services them
  through the 5 ms `periodic_access` hook, so any control op that makes one must
  run on a separate thread while the test keeps pulling — see `drive_until`.

  The tolerance (`SKEW`, 1%) absorbs one thing only: `Mixer::add` wraps each
  deck's queue in a `UniformSourceIterator` that buffers a span, so a freshly
  appended deck reaches the mixer a few hundred frames behind the one already
  playing. That skews the two ramps by a constant ≈0.4% of a 2 s fade. It is far
  tighter than any real curve error — equal-power would peak at +41%.
