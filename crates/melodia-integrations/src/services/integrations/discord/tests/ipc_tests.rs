use std::io::{Cursor, ErrorKind};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[cfg(unix)]
use parking_lot::Mutex;
use tokio::sync::watch;

#[cfg(unix)]
use super::super::DiscordPresenceService;
use super::super::DiscordStatus;
use super::super::tests::card;
use super::{Command, MAX_FRAME_LEN, StatusCell, read_frame, run_worker, write_frame};
#[cfg(unix)]
use super::{OP_FRAME, ServeOutcome, payload, serve};
#[cfg(unix)]
use melodia_core::entities::integrations::DiscordFlags;

#[test]
fn frame_round_trips() {
    let mut buf: Vec<u8> = Vec::new();
    assert!(write_frame(&mut buf, 1, b"hello world").is_ok());

    let mut cursor = Cursor::new(buf);
    assert_eq!(read_frame(&mut cursor).ok(), Some((1u32, b"hello world".to_vec())));
}

#[test]
fn empty_body_round_trips() {
    let mut buf: Vec<u8> = Vec::new();
    assert!(write_frame(&mut buf, 0, b"").is_ok());

    let mut cursor = Cursor::new(buf);
    assert_eq!(read_frame(&mut cursor).ok(), Some((0u32, Vec::new())));
}

#[test]
fn oversized_declared_length_is_rejected() {
    // A header claiming a body far larger than the cap, with no body bytes —
    // read_frame must reject on the length alone rather than allocate.
    let mut frame: Vec<u8> = Vec::new();
    frame.extend_from_slice(&1u32.to_le_bytes());
    frame.extend_from_slice(&(MAX_FRAME_LEN + 1).to_le_bytes());

    let mut cursor = Cursor::new(frame);
    let kind = read_frame(&mut cursor).err().map(|e| e.kind());
    assert_eq!(kind, Some(ErrorKind::InvalidData));
}

#[test]
fn truncated_header_errors() {
    // Fewer than the 8 header bytes → read_exact fails.
    let mut cursor = Cursor::new(vec![1u8, 0, 0]);
    assert!(read_frame(&mut cursor).is_err());
}

// --- the worker over a channel, and over a socket with a fake Discord at the other end ---

fn status() -> Arc<StatusCell> {
    let (tx, _) = watch::channel(DiscordStatus::default());
    Arc::new(StatusCell { enabled: AtomicBool::new(false), connected: AtomicBool::new(false), tx })
}

/// A worker the feature was never turned on for opens no socket and ends with its channel, however
/// many cards it was handed meanwhile.
#[test]
fn a_worker_never_enabled_ends_when_its_channel_closes() {
    let (tx, rx) = mpsc::channel();
    for command in [Command::Apply(card()), Command::Clear, Command::Disable] {
        assert!(tx.send(command).is_ok(), "the receiver is alive");
    }
    drop(tx);
    let (ended_tx, ended) = mpsc::channel();
    let probed = Arc::new(AtomicBool::new(false));
    let worker_probed = Arc::clone(&probed);

    let _worker = std::thread::spawn(move || {
        let connect = || {
            worker_probed.store(true, Ordering::Relaxed);
            None
        };
        run_worker(&rx, &status(), &connect);
        let _ = ended_tx.send(());
    });

    // A bound on a failure rather than a wait: the worker returns at once when it is right.
    let ended = ended.recv_timeout(Duration::from_secs(5)).is_ok();
    assert_eq!((ended, probed.load(Ordering::Relaxed)), (true, false), "(ended, probed)");
}

/// What a session wrote, as `(opcode, body)` in order.
#[cfg(unix)]
type Frames = Vec<(u32, Vec<u8>)>;

/// The other end of a socket pair, acking every frame as Discord does and recording it until the
/// worker's end closes.
#[cfg(unix)]
fn fake_discord(mut end: UnixStream) -> std::thread::JoinHandle<Frames> {
    std::thread::spawn(move || {
        let mut frames = Vec::new();
        while let Ok(frame) = read_frame(&mut end) {
            frames.push(frame);
            if write_frame(&mut end, OP_FRAME, b"{}").is_err() {
                break;
            }
        }
        frames
    })
}

/// The frames a session writes and how it ends, serving `commands` with `card` showing and the
/// channel closed behind them.
#[cfg(unix)]
fn session(commands: Vec<Command>) -> std::io::Result<(ServeOutcome, Frames)> {
    let (mut worker_end, discord_end) = UnixStream::pair()?;
    let discord = fake_discord(discord_end);
    let (tx, rx) = mpsc::channel();
    for command in commands {
        assert!(tx.send(command).is_ok(), "the receiver is alive");
    }
    drop(tx);
    let (mut nonce, mut desired, mut enabled) = (0, Some(card()), true);

    let outcome = serve(&mut worker_end, &rx, 7, &mut nonce, &mut desired, &mut enabled);
    drop(worker_end);

    Ok((outcome, discord.join().unwrap_or_default()))
}

/// Turning the feature off takes the card off the profile before the socket goes, rather than
/// leaving Discord to notice the close.
#[cfg(unix)]
#[test]
fn a_disable_with_a_card_showing_clears_it_before_the_session_ends() -> std::io::Result<()> {
    let (outcome, frames) = session(vec![Command::Disable])?;

    let disabled = matches!(outcome, ServeOutcome::Disabled);
    assert_eq!(
        (disabled, frames),
        (true, vec![(OP_FRAME, payload::clear_activity_json(7, "7-1"))]),
        "(ended on the disable, frames written)"
    );
    Ok(())
}

/// A quit drops the channel with the card still up, and the session ends without writing.
#[cfg(unix)]
#[test]
fn a_closed_channel_ends_the_session_without_a_write() -> std::io::Result<()> {
    let (outcome, frames) = session(Vec::new())?;

    let exited = matches!(outcome, ServeOutcome::Exit);
    assert_eq!((exited, frames), (true, Vec::new()), "(ended on the close, frames written)");
    Ok(())
}

// --- the service's workers against a stand-in Discord ---

/// A bound on a failure, never a wait: on a passing run everything below lands in milliseconds.
#[cfg(unix)]
const BOUND: Duration = Duration::from_secs(5);

/// How long a worker reads a held handshake before giving up on it.
#[cfg(unix)]
const HELD_HANDSHAKE: Duration = Duration::from_millis(200);

/// The Discord client every connection a worker opens reaches, counting them.
#[cfg(unix)]
#[derive(Default)]
struct FakeDiscord {
    /// Leaves the first connection's handshake unanswered, so its worker gives up on it after
    /// [`HELD_HANDSHAKE`] and the next connection can ask whether that socket was let go.
    hold_first: bool,
    opened: AtomicUsize,
    held: Mutex<Option<UnixStream>>,
    held_closed_when_next_opened: Mutex<Option<bool>>,
}

#[cfg(unix)]
impl FakeDiscord {
    fn open(&self) -> Option<UnixStream> {
        let (worker_end, mut discord_end) = UnixStream::pair().ok()?;
        let opened_before = self.opened.fetch_add(1, Ordering::SeqCst);
        if let Some(mut held) = self.held.lock().take() {
            *self.held_closed_when_next_opened.lock() = Some(closed(&mut held));
        }
        if self.hold_first && opened_before == 0 {
            worker_end.set_read_timeout(Some(HELD_HANDSHAKE)).ok()?;
            *self.held.lock() = Some(discord_end);
        } else {
            std::thread::spawn(move || {
                while read_frame(&mut discord_end).is_ok() {
                    if write_frame(&mut discord_end, OP_FRAME, b"{}").is_err() {
                        break;
                    }
                }
            });
        }
        Some(worker_end)
    }
}

/// Whether the worker's end of `held` has closed, reading off what it wrote before it did.
#[cfg(unix)]
fn closed(held: &mut UnixStream) -> bool {
    use std::io::Read;

    if held.set_nonblocking(true).is_err() {
        return false;
    }
    let mut written = [0u8; 256];
    loop {
        match held.read(&mut written) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) => return e.kind() == ErrorKind::ConnectionReset,
        }
    }
}

#[cfg(unix)]
fn service_over(fake: &Arc<FakeDiscord>) -> DiscordPresenceService {
    let fake = Arc::clone(fake);
    DiscordPresenceService::init(&DiscordFlags::default(), Arc::new(OnceLock::new()))
        .with_connector(Arc::new(move || fake.open()))
}

#[cfg(unix)]
fn on() -> DiscordFlags {
    DiscordFlags { discord_rpc_enabled: true, ..DiscordFlags::default() }
}

/// Whether a worker reports a connection within the bound.
#[cfg(unix)]
async fn connects(service: &DiscordPresenceService) -> bool {
    let mut status = service.subscribe_status();
    let connected = tokio::time::timeout(BOUND, status.wait_for(|status| status.connected)).await;
    connected.is_ok_and(|seen| seen.is_ok())
}

/// Whether the worker the slot holds ends within the bound.
#[cfg(unix)]
async fn worker_ends(service: &DiscordPresenceService) -> bool {
    let Some(thread) = service.worker.lock().thread.take() else {
        return true;
    };
    let joined = tokio::time::timeout(BOUND, tokio::task::spawn_blocking(move || thread.join()));
    joined.await.is_ok()
}

/// Turned off, the worker closes its socket and ends rather than parking for the session.
#[cfg(unix)]
#[tokio::test]
async fn turning_the_feature_off_ends_its_worker() {
    let fake = Arc::new(FakeDiscord::default());
    let service = service_over(&fake);
    service.set_flags(on());
    assert!(connects(&service).await, "the worker never connected");

    service.set_flags(DiscordFlags::default());

    let ended = worker_ends(&service).await;
    let read = (ended, service.status().connected, service.worker.lock().commands.is_none());
    assert_eq!(read, (true, false, true), "(ended, connected, a channel left)");
}

/// One connection to Discord at a time: a second enable reaches the worker already running.
#[cfg(unix)]
#[tokio::test]
async fn a_second_enable_reuses_the_running_worker() {
    let fake = Arc::new(FakeDiscord::default());
    let service = service_over(&fake);
    service.set_flags(on());
    service.set_flags(on());
    assert!(connects(&service).await, "the worker never connected");

    service.set_flags(DiscordFlags::default());
    assert!(worker_ends(&service).await, "the worker outlived its channel");

    assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
}

/// A worker turned off may still be closing its socket when the feature comes back on, and the new
/// one waits for it, so there is one connection to Discord and one writer of `connected`. The first
/// handshake is held so the old worker is still on its socket when the new one is spawned.
#[cfg(unix)]
#[tokio::test]
async fn a_re_enable_connects_only_after_the_last_worker_closed_its_socket() {
    let fake = Arc::new(FakeDiscord { hold_first: true, ..FakeDiscord::default() });
    let service = service_over(&fake);
    service.set_flags(on());
    service.set_flags(DiscordFlags::default());

    service.set_flags(on());
    let reconnected = connects(&service).await;

    let first_closed = *fake.held_closed_when_next_opened.lock();
    assert_eq!((reconnected, first_closed), (true, Some(true)), "(reconnected, first closed)");
}
