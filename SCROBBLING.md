# Scrobbling: Last.fm + ListenBrainz

> **Working doc.** Phased plan for the scrobbling feature. Keep the phase
> checkboxes current as work lands; delete this doc when the feature ships
> (per project convention).

## Context

Melodia tracks local play counts but shares nothing with the user's listening
history services. Scrobbling closes that gap: it reports each qualifying play to
**Last.fm** and/or **ListenBrainz** and updates a live "now playing" status.

Decisions locked with the user:
- **Both providers**, each independently connectable/toggleable (shared pipeline).
- **Plaintext-JSON credential storage** (dedicated file, `0600` on Unix; Windows
  uses the per-user AppData ACL — `dirs::data_dir()` already resolves it). Tokens
  are low-sensitivity, revocable, scoped (cannot change the account or read the
  password). Avoids the documented zbus/accesskit-`tokio` footgun an OS keyring's
  Secret Service backend risks on Linux.
- **Core scrobbling + now-playing + offline queue**, **plus** love↔favorite sync.

The design is **fully decoupled from the player state machine** — a background
task subscribes to the existing `view_model`/`position` watch channels (the same
seam OS media controls use) and to the favorite path. No changes to
`player/state.rs`, `handlers.rs`, or `actions.rs`.

### The scrobble rule (both services agree)
Submit a scrobble when a track is **> 30 s** AND **played ≥ half its duration, or
≥ 4 minutes, whichever comes first**. "Now playing" is sent at track start. The
scrobble `timestamp` is UNIX-seconds when the track *started* playing.

---

## API reference (verified from official docs)

### Last.fm (`https://ws.audioscrobbler.com/2.0/`, POST form-urlencoded)
- Requires a registered **API application** (key + shared secret) — the *app*
  identity, **not** a listening account: every user scrobbles to their own account via
  their own session key (`last.fm/api/account/create`). Keys are read at compile time via
  `option_env!("LASTFM_API_KEY")` / `option_env!("LASTFM_SHARED_SECRET")` (**not**
  hardcoded constants — nothing secret lands in the public repo). Official releases inject
  them as CI secrets; a keyless build keeps ListenBrainz fully working and renders the
  Last.fm **Connect** button inert ("not configured in this build").
- **Desktop auth**: `auth.getToken` (signed) → browser to
  `https://www.last.fm/api/auth/?api_key=KEY&token=TOKEN` → user approves →
  `auth.getSession` (signed, with `token`) → **session key, infinite lifetime**.
- **`api_sig`** = MD5 hex of params sorted by name, concatenated `name1value1…`,
  then the shared secret appended. **`format` excluded** (we send `format=json`);
  `method` included.
- **`track.updateNowPlaying`**: `artist`, `track`, `api_key`, `api_sig`, `sk` req.;
  `album`, `albumArtist`, `trackNumber`, `duration`, `mbid` opt.
- **`track.scrobble`**: array `artist[i]`, `track[i]`, `timestamp[i]` req.; `album[i]`,
  `albumArtist[i]`, `trackNumber[i]`, `mbid[i]`, `duration[i]` opt. **Max 50/POST.**
- **`track.love` / `track.unlove`**: `artist`, `track`, `api_key`, `api_sig`, `sk`.
- **Errors are in-body, not HTTP status**: a failure returns HTTP 200 with
  `{"error": <code>, "message": …}`. Classify: **9** (invalid session key) → disconnect +
  re-auth (stop retrying); **11/16** (service down) + **29** (rate limit) → keep queued,
  back off; other → log + bounded retry.

### ListenBrainz (`https://api.listenbrainz.org`, JSON)
- Auth header `Authorization: Token <user token>`; no app registration.
- **`GET /1/validate-token`** → `{ valid, user_name }`.
- **`POST /1/submit-listens`**, `Content-Type: application/json`:
  - `listen_type: "playing_now"` — `payload:[{track_metadata:{…}}]`, **no `listened_at`**.
  - `listen_type: "single"`/`"import"` — item has `listened_at` (UNIX-seconds, track
    *start*) + `track_metadata{ artist_name, track_name, release_name, additional_info{
    recording_mbid, release_mbid, tracknumber, duration_ms, media_player: "Melodia",
    submission_client: "Melodia", submission_client_version: <CARGO_PKG_VERSION> } }`.
- LB permits up to **1000** listens/request; we share the Last.fm **50** cap for
  simplicity. Respect `X-RateLimit-*` (prefer `X-RateLimit-Reset-In` seconds — resilient
  to client clock skew — over the epoch `X-RateLimit-Reset`); back off on `429`.
- **Love**: LB feedback needs a `recording_mbid`; love sync is **Last.fm-primary**,
  LB feedback best-effort only when `musicbrainz_track_id` exists.

---

## Architecture (one glance)

`Arc<ScrobbleService>` on `AppState` owns: the credential/enabled **shadow**
(`RwLock`), the durable **queue**, a cloned `reqwest::Client`, provider clients,
and a `Notify` (submitter wake).

- **Detector task** — subscribes `state.sinks.view_model.subscribe()` (track id +
  play/pause via `PlayerViewModelLight.status` — a lowercase `&'static str`
  `"playing"/"paused"/"stopped"/"loading"` — and `.current_track: Option<Arc<TrackSummary>>`,
  which carries `id`+`duration_ms` but *not* artist/album/mbids) **and**
  `state.position_tx.subscribe()` (`PositionTick{ position_ms, duration_ms }`, ~1 Hz, **no
  track id**). Decision logic is a **pure function** (mirrors `evaluate_playing_tick`); it
  correlates each position tick against the latest `current_track.id` and **resets the
  played-time accumulator on id change**. On a new-track start it fetches
  `get_scrobble_row(db, id)` **once**, fires `service.update_now_playing` (fire-and-forget,
  **only on id change** — every transition republishes `view_model`, so a per-tick fire
  would spam), and stashes that enriched row for the eventual `service.enqueue_scrobble`.
- **Submitter task** — drains queue, batches per provider (≤50), POSTs, clears the
  per-provider flag on success + persists, backs off on failure. **Failure handling is
  per-provider**: ListenBrainz via `429`/`X-RateLimit-Reset-In`; Last.fm via its **in-body
  error code** (9 → disconnect + re-auth; 11/16/29 → keep queued + back off).
- **Love sync** — `favorites::set_favorite` / `toggle_current_favorite` →
  `service.enqueue_love`.

Now-playing = ephemeral, never queued. Scrobbles/loves = durable + retried. Routine
submit failures stay **silent** (logged); only connect failures toast
(`OperationFailed`) + surface inline in the dialog.

**Why not `UpdatePlayCount`:** it fires only near-end (EOS/crossfade/gapless), so it
misses "played 60% then skipped." The detector implements the real rule from ticks.
Confirmed every transition (`handlers.rs:246`/`:270`/`:320`) republishes on
`view_model`, so no track change is missed.

**Reuse points:** shared client `state.http_client()`; settings `*Flags` +
`mutate_settings` (`services/settings/io.rs`) + `persist_blocking` + kick-after-persist
(`ui/appearance/mod.rs::persist_and_kick`); JSON state via `load_json_or_default_sync` /
`write_json_atomic_sync` (`services/mod.rs`) — the atomic-write half only; the credential
file's `#[cfg(unix)]` `0o600` chmod (`Permissions::from_mode(0o600)`) is **net-new** (no
existing secure-write helper). Queue-state shape from `services/search_history.rs`; task
shape from `tasks/play_count_flusher.rs`; browser open via `open::that` (already a dep);
`chrono::Utc::now().timestamp()`.

---

## Phases

Each phase compiles and is independently verifiable. Phases 0–2 are library-layer
(unit-testable without running the app, per the defer-long-rebuilds convention);
UI/manual verification starts in Phase 3. **Gate every phase on `cargo clippy
--all-targets -- -D warnings` + `cargo test` green.** Do not commit unless asked.

### Phase 0 — Foundations & scaffolding ✅ (landed)
- [x] `Cargo.toml`: add `md5` (simple crate — `format!("{:x}", md5::compute(s))`;
      pin latest exact version via `cargo search`). → `md5 = "0.8.1"`.
- [x] Last.fm app keys: `const LASTFM_API_KEY: Option<&str> =
      option_env!("LASTFM_API_KEY");` + `LASTFM_SHARED_SECRET` (in
      `services/scrobble/providers/lastfm.rs`). **Not** committed constants; releases
      inject them as CI env/secrets. `LASTFM_API_KEY.is_some()` gates the whole Last.fm
      surface (setter/UI/detector) so a keyless build ships ListenBrainz-only.
      → exposed as `providers::lastfm::is_configured()`.
- [x] `src/config.rs`: add `scrobble_credentials_path` + `scrobble_queue_path` to
      `Paths` + `resolve()`.
- [x] `src/services/settings/data.rs`: new `ScrobbleFlags { lastfm_enabled,
      listenbrainz_enabled, love_sync_enabled }` (default all `false`), whole-struct
      `#[serde(default)]` + `Default`, `#[serde(flatten)]` into `SettingsData` (and
      its `Default`). **No secrets here.** (Modeled on `TrayFlags` — derived `Default`.)
- [x] `src/library/settings/scrobble.rs` (new) + `mod.rs` re-exports — `set_scrobble_*`
      setters through `mutate_settings`.
- [x] `src/services/scrobble/model.rs`: `ScrobbleTrack` (artist, track, album,
      album_artist, duration_secs, track_number, recording_mbid, release_mbid) +
      **pure** `scrobble_threshold_ms(duration_ms) -> Option<u64>`: `duration_ms == 0`
      (unknown) → `Some(240_000)` (4-min fallback); `duration_ms <= 30_000` → `None`
      (too short); else → `Some(min(duration_ms/2, 240_000))`.
- [x] `src/services/scrobble/credentials.rs`: `ScrobbleCredentials` (lastfm
      `{session_key, username}`, listenbrainz `{token, username}`), `load`/`save` — reuse
      `write_json_atomic_sync`, then a **net-new** `#[cfg(unix)]`
      `set_permissions(Permissions::from_mode(0o600))` (no existing secure-write helper).
- [x] `src/services/scrobble/queue.rs`: `ScrobbleQueue { VecDeque<QueuedItem> }` (pure
      serde model + cap logic) where `QueuedItem` persists the full enriched
      `ScrobbleTrack` **and** the start `timestamp` (UNIX-seconds captured at track start,
      so an offline scrobble keeps its real time) + per-provider `remaining` flags; cap
      (`MAX_QUEUED = 5000`) + drop-oldest **with `log::warn!`**. `retain_pending()` drops
      fully-submitted items.
- [x] `src/services/scrobble/mod.rs`: `ScrobbleService` struct + `init` +
      `RwLock<ScrobbleRuntime>` shadow; add `Arc<ScrobbleService>` field to
      `AppState` (+ build in `AppState::init`). **No tasks/network yet.**
- [x] Tests: `model` (threshold edges incl. `duration_ms == 0` → 240 s and ≤30 s → None),
      `queue` (round-trip, flag clearing, cap, timestamp preserved), plus `mod` (service
      credential + queue persistence round-trip). 14 tests, `cargo test` + clippy green.

**Deviations from the spec above (for Phases 2–3 wiring):**
- `ScrobbleService::init(paths: &Paths, flags: &ScrobbleFlags)` — takes the enabled
  flags already read at `state/mod.rs:139`, avoiding a second `settings.json` read
  (spec said `init(&Paths)`).
- No separate `ScrobbleQueueState` handle — `ScrobbleService` **is** the managed
  handle (`parking_lot::Mutex<ScrobbleQueue>` + `queue_path`); `ScrobbleQueue` stays the
  pure serde model. The submitter's `Notify` + shared `reqwest::Client` join the struct
  in **Phase 2** (kept out now to avoid unused-field `dead_code`).
- Phase-0 service API for later phases: `status() -> ScrobbleStatus` (per-provider
  `ProviderStatus { connected, username, enabled }` + `love_sync_enabled`), `set_flags`,
  `set_lastfm_credentials`/`set_listenbrainz_credentials` (persist + `0o600`),
  `queued_len`, `push_scrobble` (durable-queue primitive Phase 2's `enqueue_scrobble`
  builds on).

### Phase 1 — Provider clients (pure + network fns, unwired)
- [ ] `src/services/scrobble/providers/lastfm.rs`: `sign()` (sorted params + MD5 hex,
      no `hex` dep), `get_token`, `get_session`, `update_now_playing`,
      `scrobble_batch`, `love`. All take `&reqwest::Client`; POST `format=json`. **Parse
      the in-body `{"error": code}`** into a classified result (9 = invalid session →
      permanent/disconnect; 11/16/29 = transient → retry) — never trust HTTP 200 alone.
- [ ] `src/services/scrobble/providers/listenbrainz.rs`: `validate_token`,
      `submit_playing_now`, `submit_listens`. Header `Authorization: Token …`;
      `submission_client = "Melodia"` + `submission_client_version =
      env!("CARGO_PKG_VERSION")`; read `X-RateLimit-Reset-In` for backoff.
- [ ] Tests: `sign()` known vector; Last.fm param maps (array indices, `format`
      excluded from sig) + error-code classification (9 vs 11/16/29); LB JSON shapes
      (`playing_now` has no `listened_at`).

### Phase 2 — Detection + submission tasks
- [ ] `src/entities/track.rs` + `src/database/queries/track.rs`: slim `ScrobbleRow`
      projection (artist, track, album, album_artist, duration_ms, track_number,
      `musicbrainz_track_id`, `musicbrainz_release_id`) + `SCROBBLE_ROW_COLUMNS`/
      `scrobble_row_columns()` (copy `TagEditRow`'s struct+columns block — all native
      columns, no join) + **single-id** `get_scrobble_row(db, id) -> Option<ScrobbleRow>`
      modeled on `get_track_summary_by_id`/`get_track_meta` (`track.rs:130-158`), **not**
      the multi-id `get_tag_edit_rows_by_ids`. (`recording_mbid ← musicbrainz_track_id`,
      `release_mbid ← musicbrainz_release_id`.)
- [ ] `src/services/scrobble/detector.rs`: **pure** `DetectorState` +
      `on_view_model` / `on_position` → `Effect{ NowPlaying | Scrobble | Finalize }`.
      Owns `started_at` capture, played-time accumulator with seek guard (`delta ∈
      [1, SEEK_GUARD_MS]`), **accumulator reset + one `NowPlaying` on `current_track.id`
      change**, restart detection (same id, position drops ~0), stop/shutdown finalize.
- [ ] `ScrobbleService` methods: `update_now_playing` (spawn fire-and-forget POST to
      each connected provider), `enqueue_scrobble` (enrich via `get_scrobble_row`,
      push queue, `Notify`), submitter drain/batch/retry/backoff (honor
      `429`/`X-RateLimit`), final flush on shutdown.
- [ ] `src/tasks/scrobble.rs`: `spawn(spawner, state)` → two `spawn_cancellable`
      loops (detector + submitter); declare in `tasks/mod.rs`; invoke from
      `src/boot/tasks.rs`. **No `ui::*` imports.**
- [ ] Tests: `detector` pure state machine (normal→scrobble, skip-before/after
      threshold, seek guard, restart, pause = no accumulation, **one `NowPlaying` per
      track-start despite repeated `view_model` republishes**).

### Phase 3 — Settings UI + auth flows
- [ ] `ui/settings.slint` (`Settings` global): `scrobble-{lastfm,listenbrainz}-connected
      /-username/-enabled`, `scrobble-love-sync`, callbacks `-{connect,disconnect}`,
      `-enabled-changed(bool)`, `scrobble-love-sync-changed(bool)`, localized `@tr`
      status strings.
- [ ] `ui/views/settings/scrobbling-section.slint` (per-service status + Connect/
      Disconnect pill + enable toggle gated on connection + "Sync loved tracks"
      toggle); mount in `settings-view.slint` + add to `has-matches` aggregate.
- [ ] Login dialogs via `Dialog` + body + a new `ScrobbleUi` global
      (`ui/globals.slint`): `kind == "scrobble-lastfm-login"` (two-step: **[1]** open
      browser `getToken`, **[2]** finish `getSession`), `kind ==
      "scrobble-listenbrainz-login"` (`LabeledInput` token + verify). Extend the
      single `on_closed` teardown if a dialog pins any image (none expected).
- [ ] `src/ui/scrobbling_settings.rs::install_scrobbling(app, state)` — seed/persist
      skeleton from `install_replaygain` (sync), **plus** the async-network shape from
      `install_sleep_timer` / `about.rs` (`runtime.spawn` + `upgrade_in_event_loop`), since
      `install_replaygain` itself does no network. Connect/disconnect/toggle callbacks:
      network via `state.runtime.spawn`, browser via `open::that` in `spawn_blocking`, props
      via `upgrade_in_event_loop`, persist via `persist_blocking` + creds save + service
      shadow update (kick-after-persist ordering). **Gate the whole Last.fm surface on
      `LASTFM_API_KEY.is_some()`** (Connect disabled + note when `None`). Wire in
      `boot/ui_setup.rs` beside `install_playback_settings`.
- [ ] i18n: add all new `@tr` msgids to every shipped `Melodia.po`.
- [ ] **Verify E2E**: ListenBrainz real token → Connected; play past threshold →
      listen appears + "playing now" on start. Last.fm (needs `LASTFM_API_KEY` set at
      build) → browser authorize → scrobble appears.

### Phase 4 — Love ↔ favorite sync
- [ ] `src/library/favorites.rs`: at the end of `set_favorite` (batch) and
      `toggle_current_favorite` (single), when love-sync enabled + Last.fm connected,
      look up artist/track (reuse `get_scrobble_row`) and `state.scrobble.enqueue_love
      (track, loved)`. Single choke point — all favorite UI already routes here.
- [ ] LB feedback best-effort, MBID-gated (skip untagged) — optional within phase.
- [ ] **Verify**: toggle a heart → track appears in Last.fm Loved Tracks.

### Phase 5 — Docs, offline test & polish
- [ ] `CLAUDE.md`: new "Scrobbling" conventions block (decoupled detector, provider/
      queue split, credential file, love-sync choke point).
- [ ] `README.md`: feature + Last.fm API-application setup note (`option_env!` keys;
      keyless builds are ListenBrainz-only).
- [ ] `release.yml`: pass `LASTFM_API_KEY` / `LASTFM_SHARED_SECRET` repo secrets as build
      env so official binaries ship with Last.fm enabled.
- [ ] **Offline test**: kill network mid-play → queued + `scrobble_queue.json`
      persists → reconnect → submitter drains.
- [ ] `/usr/bin/time -v` peak-RSS sanity (idle overhead negligible; under ~200 MB).

---

## Notes
- **No DB migration** — MBID/album_artist columns already exist on `tracks`.
- **No `AppError` change** — reuse `AppError::Network` (`network`/`network_msg`).
- **No new `ToastKind`** — connect errors use `OperationFailed`; routine submit
  failures stay silent (logged), per the don't-toast-spam convention.

## Follow-ups (out of scope for v1)
- OS-keyring backend (careful non-`tokio` zbus feature pinning).
- Backfilling historic local plays as ListenBrainz `import`.
- ListenBrainz feedback for non-MBID tracks (MBID lookup).
