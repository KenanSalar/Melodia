//! Opening a stream, and the reader the decoder sees instead of the scheduler behind it.
//!
//! Two halves. The reader itself is driven over a channel this module fills, since a test is a
//! child of `reader` and can seat one; what that buys is the end-of-stream and reassembly
//! behaviour without a socket. [`open`] is driven against a loopback server, because the question
//! there is which segment a client tuning in starts on, and the numbers deciding that are
//! `LIVE_EDGE_SEGMENTS` and the playlist's own length.
//!
//! One turn of the scheduler is here and the loop around it is not: `drain` is a plain `async fn`
//! over `&mut self` that a test can seat and await, where `run` is a `select!` over a sleep and a
//! closed channel and stepping it deterministically wants a seam the tree does not have yet.

use melodia_testkit::http::{TestResponse, TestServer};

use super::*;

/// A reader over `chunks`, with the sender handed back so a test can end the stream by dropping
/// it. Reachable because this module is a child of `reader`; nothing outside it can seat one.
fn reader_over(depth: usize) -> (HlsReader, mpsc::Sender<Vec<u8>>) {
    let (sender, chunks) = mpsc::channel(depth);
    let reader = HlsReader {
        chunks,
        held: Vec::new(),
        offset: 0,
        position: 0,
        shared: StreamShared::new(),
    };
    (reader, sender)
}

/// The playlist the server answers on, and what `open` is handed as the already-fetched body.
const PLAYLIST_PATH: &str = "/live.m3u8";

/// A media playlist naming `count` segments, each served by [`segment_body`].
fn media_playlist(count: usize) -> String {
    let mut lines = vec![
        "#EXTM3U".to_owned(),
        "#EXT-X-VERSION:3".to_owned(),
        "#EXT-X-TARGETDURATION:6".to_owned(),
    ];
    for index in 0..count {
        lines.push("#EXTINF:6.0,".to_owned());
        lines.push(format!("seg-{index}.aac"));
    }
    lines.join("\n") + "\n"
}

/// Distinct per segment, and deliberately not audio: `SegmentReader` passes packed bytes through
/// untouched, so the reader's output names which segment it started on.
fn segment_body(index: usize) -> String {
    format!("SEG-{index}")
}

/// Serve the playlist and every `seg-N.aac` the segment URIs resolve to beside it.
fn serve_playlist(body: String) -> std::io::Result<TestServer> {
    TestServer::start(move |request| {
        if request.path == PLAYLIST_PATH {
            return TestResponse::ok(body.clone());
        }
        TestResponse::ok(segment_at(&request.path))
    })
}

/// Whichever segment `path` names, so a test can tell which one the client asked for.
fn segment_at(path: &str) -> String {
    let index = path
        .strip_prefix("/seg-")
        .and_then(|rest| rest.strip_suffix(".aac"))
        .and_then(|index| index.parse::<usize>().ok());
    index.map_or_else(|| "UNSERVED".to_owned(), segment_body)
}

/// Read the whole primed chunk, which is what `open` staged before it spawned the scheduler.
fn read_primed(stream: &mut HlsStream) -> String {
    let mut buf = [0_u8; 64];
    let read = stream.reader.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..read]).into_owned()
}

async fn open_against(server: &TestServer, body: &str) -> Result<HlsStream, AppError> {
    let Ok(url) = Url::parse(&format!("{}{PLAYLIST_PATH}", server.base_url())) else {
        unreachable!("the server's own base is a parseable URL")
    };
    open(&reqwest::Client::new(), &url, body, StreamShared::new()).await
}

/// A client tuning in starts a cushion back from the live edge, not at the top of the window:
/// the segments before that are history, and starting on them plays yesterday before today.
#[tokio::test]
async fn a_client_starts_a_cushion_back_from_the_live_edge() -> Result<(), AppError> {
    let body = media_playlist(6);
    let server = serve_playlist(body.clone())?;

    let mut stream = open_against(&server, &body).await?;

    assert_eq!(read_primed(&mut stream), segment_body(6 - LIVE_EDGE_SEGMENTS));
    Ok(())
}

/// A window shorter than the cushion has no history to skip, so it starts at the beginning
/// rather than saturating past its own last segment.
#[tokio::test]
async fn a_window_shorter_than_the_cushion_starts_at_its_first_segment() -> Result<(), AppError> {
    let body = media_playlist(2);
    let server = serve_playlist(body.clone())?;

    let mut stream = open_against(&server, &body).await?;

    assert_eq!(read_primed(&mut stream), segment_body(0));
    Ok(())
}

/// An empty window fails at the open, where the station can still be reported as unreachable,
/// rather than at the first read with a deck already staged.
#[tokio::test]
async fn a_playlist_naming_no_segments_fails_the_open() -> Result<(), AppError> {
    let body = media_playlist(0);
    let server = serve_playlist(body.clone())?;

    let opened = open_against(&server, &body).await;

    assert!(matches!(opened, Err(AppError::Network { .. })));
    Ok(())
}

/// A fragmented stream's `EXT-X-MAP` is the header its fragments are meaningless without, so it
/// has to arrive ahead of the first one and not as a segment of its own.
#[tokio::test]
async fn the_init_header_arrives_ahead_of_the_first_segment() -> Result<(), AppError> {
    let body = format!("{}#EXT-X-MAP:URI=\"init.mp4\"\n", media_playlist(1));
    let served = body.clone();
    let server = TestServer::start(move |request| match request.path.as_str() {
        PLAYLIST_PATH => TestResponse::ok(served.clone()),
        "/init.mp4" => TestResponse::ok("HEADER"),
        path => TestResponse::ok(segment_at(path)),
    })?;

    let mut stream = open_against(&server, &body).await?;

    assert_eq!(read_primed(&mut stream), format!("HEADER{}", segment_body(0)));
    Ok(())
}

/// The variant a master playlist resolves to carries its own bitrate, which is the only place
/// the station's own numbers come from once the directory's are gone.
#[tokio::test]
async fn a_master_playlist_carries_the_variant_bitrate() -> Result<(), AppError> {
    let media = media_playlist(1);
    let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.2\"\naudio.m3u8\n";
    let server = TestServer::start(move |request| match request.path.as_str() {
        "/audio.m3u8" => TestResponse::ok(media.clone()),
        PLAYLIST_PATH => TestResponse::ok(master),
        path => TestResponse::ok(segment_at(path)),
    })?;

    let stream = open_against(&server, master).await?;

    assert_eq!(stream.bitrate_kbps, 64);
    Ok(())
}

/// A chunk boundary is not a frame boundary, so a read that spans two of them owes the caller
/// bytes from the first before it touches the second.
#[test]
fn a_read_stops_at_the_chunk_it_is_holding() {
    let (mut reader, sender) = reader_over(2);
    assert!(sender.try_send(b"abc".to_vec()).is_ok());
    assert!(sender.try_send(b"de".to_vec()).is_ok());

    let mut buf = [0_u8; 8];
    assert_eq!(reader.read(&mut buf).ok(), Some(3));
    assert_eq!(&buf[..3], b"abc");
    assert_eq!(reader.read(&mut buf).ok(), Some(2));
    assert_eq!(&buf[..2], b"de");
}

/// The scheduler going away ends the stream rather than parking the feed thread on a channel
/// nothing will ever fill again.
#[test]
fn a_dropped_scheduler_ends_the_stream() {
    let (mut reader, sender) = reader_over(1);
    assert!(sender.try_send(b"tail".to_vec()).is_ok());
    drop(sender);

    let mut buf = [0_u8; 8];
    assert_eq!(reader.read(&mut buf).ok(), Some(4), "queued audio outlives the sender");
    assert_eq!(reader.read(&mut buf).ok(), Some(0), "and then the stream is over");
}

/// The only seek a live source ever sees is the one asking where it has got to. Answering
/// anything else would let the decoder believe it can rewind a stream with no history.
#[test]
fn a_live_stream_reports_its_position_and_refuses_to_move() {
    let (mut reader, sender) = reader_over(1);
    assert!(sender.try_send(b"abcd".to_vec()).is_ok());

    let mut buf = [0_u8; 4];
    assert_eq!(reader.read(&mut buf).ok(), Some(4));

    assert_eq!(reader.stream_position().ok(), Some(4));
    assert_eq!(
        reader.seek(SeekFrom::Start(0)).map_err(|e| e.kind()),
        Err(io::ErrorKind::Unsupported),
    );
}

// --- One turn of the scheduler ----------------------------------------------
//
// `drain` is a plain `async fn` over `&mut self`, so it needs none of the seam `run` does: a test
// seats a `Scheduler` and awaits one turn against the loopback server. What it decides is which
// segments are asked for and how long to wait before asking again, and every one of those
// failures is silent — a station that plays, or plays and then quietly stops.

/// Segments a case can leave unread. Deliberately not `SEGMENT_QUEUE_DEPTH`: the cushion is not
/// what any of these are about, and a scheduler that starts fetching more than it should has to
/// fail an assertion rather than park on a full channel with nobody draining it.
const UNREAD_SEGMENTS: usize = 64;

/// A scheduler pointed at `server`, already `next_sequence` segments in.
fn scheduler_over(server: &TestServer, next_sequence: u64) -> (Scheduler, mpsc::Receiver<Vec<u8>>) {
    let (chunks, received) = mpsc::channel(UNREAD_SEGMENTS);
    let Ok(playlist_url) = Url::parse(&format!("{}{PLAYLIST_PATH}", server.base_url())) else {
        unreachable!("the server's own base is a parseable URL")
    };
    let scheduler = Scheduler {
        client: reqwest::Client::new(),
        playlist_url,
        segments: SegmentReader::default(),
        next_sequence,
        refresh: Duration::ZERO,
        stalled: 0,
        chunks,
    };
    (scheduler, received)
}

/// A reload naming `indices`, the first of them numbered `media_sequence`.
///
/// Built rather than parsed so the sequence number is the test's to choose: it is the only handle
/// on how far behind the window the client is, and a playlist text would have to spell it.
fn reload(server: &TestServer, media_sequence: u64, indices: &[usize]) -> MediaPlaylist {
    let segments = indices
        .iter()
        .filter_map(|index| Url::parse(&format!("{}/seg-{index}.aac", server.base_url())).ok())
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), indices.len(), "every segment URI has to parse");
    MediaPlaylist {
        target_duration: Duration::from_secs(6),
        media_sequence,
        segments,
        init_segment: None,
        ended: false,
    }
}

/// A server that serves every segment but the second, which it refuses.
fn refusing_segment_one() -> std::io::Result<TestServer> {
    TestServer::start(|request| {
        if request.path == "/seg-1.aac" {
            return TestResponse::status(503);
        }
        TestResponse::ok(segment_at(&request.path))
    })
}

/// Everything the scheduler pulled, in order.
fn drained(received: &mut mpsc::Receiver<Vec<u8>>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(chunk) = received.try_recv() {
        out.push(String::from_utf8_lossy(&chunk).into_owned());
    }
    out
}

/// A client that fell behind the window skips the gap rather than catching up on it: the
/// segments it missed are minutes of a live stream nobody wants played late, and fetching them
/// means never reaching the edge. A version starting from `media_sequence` plays yesterday.
#[tokio::test]
async fn a_reload_that_moved_past_the_client_is_joined_at_the_client_s_own_place() {
    let Ok(server) = serve_playlist(media_playlist(0)) else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, mut received) = scheduler_over(&server, 5);

    assert!(scheduler.drain(&reload(&server, 0, &[0, 1, 2, 3, 4, 5, 6])).await);

    assert_eq!(drained(&mut received), ["SEG-5", "SEG-6"]);
    assert_eq!(scheduler.next_sequence, 7, "the next reload starts after the last one taken");
}

/// One refused segment is a gap the demuxer resyncs past, not the end of the reload.
#[tokio::test]
async fn a_segment_the_server_refused_does_not_cost_the_ones_after_it() {
    let Ok(server) = refusing_segment_one() else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, mut received) = scheduler_over(&server, 0);

    assert!(scheduler.drain(&reload(&server, 0, &[0, 1, 2])).await);

    assert_eq!(drained(&mut received), ["SEG-0", "SEG-2"]);
}

/// The sequence moves past a refused segment as readily as past a taken one, and the case has to
/// put the refusal last or a later success carries the number past it anyway. Advancing only on
/// success re-asks the same dead URL on every reload for the rest of the session, which reads
/// from the outside as a station that is merely slow.
#[tokio::test]
async fn a_segment_the_server_refused_is_not_asked_for_a_second_time() {
    let Ok(server) = refusing_segment_one() else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, _received) = scheduler_over(&server, 0);
    let playlist = reload(&server, 0, &[0, 1]);

    assert!(scheduler.drain(&playlist).await);
    assert!(scheduler.drain(&playlist).await);

    let refused = server.requests().iter().filter(|r| r.path == "/seg-1.aac").count();
    assert_eq!(refused, 1, "the second reload asked for it again");
}

/// A reload that brought audio is the healthy case: wait the period the playlist names, and put
/// the stall count back to nothing.
#[tokio::test]
async fn a_reload_that_brought_audio_waits_the_full_period_and_clears_the_stall() {
    let Ok(server) = serve_playlist(media_playlist(0)) else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, _received) = scheduler_over(&server, 0);
    scheduler.stalled = 4;

    assert!(scheduler.drain(&reload(&server, 0, &[0])).await);

    assert_eq!(scheduler.refresh, Duration::from_secs(6));
    assert_eq!(scheduler.stalled, 0, "audio arrived, so nothing is stalling");
}

/// The other arm, and the one that decides when a station is declared dead. Half a period is what
/// the spec asks of a client that has caught up with a playlist still being written, so folding
/// the two rates together either doubles the latency or doubles the request rate.
#[tokio::test]
async fn a_reload_that_brought_nothing_waits_half_a_period_and_counts_toward_the_stall() {
    let Ok(server) = serve_playlist(media_playlist(0)) else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, _received) = scheduler_over(&server, 9);

    // Every segment in it is behind the client, so the loop fetches nothing.
    assert!(scheduler.drain(&reload(&server, 0, &[0, 1])).await);

    assert_eq!(scheduler.refresh, Duration::from_secs(3));
    assert_eq!(scheduler.stalled, 1);
}

/// The listener going away is not the stream ending, and only `false` tells `run` which it was:
/// answering `true` leaves the loop reloading a playlist for a decoder that has been dropped.
#[tokio::test]
async fn a_drain_with_nobody_listening_reports_that_it_is_over() {
    let Ok(server) = serve_playlist(media_playlist(0)) else {
        unreachable!("the loopback listener is the test's own")
    };
    let (mut scheduler, received) = scheduler_over(&server, 0);
    drop(received);

    assert!(!scheduler.drain(&reload(&server, 0, &[0])).await);
}
