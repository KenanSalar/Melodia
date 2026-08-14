//! The single-instance claim, and the socket a second launch hands its file
//! paths down.
//!
//! **Binding the name *is* the claim** — `AddrInUse` is how a second process
//! learns it is second, atomic where a probe-then-bind races two cold starts.
//!
//! **Claim early, accept late.** `main()` binds before the logger opens its
//! file; [`serve`] starts accepting only once there is a window to raise.
//! Connections wait in the listen backlog in between, so nothing is lost.
//!
//! Blocking `std` sockets on a dedicated thread, as `services::discord::ipc`
//! runs its transport: the claim predates the runtime, and a parked
//! `spawn_blocking` task would hold one of the 32 slots `main()` caps at.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, Stream, prelude::*};

/// Re-exported so `boot` can name what [`Claim::Primary`] carries without
/// depending on `interprocess` itself.
pub use interprocess::local_socket::Listener;

use crate::media::is_audio_extension;

/// Checked against the *declared* length before a buffer is sized for it, so a
/// hostile peer gets no allocation out of asking for more. A thousand paths fit.
const MAX_PAYLOAD_LEN: u64 = 64 * 1024;

const LENGTH_PREFIX_LEN: usize = size_of::<u32>();

/// A peer that connects and then says nothing must not park the accept thread.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// A listener that has stopped working errors as fast as the loop can ask, so
/// give up rather than spin.
const MAX_CONSECUTIVE_ACCEPT_FAILURES: u32 = 16;

/// Set on the child of every restart, which then waits for the name instead of
/// forwarding to it.
///
/// `Command::exec` frees the name itself (CLOEXEC on the image replace), but
/// `shutdown::spawn_detached` leaves parent and child alive together — a child
/// that forwarded and exited there would go down with the parent behind it.
pub const RESPAWN_ENV: &str = "MELODIA_RESPAWN";

/// Generous against a slow shutdown, short enough not to read as a hang.
const RESPAWN_WAIT: Duration = Duration::from_secs(5);
const RESPAWN_POLL: Duration = Duration::from_millis(50);

/// The outcome of asking to be the only Melodia over a given data directory.
pub enum Claim {
    /// Hand the listener to [`serve`] once there is a window to raise.
    Primary(Listener),
    /// A live Melodia took the paths; return without booting.
    Secondary,
    /// Not bindable, and not because a primary holds it. Boot anyway — refusing
    /// to start over a socket costs more than the duplicate window it prevents.
    Unenforced(io::Error),
}

/// Audio file paths from the command line, made absolute against *this*
/// process's working directory — a relative path forwarded to the primary would
/// otherwise resolve against that process's. The extension filter subsumes
/// flag-shaped arguments, none of which end in an audio extension.
pub fn audio_files_from_argv() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()).is_some_and(is_audio_extension)
        })
        .filter_map(|path| std::path::absolute(path).ok())
        .collect()
}

/// Claim exclusive ownership of `data_dir`, or hand `files` to the process that
/// already holds it.
pub fn claim(data_dir: &Path, files: &[PathBuf]) -> Claim {
    let name = match socket_name(data_dir) {
        Ok(name) => name,
        Err(e) => return Claim::Unenforced(e),
    };
    let restarting = std::env::var_os(RESPAWN_ENV).is_some();
    let give_up_at = Instant::now() + RESPAWN_WAIT;

    loop {
        let taken = match ListenerOptions::new().name(name.borrow()).create_sync() {
            Ok(listener) => return Claim::Primary(listener),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => e,
            Err(e) => return Claim::Unenforced(e),
        };

        if !restarting {
            return match forward(name.borrow(), files) {
                Ok(()) => Claim::Secondary,
                Err(e) => Claim::Unenforced(e),
            };
        }
        if Instant::now() >= give_up_at {
            return Claim::Unenforced(taken);
        }
        std::thread::sleep(RESPAWN_POLL);
    }
}

/// Accept forwarded launches for the rest of the process's life. `on_launch`
/// gets each one's paths — empty when someone simply started Melodia again,
/// which still wants the window raised.
pub fn serve(listener: Listener, on_launch: impl Fn(Vec<String>) + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("melodia-open".to_owned())
        .spawn(move || accept_loop(&listener, on_launch));

    if let Err(e) = spawned {
        log::warn!("single_instance: no accept thread, forwarded launches will be dropped: {e}");
    }
}

fn accept_loop(listener: &Listener, on_launch: impl Fn(Vec<String>)) {
    let mut consecutive_failures = 0_u32;

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => {
                consecutive_failures = 0;
                stream
            }
            Err(e) => {
                consecutive_failures += 1;
                log::warn!("single_instance: accept failed: {e}");
                if consecutive_failures >= MAX_CONSECUTIVE_ACCEPT_FAILURES {
                    log::error!(
                        "single_instance: no longer accepting; further launches will open a second window"
                    );
                    return;
                }
                continue;
            }
        };

        match read_payload(stream) {
            Ok(paths) => on_launch(paths),
            Err(e) => log::warn!("single_instance: forwarded launch unreadable: {e}"),
        }
    }
}

/// Keyed on the data directory rather than the executable: the database and the
/// JSON files are what must not have two writers. Hashing it also keeps the name
/// per-user, which a bare `"melodia"` would not be — the Linux abstract
/// namespace is shared across the whole network namespace, and so is
/// `\\.\pipe\`. Half a digest is far inside that namespace's 107-byte budget.
fn socket_name(data_dir: &Path) -> io::Result<Name<'static>> {
    let digest = blake3::hash(data_dir.as_os_str().as_encoded_bytes());
    format!("melodia-{}.sock", &digest.to_hex()[..32]).to_ns_name::<GenericNamespaced>()
}

fn forward(name: Name<'_>, files: &[PathBuf]) -> io::Result<()> {
    let mut stream = Stream::connect(name)?;
    stream.set_send_timeout(Some(IO_TIMEOUT))?;
    stream.set_recv_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(&encode_frame(files))?;

    // Block until the primary closes, which it does the moment it has the whole
    // frame. Exiting straight after the write would leave the payload in a pipe
    // buffer with this process's handle the last one open — harmless on a unix
    // socket, and the platform where it isn't is the one no Linux runner covers.
    let mut ack = [0_u8; 1];
    let _ = stream.read(&mut ack);
    Ok(())
}

fn read_payload(mut stream: Stream) -> io::Result<Vec<String>> {
    stream.set_recv_timeout(Some(IO_TIMEOUT))?;

    let mut declared = [0_u8; LENGTH_PREFIX_LEN];
    stream.read_exact(&mut declared)?;
    let declared = u32::from_le_bytes(declared);
    if u64::from(declared) > MAX_PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("forwarded launch declared {declared} bytes of paths"),
        ));
    }

    let mut payload = vec![0_u8; declared as usize];
    stream.read_exact(&mut payload)?;
    Ok(decode_paths(&payload))
}

/// `[u32 LE length][NUL-separated lossy UTF-8 paths]`, `services::discord::ipc`'s
/// framing.
///
/// A declared length rather than reading to EOF, because EOF means closing the
/// write half and whichever side waits on the other's close is the side that
/// hangs. NUL separates because no path may contain one; lossy because
/// `library::queue` speaks `String` throughout, as the drop path already does.
fn encode_frame(files: &[PathBuf]) -> Vec<u8> {
    let mut payload = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            payload.push(0);
        }
        payload.extend_from_slice(file.to_string_lossy().as_bytes());
    }

    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_LEN + payload.len());
    frame.extend_from_slice(&length.to_le_bytes());
    frame.append(&mut payload);
    frame
}

fn decode_paths(payload: &[u8]) -> Vec<String> {
    payload
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

#[cfg(test)]
#[path = "tests/single_instance_tests.rs"]
mod tests;
