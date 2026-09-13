# MPRIS Position frozen during playback

Working doc. Delete when the fix ships.

**Status:** steps 0 to 5 done, static gates green. Waiting on Kenan's manual test before tests and docs.

**Where the implementation moved off the plan:**
- `Cargo.lock` keeps `dbus`, `dbus-crossroads` and `libdbus-sys`. The lock is target-agnostic and souvlaki still lists them for Unix, but `cargo tree -i libdbus-sys --target x86_64-unknown-linux-gnu` prints nothing, so no Linux build compiles or links them.
- `SetPosition` ignores an id other than `/`, the spec's stale check, since zbus hands the argument over anyway.
- The optimistic volume record lives in the MPRIS setter, not on `Published`, which would leave it dead on Windows and macOS.
- `Changes::any` was dropped: the souvlaki `sync` does the same work without the early return.

## Context

**What the user sees, all three halves of it:**

1. `playerctl -p melodia position` (or `busctl --user get-property … Position`) returns the same value for the whole track. The earlier session saw 100000 µs across 25 s of playback.
2. Plasma's media popup opened mid-track draws its bar from that stale value. Plasma reads `Position` once on open, then adds a second per second. KDE Connect's phone remote shows the same stale time, because it reads `Position` into every packet it sends.
3. Seeking inside Melodia never reaches an open Plasma popup or KDE Connect. Both wait for a `Seeked` signal that is never sent.

**Why:**

- `crates/melodia-engine/src/player/engine/handlers.rs:441` pushes the position to the OS controls only under `cfg(any(windows, macos))`. Linux gets a push only when state changes (`with_state_emit` → `sync`), so `Position` keeps the value from the last play, pause or seek.
- souvlaki 0.8.3 handles `Get Position` by returning whatever `progress` it was last given in `set_playback`. It never extrapolates, and neither of its D-Bus backends emits `Seeked` for `SetPosition`. 0.8.3 is still the newest release. Master has since moved to zbus 5 but has no position fix.
- The MPRIS spec says `Position` does **not** emit PropertiesChanged. Clients read it when they need it and watch `Seeked` for jumps:
  - Plasma's libkmpris calls `updatePosition()` when the popup opens and when PlaybackStatus or Rate changes.
  - KDE Connect reads it for every packet.
  - playerctl reads it on every call.

**Why not just drop the `cfg`:** every souvlaki `set_playback` also emits `PropertiesChanged(PlaybackStatus)`, even when the status hasn't changed. KDE Connect's `mpriscontrol` plugin sends a LAN packet to every paired phone for each of those signals.
- A 1 s push means a packet per second per phone.
- A 5 s push makes playerctl and waybar step in 5 s jumps.
- Neither sends `Seeked`.

**Outcome:**
- On Linux, `Get Position` returns the latest poll tick (at most one poll old).
- Nothing is emitted on a timer.
- `Seeked` is sent on real seeks.
- souvlaki, and with it `dbus`, `dbus-crossroads` and `libdbus-sys`, leaves the Linux build.
- Windows and macOS behave as they do today.

## Approach

Serve MPRIS ourselves on Linux over zbus 5.18. It is already a Linux dependency (`melodia-platform`), and its tokio footgun is already documented. Keep souvlaki for SMTC and macOS. The split mirrors the tray's façade (`melodia-platform/…/tray/mod.rs`: `ksni_backend` / `tray_icon_backend` behind `pub use`).

### Structure: `crates/melodia-integrations/src/services/integrations/media_controls/`

| file | owns |
|---|---|
| `mod.rs` | The façade: `#[cfg(target_os = "linux")] mod mpris_backend;` and `#[cfg(any(windows, macos))] mod souvlaki_backend;`, each doing `pub use …::{MediaControlsHandle, init_media_controls}`. Shared code: `spawn_event_receiver` (channel item becomes `PlayerEvent`, no translation step left), `volume_percent(amplitude: f64) -> u32` (the clamp and round lifted out of `translate_event`), and `cover_url(path)` (the `file://` prefix). |
| `published.rs` | `PublishedMetadata`, moved unchanged. `Published` is the last-sent snapshot (metadata, status, position, volume, muted). `changes(&self, vm, status) -> Changes` asks the question and `record(&mut self, …)` stores the answer. Today's diff inside `sync()` moves here, so both backends share one copy. |
| `souvlaki_backend.rs` (windows, macos) | Today's handle, `attach_smtc` and `create_controls`, moved. `translate_event` now yields `PlayerEvent` inside souvlaki's callback, before `try_send`. `sync` applies `Changes` the way it does today. `update_position` rate-limits itself (`last_timeline_push: Option<Instant>`, reset by `sync`'s `set_playback`), and the 5 s constant moves here from `handlers.rs`, since only this backend pays per push. |
| `mpris_backend/mod.rs` (linux) | The handle holds `Arc<parking_lot::Mutex<Published>>`, shared with the interfaces, and a `std::sync::mpsc::Sender<Emit>` (`Metadata`, `PlaybackStatus`, `Volume`, `Seeked(u64)`). A thread named `mpris-signals` (13 bytes) owns the blocking zbus connection and drains the channel. `sync` diffs, records and sends. `update_position` only records. `seeked` sends. On init failure (no session bus, or the name is taken) it logs a warning through `error::describe` and the handle stays inert, as today. |
| `mpris_backend/interface.rs` | `#[zbus::interface]` for `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player`. Getters read the shared `Published`. Also holds a pure `seek_target(position_us, length_us) -> Option<u64>` for `SetPosition`. |

**Emission rules**
- D-Bus I/O happens only on `mpris-signals`: `zbus::block_on` over the generated async `<prop>_changed` and the `seeked` signal.
- `sync` can run on the Slint thread (sync-wired callbacks), so it may only send on the channel.
- `<prop>_changed` re-reads through the getter, so a burst of emits always publishes the current value, whatever order they land in.
- `Seeked` carries the seek target explicitly, so a poll tick landing in between can't overwrite it.
- Lock order: zbus's interface lock, then `Published`. `sync` holds `Published` and never takes zbus's lock, so the two can't deadlock.

**What the Linux server exposes.** It matches souvlaki except where the bug needs otherwise:
- `Position`: `emits_changed_signal = "false"`. Recorded ms × 1000 as saturating `i64`, or 0 when Stopped or Loading.
- `Seeked(x)`: new signal.
- `PlaybackStatus`: Stopped for both Stopped and Loading, as today.
- `Metadata`: `mpris:trackid` stays `/`, plus `mpris:length` (absent for a live source), `mpris:artUrl`, `xesam:title`, `xesam:artist` (as an array) and `xesam:album`.
- `Volume` (read/write): the setter sends `PlayerEvent::SetVolume(volume_percent(v))`.
- `Rate`, `MinimumRate` and `MaximumRate` stay 1.0.
- `CanGoNext`, `CanGoPrevious`, `CanPlay`, `CanPause`, `CanSeek` and `CanControl` stay true.
- `CanRaise` and `CanQuit` become **false**, because nothing answers `Raise` or `Quit` (both are dropped today). `HasTracklist` is false. `Identity` is "Melodia".
- Methods:
  - Next, Previous, Pause, PlayPause, Stop and Play each send their `PlayerEvent`.
  - `SetPosition` goes through `seek_target`, which ignores a negative position or one past the length, then sends `SeekTo`.
  - `Seek` and `OpenUri` log at debug level and do nothing, as today.

### Engine: `crates/melodia-engine/src/player/engine/`

- `event_sink.rs`: `update_position` becomes "called on every poll while playing". Add `fn seeked(&self, _position_ms: u64) {}` with a default no-op. The souvlaki backend keeps it as a no-op, because `sync` already pushed the new timeline.
- `handlers.rs`: delete `MEDIA_POSITION_INTERVAL_MS`, `MEDIA_POSITION_EVERY_N_TICKS`, their `const` assert, `media_tick_counter` and its reset, all of them cfg-gated. The `Playing` arm calls `mc.update_position(tick.position_ms)` on every platform.
- `actions.rs`: the `PlayerAction::Seek` arm calls `mc.seeked(position_ms)` after `engine.seek`. `build_move_to_actions` is the only producer of that action, so Previous restarting the track sends `Seeked(0)` too, as the spec requires.

### Wiring and manifests

- `crates/melodia-app/src/state/mod.rs`: `media_control_rx` becomes `mpsc::Receiver<PlayerEvent>`. Remove `souvlaki` (and its comment) from `crates/melodia-app/Cargo.toml`.
- `crates/melodia-integrations/Cargo.toml`: `souvlaki` moves to `[target.'cfg(any(target_os = "windows", target_os = "macos"))'.dependencies]`, and `zbus.workspace = true` goes under `[target.'cfg(target_os = "linux")'.dependencies]`. Each module's cfg matches its dependency's gate exactly.
- Root `Cargo.toml`: move `souvlaki = "0.8.3"` into the platform-specific block, and add MPRIS to the zbus comment.
- Run one clippy **without** `--locked` so the lock is rewritten, then confirm `cargo tree -i libdbus-sys --target x86_64-unknown-linux-gnu` is empty.
- CI: read `.claude/rules/ci-packaging.md` first. Then drop `libdbus-1-dev` from `.github/actions/linux-system-deps/action.yml:47`, and remove libdbus from the comment at `scripts/build-rpm.sh:145`.

## Order of work

0. Copy this plan to `docs/plans/MPRIS_POSITION.md`, keep its step markers current, and delete it when the fix ships.
1. Extract `published.rs` and move the souvlaki code into `souvlaki_backend.rs`. This step changes no behaviour.
2. Engine: the trait, the monitor's per-tick call and the `seeked` call in the seek arm.
3. `mpris_backend/`.
4. Manifests, the lock and the CI package list.
5. Static gates: `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings` and `cargo test --locked --workspace`. The Windows and macOS half is only compiled by the PR's `clippy-windows` and `test-windows` jobs.

**Stop there.** No launch. Kenan tests by hand.

## Verification (manual, Kenan)

- `watch -n1 playerctl -p melodia position` climbs while playing, holds when paused, and reads 0 when stopped.
- `dbus-monitor --session "type='signal',path='/org/mpris/MediaPlayer2'"` shows **no** periodic signals while playing. It shows PropertiesChanged on play/pause, track change and volume, and a `Seeked` for a seek from Melodia's UI and for Previous restarting a track.
- `playerctl -p melodia position 60` seeks, and a `Seeked` follows.
- Plasma popup: opened mid-track it shows the right time. With it open, seeking in Melodia moves its bar.
- The media keys and the Plasma transport buttons still work, and the volume slider in the popup moves Melodia's.
- Optional: the KDE Connect phone remote shows the current time.

## After the go: tests, then docs

**Tests**
- Move the existing `PublishedMetadata` tests to `published.rs`, and add `changes()` partitions (status, position, volume, mute, each on its own).
- The `volume_percent` clamp and round moves to the shared tests. The transport-mapping tests move with `translate_event`, so they run under `test-windows` only.
- `seek_target` boundaries: -1, 0, the length, length + 1, and no length at all.
- The µs conversion: saturation, and 0 when Stopped or Loading.
- `actions_tests`: a recording `MediaControlsSync` fake sees `seeked` after `Seek`. `handlers_tests`: `update_position` fires on every playing tick.

**Docs**
- `.claude/rules/desktop-shell.md`: the "OS media controls" and Shutdown bullets.
- The root `CLAUDE.md` zbus footgun line (add MPRIS as a `zbus::blocking` user).
- `crates/melodia/src/main.rs:516` (the "souvlaki's MPRIS thread" comment).
- The `event_sink.rs` doc comments and `melodia-views/…/shell/event_sink.rs` `//!`.

## Not in this change

- `Rate` following playback speed.
- Relative `Seek` (still needs a library API).
- Truthful `CanGoNext` / `CanGoPrevious`.
- A per-track `mpris:trackid`.
