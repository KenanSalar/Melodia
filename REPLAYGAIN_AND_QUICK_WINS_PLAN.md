# Melodia — Implementation Plan: ReplayGain + Quick Wins

## Context

Following a state assessment (Melodia rated 74/100; strong core, missing a power-user
cluster), we chose to build the **highest-leverage gaps that are already half-built in the
schema**. Scrobbling was considered and **deferred**. This plan covers two efforts:

1. **Apply ReplayGain / loudness normalization** — the `replaygain_*` columns are already
   populated from tags but never applied at playback.
2. **Quick half-built wins** — activate the inert `rating` column (star ratings), add a
   Recently-Played view (the `last_played` timestamp is already written), and add a sleep
   timer.

All three reuse existing, proven patterns; **no new DB migration is required** (every
column already exists). All new user-visible behavior **defaults to off/neutral** per the
live-release convention.

---

## Feature 1 — Apply ReplayGain at playback

**Design:** Mirror the existing equalizer architecture exactly. The per-track gain is
*baked into the source at construction*; the master controls (enabled / mode / preamp /
prevent-clipping) live in lock-free shared atomics so they apply live to both the playing
and gapless-preloaded sources, just like EQ. Clip protection is **free** — fold the gain
into `EqSource` as a pre-gain so its existing soft-knee limiter already guards the boost.

### 1a. Make per-track gain available at play time
- `src/entities/track.rs` — add the 4 RG fields to **`TrackSummary`** (lines 10–31), to
  `TRACK_SUMMARY_COLUMNS` (lines 117–129), and to its `From` impls. This is the playback
  projection (queue/NP) and play time is exactly when RG is needed.
  - Add: `replaygain_track_gain/peak`, `replaygain_album_gain/peak` (all `Option<f64>`).

### 1b. Master RG config as lock-free shared state (mirror `EqShared`)
- `src/player/equalizer.rs` (or a sibling `src/player/replaygain.rs`) — add a
  `ReplayGainShared` with atomics: `enabled: AtomicBool`, `mode: AtomicU8` (Off/Track/Album),
  `preamp_db: AtomicU32` (f32 bits), `prevent_clipping: AtomicBool`, `generation: AtomicU64`.
  Setters bump `generation` (`Ordering::Release`) — copy `EqShared::set_*` (lines 177–204).

### 1c. Fold pre-gain into `EqSource`
- `src/player/equalizer.rs` — extend `EqSource::new(inner, eq_shared, rg_shared, track_rg)`
  where `track_rg` carries the per-track gain/peak (baked). In `rebuild()` (lines 378–436),
  compute the effective linear pre-gain from `rg_shared` (live master) × baked track value:
  `10^((gain_db + preamp_db)/20)`, clamped by peak when `prevent_clipping`. Multiply each
  channel sample by pre-gain **before** the bands (lines 439–504); the existing limiter then
  protects the boost. **Adjust the bypass condition** (line ~430): bypass only when EQ is
  flat AND pre-gain == 1.0 — so RG-only (EQ off) still processes.

### 1d. Thread track RG through the source-build chain
- `src/player/state.rs` — `play_track_inner` (line 365) already holds `Arc<TrackSummary>`;
  put the track's RG fields into `PlayerAction::PlayMedia` (enum at lines 143–148). Add a
  small `replaygain: TrackReplayGain` field.
- `src/player/actions.rs` (lines 32–50) — pass it into `rodio_player.play_media(...)`.
- `src/player/rodio_backend.rs` — `play_media` (line 169) and `preload_gapless` (line 287)
  gain the `track_rg` arg and pass it to `EqSource::new`. Hold a `rg: ReplayGainShared` cell
  on `RodioPlayer` (init at line 152) plus `set_replaygain_*` methods (mirror lines 246–263).

### 1e. Infallible setters (mirror `player_set_eq_*`)
- `src/library/playback.rs` (lines 288–314) — add `player_set_replaygain_enabled/mode/
  preamp/prevent_clipping(ctx, ...)` → direct `ctx.rodio.set_replaygain_*()`, no
  `with_state_emit`.

### 1f. Persist + boot-hydrate (mirror `EqualizerFlags`)
- `src/services/settings/data.rs` (around lines 133–154) — add `ReplayGainFlags`
  `#[serde(default)]`: `rg_enabled: bool` (default **false**), `rg_mode: String` (default
  `"album"`), `rg_preamp: f32` (default `0.0`), `rg_prevent_clipping: bool` (default
  **true**). `#[serde(flatten)]` into `SettingsData` (near line 446).
- `src/library/settings/` (new `replaygain.rs`, mirror `equalizer.rs`) — `set_replaygain_*`
  via `services::settings::mutate_settings` (kick-after-persist).
- `src/state/mod.rs` (lines 154–161) — after EQ hydration, call `rodio.set_replaygain_*`
  from `settings.replaygain`.

### 1g. UI
- Settings audio section (alongside the equalizer entry): an enable toggle, a mode dropdown
  (Off / Track / Album), a preamp slider (reuse `SliderTrack` like the EQ preamp), and a
  "prevent clipping" toggle. New `@tr` strings added to **every** shipped `.po`. Wire a
  `ReplayGain` global (mirror `Equalizer` in `src/ui/equalizer.rs` + `ui/globals.slint`).

---

## Feature 2 — Star ratings (activate the inert `rating` column)

**Design:** 0–5 stars stored in the existing `tracks.rating` i32. Mirror the favorite-toggle
path end-to-end.

- **Projection + models:** add `rating` to `TrackListRow` (`src/entities/track.rs`) and to
  `ui/models.slint` `TrackListRow` (line ~39) — beside `is_favorite`.
- **DB query:** `src/database/queries/track.rs` — add `set_rating(db, ids, rating)` (clamp
  0–5), copy `set_favorite` (lines 273–287): `UPDATE tracks SET rating = ? WHERE id IN (...)`
  via `crate::database::placeholders(n)`.
- **Library fn:** `src/library/ratings.rs` (new, mirror `src/library/favorites.rs:9-19`) —
  `set_rating(state, ids, rating)` → query + `library_changed_tx.send_modify(...)`. Add
  `set_current_rating(state, rating)` for Now Playing (mirror `toggle_current_favorite`).
- **Callbacks:** `src/ui/callbacks/` — `on_set_row_rating(ids, rating)` (mirror
  `favorites/tracklist.rs:51-80`) and `on_set_current_rating(rating)` (mirror
  `now_playing.rs:43-69` fan-out).
- **UI control:** new `ui/components/star-rating.slint` (5 `MaterialIcon` `star`/
  `star_border`, click sets index+1, click same star clears to 0). Place in the
  track-list-row title cell near the heart (`track-list-row.slint:605-612`), a context-menu
  "Rate" entry (lines 771–775), and the Now Playing bar (`now-playing-view.slint:130-143`).
- **Sort:** add a "Rating" sort key to the track-list sort row (reuse the existing
  `RowSearchKey`/sort plumbing) so users can sort by rating; persists in `views.json` like
  other per-view sorts.

---

## Feature 3 — Recently-Played view

**Design:** New library view that mirrors the **Favorites** view, querying
`last_played DESC`. `last_played` is already written by `queries::track::update_play_count`
(`src/database/queries/track.rs:244-254`).

- **Query:** `src/database/queries/track.rs` — `get_recently_played(db, limit)` (mirror
  `get_most_played_favorites`, lines 524–539): `... WHERE last_played IS NOT NULL ORDER BY
  last_played DESC LIMIT ?`. (Optionally also `get_most_played(db, limit)` over the whole
  library for a second section.)
- **Library wrapper:** `src/library/` — thin async wrappers around the queries.
- **Nav + routing:** add a sidebar entry (`ui/layout/sidebar.slint:245` pattern) with icon
  `history` and a new nav index; add the view gate in `ui/app-window.slint` (~742–746); add
  a `view_id::RECENTLY_PLAYED` const + nav-index constant
  (`src/ui/callbacks/favorites/mod.rs:42` pattern). Confirm `history` is in
  `scripts/icons.txt` (add + re-run subset if not — `scripts/check-icons.py` enforces it).
- **View component:** `ui/views/recently-played-view.slint` — reuse the favorites view
  layout (`HorizontalCardStrip` for Most Played + a filterable `TrackList`). Add `@tr`
  strings to all `.po`.
- **View handle + callbacks:** `src/ui/recently_played/` + `wire_recently_played(...)`
  (mirror `src/ui/favorites/` + `callbacks/favorites/mod.rs`): section-active gating,
  `library_changed`-subscriber refresh (also bump-aware of `stats_changed_tx`, since plays
  change ordering), cover prewarm via `ui::grid_prewarm::unique_artwork_paths`.

---

## Feature 4 — Sleep timer

**Design:** A cancellable tokio timer that pauses (with a short fade) after N minutes. UI
lives in the Now-Playing overflow menu as a flyout, mirroring the **playback-speed flyout**.

- **Timer handle:** a small `SleepTimer { token: Mutex<Option<CancellationToken>> }` (or
  store a `JoinHandle`) owned alongside the playback context so a new selection cancels the
  prior timer. On fire: `library::playback::player_pause(ctx)` (`src/library/playback.rs:88-102`).
  Optionally fade volume out over ~5s before pausing, then restore the volume value (don't
  persist the dip).
- **Task:** spawn via `TaskSpawner::spawn_cancellable` (mirror `src/tasks/heap_trim.rs:37-48`):
  `select!` on `shutdown.cancelled()` / the per-timer token / `tokio::time::sleep(mins*60)`.
- **UI:** `ui/components/now-playing/overflow-menu.slint` — add a permanent "Sleep timer"
  `OverflowRow` (lines 15–84) opening a `SleepTimerFlyout` inside the **same** PopupWindow
  (single-popup discipline — mirror `speed-flyout.slint` + the fixed-reserve geometry at
  lines 207–219/249–259). Presets: Off, 15, 30, 45, 60, 90 min, plus "End of current track".
  The active row shows the remaining time as `trailing-text`.
- **Callback:** `Player.set-sleep-timer(minutes)` wired in `src/ui/callbacks/now_playing.rs`
  (mirror the speed callback) → (re)spawn/cancel the timer task. The timer is **session-only**
  (not persisted).

---

## Cross-cutting notes

- **No migration needed** — `rating`, `last_played`, and all `replaygain_*` columns already
  exist in `migrations/20260514000000_initial_schema.sql`.
- **Models mirror exactly** — any field added to a Rust boundary struct must be added to the
  matching `ui/models.slint` struct (and vice-versa).
- **i18n** — every new literal wrapped in `@tr(...)` must be added to all seven `.po` files.
- **Docs** — update `CLAUDE.md` (and `MIGRATION.md` if present) per the post-phase doc
  convention: ReplayGain source-chain note, ratings path, Recently-Played view wiring,
  sleep-timer handle.
- **No autonomous commits.**

## Suggested build order
1. ReplayGain (1a→1g) — self-contained in the player/settings layers, highest value.
2. Star ratings (Feature 2) — smallest, reuses the favorite path 1:1.
3. Recently-Played view (Feature 3) — reuses the Favorites view.
4. Sleep timer (Feature 4) — independent, reuses the speed-flyout.

---

## Verification

- `cargo clippy --all-targets -- -D warnings` clean after each feature (not `cargo check`).
- `cargo test` green; add unit tests beside the new code (per-module `tests/` subdir):
  - ReplayGain: gain-math (dB→linear, peak clamp, bypass condition) in `equalizer` tests;
    `ReplayGainFlags` serde defaults round-trip.
  - Ratings: `queries::track::set_rating` clamps 0–5 and updates only the targeted ids
    (use `setup_seeded_db`).
  - Recently-Played: `get_recently_played` ordering + `last_played IS NULL` exclusion.
  - Sleep timer: timer fires/cancels (use `#[tokio::test(start_paused = true)]` +
    `tokio::time::advance`).
- Manual run (`RUST_LOG=info cargo run`):
  - **ReplayGain:** play a track with RG tags, toggle ReplayGain on, switch Track/Album mode
    and the preamp — audible level change, no clipping on boosted tracks; setting persists
    across restart; defaults off on first run.
  - **Ratings:** rate tracks in a list, in the context menu, and in Now Playing; sort by
    rating; values survive restart.
  - **Recently-Played:** play several tracks; the view lists them newest-first and updates
    after each play (`stats_changed_tx`).
  - **Sleep timer:** set a short interval, confirm fade+pause fires; selecting a new
    interval cancels the previous; "Off" cancels.
- Release/RSS gate only at the end (per memory: defer until the binary exercises the code):
  `/usr/bin/time -v target/release/Melodia` — stay under the ~200 MB ceiling (new caches are
  none; LRUs unchanged).
