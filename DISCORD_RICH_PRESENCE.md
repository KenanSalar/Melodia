# Discord Rich Presence

> **Working doc.** Phase markers are kept current as work lands. Delete this file once the feature
> ships — its durable content moves to `CLAUDE.md` (conventions) + `README.md` (feature + privacy)
> in Phase 5.

| Phase | Status |
|---|---|
| 0 — Discord application + working doc | ⏳ working doc done; **app registration deferred** (see note) |
| 1 — IPC transport (`services/discord/ipc.rs`) | ✅ done |
| 2 — Pure presence model + detector task | ✅ done |
| 3 — Settings: flags, persistence, section card | ☐ (`DiscordFlags` struct + `AppState` wiring landed early with 1–2) |
| 4 — Album artwork + link button | ☐ |
| 5 — Docs + gates | ☐ |

> **Phase 0 deferred.** The application id is scaffolded as a placeholder const
> (`services/discord/mod.rs::DISCORD_APP_ID`, with a `MELODIA_DISCORD_APP_ID`
> `option_env!` override). Registration + art-asset upload aren't needed to
> compile/lint/test and can't be exercised until Phase 3 adds the enable toggle,
> so do them **before the first live verification** (around Phase 3/4). Entry
> point: sign in at <https://discord.com/developers>, then **Applications → New
> Application**, name it exactly `Melodia`.

## Context

Melodia already pushes "what's playing" to three external surfaces — OS media controls
(souvlaki/MPRIS), the system tray tooltip, and the Last.fm / ListenBrainz scrobblers. Discord Rich
Presence is the same shape of feature: read the player's published view-model, project it into a
third-party's payload, ship it over a local transport. Nothing about the player state machine needs
to change.

The outcome: with Discord running and the feature enabled, a user's Discord profile shows
**Listening to \<song\>** with artist, album cover, a live progress bar, and a link button —
updated on track change, pause, resume, seek and stop, and cleared when playback stops or Melodia
quits.

Two constraints drive the design:

1. **The Discord IPC transport is blocking** (a unix socket / named pipe). It cannot touch a tokio
   worker — this app runs two.
2. **Discord cannot read local files** for the artwork. The big image is either an asset key
   uploaded to the Discord application or an external `https://` URL that Discord's CDN fetches
   server-side. Album art therefore requires an outbound cover lookup — which is why it gets its
   own toggle and its own line in the privacy docs.

Scope decisions: album covers ship in scope (not deferred), paused playback keeps the card up with
a paused marker plus a "Hide while paused" escape hatch, the settings live in their own **Discord**
section, all three polish items are in (song title in the member-list status line, the progress
bar, a link button), and the IPC is **hand-rolled rather than taken as a dependency**.

## What we use — and why no new dependency

Zero new crates. `serde_json` (already a dep) serializes the payload; `std::os::unix::net::UnixStream`
(Linux, macOS) and `std::fs::File` on `\\?\pipe\…` (Windows) are the transport — both plain `std`,
no `unsafe`.

The obvious candidate, **`discord-rich-presence 1.1.0`**, hard-requires `uuid ^0.8` — purely to
generate a per-command nonce. That is semver-incompatible with the `uuid 1.23.3` already in the
lock, so Cargo would compile both, and `[patch.crates-io]` cannot bridge a `^0.8` requirement with
1.x. The alternative, `discord-presence 3.2.0` (latest, 2026-01-20), *does* unify on `uuid ^1.19`
but pays for it with `byteorder`, `bytes`, `crossbeam-channel`, `num-derive`, `num-traits`, `paste`
and `quork`, plus a client that owns its own threading model.

Neither trade is necessary, because the nonce only has to be *unique* — a counter is fine — and the
protocol itself is small.

Two crates advertise the dependency-free framing this doc argues for — `rpresence` and
`presenceforge` — and both are pre-1.0 (`0.0.1` and `0.2.1`). For a publicly released app with an
auto-updater, a leaf crate that young is a worse bet than ~200 lines we own outright.

### Why hand-roll it

1. **The dependency bought us almost nothing essential.** Strip out the nonce (a counter) and the
   socket-path table (a 7-entry list), and what remains is a length-prefixed frame writer over a
   socket. Taking a crate to get that is paying a dependency for `write_all`.
2. **No duplicate `uuid` major.** `discord-rich-presence` pins `uuid ^0.8` against the tree's
   1.23.3. One extra small leaf crate is survivable — this lockfile already carries `windows-sys`
   ×5, `hashbrown` ×4, `getrandom` ×3 — but paying it for a nonce is the wrong reason to add a
   duplicate.
3. **It closes a real failure mode.** The crate's handshake does a blocking read with **no
   timeout**, so a wedged Discord parks the worker thread forever and silently swallows a later
   "disable". `UnixStream::set_read_timeout` fixes that on Linux and macOS, and the crate's API
   doesn't expose the socket to set it.
4. **No conversion layer.** Our `Presence` is owned (it crosses a channel); the crate's
   `Activity<'a>` borrows, so every push would rebuild a borrowing builder from owned strings.
   Serializing our own struct is both less code and less garbage.
5. **Smaller third-party surface in a shipped app.** Melodia is publicly released with an
   auto-updater; every crate in the tree is code we ship to strangers. A transport we can read in
   one sitting is auditable in a way a transitive tree isn't.
6. **It's the established stance in this repo.** The M3U8 playlist writer/parser is hand-rolled
   ("no crate" is called out in `CLAUDE.md`), so is the EQ biquad cascade and its soft-knee
   limiter, `regex` was deliberately trimmed out of `env_logger`, and winit is vendored rather than
   worked around. A ~200-line framed-JSON client is squarely inside that line.

What we give up: the *maintained* socket-path table. That is the one part with genuine upstream
value, so we copy it with credit (MIT) and accept that a new Discord packaging variant (some future
Flatpak id) needs a one-line addition here instead of a version bump.

**Reversal condition, worth writing down:** if upstream bumps `discord-rich-presence` to `uuid 1.x`,
the calculus flips — we could delete `ipc.rs` and keep `model.rs` unchanged, since the split
already isolates the transport behind `Command` / `Presence`. Design the module boundary so that
stays a one-file swap.

### The wire protocol

```
frame = [u32 LE opcode][u32 LE byte-length][JSON payload]
opcodes: 0 HANDSHAKE · 1 FRAME · 2 CLOSE · 3 PING · 4 PONG
handshake payload  {"v":1,"client_id":"<app id>"}
set activity       {"cmd":"SET_ACTIVITY","args":{"pid":<pid>,"activity":{…}},"nonce":"<counter>"}
clear activity     …same, with "activity": null
```

## Architecture

The seam is the one the scrobbler and Material You already use — `state.sinks.view_model`, a
`watch::Sender<Option<PlayerViewModelLight>>`. Crucially it publishes on **state changes only** —
`with_state_emit` is its single writer. The playback monitor loops at 500 ms and writes
`state.position_ms` on every tick (`evaluate_playing_tick`) but re-publishes no view-model; its
position ticks go out on the separate `position_tx`, and only on a whole-second change (1 Hz). So
subscribing to `view_model` alone gives exactly the events Discord cares about — track change,
pause, resume, seek, stop — and nothing per-second.

```
sinks.view_model ──► tasks/discord_presence.rs      (async, tokio, no ui::* imports)
   (watch)              │  pure PresenceState: dedupe / re-anchor / Clear
                        │  15 s throttle + trailing-edge flush (watch holds latest)
                        │  artwork lookup (async, cached, 2 s budget)
                        ▼
              services/discord/  DiscordPresenceService
                        │  flags shadow (RwLock) · watch<DiscordStatus> · std::mpsc::Sender
                        ▼
              ipc.rs worker  ── dedicated std thread ──► framed socket ──► Discord
                    (blocking connect/write, reconnect backoff, re-apply on reconnect)
```

Mirrors `services/scrobble/` + `tasks/scrobble.rs` deliberately: pure decision machine in
`services/`, impure driver in `tasks/`, blocking third-party transport isolated on its own thread
the way `services/tray/` isolates `ksni::blocking`.

---

## Phase 0 — Discord application + working doc (no Rust)

1. Register an application at <https://discord.com/developers/applications>. **Name it exactly
   `Melodia`** — Discord renders the application name as the card header ("Listening to Melodia")
   and there is no way to override it from the client.
2. Rich Presence → Art Assets: upload two assets, keys `melodia` (the logo — reuse
   `ui/assets/icons/logo-with-background.svg` rasterized to ≥1024×1024 PNG, Discord's recommended
   minimum) and `paused`. Asset propagation takes a few minutes after upload.
3. Copy the Application ID. **It is not a secret** — it ships inside every presence payload and is
   visible to everyone, so unlike the Last.fm keys it needs no CI secret and no
   configured-or-inert gate. Hardcode it as a `const DISCORD_APP_ID: &str` in
   `services/discord/mod.rs`, with an `option_env!("MELODIA_DISCORD_APP_ID")` override for anyone
   building against their own app. The override still has to route through the `non_empty_env`
   const-fn shape from `services/scrobble/providers/lastfm.rs` — `option_env!` reports a
   present-but-empty var as `Some("")`, and a build environment that substitutes one would otherwise
   ship an empty `client_id` that no Discord client will accept.
4. ✅ This working doc.

## Phase 1 — The IPC transport (`src/services/discord/ipc.rs`)

> **✅ Landed (round 1), with these deltas from the sketch below:** framing
> (`write_frame`/`read_frame`) is generic over `impl Write`/`impl Read` and
> returns `io::Result`, not `AppResult` — any error just drops the connection, so
> the transport needs no `AppError` classification. `Command` gained an `Enable`
> variant (alongside `Apply`/`Clear`/`Disable`) so re-enabling while the worker is
> parked wakes it; there is no `Shutdown` variant (the sender lives for the
> program, so `recv()` only errors at exit). Connection status is published to the
> UI through a small shared `Arc<StatusCell>` held by both the service and the
> worker (enable/connect atomics + the `watch` sender) — the split that lets the
> worker report `connected` without an `Arc<Service>` cycle.

Roughly 200 lines of blocking `std`, no new dependencies. Unit-testable in two halves: the framing
and the payload JSON are pure; only `connect` touches the OS.

**Socket discovery** — lifted from `discord-rich-presence` (MIT, credited in the module doc). For
each of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP` (falling back to `/tmp`), for each subdirectory,
for `i in 0..10`, try `UnixStream::connect(dir/sub/discord-ipc-{i})` and take the first that opens:

```
""  ·  "app/com.discordapp.Discord/"  ·  "app/dev.vencord.Vesktop/"
".flatpak/com.discordapp.Discord/xdg-run/"  ·  ".flatpak/dev.vencord.Vesktop/xdg-run/"
"snap.discord/"  ·  "snap.discord-canary/"
```

Flatpak Discord is extremely common on the desktops Melodia targets, so this table is the whole
reason the crate was tempting — owning it is a one-time copy of a list that changes rarely.

The table solves a sandboxed *Discord*. The reverse doesn't work: a sandboxed *Melodia* can't reach
the socket at all without a filesystem hole in its manifest. Not a concern for the formats we ship
(AppImage, RPM, DEB, MSI), but it would need a `--filesystem=xdg-run/…` override the day a Flatpak
is on the table.

**Platform coverage — all three targets, `std` only, no `unsafe`, no extra deps:**

| | transport | connect |
|---|---|---|
| Linux | unix socket | `UnixStream::connect` over the table above |
| macOS | unix socket | same, resolved through `TMPDIR` |
| Windows | named pipe | `OpenOptions::new().access_mode(0x3).open(r"\\?\pipe\discord-ipc-{i}")`, `i in 0..10` |

The Windows form is the one `discord-rich-presence`'s `ipc_windows.rs` uses and was verified against
it: `std::os::windows::fs::OpenOptionsExt::access_mode(0x3)` on the `\\?\pipe\` device path, then
plain `write_all` / `read_exact` on the resulting `std::fs::File`. No winapi, no `OVERLAPPED`, no
`unsafe` — so it satisfies the crate's `unsafe_code = "deny"` with no scoped allow. Copy the crate's
literal `0x3` verbatim and do **not** "fix" it to the `GENERIC_*` constants: `0x3` is
`FILE_READ_DATA | FILE_WRITE_DATA` (the file-specific read/write pair a named pipe accepts), **not**
`GENERIC_READ | GENERIC_WRITE`, which is `0xC0000000`. The single platform difference is the read
timeout below.

**Framing** — `write_all` the LE opcode, the LE length, then the body; reads take an 8-byte header
then exactly `len` bytes. A length above a sane cap (say 64 KiB) is a protocol desync → drop the
connection rather than allocate on a bogus header.

**Handshake** — send opcode 0 with `{"v":1,"client_id":…}`, then read one frame (Discord's `READY`
dispatch). Connected once that returns.

**Draining** — Discord replies with one frame per command, so we read one reply per write, keeping
the socket buffer balanced without any peeking. The read is a small loop rather than a single
`read_frame`: if a read yields opcode 3 (`PING`), echo the same payload back as opcode 4 (`PONG`) and
**keep reading** until the command's opcode-1 `FRAME` reply arrives — a stray unsolicited `PING`
consumed *as* the reply would otherwise leave the real reply buffered and desync the
one-reply-per-write accounting for the rest of the connection. Cheap insurance; the desktop IPC
rarely pings, but the loop makes the accounting robust either way. On Unix,
`set_read_timeout(Some(2s))` means a wedged Discord surfaces as `WouldBlock` / `TimedOut` → drop and
reconnect instead of parking the thread. Windows' std named-pipe handle has no read-timeout API, so
it keeps the plain blocking read and inherits that edge — survivable precisely because the worker is
a detached thread nobody waits on: a wedged handshake parks that thread, not the shutdown path. A
later upgrade can use `PeekNamedPipe` via the `windows-sys` dep we already have, at the cost of one
scoped `#[expect(unsafe_code, reason = …)]` (the `libc::mallopt` precedent). Not worth it up front.

**Nonce** — a `u64` counter, `format!("{pid}-{n}")`. No uuid, no randomness, still unique per
connection.

**Worker loop** — `enum Command { Apply(Presence), Clear, Disable, Shutdown }` over
**`std::sync::mpsc`, not `tokio::sync::mpsc`**, deliberately: `Receiver::recv_timeout` gives the
reconnect-backoff-while-idle loop for free, which tokio's channel has no blocking equivalent for.
`Sender<T>` has been `Sync` since Rust 1.72, so it lives in the `Arc<Service>` fine on the 1.97 pin.

- Latest-wins: after `recv` / `recv_timeout`, drain with `try_recv` and keep only the last command.
  Presence is pure latest-state; queued intermediates are noise.
- Keeps `desired: Option<Presence>` and re-applies it after every successful (re)connect, so a
  Discord restart mid-song repaints the card.
- Connect backoff 5 s → 60 s doubling, reset on success. While disabled, park on a plain blocking
  `recv()` — no polling thread while the feature is off.
- A write error means the socket died → drop it, publish `connected: false`, reconnect. Connect and
  reconnect failures stay **silent** — no toast, only the `connected: false` status the settings row
  reflects ("Discord not running") plus a log line. This is the deliberate opposite of Last.fm's
  *user-initiated* OAuth connect (which toasts `OperationFailed`): the Discord socket is probed in
  the background on a loop, so toasting each failed attempt would spam. Matches the repo convention
  that routine/background failures stay silent while only user-triggered actions surface a toast.
  The worker holds no `ui::*` types — same rule as `tasks/` and `services/scrobble/`.
- Detached thread, **not** registered with `TaskTracker`. `main()` ends in `process::exit(0)`, so a
  teardown handshake wouldn't reliably land anyway — and doesn't need to: **at quit the mechanism is
  the socket closing**, which makes Discord drop the card on its own. The commands that genuinely
  need to clear are the two where the process keeps running: `Disable` (toggle off) and `Clear`
  (playback stopped). `Shutdown` exists to end the loop, and its clear + opcode-2 close is a courtesy
  on the paths that do get to run it, not the thing correctness rests on.

**`src/services/discord/mod.rs`** — `DiscordPresenceService`, held as
`pub discord: Arc<DiscordPresenceService>` on `AppState` and constructed in `AppState::init`
immediately after `scrobble` (`src/state/mod.rs`), handed the same `Arc<OnceLock<reqwest::Client>>`
so the artwork lookup reuses the one connection pool:

```rust
DiscordPresenceService::init(&settings.discord, http_client.clone())
```

Note the signature is `(flags, http)` — two args, not the three of
`ScrobbleService::init(&paths, &flags, http)`. The service owns **no on-disk state of its own**: no
durable queue, no credentials file (the app ID is a compile-time `const`, not a secret to persist),
and its only settings live in `settings.json` via the flags struct. So it needs no `&Paths`, which
is the one field the scrobble service carries that this one drops.

Fields mirror `ScrobbleService`: `runtime: RwLock<DiscordFlags>` (read synchronously, never across
`.await`), `status_tx: watch::Sender<DiscordStatus>` (`{ enabled, connected }` — written by
`set_flags` *and* by the worker, so a Discord restart shows up without reopening the section),
`worker: Mutex<Option<mpsc::Sender<Command>>>` (lazily spawned — a user who never enables the
feature pays for no thread and no socket probing), plus `http` and the Phase 4 artwork LRU.

Public surface: `armed()` (the cheap synchronous gate the task checks), `set_flags(DiscordFlags)`,
`status()`, `subscribe_status()`, `apply(Presence)`, `clear()`.

Gate: `cargo clippy --all-targets -- -D warnings`.

## Phase 2 — Pure presence model + the detector task

> **✅ Landed (round 1), with these deltas from the sketch below:** the throttle
> is applied *before* evaluating the model — the task waits out the remaining
> window, then re-reads the watch and calls `on_view_model` *once* at send time,
> so the model's dedupe `last` only advances on an actual send (deferring after a
> compute would have deduped the deferred update into a no-op). The service `init`
> takes `&DiscordFlags` **only** (no `http` arg / artwork LRU yet — those arrive
> in Phase 4 when they're read, to avoid an unread field). The detector subscribes
> to `sinks.view_model` alone (no `position_tx` — the anchor is invariant across
> republishes, so per-second ticks aren't needed).

**`src/services/discord/model.rs`** — no I/O, no clock reads (`now_ts` is an input), matching
`services/scrobble/detector.rs` and `player::handlers::evaluate_playing_tick`.

- `struct Presence { details, state, large_text, large_image: Option<String>, paused, start_ts,
  end_ts }` — owned `String`s, since it crosses a channel. **Serialize via a `#[derive(Serialize)]`
  mirror DTO, not a hand-written `Serialize` impl.** The activity JSON is highly conditional
  (timestamps omitted when paused, `large_image` omitted when `None`, `small_image`/`small_text`
  swapping on pause), which is exactly what `#[serde(skip_serializing_if = "Option::is_none")]` on a
  derived struct expresses cleanly; the repo's own serde rule is "always prefer derive — manual impls
  are error-prone and rarely needed." Build the DTO from `Presence` at send time and hand it to
  `serde_json::to_vec`. The one field that can't be a bare `Option` is the pause-driven `small_image`
  swap (`paused` asset vs. `melodia`), which is a plain match into two owned fields, not a skip.
- Field mapping — all three read off `vm.current_track: Option<Arc<TrackSummary>>` (the VM carries no
  flat `title`/`artist`/`album`; identity is the `Arc<TrackSummary>` or `None`): `details` =
  `title` (a plain `String`), `state` = `artist` (`Option<String>`), `large_text` = `album`
  (`Option<String>`, fallback `"Melodia"`). Plus `"type": 2` (Listening — RPC accepts only 0/2/3/5)
  and `"status_display_type": 2` (Details) so the member-list line reads "Listening to \<song\>"
  rather than "…to Melodia". **Activity `type` over the RPC `SET_ACTIVITY` IPC path is a *recent*
  client capability** — Playing/Listening/Watching/Competing only started being honored over local
  IPC around mid-2024; older clients ignored `type` and forced "Playing". `status_display_type` is a
  further step: documented for the Social SDK and the gateway activity object (0 Name / 1 State /
  2 Details) but **not** in the RPC command reference, so send it and confirm at runtime
  (verification step 2). Both failure modes are graceful — Discord ignores activity fields it doesn't
  know, so an old client just falls back to the app name; nothing else on the card breaks.
- `timestamps.start = now_ts - position_secs`, `end = start + duration_secs`, **seconds not ms**
  (matching Discord's own `time(nullptr)` example), emitted **only when playing** with a known
  duration. Discord animates the bar client-side off that anchor, which is what makes a throttle as
  coarse as the 15 s one below invisible during normal playback — we only ever need to re-send the
  anchor, never to tick it. Discord requires activity type Listening or Watching when an `end`
  timestamp is present — which is exactly what we send.
- `small_image` = the `paused` asset key + `small_text` `"Paused"` when paused, else the `melodia`
  key. Presence strings stay **English-only**, the same call the tray labels made.
- `truncate_field(&str) -> String` — 128 chars max on a **char boundary**; a field under 2 chars is
  padded/omitted (older Discord clients reject 1-char `state` / `details`).
- `PresenceState::on_view_model(vm: Option<&PlayerViewModelLight>, now_ts, &DiscordFlags)
  -> Option<Update>` where `Update = Set(Presence) | Clear`. It reads `vm.status` — a
  **`&'static str`**, one of `"stopped"` / `"playing"` / `"paused"` / `"loading"` (from
  `PlaybackStatus::as_str()`), so the branch conditions below are plain `&str` matches, not enum
  arms — `vm.current_track` (the `Option<Arc<TrackSummary>>`), and `vm.position_ms` / `vm.duration_ms`
  (both `u64` on the VM). There is no separate `paused` bool: pause is `status == "paused"`.

The dedupe rule is the load-bearing bit — `view_model` republishes on *every* state change
including volume, and each republish must not become an IPC write:

- `status == "stopped"` or no current track → `Clear` (idempotent: skipped if already cleared).
- `status == "loading"` → **nothing**, holding the previous card. Every track change passes through
  it, so mapping it to `Clear` would flash the card off and back on — and spend one of the update
  windows below doing it.
- While playing, `now_ts - position_secs` is **invariant** — it only moves on a seek. So identity is
  `(track_id, paused, anchored_start)` with a **2 s drift tolerance** (the monitor's position is up
  to 500 ms stale and seconds truncate). A volume change recomputes the same anchor → no update. A
  seek moves it → re-anchor and send.
- While paused there are no timestamps, so identity collapses to `(track_id, paused)` — seeking
  while paused changes nothing visible and sends nothing.
- `hide_when_paused` turns the paused branch into `Clear`.

**`src/tasks/discord_presence.rs`** — `spawn(&TaskSpawner, &AppState)`, registered in
`boot/tasks.rs` beside `tasks::scrobble::spawn`. Always spawned and inert while disabled (same as
the scrobble tasks and `mbid_backfill`); one `watch` subscriber costs nothing.

- Do-while shaped like `tasks/scrobble.rs::run_detector`: process the primed value, then `select!`
  on `shutdown.cancelled()` / `vm_rx.changed()`.
- `service.armed()` guard first; on shutdown, `Clear`.
- Throttle `MIN_UPDATE_INTERVAL = Duration::from_secs(15)`. Discord caps presence at **one update
  per 15 s** and **silently drops** the ones inside that window — no error, no retry. Its own SDKs
  hide this by queueing newest-wins; over raw IPC there is no such queue, so anything we push early
  is simply lost. (The "5 updates per 20 s" figure that circulates is not in the current docs; don't
  design against it.) When an update lands early, sleep the remainder (inside a `select!` with
  shutdown) then **re-read the watch**. That trailing-edge flush is load-bearing, not an
  optimization: the watch holds only the latest value, so the suppressed intermediates collapse into
  one write that always lands on current truth within a single window.
- The cost is honest and small: a pause, resume or seek shortly after a track start can take up to
  15 s to appear. Track changes on real songs are further apart than that, so the common case is
  untouched, and the progress bar keeps animating client-side off the last anchor meanwhile.

**Tests** — `src/services/discord/tests/model_tests.rs` and
`src/services/discord/tests/ipc_tests.rs` (per-module `tests/` subdir + `#[path]`, per house
convention). Mirror the `summary(id, duration_ms)` / `vm(status, track)` fixtures from
`src/services/scrobble/tests/detector_tests.rs` — they're private to that `#[path]`-included module,
so this is a copy, not an import. Cases: volume republish sends nothing; seek re-anchors; pause drops
timestamps; resume re-anchors; stop clears; `loading` holds the previous card; a 128+ char multi-byte
title truncates on a char boundary; `hide_when_paused` clears; frame round-trips through
encode/decode against a `Vec<u8>`; an oversized declared length is rejected; the activity JSON
matches a fixture (`type: 2`, no timestamps when paused).

## Phase 3 — Settings: flags, persistence, section card

**`src/services/settings/data.rs`** — `DiscordFlags`, `#[derive(Default)]` + `#[serde(default)]` on
the struct, `#[serde(flatten)]`'d into `SettingsData` next to `ScrobbleFlags`. The `#[serde(default)]`
is not optional: Melodia is publicly released with a live updater, so an existing user's
`settings.json` predates these keys and must still load — the struct-level default fills every field
when the keys are absent, exactly as `ScrobbleFlags` does. Fields:

| field | default | note |
|---|---|---|
| `discord_rpc_enabled` | `false` | opt-in, per the shipped-app rule for new visible behavior |
| `discord_rpc_artwork` | `true` | inert until the parent is on; the row description states the lookup |
| `discord_rpc_hide_when_paused` | `false` | keep the card up by default |

Three bools sits at clippy's `struct_excessive_bools` cap — a fourth needs the `#[expect(…)]`
`ScrobbleFlags` carries. `discord_rpc_artwork` defaults *on* because it only takes effect under an
explicitly-enabled parent toggle.

**`src/library/settings/discord.rs`** — three `mutate_settings` setters, verbatim shape of
`src/library/settings/scrobble.rs`; re-export from `library/settings/mod.rs`.

**`ui/settings.slint`** — `discord-*` props + `*-changed` callbacks + `discord-connected`, following
the `scrobble-*` block.

**`ui/views/settings/discord-section.slint`** — new section card, mounted in
`ui/views/settings-view.slint` right after `scrob-sec := ScrobblingSection {}`. Mounting it is two
edits, not one: the "No matching settings" placeholder below the sections is gated on
`!<id>.has-matches` for **every** section, so the new `disc-sec` needs its own `&& !disc-sec.has-matches`
term or the placeholder stops appearing entirely. Copy the
`row-visible` / `has-matches` search-filter shape from `scrobbling-section.slint`. Rows: the enable
toggle (description names Discord and what is broadcast), a status line ("Connected to Discord" /
"Discord not running"), then `if Settings.discord-enabled:` the artwork toggle (description naming
the Deezer lookup) and the hide-while-paused toggle. Sub-rows mount under `if`, never
`visible: false` (slint#7377). Note the sub-rows gate on `discord-enabled` (the feature toggle),
**deliberately unlike** `scrobbling-section.slint`, which gates its sub-rows on the per-provider
`scrobble-*-connected` state: artwork and hide-while-paused are meaningful to configure whenever the
feature is enabled, regardless of whether Discord happens to be running right now — don't "align" it
to the connected-gated scrobble shape.

**`src/ui/discord_settings.rs`** — `install_discord(app, state)`, called from `boot/ui_setup.rs`
after `install_scrobbling`. Seed from `service.status()`, then a `subscribe_status()`
`slint::spawn_local(Compat::new(…))` paint loop as the single writer of the props — same structure
as `ui/scrobbling_settings.rs::install_scrobbling`. Toggles go through a `discord_toggle_binding`
modelled on `scrobble_toggle_binding`: flip the field in a fresh snapshot, `set_flags`
synchronously (so the task and worker see it at once), then `state.persist_blocking(...)`.

**Icon** — nothing in `scripts/icons.txt` fits; add one (`chat` or `groups`), then re-run
`scripts/subset-icon-fonts.sh` and `scripts/check-icons.py`.

**i18n** — every new literal wrapped in `@tr(...)`, and the same msgid/msgstr added to all six
shipped `translations/<lang>/LC_MESSAGES/Melodia.po` files (English is the msgid baseline).

## Phase 4 — Album artwork + link button

**`src/media/deezer.rs`** — add `search_album_cover(client, artist, album)` alongside
`search_artist_image_url`. Match the sibling's exact shape: signature returns
**`Result<Option<String>, AppError>`** (the explicit form the existing fn uses — *not* the
`AppResult<T>` alias), request is `https://api.deezer.com/search/album` with `q` + `limit=1`,
non-success status → `Ok(None)`, errors wrapped with `AppError::network(msg, source)`. It needs its
**own** response struct — albums expose `cover_big` / `cover_medium`, so the existing private
`DeezerArtist { picture_medium }` can't be reused; add a parallel `DeezerAlbum { cover_big: Option<String> }`
+ `DeezerAlbumSearch { data: Vec<DeezerAlbum> }` and return `body.data.first().and_then(|a| a.cover_big.clone())`.
`cover_big` is 500×500 (Discord renders ~300). The album's `link` is deliberately *not* returned —
the button below is fixed, so nothing would read it. No download, no disk cache: Discord's CDN
fetches the URL server-side, so we only pass the string along. The `q` should combine artist + album
(e.g. `artist:"…" album:"…"` or a plain concatenation) for a tighter match than album title alone.

**`src/services/discord/artwork.rs`** — `LruCache<ArtKey, Option<String>>` behind a
`parking_lot::Mutex` on the service, cap 64, keyed on lowercased `(artist, album)`. Bounded well
inside the memory rules. `None` → fall back to the `melodia` asset key. A 2 s timeout keeps presence
from lagging behind a slow Deezer — and it has to be an explicit
`tokio::time::timeout(Duration::from_secs(2), search_album_cover(...))` wrapper, because the shared
`reqwest::Client` from `build_http_client` carries a `read_timeout` of **one minute** (fine for the
updater's large downloads, far too slow for a presence update). The client's timeout is the backstop;
the 2 s budget is the presence-specific cap layered on top.

Only a **definitive** miss is cached — Deezer answered and matched nothing, so a repeat-play must not
re-query. A timeout or transport error caches nothing: caching one would blank that album's cover for
the rest of the session over a momentary hiccup.

**Task integration** — resolve only on a track-change update and only when `discord_rpc_artwork` is
on; a cache hit is a synchronous fast path, so pause/resume/seek never touch the network. Tracks with
no artist or no album (both `Option<String>` on `TrackSummary`) skip the lookup outright — an
untagged library would otherwise spend a request per track to search for nothing.

**Button** — one fixed entry,
`{"label":"Get Melodia","url":"https://github.com/KenanSalar/Melodia"}`. Fixed rather than the
resolved Deezer album link: no per-track state and it can never point somewhere wrong. Known Discord
behavior — buttons are invisible to *you* and render for everyone else viewing your profile.

## Phase 5 — Docs + gates

- **CLAUDE.md** — one long-form convention bullet in the Important Conventions list: the view-model
  seam; the invariant-anchor dedupe rule and its 2 s tolerance; the 15 s throttle against Discord's
  one-per-15 s silent-drop cap, and why the trailing-edge flush is what makes that safe; why the
  worker is a detached `std::sync::mpsc` thread rather than
  `spawn_blocking`; **why the IPC is hand-rolled** (the candidate crate's `uuid ^0.8` pin vs the
  tree's 1.x, and a nonce only needs uniqueness) with the frame layout and the socket-path table
  called out as the load-bearing bits; the two non-obvious protocol gotchas — activity
  `timestamps` are Unix **seconds** not milliseconds, and the Windows pipe `access_mode(0x3)` literal
  is `FILE_READ_DATA | FILE_WRITE_DATA`, not the `GENERIC_*` pair, so don't "correct" it; that
  external `https://` URLs work for `large_image` only; and that the app ID is public (no CI secret,
  unlike the Last.fm keys).
- **README** — feature bullet beside the scrobbling one and a note block near the Last.fm
  credentials note: what leaves the machine (track/artist/album/artwork URL → Discord; artist+album
  → Deezer when artwork is on), that it is off by default, and the Discord application ID
  requirement for anyone building a fork.
- **This file** deleted once shipped.

## Verification

```bash
cargo clippy --all-targets -- -D warnings   # the real gate; never cargo check
cargo test                                   # model + framing + Deezer-parse unit tests
python3 scripts/check-icons.py               # after touching scripts/icons.txt
cargo build && RUST_LOG=info target/debug/Melodia
```

End-to-end, with Discord desktop running:

1. Settings → Discord → enable. Row flips to "Connected to Discord"; log shows the socket path that
   matched.
2. Play a track → profile shows **Listening to Melodia**, title/artist/album cover, progress bar
   advancing; the button shows on a second account. Member-list status should read the song title —
   if it reads "Melodia" instead, this client ignores `status_display_type`; note it and move on,
   the rest of the card is unaffected.
3. Pause → timer disappears, paused marker appears, card stays. Toggle "Hide while paused" → card
   disappears on pause.
4. Seek → the bar jumps to the new position within ~15 s (one throttle window). Change volume 10× →
   no presence churn (verify at `RUST_LOG=debug`: no IPC writes). Skip through five tracks in quick
   succession → the card settles on the *last* one, not an intermediate.
5. Stop / end of queue → card clears. Quit Melodia → card clears (socket close).
6. Quit *Discord* mid-song → status row flips to "Discord not running"; restart Discord → the card
   comes back without touching playback.
7. Disable the toggle → card clears immediately, worker parks.
8. With Discord **not** installed: enable the toggle, confirm nothing hangs and playback is
   unaffected (backoff loop only).
9. **Flatpak Discord on Linux** — the direct test of our socket-path table.
10. **Windows** — the named-pipe path is a different `connect` branch, so it needs its own run:
    steps 1–7 on Windows with Discord desktop installed. Cross-compilation only proves it builds;
    the release CI matrix already covers `x86_64` / `aarch64` Windows for the build half.
11. Settings search: type a nonsense query → the "No matching settings" placeholder still appears
    (it's gated on *every* section reporting no match, and this feature adds one).
12. `cargo tree -d | grep uuid` → still exactly one `uuid`.
13. `cargo build --release && /usr/bin/time -v target/release/Melodia` — peak RSS stays under the
    200 MB ceiling (one extra thread + a 64-entry URL LRU should be noise). Measure with the artwork
    toggle **on**: it's the first thing that forces the lazily-built `reqwest` client + rustls stack
    to construct, which `AppState` deliberately defers off the boot/idle footprint.
