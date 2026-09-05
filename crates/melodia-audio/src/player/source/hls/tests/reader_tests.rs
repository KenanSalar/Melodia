//! Opening a stream, and the reader the decoder sees instead of the scheduler behind it.
//!
//! Two halves. The reader itself is driven over a channel this module fills, since a test is a
//! child of `reader` and can seat one; what that buys is the end-of-stream and reassembly
//! behaviour without a socket. [`open`] is driven against a loopback server, because the question
//! there is which segment a client tuning in starts on, and the numbers deciding that are
//! `LIVE_EDGE_SEGMENTS` and the playlist's own length.
//!
//! The scheduler loop is not here. It runs on the runtime behind a channel the reader parks on,
//! and stepping it deterministically wants a seam the tree does not have yet.

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
