//! The blocking Discord IPC transport: framed JSON over a local unix socket
//! (Linux/macOS) or named pipe (Windows), run on a dedicated `std::thread`.
//!
//! `std` only — no new dependency, no `unsafe`. The framing and payloads are
//! pure (unit-tested over a `Vec<u8>`); only [`connect`] touches the OS. The
//! socket-discovery table is lifted from `discord-rich-presence` (MIT) — the one
//! part with real upstream value, so a new Discord packaging variant is a
//! one-line addition here rather than a dependency bump.
//!
//! Wire frame: `[u32 LE opcode][u32 LE len][JSON body]`. Opcodes: 0 handshake,
//! 1 frame, 2 close, 3 ping, 4 pong.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use super::model::Presence;
use super::{DISCORD_APP_ID, StatusCell, payload};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Reject a declared body length above this — a bogus header must not make us
/// allocate. Real Discord frames are a few hundred bytes.
const MAX_FRAME_LEN: u32 = 64 * 1024;

/// Reconnect backoff bounds while Discord is unreachable.
const CONNECT_BACKOFF_MIN: Duration = Duration::from_secs(5);
const CONNECT_BACKOFF_MAX: Duration = Duration::from_mins(1);

/// Read deadline so a wedged Discord surfaces as an error → reconnect, rather
/// than parking the worker thread forever. Unix only — a `std` named-pipe handle
/// has no read-timeout API, so Windows inherits the blocking edge (survivable:
/// the worker is a detached thread nobody waits on).
#[cfg(unix)]
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A connected IPC endpoint. Both concrete types implement `Read + Write`, so
/// the framing/worker code is generic over the platform.
#[cfg(unix)]
type Connection = UnixStream;
#[cfg(windows)]
type Connection = std::fs::File;

/// A request from the service to the worker. All four variants are used — the
/// service sends `Enable`/`Disable` from the settings toggle and `Apply`/`Clear`
/// from the detector task.
pub(crate) enum Command {
    /// Feature turned on — start connecting (and repaint any held presence).
    Enable,
    /// Feature turned off — clear the card and park until re-enabled.
    Disable,
    /// Show this card.
    Apply(Presence),
    /// Remove the card (playback stopped) but stay connected.
    Clear,
}

/// Outcome of the connected serve loop.
enum ServeOutcome {
    /// All senders dropped — the process is exiting; end the worker.
    Exit,
    /// A `Disable` arrived — park until re-enabled.
    Disabled,
    /// The socket died mid-session — reconnect.
    Disconnected,
}

/// The worker entry point, run on the dedicated Discord thread. Owns the socket;
/// never registered with the task tracker (at quit the socket simply closes,
/// which makes Discord drop the card). Publishes connection state through
/// `status`; connect/reconnect failures stay silent (logged), never a toast.
pub(crate) fn run_worker(rx: &mpsc::Receiver<Command>, status: &Arc<StatusCell>) {
    let pid = std::process::id();
    let mut nonce: u64 = 0;
    let mut desired: Option<Presence> = None;
    let mut enabled = false;
    let mut backoff = CONNECT_BACKOFF_MIN;

    loop {
        // Parked while disabled: no polling thread, no socket probing.
        while !enabled {
            match rx.recv() {
                Ok(cmd) => handle_command(cmd, &mut desired, &mut enabled),
                Err(_) => return,
            }
        }

        let Some(mut conn) = connect() else {
            match wait_and_backoff(rx, status, &mut backoff, &mut desired, &mut enabled) {
                WaitOutcome::Exit => return,
                WaitOutcome::Continue => continue,
            }
        };

        if let Err(e) = handshake(&mut conn) {
            log::debug!("discord: handshake failed: {e}");
            match wait_and_backoff(rx, status, &mut backoff, &mut desired, &mut enabled) {
                WaitOutcome::Exit => return,
                WaitOutcome::Continue => continue,
            }
        }

        backoff = CONNECT_BACKOFF_MIN;
        status.set_connected(true);

        // Repaint the held presence after a (re)connect — a Discord restart
        // mid-song brings the card back.
        if let Some(presence) = &desired {
            let body = payload::set_activity_json(presence, pid, &next_nonce(pid, &mut nonce));
            if let Err(e) = send_frame_and_ack(&mut conn, &body) {
                log::debug!("discord: re-apply failed: {e}");
                status.set_connected(false);
                continue;
            }
        }

        match serve(&mut conn, rx, pid, &mut nonce, &mut desired, &mut enabled) {
            ServeOutcome::Exit => return,
            ServeOutcome::Disabled | ServeOutcome::Disconnected => status.set_connected(false),
        }
    }
}

/// Serve commands over a live connection until the socket dies or we're disabled.
/// Latest-wins: after a blocking `recv`, any queued commands are drained so a
/// burst of seeks/skips collapses into one write.
fn serve(
    conn: &mut Connection,
    rx: &mpsc::Receiver<Command>,
    pid: u32,
    nonce: &mut u64,
    desired: &mut Option<Presence>,
    enabled: &mut bool,
) -> ServeOutcome {
    loop {
        let Ok(cmd) = rx.recv() else {
            return ServeOutcome::Exit;
        };
        let prev = desired.clone();
        handle_command(cmd, desired, enabled);
        drain_commands(rx, desired, enabled);

        if *desired != prev {
            let body = match desired.as_ref() {
                Some(presence) => payload::set_activity_json(presence, pid, &next_nonce(pid, nonce)),
                None => payload::clear_activity_json(pid, &next_nonce(pid, nonce)),
            };
            if let Err(e) = send_frame_and_ack(conn, &body) {
                log::debug!("discord: send failed: {e}");
                return ServeOutcome::Disconnected;
            }
        }
        if !*enabled {
            return ServeOutcome::Disabled;
        }
    }
}

/// Result of waiting out a reconnect backoff.
enum WaitOutcome {
    /// All senders dropped — exit the worker.
    Exit,
    /// A command arrived or the backoff elapsed — re-evaluate.
    Continue,
}

/// Wait up to `timeout` for a command while disconnected, applying it if one
/// arrives, so a `Disable`/`Apply` during backoff is handled promptly.
fn idle_wait(
    rx: &mpsc::Receiver<Command>,
    timeout: Duration,
    desired: &mut Option<Presence>,
    enabled: &mut bool,
) -> WaitOutcome {
    match rx.recv_timeout(timeout) {
        Ok(cmd) => {
            handle_command(cmd, desired, enabled);
            drain_commands(rx, desired, enabled);
            WaitOutcome::Continue
        }
        Err(mpsc::RecvTimeoutError::Timeout) => WaitOutcome::Continue,
        Err(mpsc::RecvTimeoutError::Disconnected) => WaitOutcome::Exit,
    }
}

/// After a failed connect or handshake: mark disconnected, wait out the current
/// backoff (applying any command that lands during it), and grow the backoff on
/// a plain timeout. `Exit` means all senders dropped — the caller ends the worker.
fn wait_and_backoff(
    rx: &mpsc::Receiver<Command>,
    status: &Arc<StatusCell>,
    backoff: &mut Duration,
    desired: &mut Option<Presence>,
    enabled: &mut bool,
) -> WaitOutcome {
    status.set_connected(false);
    let outcome = idle_wait(rx, *backoff, desired, enabled);
    if matches!(outcome, WaitOutcome::Continue) {
        *backoff = (*backoff * 2).min(CONNECT_BACKOFF_MAX);
    }
    outcome
}

fn handle_command(cmd: Command, desired: &mut Option<Presence>, enabled: &mut bool) {
    match cmd {
        Command::Enable => *enabled = true,
        Command::Disable => {
            *enabled = false;
            *desired = None;
        }
        Command::Apply(presence) => *desired = Some(presence),
        Command::Clear => *desired = None,
    }
}

/// Apply any already-queued commands without blocking (latest state wins).
fn drain_commands(rx: &mpsc::Receiver<Command>, desired: &mut Option<Presence>, enabled: &mut bool) {
    while let Ok(cmd) = rx.try_recv() {
        handle_command(cmd, desired, enabled);
    }
}

fn next_nonce(pid: u32, nonce: &mut u64) -> String {
    *nonce += 1;
    format!("{pid}-{nonce}")
}

/// Send the handshake and read Discord's `READY` dispatch. Connected once it
/// returns `Ok`.
fn handshake(conn: &mut Connection) -> io::Result<()> {
    write_frame(conn, OP_HANDSHAKE, &payload::handshake_json(DISCORD_APP_ID))?;
    read_reply(conn)?;
    Ok(())
}

/// Write a `FRAME` command and consume its one reply, keeping the
/// one-reply-per-write socket accounting balanced.
fn send_frame_and_ack(conn: &mut Connection, body: &[u8]) -> io::Result<()> {
    write_frame(conn, OP_FRAME, body)?;
    read_reply(conn)?;
    Ok(())
}

/// Read frames until the command's reply arrives, echoing a stray `PING` as a
/// `PONG` so an unsolicited ping isn't mistaken for the reply (which would
/// desync the per-write accounting). A `CLOSE` is treated as a disconnect.
fn read_reply(conn: &mut Connection) -> io::Result<(u32, Vec<u8>)> {
    loop {
        let (opcode, body) = read_frame(conn)?;
        match opcode {
            OP_PING => write_frame(conn, OP_PONG, &body)?,
            OP_CLOSE => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "discord sent CLOSE",
                ));
            }
            _ => return Ok((opcode, body)),
        }
    }
}

/// Write one frame: LE opcode, LE length, body. Generic over the sink so it's
/// exercised over a `Vec<u8>` in tests and the socket in production.
fn write_frame(w: &mut impl Write, opcode: u32, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame body too large"))?;
    w.write_all(&opcode.to_le_bytes())?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// Read one frame. A declared length above [`MAX_FRAME_LEN`] is a protocol
/// desync → `InvalidData`, so a bogus header drops the connection rather than
/// allocating on it.
fn read_frame(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut op_bytes = [0u8; 4];
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut op_bytes)?;
    r.read_exact(&mut len_bytes)?;
    let opcode = u32::from_le_bytes(op_bytes);
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length exceeds cap",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok((opcode, body))
}

/// Discovery subdirs (relative to each base) where Discord exposes its socket,
/// covering native, Flatpak (Discord + Vesktop) and Snap installs. Lifted from
/// `discord-rich-presence` (MIT).
#[cfg(unix)]
const SOCKET_SUBDIRS: &[&str] = &[
    "",
    "app/com.discordapp.Discord/",
    "app/dev.vencord.Vesktop/",
    ".flatpak/com.discordapp.Discord/xdg-run/",
    ".flatpak/dev.vencord.Vesktop/xdg-run/",
    "snap.discord/",
    "snap.discord-canary/",
];

/// Runtime dirs to probe, in order — `XDG_RUNTIME_DIR` first, then the temp-dir
/// env vars, then `/tmp`.
#[cfg(unix)]
fn socket_bases() -> Vec<String> {
    let mut bases: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .filter_map(|var| std::env::var(var).ok())
        .filter(|value| !value.is_empty())
        .collect();
    bases.push("/tmp".to_owned());
    bases
}

/// Probe `discord-ipc-{0..10}` across every base/subdir and take the first that
/// opens. `None` when Discord isn't running.
#[cfg(unix)]
fn connect() -> Option<Connection> {
    for base in socket_bases() {
        for sub in SOCKET_SUBDIRS {
            for i in 0..10 {
                let path = format!("{base}/{sub}discord-ipc-{i}");
                if let Ok(stream) = UnixStream::connect(&path) {
                    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
                    log::info!("discord: connected via {path}");
                    return Some(stream);
                }
            }
        }
    }
    None
}

/// Windows named-pipe connect. `access_mode(0x3)` is `FILE_READ_DATA |
/// FILE_WRITE_DATA` (the file-specific read/write pair a named pipe accepts) —
/// **not** `GENERIC_READ | GENERIC_WRITE` (`0xC0000000`); do not "correct" it.
#[cfg(windows)]
fn connect() -> Option<Connection> {
    use std::os::windows::fs::OpenOptionsExt;
    for i in 0..10 {
        let path = format!(r"\\?\pipe\discord-ipc-{i}");
        if let Ok(file) = std::fs::OpenOptions::new().access_mode(0x3).open(&path) {
            log::info!("discord: connected via {path}");
            return Some(file);
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/ipc_tests.rs"]
mod tests;
