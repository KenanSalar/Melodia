//! The single-instance claim, and the socket a second launch hands its file
//! paths down.
//!
//! **Binding the name *is* the claim** — a failed bind is how a second process
//! learns it is second, atomic where a probe-then-bind races two cold starts.
//! [`name_is_taken`] holds the two spellings that failure has.
//!
//! **Claim early, accept late.** `main()` binds before the logger opens its
//! file; [`serve`] starts accepting only once there is a window to raise.
//! Connections wait in the listen backlog in between, so nothing is lost.
//!
//! Blocking `std` sockets on a dedicated thread, as `services::discord::ipc`
//! runs its transport: the claim predates the runtime, and a parked
//! `spawn_blocking` task would hold one of the 32 slots `main()` caps at. The
//! accept thread only accepts — reading a connection is [`spawn_reader`]'s, for
//! the same reason.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, Stream, prelude::*};

/// Re-exported so `boot` can name what [`Claim::Primary`] carries without
/// depending on `interprocess` itself.
pub use interprocess::local_socket::Listener;

use crate::utils::audio_ext::is_audio_extension;

/// Checked against the *declared* length before a buffer is sized for it, so a
/// hostile peer gets no allocation out of asking for more. A thousand paths fit.
const MAX_PAYLOAD_LEN: u64 = 64 * 1024;

const LENGTH_PREFIX_LEN: usize = size_of::<u32>();

/// How long a peer that connects and then says nothing is given, where the transport takes a
/// deadline at all.
///
/// One doesn't — a Windows named pipe refuses outright ([`allow_missing_timeout`]) — so this
/// bounds nothing there and [`spawn_reader`] carries the guarantee instead.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Reads in flight, past which a connection is answered without being read.
///
/// One stalled peer costs a parked thread rather than every launch after it, and a flood of them
/// costs a bounded number — the socket is reachable by any local process, and this is a
/// memory-disciplined app to hand an unbounded thread count.
///
/// A slot comes back on [`IO_TIMEOUT`] only where the transport honours one. On a named pipe the
/// read has no deadline to give up on, so a peer that holds its handle open holds the slot for the
/// session; eight of those and every later launch raises a bare window rather than opening its
/// file. Bounding that in turn would mean `set_nonblocking` and a sleep-poll — a busy loop traded
/// for a parked thread, against an attacker who is already a local process.
const MAX_CONCURRENT_READS: usize = 8;

/// A listener that has stopped working errors as fast as the loop can ask, so
/// give up rather than spin.
const MAX_CONSECUTIVE_ACCEPT_FAILURES: u32 = 16;

/// Set on the child of a *detached* restart, which then waits for the name
/// instead of forwarding to it: `shutdown::spawn_detached` leaves parent and
/// child alive together, and a child that forwarded and exited there would go
/// down with the parent behind it.
///
/// Not set on the `exec` arm, which needs no wait — CLOEXEC frees the name at
/// the image replace — and where it would outlive its purpose, inherited by
/// every process the restarted Melodia goes on to spawn.
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
            Err(e) if name_is_taken(&e) => e,
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

/// Whether a bind failed because another Melodia already holds the name.
///
/// Two spellings, because `interprocess` hands both back as it found them. Unix
/// `bind` says `EADDRINUSE`; a Windows named pipe is created under
/// `FILE_FLAG_FIRST_PIPE_INSTANCE`, whose second instance fails with
/// `ERROR_ACCESS_DENIED` — `PermissionDenied` to std, and nowhere near
/// `AddrInUse`. Reading only the first left *every* Windows launch
/// [`Claim::Unenforced`], on the one platform the MSI registers file
/// associations for, and no Linux runner can see it.
///
/// A real ACL denial takes the same arm and fails at the connect instead,
/// landing back on `Unenforced` — the graceful outcome either way.
///
/// Spells `cfg!(windows)` once, over [`name_is_taken_on`]; the split is what
/// makes the Windows answer reachable from the only runner that runs tests.
fn name_is_taken(e: &io::Error) -> bool {
    name_is_taken_on(e, cfg!(windows))
}

/// The pure half of [`name_is_taken`], as `services::redact_prefix` is `redact_home`'s. A `cfg!`
/// inside the predicate is a branch CI compiles out and can never exercise, so the platform
/// arrives as an argument instead — otherwise a "simplification" back to one spelling merges green
/// on a Linux-only gate.
fn name_is_taken_on(e: &io::Error, windows: bool) -> bool {
    e.kind() == io::ErrorKind::AddrInUse || (windows && e.kind() == io::ErrorKind::PermissionDenied)
}

/// Accept forwarded launches for the rest of the process's life. `on_launch`
/// gets each one's paths — empty when someone simply started Melodia again,
/// which still wants the window raised.
/// `Sync` because each connection is read on its own thread — see [`spawn_reader`].
pub fn serve(listener: Listener, on_launch: impl Fn(Vec<String>) + Send + Sync + 'static) {
    let spawned = std::thread::Builder::new()
        .name("melodia-open".to_owned())
        .spawn(move || accept_loop(&listener, on_launch));

    if let Err(e) = spawned {
        log::warn!("single_instance: no accept thread, forwarded launches will be dropped: {e}");
    }
}

fn accept_loop(listener: &Listener, on_launch: impl Fn(Vec<String>) + Send + Sync + 'static) {
    let mut consecutive_failures = 0_u32;
    let on_launch = Arc::new(on_launch);
    let reads_in_flight = Arc::new(AtomicUsize::new(0));

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

        spawn_reader(stream, &on_launch, &reads_in_flight);
    }
}

/// Read one forwarded launch, off the accept loop.
///
/// The read has no portable deadline — a Windows named pipe takes none — so the peer that
/// connects and then says nothing is held off a thread of its own rather than a timeout.
/// Parking one costs a thread; parking [`MAX_CONCURRENT_READS`] of them costs the paths, never
/// the accept loop.
///
/// **Every arm still calls `on_launch`.** The connection is itself proof of a launch, so a
/// selection past [`MAX_PAYLOAD_LEN`], a peer that stalled past [`IO_TIMEOUT`], and a read we
/// declined to start should all land the user in a window rather than in nothing at all.
fn spawn_reader<F>(stream: Stream, on_launch: &Arc<F>, reads_in_flight: &Arc<AtomicUsize>)
where
    F: Fn(Vec<String>) + Send + Sync + 'static,
{
    let Some(slot) = ReadSlot::take(reads_in_flight) else {
        log::warn!("single_instance: {MAX_CONCURRENT_READS} launches unread; raising bare");
        on_launch(Vec::new());
        return;
    };

    let reader = {
        let on_launch = Arc::clone(on_launch);
        std::thread::Builder::new().name("melodia-read".to_owned()).spawn(move || {
            let paths = read_payload(stream).unwrap_or_else(|e| {
                log::warn!("single_instance: forwarded launch unreadable: {e}");
                Vec::new()
            });
            // The read is what the cap counts; `on_launch` only schedules.
            drop(slot);
            on_launch(paths);
        })
    };

    // The slot went into the closure, which a failed spawn drops for us.
    if let Err(e) = reader {
        log::warn!("single_instance: no reader thread: {e}");
        on_launch(Vec::new());
    }
}

/// One read's place under [`MAX_CONCURRENT_READS`], given back on drop.
///
/// A trailing `fetch_sub` would be equivalent right up to the read that panics, and a slot lost
/// that way is lost for the process's life — [`MAX_CONCURRENT_READS`] of them and forwarding stops
/// working with nothing to see for it.
struct ReadSlot(Arc<AtomicUsize>);

impl ReadSlot {
    /// `None` once the cap is met, with the count left where it was found.
    fn take(reads_in_flight: &Arc<AtomicUsize>) -> Option<Self> {
        if reads_in_flight.fetch_add(1, Ordering::Relaxed) < MAX_CONCURRENT_READS {
            return Some(Self(Arc::clone(reads_in_flight)));
        }
        reads_in_flight.fetch_sub(1, Ordering::Relaxed);
        None
    }
}

impl Drop for ReadSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
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

/// Apply an I/O deadline, tolerating a transport that has none.
///
/// The timeouts guard against a peer that connects and then stops reading, and every
/// transport but one honours them: `interprocess`'s Windows named pipes refuse outright
/// with `ErrorKind::Unsupported`. Propagating that is far worse than going without, because
/// [`claim`] reads a failed [`forward`] as `Unenforced` and boots a **second** Melodia — so
/// the launch that should have handed its files over opens a second window and a second
/// writer onto one database, on the platform whose installer registers the file
/// associations. Every other failure still propagates.
///
/// What the deadline was *for* doesn't go away with it — [`spawn_reader`] and
/// [`wait_for_close`] hold that end, and hold it on every transport.
fn allow_missing_timeout(applied: io::Result<()>) -> io::Result<()> {
    match applied {
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Ok(()),
        other => other,
    }
}

fn forward(name: Name<'_>, files: &[PathBuf]) -> io::Result<()> {
    let mut stream = Stream::connect(name)?;
    allow_missing_timeout(stream.set_send_timeout(Some(IO_TIMEOUT)))?;
    allow_missing_timeout(stream.set_recv_timeout(Some(IO_TIMEOUT)))?;
    stream.write_all(&encode_frame(files))?;

    wait_for_close(stream);
    Ok(())
}

/// Block until the primary closes, which it does the moment it has the whole frame.
///
/// Returning straight after the write would leave the payload in a pipe buffer with this
/// process's handle the last one open — harmless on a unix socket, and the platform where it
/// isn't is the one no Linux runner covers.
///
/// Bounded off a thread rather than by [`IO_TIMEOUT`], which the same named pipe won't take.
/// Unbounded, a primary that is merely wedged parks this process here for good — no window of
/// its own is coming ([`Claim::Secondary`] is a bare return), so what the user is left with is a
/// Melodia that shows nothing and never exits, and a file manager still waiting on it. Giving up
/// early is safe: the frame is already written, and the wait only buys the primary time to drain
/// it.
fn wait_for_close(mut stream: Stream) {
    let (closed_tx, closed_rx) = mpsc::channel();
    let waiter = std::thread::Builder::new().name("melodia-forward".to_owned()).spawn(move || {
        let mut ack = [0_u8; 1];
        let _ = stream.read(&mut ack);
        let _ = closed_tx.send(());
    });

    // A thread we couldn't start is the one case worth *not* waiting for: nothing will ever
    // send, and the write has already landed.
    if waiter.is_ok() {
        let _ = closed_rx.recv_timeout(IO_TIMEOUT);
    }
}

fn read_payload(mut stream: Stream) -> io::Result<Vec<String>> {
    allow_missing_timeout(stream.set_recv_timeout(Some(IO_TIMEOUT)))?;

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
