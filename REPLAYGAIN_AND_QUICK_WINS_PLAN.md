# Melodia — Implementation Plan: ReplayGain + Quick Wins

## Context

Melodia has a strong core but is missing a power-user cluster. This plan builds the
**highest-leverage gaps that are already half-built in the schema**. Scrobbling was
considered and **deferred** (it remains the top user-visible parity gap for a follow-up).
This plan covers two efforts:

1. **Apply ReplayGain / loudness normalization** — the `replaygain_*` columns are already
   populated from tags but never applied at playback.
2. **Quick half-built wins** — activate the inert `rating` column (star ratings), add a
   Recently-Played view (the `last_played` timestamp is already written), and add a sleep
   timer.

All three reuse existing, proven patterns; **no new DB migration is required** (every
column already exists in `migrations/20260514000000_initial_schema.sql`). All new
user-visible behavior **defaults to off/neutral** per the live-release convention.

> **This plan was validated against the code.** Line numbers below are anchors captured at
> validation time — treat them as approximate (they drift as files change). The critical
> DSP gotchas (⚠ M1–M4 in Feature 1) are load-bearing; read them before implementing.
>
> **Re-validated 2026-07-04.** The overall approach was re-confirmed against the current tree —
> the ReplayGain DSP design and ⚠ M1–M4 are all accurate. Corrections were folded in: the ratings
> sort mechanism (it is **not** `RowSearchKey`), the Recently-Played cover-prewarm call and the
> `last_played` timestamp check (already sortable), and the sleep-timer handle ownership +
> end-of-track scope. Two scope forks were resolved with the user: **ratings ship at full parity**
> (inline editable star in every row + optimistic per-view fan-out) and **the sleep timer includes
> "End of current track"** (⚠ M6 is now in-scope for v1).

### Verified facts the design relies on

- **RG units.** `src/media/metadata.rs:84-92` — `parse_replaygain_gain` stores the raw **dB**
  value (`"-6.48 dB"` → `-6.48`, via `trim_end_matches("dB")`); `parse_replaygain_peak` stores
  the raw **linear** value (`"0.977295"`). Both return `Option<f64>`. So playback must apply
  `10^(gain_db/20)` itself, and `peak` is already the linear value to clamp against.
- **Schema.** `replaygain_{track,album}_{gain,peak} REAL` (nullable), `rating INTEGER NOT NULL
  DEFAULT 0`, `last_played TEXT` (nullable). Partial indexes **already exist**:
  `idx_tracks_rating (WHERE rating > 0)` and `idx_tracks_last_played (WHERE last_played IS NOT
  NULL)` — Features 2 & 3 need no index work.
- **RG is only carried on the full `Track` struct** (`src/entities/track.rs`, RG fields
  ~lines 301-309). `TrackSummary`, `TrackListRow`, `TrackMeta` do **not** carry RG today. There
  is no `TrackFull` — the full row is named `Track`.

---

## Feature 1 — Apply ReplayGain at playback ✅ **DONE (2026-07-04)**

> **Shipped.** Implemented end-to-end: per-track gain baked per `EqSource`, master state in a
> lock-free `ReplayGainShared` (mirrors `EqShared`), reused EQ limiter, persisted `ReplayGainFlags`,
> boot hydration, and an overflow-menu dialog. `cargo clippy --all-targets -- -D warnings` clean;
> all unit tests green (RG gain math, mode/fallback, peak clamp, ⚠ M1/M2 guards, serde defaults).
> New files: `src/player/replaygain.rs`, `src/library/settings/replaygain.rs`, `src/ui/replaygain.rs`,
> `ui/components/dialog/replaygain-body.slint`. Manual audible verification (listening for the
> level change, no clipping, gapless continuity) is the remaining user-side check.
>
> **Two plan errors were corrected during implementation:**
> 1. **Gapless RG is threaded through `src/player/handlers.rs`, NOT `PlayerAction::PreloadGapless`.**
>    The real preload is a direct `rodio_player.preload_gapless(...)` call in the playback monitor
>    (reads `state.queue.peek_next()`); the enum variant is only ever `PreloadGapless(None)` (a
>    *clear*). So the enum was left unchanged; `preload_gapless`'s signature gained a `baked_rg`
>    param and `handlers.rs` captures `t.replaygain()` alongside the path. The `preload_gapless`
>    trait method (and its `MockBackend` impl) also took the new param.
> 2. **The UI is an overflow-menu dialog, not a settings-page row** — the EQ has no settings-page
>    entry; both live in `ui/components/now-playing/overflow-menu.slint` → a `Dialog` (`kind ==
>    "replaygain"` → `ReplayGainBody`). Icon: `tune` (already in `scripts/icons.txt`, no re-subset).

**Design:** The per-track gain values are **baked into each `EqSource` at construction** (they
must be per-source, not shared — see ⚠ M3). The master controls (enabled / mode / preamp /
prevent-clipping) live in lock-free shared atomics so they apply live to both the playing and
gapless-preloaded sources, exactly like `EqShared`. Clip protection is **reused for free** —
apply the RG gain as a pre-gain inside `EqSource` (before the bands) so its existing soft-knee
`Limiter` (equalizer.rs:256-307, run at :485-494) already guards any boost.

Signal chain: `input → ×(preamp × rg_gain) → EQ bands → coupled limiter → clamp ±1.0`, then
rodio applies user volume (≤1.0) after the source. Safe ordering: normalize, EQ, limit, volume.

### 1a. Make per-track gain available at play time
- `src/entities/track.rs` — add the 4 RG fields to **`TrackSummary`** (the playback/queue/NP
  projection; `play_track_inner` already holds `Arc<TrackSummary>`) and to
  `TRACK_SUMMARY_COLUMNS`, plus its `From`/`FromRow` wiring.
  - **Store them as `f32`, cast from the DB `f64`/REAL** — EQ/RG math is f32 anyway, and this
    halves the per-queue-row memory cost (memory discipline). Prefer a single newtype
    `TrackReplayGain { track_gain, track_peak, album_gain, album_peak: Option<f32> }` over four
    loose `Option` fields — it threads cleanly through the actions (1d).

### 1b. Master RG config as lock-free shared state (mirror `EqShared`)
- New `src/player/replaygain.rs` (sibling to `equalizer.rs`) — `ReplayGainShared` with atomics:
  `enabled: AtomicBool`, `mode: AtomicU8 {Track, Album}` (**no redundant `Off`** — `enabled`
  already gates, exactly like EQ), `preamp_db: AtomicU32` (f32 bits),
  `prevent_clipping: AtomicBool`, `generation: AtomicU64`. Setters bump `generation`
  (`Ordering::Release`) — copy `EqShared` setters (equalizer.rs:181-204) and the generation
  seeding (`new()` starts generation at 1).
  - Define **RG-specific range constants** `RG_MIN_PREAMP_DB` / `RG_MAX_PREAMP_DB` (conventionally
    symmetric, e.g. ±15 dB, default 0) — do **not** reuse the EQ preamp's asymmetric −12/+6.

### 1c. Fold RG pre-gain into `EqSource` (⚠ the load-bearing DSP changes)
- `src/player/equalizer.rs` — `EqSource::new` (currently `:342`, signature
  `new(input: S, shared: Arc<EqShared>)`) gains two params: `rg_shared: Arc<ReplayGainShared>`
  and `baked_rg: TrackReplayGain`. Add fields `rg_gain: f32` (init 1.0), the baked RG values,
  and RG generation tracking.

- **⚠ M1 — do NOT let the EQ-off early return kill RG.** `rebuild()` (`:381`) currently starts
  with `if !self.shared.enabled() { self.bypass = true; …; return; }` (`:382-386`). RG is
  independent of the EQ toggle, so restructure: compute `rg_gain` from `rg_shared` **first**
  (before that check); in the EQ-off branch set `self.bypass = rg_is_unity` instead of an
  unconditional `true`. Final active-path bypass (`:435`) becomes
  `self.bypass = !any_active && preamp_is_unity && rg_is_unity`.

- **⚠ M2 — `next()` must poll BOTH generations.** `next()` (`:442`; its generation poll is at
  `:453`) only watches `eq_shared.generation()`, so live RG changes wouldn't apply. Rebuild when **either** counter
  moves: track a separate `last_rg_generation`, or compare the sum `eq_gen + rg_gen` (both
  monotonic → any bump increases the sum) against a cached sum.

- **⚠ M4 — bake all four; pick track/album + fallback at rebuild from the live mode.** Because
  `mode` is a live master control, the source can't pre-select. In `rebuild()`, choose per
  `rg_shared.mode`: **Album** → album_gain, falling back to track_gain if album is `None`;
  **Track** → track_gain, falling back to album_gain if track is `None`; **neither present** →
  unity (0 dB) so untagged tracks are unchanged.

- **RG gain formula (with prevent-clipping):**
  `rg_lin = 10^((gain_db + preamp_db)/20)`; then if `prevent_clipping` and the matching `peak`
  is `Some(p)` with `p > 0`: `rg_lin = rg_lin.min(1.0 / p)`; if peak is `None`, skip the clamp
  and rely on the limiter. Clamp the **total** effective gain. (prevent-clip is a static
  peak-based guard; the limiter is a dynamic guard that also catches EQ-band boosts — keep both.)

- **Apply site:** at `:471` the active path does `let mut out = x * self.preamp_gain;` — change
  to `x * self.preamp_gain * self.rg_gain`. Use a **separate `rg_gain` field**; do not fold into
  `preamp_gain` (different clamp range/semantics).

- Update the `EqSource` module doc — it now applies EQ **and** ReplayGain.

### 1d. Thread baked track RG through the source-build chain (⚠ M3 — gapless-critical)
Per-track gain **must be baked per source**, not held on a shared cell: the gapless-preloaded
next track (`EqSource` built in `preload_gapless`, `:287`) has different RG than the playing
track, so a shared "current track RG" cell would feed the preloaded source the wrong value.

- `src/player/state.rs` — extend **both** actions in the `PlayerAction` enum (`:141-164`):
  `PlayMedia { … }` (`:143-148`) gains a `replaygain: TrackReplayGain` field; and
  `PreloadGapless(Option<String>)` (`:157`) must also carry the next track's RG — change it to a
  struct/tuple like `PreloadGapless { file_path: Option<String>, replaygain: TrackReplayGain }`.
  Populate both from the `Arc<TrackSummary>` in scope: `play_track_inner` (fn header `:365`,
  emits `PlayMedia` `:387-392`) for PlayMedia, and the PreloadGapless build site (the queue's next
  item is a `TrackSummary`) — covers `build_next_actions` (`:282`), `build_previous_actions`
  (`:307`), and `resume_from_stopped` (`:405`), which all route through `play_track_inner`.
- `src/player/actions.rs` — update the `PlayMedia` destructure/handler (`:32-45`) and the
  `PreloadGapless` handler (`:60-62`) to pass RG into the backend calls.
- `src/player/rodio_backend.rs` — hold `rg: Arc<ReplayGainShared>` on `RodioPlayer` (`:134-143`,
  init in `new` at `:146-154` mirroring `eq` at `:152`); `play_media` (`:158-178`) and
  `preload_gapless` (`:271-302`) gain a `baked_rg: TrackReplayGain` arg and pass it +
  `self.rg.clone()` to `EqSource::new` (`:169`, `:287`). Add inherent `set_replaygain_*` methods
  (mirror `set_eq_*` at `:246-263`).

### 1e. Infallible live setters (mirror `player_set_eq_*`)
- `src/library/playback.rs` (`:297-313`) — add `player_set_replaygain_{enabled,mode,preamp,
  prevent_clipping}(ctx, …)` → direct `ctx.rodio.set_replaygain_*()`, no `with_state_emit`.

### 1f. Persist + boot-hydrate (mirror `EqualizerFlags`) — **apply-live-first, then persist**
- `src/services/settings/data.rs` — add `ReplayGainFlags` (mirror `EqualizerFlags` at
  `:133-143`, whole-struct `#[serde(default)]`): `rg_enabled: bool` (default **false**),
  `rg_mode: String` (default `"album"`), `rg_preamp: f32` (default `0.0`), `rg_prevent_clipping:
  bool` (default **true**). `#[serde(flatten)]` into `SettingsData` (beside `equalizer` at
  `:445-446`), and set it in `SettingsData::Default`.
- `src/library/settings/replaygain.rs` (new, mirror `equalizer.rs`) — `set_replaygain_*` via
  `services::settings::mutate_settings`. **Note the real EQ ordering: there is NO
  "kick-after-persist".** The live/runtime apply happens *first*, in the UI callback (via the
  `player_set_replaygain_*` setters from 1e), and these functions are pure `mutate_settings`
  disk writes called afterward. Mirror that "apply-live-then-persist" ordering.
- `src/state/mod.rs` (`:159-161`, right after EQ hydration) — seed `rodio.set_replaygain_*` from
  `settings.replaygain` before playback starts.

### 1g. UI
- `Equalizer`-style global `ReplayGain` (`src/ui/replaygain.rs` + `ui/globals.slint`, mirror
  `install_equalizer` at `src/ui/equalizer.rs:26` and the `Equalizer` global at
  `ui/globals.slint:144-168`). Rust-owned; seed dB ranges from the `RG_*_PREAMP_DB` constants
  (single source of truth), mirroring how `install_equalizer` seeds `min/max` (`:67-70`).
- Settings audio section (beside the equalizer entry): an **enable toggle**, a **mode dropdown
  (Track / Album)** — no "Off" (the toggle gates), a **preamp slider** (reuse `SliderTrack` like
  the EQ preamp, with the set-live `set-preamp` / commit-on-release `commit-preamp` split), and a
  **"prevent clipping" toggle**. New `@tr` strings added to **every** shipped `.po`.

---

## Feature 2 — Star ratings (activate the inert `rating` column)

**Design:** 0–5 stars stored in the existing `tracks.rating` (`INTEGER NOT NULL DEFAULT 0`).
Mirror the favorite-toggle path end-to-end. Sort-by-rating is already index-backed by
`idx_tracks_rating`.

- **Projection + models:** `rating: i32` **already exists on the full `Track` struct**
  (`src/entities/track.rs:333`), so the write path can round-trip it — the work is surfacing it in
  the list projection: add `rating` to `TrackListRow` (struct `:77-109`) **and**
  **`TRACK_LIST_COLUMNS`** (`:133-137`, 19→20 columns), then mirror it in `ui/models.slint`
  `TrackListRow` (struct opens `:30`) beside `is_favorite` (`:39`).
- **DB query:** `src/database/queries/track.rs` — `set_rating(db, ids, rating)` (clamp 0–5), copy
  `set_favorite` (`:274-291`): `UPDATE tracks SET rating = ? WHERE id IN (...)` via
  `crate::database::placeholders(n)` (defined `src/database/mod.rs:17`). No `set_rating` exists yet.
- **Library fn:** `src/library/ratings.rs` (new, mirror `src/library/favorites.rs:9-19`) —
  `set_rating(state, ids, rating)` → query + `library_changed_tx.send_modify(...)`, plus
  `set_current_rating(state, rating)` for Now Playing (mirror `toggle_current_favorite`).
- **Callbacks (⚠ full-parity fan-out):** row toggle `on_set_row_rating(ids, rating)` mirrors
  `src/ui/callbacks/favorites/tracklist.rs:61-79`; the Now-Playing fan-out
  `on_set_current_rating(rating)` mirrors **`src/ui/callbacks/now_playing.rs:43-69`** (NOT under
  `favorites/`). Full parity means replicating the optimistic
  `flip_favorite`/`apply_row_favorite` (+ `flip_detail_favorite`/`apply_detail_row_favorite`)
  pattern across the **same ~6 surfaces** the favorite path already fans into: tracks
  (`callbacks/tracks.rs:149`), browse, search, playlists/detail, and albums/artists/genres detail.
  This is the bulk of the feature — it is *not* a single-site change.
- **UI control:** new `ui/components/star-rating.slint` (5 `MaterialIcon` `star`/`star_border`,
  click sets index+1, click same star clears to 0). **⚠ Icons:** `scripts/icons.txt` has **no**
  star glyphs today — add `star` + `star_border` (+ `star_half` if half-stars), then re-run
  `scripts/subset-icon-fonts.sh` and `scripts/check-icons.py` or they render as tofu. Placement
  (the three favorite-heart sites): `track-list-row.slint` title cell (badge `:527-530`, hover
  toggle `:602-612`, context-menu "Rate" entry near `:765-775`), `now-playing-bar.slint:375-386`,
  and the Now-Playing `overflow-menu.slint`.
- **Sort (⚠ mechanism — `RowSearchKey` is the *filter/search* key, not sort plumbing):** a Rating
  sort touches **both** sort paths — (a) the in-memory `sort_track_rows_by` in
  `src/ui/track_sort.rs` (add a `"rating"` arm; used by Tracks/Album/Artist/Genre/Search — needs
  `rating` on `TrackListRow`, above), and (b) the DB `track_list_order_by` in
  `src/database/queries/track.rs:21-44` (add a `Some("rating")` arm; used by the Favorites/list
  re-fetch). Add a header cell in `ui/components/track-list/track-list-header.slint`. Persistence
  is free — `ViewSort { field, dir }` in `views.json` takes an arbitrary field string.

---

## Feature 3 — Recently-Played view ✅ **DONE (2026-07-05)**

> **Shipped.** A new sidebar view (nav index 8; Settings renumbered to 9) mirroring a **trimmed**
> Favorites — no hero mosaic / artist strip. Header (count + duration + Play All / Shuffle) → a
> **non-collapsible** "Most Played" `HorizontalCardStrip` → a filterable `TrackList` of the 200
> most-recently-played tracks (`last_played DESC`). Membership is fixed to that set; the search
> filter and column re-sort re-walk the cached rows **in memory** (`RECENCY_SORT` sentinel keeps
> fetch order; real fields via `sort_track_rows_by`) — never re-querying. It is the **2nd**
> subscriber to `stats_changed_tx` (Favorites was sole). `cargo clippy --all-targets -- -D warnings`
> clean; `cargo test` green (791 lib tests incl. `get_recently_played` / `get_most_played` ordering
> + exclusion + LIMIT). New files: `src/library/recently_played.rs`, `src/ui/recently_played/*`,
> `src/ui/callbacks/recently_played/*`, `ui/views/recently-played-view.slint`, `RecentlyPlayed`
> global. `history` icon added + re-subset; 3 new `@tr` strings in all 6 `.po` files. Manual GUI
> verification (play tracks → newest-first list, auto-update on flush, Most Played strip, filter +
> column sort, restart persistence) is the remaining user-side check.
>
> **Two scope decisions during implementation:** (1) the Most Played strip is **non-collapsible** —
> `ViewStateData` is already at clippy's `struct_excessive_bools` cap (3 collapse bools), so a 4th
> persisted collapse flag would trip the lint; a fixed ~10-card strip doesn't need collapse. (2) The
> track sort is **in-memory** (not the DB `track_list_order_by` path Favorites uses) because the
> recency set's membership must stay fixed to the 200 most-recent — a DB re-sort would change which
> rows appear.
>
> **Follow-up (post-review polish):** on request, the view was brought to **full Favorites parity** —
> the trimmed plain header was replaced with the Favorites-style **hero** (live 2×2 blur cover mosaic
> of the 4 most-recently-played distinct covers via a new `hero.rs` + `mosaic_thumbs` tier, reusing
> `write_crossfade_slot`), so the below-hero unified scroll now reads identically to Favorites. The
> sidebar entry was **moved to render directly under Favorites** while keeping routing `index: 8`
> (sidebar visual order = source order, so no tab-renumber was needed). Favorite-Artists strip stays
> omitted; Most Played stays non-collapsible.
>
> **Correction to the "no migration needed" premise:** the `last_played` **column** always existed,
> but its partial index `idx_tracks_last_played` had been **dropped** in
> `20260612000000_drop_unused_track_indexes.sql` (rationale: `last_played` was write-only, "no
> recently-played surface exists"). This view *is* that surface, so a new migration
> `20260705000000_readd_last_played_index.sql` re-creates it. `EXPLAIN QUERY PLAN` confirms the
> `ORDER BY last_played DESC LIMIT 200` query goes from `SCAN tracks` + `USE TEMP B-TREE FOR ORDER
> BY` (no index) to `SEARCH tracks USING INDEX idx_tracks_last_played` (with it). Symmetric with the
> `idx_tracks_play_count` index that same drop migration deliberately kept for Most Played.

**Design:** New library view mirroring **Favorites**, querying `last_played DESC`. `last_played`
is already written by `queries::track::update_play_count` and index-backed by
`idx_tracks_last_played`.

- **Timestamp format — ✅ confirmed sortable, no fix needed.** `last_played` is written as
  `crate::utils::now_rfc3339()` = `chrono::Utc::now().to_rfc3339()` (fixed `+00:00` UTC offset,
  fixed-width prefix), so it is lexically == chronologically ordered and `ORDER BY last_played
  DESC` is correct on the `TEXT` column. Note the **real writer is the batching
  `src/tasks/play_count_flusher.rs`** (`update_play_count` at `:244-254` is only the non-flusher
  fallback); both write the same `now_rfc3339()` value.
- **Query:** `src/database/queries/track.rs` — `get_recently_played(db, limit)` mirrors
  `get_most_played_favorites` (`:524-539`) **but returns the `TrackListRow` projection** (for the
  filterable `TrackList`, not the 6-col `MostPlayedFavorite` strip type): `SELECT
  {TRACK_LIST_COLUMNS} ... WHERE last_played IS NOT NULL ORDER BY last_played DESC LIMIT ?`.
  Neither `get_recently_played` nor a generic `get_most_played` exists yet. (Optionally add
  `get_most_played(db, limit)` — a 6-col strip like the favorites one — for a 2nd section.)
- **Library wrapper:** thin async wrappers around the queries.
- **Nav + routing (⚠ index-renumber ripple):** a new sidebar entry means inserting a `SidebarItem`
  in `ui/layout/sidebar.slint:239-256` and **renumbering every index after it** (Settings is index
  8 today), which ripples through the per-index `if Nav.selected-index == N` branches, the
  section-active mirror block, the tab-title map, and the fallback guard in `ui/app-window.slint`,
  plus the `NAV_*` consts (`callbacks/favorites/mod.rs:42`, `cross_tab_nav.rs`) and `view_id::*`
  (`src/ui/track_list_view.rs:102-116`). **Recommend appending the entry just before Settings** to
  minimize churn (accept a mid-list insert only if the ordering matters). `history` is **absent**
  from `scripts/icons.txt` — add it + re-run `scripts/subset-icon-fonts.sh`
  (`scripts/check-icons.py` enforces it).
- **View component:** `ui/views/recently-played-view.slint` — reuse the favorites layout
  (`HorizontalCardStrip` for Most Played + a filterable `TrackList`). `@tr` strings to all `.po`.
- **View handle + callbacks:** `src/ui/recently_played/` + `wire_recently_played(...)` (mirror
  `FavoritesUi` + `callbacks/favorites/mod.rs:48-61`): section-active gating, and the joined
  `library_changed` **+ `stats_changed_tx`** `tokio::select!` refresh loop
  (`favorites/lifecycle.rs:92-119`) — play-count flushes bump `stats_changed_tx` (not
  `library_changed_tx`) and change this view's ordering, so it becomes that channel's **2nd
  subscriber** (Favorites is sole today, `lifecycle.rs:93`). **Cover prewarm:** mirror Favorites —
  call `AsyncThumbnailCache::prewarm` directly with a `Vec<PathBuf>` (`favorites/sections.rs:95/98`,
  `favorites/tracks.rs:80`); it self-dedupes. (Favorites does **not** use
  `ui::grid_prewarm::unique_artwork_paths` — that helper is for the entity grids/detail views.)

---

## Feature 4 — Sleep timer

**Design:** A cancellable tokio timer that pauses (with a short fade) after N minutes. UI lives
in the Now-Playing overflow menu as a flyout, mirroring the **playback-speed flyout**.

- **Timer handle (⚠ lives on `AppState`, not the playback context):** `PlaybackContext`
  (`src/state/contexts.rs:32-38`) is an **ephemeral per-call snapshot** rebuilt by
  `state.playback_ctx()`, so a persistent `SleepTimer { token: Mutex<Option<CancellationToken>> }`
  (+ the `pause_after_current_track` flag from ⚠ M6) must live on **`AppState`**
  (`src/state/mod.rs`, beside `shutdown_token`/`task_tracker`). There is no existing per-request
  cancellable-task pattern — this is new. On fire: `library::playback::player_pause(&ctx)`
  (`src/library/playback.rs:88-102`, takes `&PlaybackContext`). Optionally fade volume out over
  ~5 s before pausing, then restore the volume value (don't persist the dip; guard against the user
  changing volume mid-fade and against the track having already changed).
- **Task (⚠ per-timer token ≠ global shutdown):** `TaskSpawner::spawn_cancellable`
  (`src/tasks/spawner.rs:70-77`, mirror `src/tasks/heap_trim.rs:37-48`) hands the closure only the
  **global** `shutdown` token. For per-selection cancellation, mint a separate per-timer
  `CancellationToken`, store it in the AppState handle, capture it into the closure, and `select!`
  on **all three**: `shutdown.cancelled()` (global), the per-timer token, and
  `tokio::time::sleep(mins*60)`.
- **⚠ M6 — "End of current track" is NOT a fixed timer — build it in v1.** A duration `sleep`
  can't express "pause when this track ends," so implement it separately: set a
  `pause_after_current_track` flag (on `AppState`, alongside the timer handle) and check it at the
  track-advance boundary. **The real loop is `spawn_playback_monitor` in
  `src/player/handlers.rs`** (`tasks/playback_monitor.rs` is only a thin wrapper). Two boundaries,
  both inside the single `with_state_emit` state lock: `PlaybackCheck::EndOfStream` (~`:126-146`) —
  check the flag before `state.queue.advance()` (~`:136`) and pause instead. `PlaybackCheck::
  GaplessTransition` (~`:103-125`) is the trickier case: the next track is **already
  preloaded/playing in rodio**, so also gate the late-preload block in the `Playing` branch
  (~`:168-177`) when the flag is armed, so it never preloads the next track and the "pause after
  current" actually lands. Keep "End of current track" in the preset list.
- **UI:** `ui/components/now-playing/overflow-menu.slint` — permanent "Sleep timer" `OverflowRow`
  opening a `SleepTimerFlyout` **inside the same PopupWindow** (single-popup discipline — mirror
  `speed-flyout.slint` + the fixed-reserve geometry at `overflow-menu.slint:175-184`). Presets:
  Off, 15, 30, 45, 60, 90 min, **and "End of current track"** (in-scope per ⚠ M6). Active row shows
  remaining time as `trailing-text` (or "current track" for the end-of-track mode).
- **Callback:** `Player.set-sleep-timer(minutes)` wired in **`src/ui/callbacks/mod.rs:139-155`**
  (where `on_set_playback_speed` lives — NOT `now_playing.rs`); mirror the speed callback's
  synchronous-apply step but **drop the persist step** → (re)spawn/cancel the timer task, or arm
  the `pause_after_current_track` flag for the end-of-track preset. **Session-only** (not
  persisted).

---

## Cross-cutting notes

- **No migration needed** — `rating`, `last_played`, and all `replaygain_*` columns already
  exist in `migrations/20260514000000_initial_schema.sql` (confirmed).
- **Per-track RG must be baked per source, master RG on a shared cell** (⚠ M3) — the split is
  load-bearing for gapless. Don't collapse per-track gain onto a shared cell.
- **Models mirror exactly** — any field added to a Rust boundary struct must be added to the
  matching `ui/models.slint` struct (and vice-versa).
- **i18n** — every new literal wrapped in `@tr(...)` must be added to all seven `.po` files.
- **Docs** — update `CLAUDE.md` per the post-phase doc convention: the ReplayGain source-chain
  note (baked per-track gain + shared master + reused limiter), ratings path, Recently-Played
  view wiring, sleep-timer handle.
- **No autonomous commits.**

## Suggested build order
1. ReplayGain (1a→1g) — self-contained in the player/settings/UI layers, highest value.
2. Recently-Played view (Feature 3) — reuses the Favorites view; contained.
3. Star ratings (Feature 2) — reuses the favorite path, but at **full parity** it is the widest
   change (new projection column + models mirror + both sort paths + icon re-subset + optimistic
   fan-out across ~6 view surfaces). Do it after the smaller wins.
4. Sleep timer (Feature 4) — independent, reuses the speed-flyout; the end-of-track mode (⚠ M6)
   touches the playback-monitor loop, so land the fixed-duration presets first, then M6.

---

## Verification

- `cargo clippy --all-targets -- -D warnings` clean after each feature (not `cargo check`).
- `cargo test` green; add unit tests beside the new code (per-module `tests/` subdir):
  - ReplayGain gain-math: dB→linear, mode selection **with track/album fallback**, unity for
    untagged tracks, peak clamp under `prevent_clipping` (and no-clamp when peak is `None`).
  - **⚠ M1 guard:** RG enabled + EQ **disabled** still processes (not bypassed).
  - **⚠ M2 guard:** a live RG-only change (no EQ change) triggers a rebuild / takes effect.
  - `ReplayGainFlags` serde defaults round-trip (off / "album" / 0.0 / prevent-clip true).
  - Ratings: `queries::track::set_rating` clamps 0–5 and updates only the targeted ids
    (`setup_seeded_db`).
  - Recently-Played: `get_recently_played` ordering (over flusher-written RFC3339 timestamps) +
    `last_played IS NULL` exclusion.
  - Sleep timer: fixed-duration fires/cancels (`#[tokio::test(start_paused = true)]` +
    `tokio::time::advance`); **end-of-track (⚠ M6):** arming `pause_after_current_track` makes the
    advance boundary pause instead of advancing (assert on both `EndOfStream` and
    `GaplessTransition` paths).
- Manual run (`RUST_LOG=info cargo run`):
  - **ReplayGain:** play a track with RG tags; toggle on; switch Track/Album mode and the
    preamp — audible level change, no clipping on boosted tracks; **works with the EQ off**;
    persists across restart; defaults off on first run.
  - **Ratings:** rate in a list, context menu, and Now Playing; sort by rating; survives restart.
  - **Recently-Played:** play several tracks; view lists them newest-first and updates after each
    play (via `stats_changed_tx`).
  - **Sleep timer:** short interval → fade+pause fires; new interval cancels the previous; "Off"
    cancels; **"End of current track"** pauses at the next track boundary (verify across a gapless
    transition, not just end-of-queue).
- Release/RSS gate only at the end (defer until the binary exercises the code):
  `/usr/bin/time -v target/release/Melodia` — stay under the ~200 MB ceiling (widening
  `TrackSummary` by 4 `f32`s is negligible; no new caches).
