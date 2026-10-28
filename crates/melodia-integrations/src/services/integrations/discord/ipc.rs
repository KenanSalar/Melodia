//! The blocking Discord IPC transport: framed JSON over a unix socket, or a named pipe on
//! Windows, on a thread of its own. Hand-rolled on `std` alone
//! (`docs/adr/0024-discord-ipc-is-hand-rolled.md`), so should the reason lift this file deletes
//! and [`super::model`] is untouched. Only [`connect`] touches the OS.
//!
//! Wire frame: `[u32 LE opcode][u32 LE len][JSON body]`.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use melodia_core::error::describe;

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

/// A wedged Discord surfaces as an error and a reconnect rather than a parked thread. Unix only:
/// a `std` named pipe has no read timeout, which costs only the next worker a wait, and a Discord
/// that wedged this one would wedge that one too.
#[cfg(unix)]
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Both are `Read + Write`, so the framing and the worker never branch on the platform.
#[cfg(unix)]
pub(crate) type Connection = UnixStream;
#[cfg(windows)]
pub(crate) type Connection = std::fs::File;

/// `Enable` and `Disable` come off the settings toggle, `Apply` and `Clear` off the detector task.
pub(crate) enum Command {
    /// Start connecting, and repaint any held presence.
    Enable,
    /// Clear the card and close the socket. The service drops the channel right after, which ends
    /// the worker.
    Disable,
    Apply(Presence),
    /// Remove the card but stay connected.
    Clear,
}

enum ServeOutcome {
    /// All senders dropped — end the worker.
    Exit,
    /// A `Disable` arrived — close the socket.
    Disabled,
    /// The socket died mid-session — reconnect.
    Disconnected,
}

/// A failure to connect stays in the log, never a toast. `connect` is [`connect`] in the app.
pub(crate) fn run_worker(
    rx: &mpsc::Receiver<Command>,
    status: &Arc<StatusCell>,
    connect: &dyn Fn() -> Option<Connection>,
) {
    let pid = std::process::id();
    let mut nonce: u64 = 0;
    let mut desired: Option<Presence> = None;
    let mut enabled = false;
    let mut backoff = CONNECT_BACKOFF_MIN;

    loop {
        // Blocks without probing until enabled. Past a `Disable` the channel is already closed,
        // so this returns.
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
            log::debug!("discord: handshake failed: {}", describe(&e));
            match wait_and_backoff(rx, status, &mut backoff, &mut desired, &mut enabled) {
                WaitOutcome::Exit => return,
                WaitOutcome::Continue => continue,
            }
        }

        backoff = CONNECT_BACKOFF_MIN;
        status.set_connected(true);

        // So a Discord restarted mid-song brings the card back.
        if let Some(presence) = &desired {
            let body = payload::set_activity_json(presence, pid, &next_nonce(pid, &mut nonce));
            if let Err(e) = send_frame_and_ack(&mut conn, &body) {
                log::debug!("discord: re-apply failed: {}", describe(&e));
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

/// Latest wins: what has queued behind a blocking `recv` drains with it, so a burst of skips is
/// one write.
fn serve(
    conn: &mut Connection,
    rx: &mpsc::Receiver<Command>,
    pid: u32,
    nonce: &mut u64,
    desired: &mut Option<Presence>,
    enabled: &mut bool,
) -> ServeOutcome {
    // Seeded from `desired`, which the caller has just repainted, so a batch that changes nothing
    // visible (a lone `Enable`, an identical card) spends no write.
    let mut last_sent = desired.clone();
    loop {
        let Ok(cmd) = rx.recv() else {
            return ServeOutcome::Exit;
        };
        handle_command(cmd, desired, enabled);
        drain_commands(rx, desired, enabled);

        if *desired != last_sent {
            let body = match desired.as_ref() {
                Some(presence) => {
                    payload::set_activity_json(presence, pid, &next_nonce(pid, nonce))
                }
                None => payload::clear_activity_json(pid, &next_nonce(pid, nonce)),
            };
            if let Err(e) = send_frame_and_ack(conn, &body) {
                log::debug!("discord: send failed: {}", describe(&e));
                return ServeOutcome::Disconnected;
            }
            last_sent.clone_from(desired);
        }
        if !*enabled {
            return ServeOutcome::Disabled;
        }
    }
}

enum WaitOutcome {
    /// All senders dropped — exit the worker.
    Exit,
    /// A command arrived or the backoff elapsed — re-evaluate.
    Continue,
}

/// A command landing mid-backoff is applied at once rather than after it.
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

fn drain_commands(
    rx: &mpsc::Receiver<Command>,
    desired: &mut Option<Presence>,
    enabled: &mut bool,
) {
    while let Ok(cmd) = rx.try_recv() {
        handle_command(cmd, desired, enabled);
    }
}

fn next_nonce(pid: u32, nonce: &mut u64) -> String {
    *nonce += 1;
    format!("{pid}-{nonce}")
}

/// Connected once Discord's `READY` has been read.
fn handshake(conn: &mut Connection) -> io::Result<()> {
    write_frame(conn, OP_HANDSHAKE, &payload::handshake_json(DISCORD_APP_ID))?;
    read_reply(conn)?;
    Ok(())
}

/// Reads the one reply each write gets, which keeps the socket's accounting balanced. Discord
/// answers a rejected `SET_ACTIVITY` with `evt: "ERROR"` at a success's opcode, so it is logged
/// here: a retry can't fix a deterministic rejection, and dropping the connection would be worse.
fn send_frame_and_ack(conn: &mut Connection, body: &[u8]) -> io::Result<()> {
    write_frame(conn, OP_FRAME, body)?;
    let (_opcode, reply) = read_reply(conn)?;
    log_error_ack(&reply);
    Ok(())
}

fn log_error_ack(reply: &[u8]) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(reply) else {
        return;
    };
    if value.get("evt").and_then(serde_json::Value::as_str) != Some("ERROR") {
        return;
    }
    let data = value.get("data");
    let code = data.and_then(|d| d.get("code")).and_then(serde_json::Value::as_i64);
    let message = data
        .and_then(|d| d.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    log::debug!("discord: activity rejected (code {code:?}): {message}");
}

/// Answers a stray `PING` rather than taking it for the reply, which would desync the
/// accounting. A `CLOSE` is a disconnect.
fn read_reply(conn: &mut Connection) -> io::Result<(u32, Vec<u8>)> {
    loop {
        let (opcode, body) = read_frame(conn)?;
        match opcode {
            OP_PING => write_frame(conn, OP_PONG, &body)?,
            OP_CLOSE => {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "discord sent CLOSE"));
            }
            _ => return Ok((opcode, body)),
        }
    }
}

fn write_frame(w: &mut impl Write, opcode: u32, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame body too large"))?;
    w.write_all(&opcode.to_le_bytes())?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(body)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut op_bytes = [0u8; 4];
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut op_bytes)?;
    r.read_exact(&mut len_bytes)?;
    let opcode = u32::from_le_bytes(op_bytes);
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame length exceeds cap"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok((opcode, body))
}

/// Where the socket sits under each base for native, Flatpak (Discord and Vesktop) and Snap
/// installs. Lifted from `discord-rich-presence` (MIT), so a new packaging is a line here.
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

/// Read from the environment once rather than on every reconnect probe.
#[cfg(unix)]
fn socket_bases() -> &'static [String] {
    static BASES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        let mut bases: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .filter_map(|var| std::env::var(var).ok())
            .filter(|value| !value.is_empty())
            .collect();
        bases.push("/tmp".to_owned());
        bases
    });
    &BASES
}

/// `None` when Discord isn't running.
#[cfg(unix)]
pub(super) fn connect() -> Option<Connection> {
    use std::fmt::Write as _;
    let mut path = String::with_capacity(64);
    for base in socket_bases() {
        for sub in SOCKET_SUBDIRS {
            for i in 0..10 {
                path.clear();
                let _ = write!(path, "{base}/{sub}discord-ipc-{i}");
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

/// `access_mode(0x3)` is `FILE_READ_DATA | FILE_WRITE_DATA`, the pair a named pipe accepts, and
/// **not** `GENERIC_READ | GENERIC_WRITE` (`0xC0000000`); don't "correct" it.
#[cfg(windows)]
pub(super) fn connect() -> Option<Connection> {
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
