---
paths:
  - src/services/scrobble/**/*.rs
  - src/tasks/scrobble.rs
  - src/tasks/mbid_backfill.rs
  - src/library/favorites.rs
  - src/library/mbid.rs
  - src/ui/settings/scrobbling_settings.rs
  - melodia-ui/ui/views/settings/scrobbling-section.slint
---

# Scrobbling — Last.fm + ListenBrainz, and love↔favorite sync

The whole subsystem hangs off two read-only seams (`sinks.view_model`, `position_tx`)
and writes nothing back into the player. Keep it that way: a change here should never
need an edit in `player/state.rs`, `handlers.rs` or `actions.rs`.

## The seam

- **Scrobbling (Last.fm + `ListenBrainz`) is fully decoupled from the player state machine** (`src/services/scrobble/`). `Arc<ScrobbleService>` on `AppState` owns the credential/enabled **shadow** (`RwLock<ScrobbleRuntime>`, read synchronously, never across `.await`), the durable offline **queue** (`parking_lot::Mutex<ScrobbleQueue>`), a submitter-wake `Notify`, a `watch<ScrobbleStatus>`, and the **same** `Arc<OnceLock<reqwest::Client>>` as `AppState`. Two `spawn_cancellable` loops in `src/tasks/scrobble.rs` (**no `ui::*`**): a detector (subs `sinks.view_model` + `position_tx`) and a submitter. No changes to `player/state.rs`/`handlers.rs`/`actions.rs`. `entities::track::ScrobbleRow` is the slim projection (`get_scrobble_row`).
- **The scrobble decision is pure and fires at play-END.** `detector::DetectorState` (mirrors `evaluate_playing_tick`) accumulates played-time against the latest `current_track.id`, resets on id change, emits `Effect { NowPlaying | Scrobble | Finalize }`. Scrobble queued when a play *ends* (successor/restart/stop/shutdown), gated on `model::scrobble_threshold_ms(duration_ms)` (**> 30 s** AND **≥ half its length or ≥ 4 min**; `0` → 4-min fallback; `≤ 30 s` → never) — the durable queue, not mid-play firing, covers outages. `NowPlaying` fires **once per track-start** (a per-tick fire would spam). Enrichment via `ScrobbleTrack::from_row` (non-empty artist+title required).

## The queue

- **Now-playing is ephemeral** (never queued/retried); **scrobbles + loves are durable + retried.** `ScrobbleQueue` (`queue.rs`) holds `items` + `#[serde(default)] loves` (the `default` keeps pre-love-sync files loadable), each with per-provider `*_remaining` flags; cap `MAX_QUEUED` drop-oldest. Mutating methods apply under the `parking_lot` lock synchronously, then offload the blocking JSON write via `spawn_blocking` (the wake fires only after commit). **Failure handling is per-provider**: Last.fm via its in-body error code (9 → auto-disconnect; 11/16/29 → keep queued + back off); `ListenBrainz` via `429`/`X-RateLimit-Reset-In` (401 → auto-disconnect). Routine failures stay **silent** (logged); only connect failures toast.

## Providers and credentials

- **Provider clients** (`providers/{lastfm,listenbrainz}.rs`) return **classified error enums** (`LastfmError`/`ListenBrainzError`, not `AppResult` — the retry policy needs a classification `AppError` can't carry); their pure param/payload builders are unit-tested, the `async` POST paths network-unexercised. Last.fm signs via `sign()` (sorted `BTreeMap` + MD5; `format`/`api_sig` added post-sign in `finalize_and_sign`).
- **Credentials live in their own `scrobble_credentials.json`** (`credentials.rs`; `#[cfg(unix)] 0o600` after the atomic write), **never** `settings.json`. Only the **enabled flags** (`ScrobbleFlags`, all default `false`) sit in `settings.json` (`#[serde(flatten)]`, `#[expect(struct_excessive_bools)]` since nesting would break the on-disk shape), set via `library::settings::scrobble` → `ScrobbleService::set_flags`. The two `*_love_enabled` flags are **per-provider and independent**. The mutators publish the `watch<ScrobbleStatus>` so the UI reflects both its own connect/disconnect and the submitter's auto-disconnect.
- **Last.fm app keys are compile-time `option_env!`** (`LASTFM_API_KEY`/`LASTFM_SHARED_SECRET`), never committed; `release.yml` injects them as job-level CI secrets. `non_empty_env` folds a present-but-empty var back to `None`, and **`lastfm::is_configured()` gates the whole Last.fm surface** (setter/UI/detector/submitter) — a keyless build ships **`ListenBrainz`-only** with an inert Connect button. `ListenBrainz` needs no app registration (per-user token, `Authorization: Token …`).

## Loves

- **Love ↔ favorite sync has one choke point:** `library::favorites::sync_love`, called at the end of `set_favorite`/`toggle_current_favorite`. It no-ops unless `love_sync_active()`, then fetches the id set in **one** `get_scrobble_rows_by_ids` and `enqueue_loves` under one queue lock + save + wake — O(1) round-trips, not O(N). **Best-effort** — errors logged, never propagated, so love sync can't fail the favorite write. Each provider is armed by **its own** love toggle (independent of scrobble-enable): Last.fm on `lastfm_love_enabled` + `is_configured()` + connected; **`ListenBrainz` MBID-gated** on `listenbrainz_love_enabled` + connected + `recording_mbid.is_some()`. `push_love` **coalesces** a repeat toggle (newest wins); the writeback guards each clear on the queued `loved` still matching what was POSTed, so a favorite reversed mid-POST stays pending.
- **Retroactive favorite→love backfill** syncs an already-favorited library without re-toggling each heart. `library::favorites::backfill_loves(state, LoveTarget)` (spawned from `ui::settings::scrobbling_settings`) runs when a provider's love toggle is turned on (or the provider connects while it's already on), self-gates on `love_target_armed(target)`, fetches every favorite via `get_favorite_scrobble_rows`, then `ScrobbleService::backfill_loves(rows, target)` batches the whole set into **one** queue lock + save + wake — arming **only** `target`'s `*_remaining` (so connecting one service doesn't re-love everything on the other; LB skips MBID-less rows). Idempotent. Reports via a `ToastKind::LoveSync` toast (silent when there are no favorites; for a `ListenBrainz`-only untagged library it points at MBID auto-tagging below).
- **MBID auto-tagging makes `ListenBrainz` loves work on untagged libraries** (`ScrobbleFlags.mbid_auto_tag`, opt-in; `tasks/mbid_backfill.rs`, no `ui::*`; write path `library::mbid::write_resolved_mbids`). LB feedback keys on a `recording_mbid`; a backfill resolves each track's ID via LB's `metadata/lookup` (≤50/POST) and **writes it into the file *and* the DB** (mark `SelfWrites` → `tag_writer::apply_to_file` → re-extract → `update_track_metadata`), **without** bumping `library_changed_tx` (so the task can't wake itself). **ID3v2 stores this id in a `UFID` frame, not a text frame** — insert it *unchecked* (`Tag::insert_text` refuses the key and silently drops it on MP3). Keeps a **persisted** `attempted` set (`scrobble_mbid_attempted.json`) so unmatched tracks aren't re-looked-up on later bumps or restarts — a manual "Look up missing IDs" clears it (memory + file). **Caveats:** an already-attempted track retagged later won't auto-re-resolve on reboot (id-keyed set) — use the button; writing tags stales `#MELODIA-HASH` lines. The resolve is **text-only** (artist+title), so a loosely-tagged library matches poorly — acoustic-fingerprint tagging is intentionally **not** built in; the README points those users to **MusicBrainz Picard**.
