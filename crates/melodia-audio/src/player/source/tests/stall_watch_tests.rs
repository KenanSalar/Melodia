//! When a live connection is ended underneath the feed thread.
//!
//! **Paused clock throughout**, which is safe here only because nothing opens a socket: the watch
//! measures quiet on tokio's clock, so a paused one walks the whole timeout in no wall time.
//! Progress is reported by storing into `delivered`, the cell stream-download's progress callback
//! writes, since `Settings` keeps that callback where nothing outside the crate can call it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{STALL_TIMEOUT, StallWatch};
use crate::player::source::audio::SourceFormat;
use crate::player::source::prebuffer::{PrebufferSource, RingWriter, StreamShared};
use crate::player::source::stream_source::ABANDON_POLL;
use crate::player::source::tests::helpers::shape;

/// A poll either side of the timeout: the watch notices quiet only when it polls, so this is the
/// tightest window it can be held to.
const JUST_SHORT: Duration = STALL_TIMEOUT.saturating_sub(ABANDON_POLL);
const JUST_PAST: Duration = STALL_TIMEOUT.saturating_add(ABANDON_POLL);

/// A watch over one connection, and what a test drives it through. The source has to be held for
/// as long as the station counts as playing, its drop being what abandons the stream.
struct Watched {
    token: CancellationToken,
    delivered: Arc<AtomicU64>,
    source: PrebufferSource,
    _writer: RingWriter,
}

fn watching() -> Watched {
    let shared = StreamShared::new();
    let (source, writer) =
        PrebufferSource::new(shared.clone(), shape(2, 48_000), SourceFormat::F32);
    let watch = StallWatch::new();
    let delivered = Arc::clone(&watch.delivered);
    let token = CancellationToken::new();
    watch.spawn(token.clone(), shared);
    Watched { token, delivered, source, _writer: writer }
}

/// Long enough to ride out a drop TCP survives: a connection is not ended for a gap shorter than
/// the timeout.
#[tokio::test(start_paused = true)]
async fn a_quiet_connection_is_kept_until_the_timeout_runs_out() {
    let watched = watching();

    let cut = timeout(JUST_SHORT, watched.token.cancelled()).await;

    assert!(cut.is_err(), "the connection was dropped before it had been quiet for the timeout");
}

/// Within one poll of the timeout, so the reconnect starts while the decoded ring still holds
/// audio.
#[tokio::test(start_paused = true)]
async fn a_quiet_connection_is_dropped_within_a_poll_of_the_timeout() {
    let watched = watching();

    let cut = timeout(JUST_PAST, watched.token.cancelled()).await;

    assert!(cut.is_ok(), "a connection quiet for the whole timeout was left open");
}

/// Quiet is measured from the last write, not from the connect, or every station would be cut
/// one timeout after it started playing.
#[tokio::test(start_paused = true)]
async fn a_write_restarts_the_quiet_clock() {
    let watched = watching();
    // On a paused clock this walks the watch through its polls rather than waiting on anything.
    tokio::time::sleep(JUST_SHORT).await;
    watched.delivered.store(1, Ordering::Relaxed);

    let cut = timeout(JUST_SHORT, watched.token.cancelled()).await;

    assert!(cut.is_err(), "a connection still delivering was dropped");
}

/// The cancel is the only thing that wakes a feed thread parked on a read, so a station stopped
/// mid-outage lets go of its socket here or not until the network comes back.
#[tokio::test(start_paused = true)]
async fn a_dropped_source_ends_its_connection_without_waiting_out_the_timeout() {
    let watched = watching();

    drop(watched.source);
    let cut = timeout(ABANDON_POLL * 2, watched.token.cancelled()).await;

    assert!(cut.is_ok(), "the source is gone and its connection is still open");
}

#[tokio::test(start_paused = true)]
async fn the_watch_lets_go_once_the_download_ends_first() {
    let token = CancellationToken::new();
    let watch = tokio::spawn(StallWatch::new().run(token.clone(), StreamShared::new()));

    token.cancel();
    let ended = timeout(ABANDON_POLL, watch).await;

    assert!(
        matches!(ended, Ok(Ok(()))),
        "a watch over a finished download kept polling, one per reconnect"
    );
}
