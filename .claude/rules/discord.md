---
paths:
  - src/services/discord/**/*.rs
  - src/tasks/discord_presence.rs
  - src/media/deezer.rs
  - src/media/itunes.rs
  - src/ui/settings/discord_settings.rs
  - melodia-ui/ui/views/settings/discord-section.slint
---

# Discord Rich Presence

Same shape as scrobbling — a read-only tap on `sinks.view_model`, a pure model, and a transport
that owns all the I/O. See `.claude/rules/scrobbling.md` for the sibling.

- **Rich Presence is decoupled from the player state machine**, like scrobbling
  (`src/services/discord/`, `src/tasks/discord_presence.rs`, **no `ui::*`**): it reads the
  `state.sinks.view_model` seam through a **pure** model and ships it over a hand-rolled
  framed-JSON IPC. The dedupe (`model.rs`, pure) keys on the **invariant** play anchor
  `now - position` (`±2 s`), so a volume republish is a no-op but a seek re-anchors; **timestamps
  are UNIX seconds, not ms** — don't "fix" them. Self-throttled to **one write per ~4 s**
  (`MIN_UPDATE_INTERVAL`): the Rich Presence SDK docs cite 15 s, conservative for local IPC — the
  client tolerates faster and only drops the presence under real hammering, and the oft-cited
  **5-per-20 s** figure is the *gateway* presence limit, a different transport. The task waits the
  window out *then* re-reads the watch, so the **trailing-edge flush** always lands one write on
  current truth.

- **The IPC is hand-rolled** (`services/discord/ipc.rs`, ~200 lines of blocking `std`, no new deps)
  because `discord-rich-presence` pins `uuid ^0.8` against the tree's 1.x; if upstream bumps,
  `ipc.rs` deletes with `model.rs` unchanged. Frame is `[u32 LE opcode][u32 LE len][JSON]`, and the
  socket-path table is copied from it (MIT, credited). **Windows `\\?\pipe\` uses
  `access_mode(0x3)`** (`FILE_READ_DATA | FILE_WRITE_DATA`), **not** the `GENERIC_*` pair — don't
  "correct" it. The worker is a **detached `std::thread` + `std::sync::mpsc`** (blocking socket;
  `recv_timeout` gives the reconnect-backoff loop for free), not `TaskTracker`-registered, and
  connect failures stay **silent**. **The app id is public** (`const DISCORD_APP_ID` +
  `MELODIA_DISCORD_APP_ID` override) — no CI secret, unlike the Last.fm keys.

- **Album artwork is an external `https://` URL, never a local file** — Discord's CDN fetches
  `large_image` server-side. A Deezer album search (`media::deezer::search_album_cover`) falls back
  to the keyless **iTunes Search API** (`media::itunes::search_album_cover`, `artworkUrl100`
  upsized to 512 px) when Deezer has no match, their misses differing. Both sit behind a 64-entry
  `Mutex<LruCache>` (`services/discord/artwork.rs`, keyed lowercased `(artist,album)`), each hop
  under a **1.5 s budget**, so the fallback path tops out ~3 s and only on a Deezer miss. Nothing
  is downloaded here, so the iTunes `mzstatic.com` host never touches
  `download_and_cache_artist_image`'s domain allowlist. The pure model leaves `large_image` `None`
  (I/O) and **the task injects it**, keyed by a task-side `last_art: (track_id, url)` so
  pause/resume/seek reuse it lock-free. Enable / artwork / hide-while-paused are three bools;
  `discord_rpc_artwork` defaults *on*, inert until the parent toggle is.

- **Only a *definitive* result is cached** — a `Some` from either provider, or a `None` when
  **both** answered empty; a timeout or transport error on either hop caches nothing. That is
  keepable only because `search_album_cover` returns `Ok(None)` for nothing but an empty result set
  — a property of **both** providers, not just the one whose refusals are interesting. Deezer
  states a tripped quota as **HTTP 200** carrying `{"error":{…}}` where `data` belongs, so a decode
  straight into the success type reports the API's own refusal as a malformed body:
  `media::deezer::classify` peels the error object off first (the two shapes are disjoint, neither
  `data` nor `error` having a serde default), and both that and a non-success status come back as
  `Err` for `run_lookup` to read as `Unavailable`. Fold either into `Ok(None)` and a
  five-second rate-limit window is cached as "this album has no cover" for the rest of the session.
  **iTunes did exactly that** — a bare `if !status.is_success() { return Ok(None) }` — so the
  failure reached the cache through the *other* provider, and only on the Deezer-missed path, the
  one that ends in a `put`; its status check returns `Err` now. Pinned at the seam in
  `services::discord::artwork::tests` rather than at either provider: `run_lookup` is where
  `Ok(None)` becomes `Miss`, so one test covers the rule for a third provider too.

- **The album path stops at naming the refusal; the *artist*-image pass reads the code**, since it
  runs a batch — `deezer::halts_a_batch` halts on the two codes about our own pace (`4` quota,
  `700` busy) and not on the ones answering the query that asked (`600` invalid query, `500`/`501`
  parameter, `800` no data). `deezer::quotable` exists one edge over for the same reason: the
  advanced-search string this path builds is quote-delimited with no escape, so an embedded `"` in
  an album title is a self-inflicted `600`.
