---
paths:
  - src/services/discord/**/*.rs
  - src/tasks/discord_presence.rs
  - src/media/deezer.rs
  - src/media/itunes.rs
  - src/ui/discord_settings.rs
  - ui/views/settings/discord-section.slint
---

# Discord Rich Presence

Same shape as scrobbling — a read-only tap on `sinks.view_model`, a pure model, and a
transport that owns all the I/O. See `.claude/rules/scrobbling.md` for the sibling.

- **Discord Rich Presence is decoupled from the player state machine** like scrobbling (`src/services/discord/`, `src/tasks/discord_presence.rs`, **no `ui::*`**): reads the `state.sinks.view_model` seam through a **pure** model, ships it over a hand-rolled framed-JSON IPC. The dedupe (`model.rs`, pure) keys on the **invariant** play anchor `now - position` (`±2 s`) so a volume republish is a no-op but a seek re-anchors; **timestamps are UNIX seconds not ms** (do not "fix" to ms). Self-throttled to **one write per ~4 s** (`MIN_UPDATE_INTERVAL`): the Rich Presence SDK docs cite 15 s, but that's conservative for local IPC — the client tolerates faster and only drops the presence under real hammering, and the oft-cited **5-per-20 s** figure is the *gateway* presence limit (a different transport), not this path. The task waits the window out *then* re-reads the watch, so the **trailing-edge flush** always lands one write on current truth.
- **The IPC is hand-rolled** (`services/discord/ipc.rs`, ~200 lines blocking `std`, no new deps) because `discord-rich-presence` pins `uuid ^0.8` vs the tree's 1.x. Frame = `[u32 LE opcode][u32 LE len][JSON]`; the socket-path table is copied from it (MIT, credited). **Windows** `\\?\pipe\` uses `access_mode(0x3)` (`FILE_READ_DATA | FILE_WRITE_DATA`), **not** the `GENERIC_*` pair — don't "correct" it. Worker is a **detached `std::thread` + `std::sync::mpsc`** (blocking socket; `recv_timeout` gives the reconnect-backoff loop for free), not `TaskTracker`-registered; connect failures stay **silent**. If upstream bumps to `uuid 1.x`, `ipc.rs` can be deleted with `model.rs` unchanged. **The app id is public** (`const DISCORD_APP_ID` + `MELODIA_DISCORD_APP_ID` override) — **no CI secret, unlike the Last.fm keys**.
- **Album artwork is an external `https://` URL, never a local file** (Discord's CDN fetches `large_image` server-side). Resolved via a Deezer album search (`media::deezer::search_album_cover`), falling back to the keyless **iTunes Search API** (`media::itunes::search_album_cover`, `artworkUrl100` upsized to 512 px) when Deezer has no match — their misses differ. Both behind a 64-entry `Mutex<LruCache>` (`services/discord/artwork.rs`, keyed lowercased `(artist,album)`), each hop under a **1.5 s budget** (so the fallback path tops out ~3 s, only on a Deezer miss). Only a **definitive** result is cached — a `Some` from either provider, or a `None` when **both** answered empty; a timeout/transport error on either hop caches nothing. Neither provider is downloaded here (Discord fetches the URL), so the iTunes `mzstatic.com` host never touches `download_and_cache_artist_image`'s domain allowlist. The pure model leaves `large_image` `None` (I/O); the **task injects** it, keyed by a task-side `last_art: (track_id, url)` so pause/resume/seek reuse it lock-free. Enable/artwork/hide-while-paused are 3 bools; `discord_rpc_artwork` defaults *on* (inert until the parent toggle is on).
