//! Turning a station's URL into something the decks can play.
//!
//! Three jobs, in the order a station meets them: follow a playlist URL to the audio behind it,
//! open that as a seekable reader Symphonia will accept, and keep it fed from a thread of its own.
//! The ring that thread writes into is [`super::prebuffer`], which argues why the audio callback
//! must never see a socket.
//!
//! **Reconnect lives here rather than in the playback monitor.** The feed thread already holds the
//! URL, the client and the ring, so when its decoder ends it re-opens and keeps filling the *same*
//! ring: the source never ends, the deck never blinks, and the state machine needs no
//! reconnect path at all. Only once the attempt budget is spent, or a server comes back with a
//! format the already-appended source cannot carry, does the thread give up and let the deck
//! drain — which the monitor reads as the end of the station.
//!
//! Nothing here logs a stream URL. They routinely carry a session token in the query string, and
//! `services::diagnostics` puts the log tail in front of a public GitHub issue.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use icy_metadata::{IcyHeaders, IcyMetadataReader, RequestIcyMetadata};
use reqwest::Url;
use stream_download::http::{Client as StreamClient, HttpStream, format_range_header_bytes};
use stream_download::storage::bounded::BoundedStorageProvider;
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};

use crate::error::AppError;
use crate::error::describe;

use super::audio::Shape;
use super::hls;
use super::prebuffer::{PrebufferSource, RingWriter, StreamShared};
use super::stream_decode::{LiveSource, StreamDecoder};

/// The circular buffer between the socket and the decoder, in compressed bytes.
///
/// This is the *network* cushion, the one that decides how long a stalled connection can go
/// unnoticed. It is sized in bytes rather than seconds because that is what the layer takes, and a
/// byte budget buys wildly different amounts of audio across the bitrates stations advertise —
/// which is the right way round, since a high-bitrate station is also the one whose stall costs
/// most to ride out.
const DOWNLOAD_BUFFER_BYTES: NonZeroUsize = match NonZeroUsize::new(512 * 1024) {
    Some(bytes) => bytes,
    None => NonZeroUsize::MIN,
};

/// How much of the stream to pull down before the decoder is allowed to read.
///
/// The floor and ceiling exist because the real figure is derived per station from the bitrate the
/// server states: too little and the first seconds stutter, too much and the station takes
/// noticeably long to start. The fallback covers a server that states no bitrate at all.
const PREFETCH_MIN_BYTES: u64 = 32 * 1024;
const PREFETCH_MAX_BYTES: u64 = 128 * 1024;
const PREFETCH_FALLBACK_BYTES: u64 = 64 * 1024;
/// How many seconds of audio the prefetch aims at, where the bitrate is known.
const PREFETCH_SECONDS: u64 = 2;

/// How many times the feed thread re-opens a stream that ended before giving up.
///
/// Bounded rather than endless because a station that has stopped broadcasting is indistinguishable
/// from one whose network is flaky, and the honest answer to the first is to say so and stop.
const RECONNECT_ATTEMPTS: u32 = 5;
/// The first backoff step; each attempt doubles it up to [`RECONNECT_MAX_DELAY`].
const RECONNECT_BASE_SECS: u64 = 1;
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(16);
/// Granularity at which a blocking wait notices the source was dropped. Shared with
/// [`super::hls`], whose reader parks on the same question from the same thread.
pub(super) const ABANDON_POLL: Duration = Duration::from_millis(100);

/// Whole-request ceiling for fetching a playlist file, which is a small text document nobody
/// should be waiting on. The audio request itself is deliberately unbounded — it never ends.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of a playlist body to read before refusing it. A `.pls` or `.m3u` pointing at a
/// station is a few hundred bytes; anything past this is not the document we were promised.
const PLAYLIST_MAX_BYTES: u64 = 64 * 1024;
/// URL extensions that mean "this is a pointer, not audio".
const PLAYLIST_EXTENSIONS: [&str; 4] = ["pls", "m3u", "m3u8", "asx"];
/// Response content types that mean the same, for the mounts that carry no extension.
const PLAYLIST_CONTENT_TYPES: [&str; 7] = [
    "audio/x-mpegurl",
    "audio/mpegurl",
    "application/x-mpegurl",
    "application/vnd.apple.mpegurl",
    "audio/x-scpls",
    "application/pls+xml",
    "video/x-ms-asf",
];

/// The reader stack a station is decoded from: metadata blocks stripped out of a bounded circular
/// download buffer over an HTTP response.
type StreamReader =
    IcyMetadataReader<StreamDownload<BoundedStorageProvider<MemoryStorageProvider>>>;

/// The shared `reqwest::Client`, wearing the one header `stream-download` gives no other way to
/// send.
///
/// `Icy-MetaData: 1` is what makes a server interleave track titles into the audio, and
/// `RequestIcyMetadata` is implemented only on `reqwest::ClientBuilder` and `RequestBuilder`.
/// stream-download's [`StreamClient::get`] is `self.get(url).send()` with nowhere to hand a
/// `RequestBuilder` in, so the choice is a newtype or a second `reqwest::Client` — and a second
/// client means a second connection pool and the loss of the `Melodia/<version>` User-Agent that
/// some Icecast servers gate on.
struct IcyClient(reqwest::Client);

impl StreamClient for IcyClient {
    type Url = Url;
    type Headers = reqwest::header::HeaderMap;
    type Response = reqwest::Response;
    type Error = reqwest::Error;

    /// Only reachable through `StreamDownload::new`/`new_http`, which this tree never calls
    /// (pinned by `nothing_reaches_the_convenience_constructors`): every open goes through
    /// [`HttpStream::new`] with the shared client. Delegating to reqwest's own impl hands back
    /// stream-download's internal default rather than building a third pool.
    fn create() -> Self {
        Self(<reqwest::Client as StreamClient>::create())
    }

    async fn get(&self, url: &Self::Url) -> Result<Self::Response, Self::Error> {
        self.0.get(url.clone()).request_icy_metadata().send().await
    }

    /// The reconnect path. It carries the header for the same reason [`Self::get`] does: without
    /// it a station keeps playing after a mid-stream reconnect but stops naming its tracks, which
    /// is the kind of fault nobody traces back to a missing header.
    async fn get_range(
        &self,
        url: &Self::Url,
        start: u64,
        end: Option<u64>,
    ) -> Result<Self::Response, Self::Error> {
        self.0
            .get(url.clone())
            .header(reqwest::header::RANGE, format_range_header_bytes(start, end))
            .request_icy_metadata()
            .send()
            .await
    }
}

/// An opened stream, before it has a ring or a thread.
struct OpenedStream {
    decoder: StreamDecoder,
    shape: Shape,
    facts: StationFacts,
}

/// What a server said about itself while the stream was being opened.
///
/// Every field is already parsed on the way to playback and thrown away, so carrying them out
/// costs nothing and is what lets [`probe`] describe a hand-typed URL. `logo_url` and `homepage`
/// are the two Icecast fields a directory row would otherwise be the only source of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationFacts {
    pub name: Option<String>,
    pub genre: String,
    pub homepage: Option<String>,
    pub logo_url: Option<String>,
    /// A short display token — `MP3`, `AAC` — off the response's content type, in the directory's
    /// own spelling for its `codec` column. Empty where the server sent a type we have no name for.
    pub codec: String,
    /// Advertised kbps, `0` where the server does not say. Same display hint as the directory's.
    pub bitrate: i32,
    /// Whether what answered was a segment playlist rather than one endless response. Carried so a
    /// hand-typed station records the same fact the directory's own `hls` column states.
    pub hls: bool,
}

/// What was actually behind a URL. A mount with no extension can only be told apart from a pointer
/// at it by what the server says it is, so [`connect`] reports that rather than guessing from the
/// shape of a failure.
enum Opened {
    Audio(Box<OpenedStream>),
    Playlist,
}

/// What a playlist body turned out to name.
enum Followed {
    /// A segment playlist, already opened: the playlist *is* the station, and there is no further
    /// URL to chase.
    Segments(Box<OpenedStream>),
    /// A pointer at one audio mount.
    Mount(Url),
}

/// How the feed thread gets back to a station whose stream ended.
#[derive(Clone, Copy)]
enum Reopen {
    /// Re-open the audio mount, which is what the first connect resolved to.
    Mount,
    /// Re-fetch the segment playlist and start a fresh scheduler behind it. A segmented station
    /// has no single mount to return to, so its playlist URL is what gets carried.
    Segments,
}

/// A station opened and ready to be fed, with what a reconnect would need to find it again.
struct Resolved {
    opened: OpenedStream,
    url: Url,
    reopen: Reopen,
}

/// A live stream staged and ready to be appended to a deck.
///
/// Its feed thread is already running by the time this exists, so dropping one without playing it
/// is how a superseded station cancels: the source's `Drop` tells the thread to stop and
/// `StreamDownload` closes the connection with it.
pub struct PreparedStream {
    source: PrebufferSource,
    shared: Arc<StreamShared>,
}

impl PreparedStream {
    /// Split into the source the deck plays and the cell everyone else watches.
    pub fn into_parts(self) -> (PrebufferSource, Arc<StreamShared>) {
        (self.source, self.shared)
    }
}

/// Open `url` and start feeding it, following one level of playlist indirection first.
///
/// Directory stations arrive already resolved in `url_resolved`, so the follow is for hand-typed
/// URLs and for the occasional directory row that points at a `.pls` anyway.
pub async fn open(client: &reqwest::Client, url: &str) -> Result<PreparedStream, AppError> {
    let url = Url::parse(url).map_err(|e| AppError::network("Invalid station URL", e))?;
    let shared = StreamShared::new();
    let Resolved {
        opened,
        url,
        reopen,
    } = connect_following_playlist(client, url, &shared).await?;

    let (source, writer) = PrebufferSource::new(shared.clone(), opened.shape);
    spawn_feed(FeedContext {
        decoder: opened.decoder,
        writer,
        shared: shared.clone(),
        client: client.clone(),
        url,
        reopen,
        shape: opened.shape,
        runtime: tokio::runtime::Handle::current(),
    });

    Ok(PreparedStream { source, shared })
}

/// Open `url` far enough to know it plays, then let it go.
///
/// The same path [`open`] takes minus the ring and the thread, which is what makes it a real
/// answer rather than a reachability check: the playlist indirection is followed, the response is
/// buffered, and Symphonia probes the container — so a mount that is a web page, an encrypted
/// segment playlist or a codec with no decoder is refused **here**, when the user is looking at a
/// dialog, rather than at the moment they click play. Dropping the returned stream closes the
/// socket.
///
/// The station's own headers come back with it, so a hand-typed URL can name itself.
pub async fn probe(client: &reqwest::Client, url: &str) -> Result<StationFacts, AppError> {
    let url = Url::parse(url).map_err(|e| AppError::network("Invalid station URL", e))?;
    // Throwaway: the title callback writes into it and nothing ever reads it back.
    let shared = StreamShared::new();
    Ok(connect_following_playlist(client, url, &shared).await?.opened.facts)
}

/// Open `url`, and if what came back is a playlist rather than audio, follow it once.
///
/// Returns whatever a reconnect would need to find the station again: the audio mount for an
/// ordinary stream, and the playlist itself for a segmented one, which has no single mount.
async fn connect_following_playlist(
    client: &reqwest::Client,
    url: Url,
    shared: &Arc<StreamShared>,
) -> Result<Resolved, AppError> {
    // Extension first: a hand-typed `.pls` is worth spotting before opening a stream we would only
    // throw away, and it is the shape most custom stations arrive in.
    let url = if is_playlist_url(&url) {
        match follow_playlist(client, &url, shared).await? {
            Followed::Segments(opened) => {
                return Ok(Resolved {
                    opened: *opened,
                    url,
                    reopen: Reopen::Segments,
                });
            }
            Followed::Mount(mount) => mount,
        }
    } else {
        url
    };

    match connect(client, &url, shared).await? {
        Opened::Audio(opened) => Ok(Resolved {
            opened: *opened,
            url,
            reopen: Reopen::Mount,
        }),
        // An extensionless mount that turned out to be a pointer. Depth stays at one: what it
        // names is opened as audio or not at all.
        Opened::Playlist => match follow_playlist(client, &url, shared).await? {
            Followed::Segments(opened) => Ok(Resolved {
                opened: *opened,
                url,
                reopen: Reopen::Segments,
            }),
            Followed::Mount(mount) => match connect(client, &mount, shared).await? {
                Opened::Audio(opened) => Ok(Resolved {
                    opened: *opened,
                    url: mount,
                    reopen: Reopen::Mount,
                }),
                Opened::Playlist => {
                    Err(AppError::network_msg("Station playlist points at another playlist"))
                }
            },
        },
    }
}

/// Re-open a station the feed thread has lost, by whichever route it arrived on.
async fn reopen(
    client: &reqwest::Client,
    url: &Url,
    shared: &Arc<StreamShared>,
    how: Reopen,
) -> Result<OpenedStream, AppError> {
    match how {
        Reopen::Mount => match connect(client, url, shared).await? {
            Opened::Audio(opened) => Ok(*opened),
            Opened::Playlist => {
                Err(AppError::network_msg("Station stream URL now returns a playlist"))
            }
        },
        Reopen::Segments => {
            let body = fetch_playlist(client, url).await?;
            open_segments(client, url, &body, shared).await
        }
    }
}

/// Open a segment playlist as a stream, which is the whole of [`hls`]'s job plus the probe.
///
/// `shared` reaches the reader for its abandon flag alone, never for a title: HLS carries no ICY
/// metadata, so nothing here has one to publish.
async fn open_segments(
    client: &reqwest::Client,
    url: &Url,
    body: &str,
    shared: &Arc<StreamShared>,
) -> Result<OpenedStream, AppError> {
    let stream = hls::open(client, url, body, Arc::clone(shared)).await?;
    let facts = StationFacts {
        codec: stream.codec.to_owned(),
        bitrate: stream.bitrate_kbps,
        hls: true,
        ..StationFacts::default()
    };

    // On the blocking pool for the reason `connect` spells out: the probe reads, and this reader's
    // read parks until the scheduler behind it delivers, which needs a worker free to run on.
    let decoder = tokio::task::spawn_blocking(move || {
        StreamDecoder::open(Box::new(LiveSource(stream.reader)), None)
    })
    .await
    .map_err(AppError::io_source)??;

    Ok(OpenedStream {
        shape: decoder.shape(),
        decoder,
        facts,
    })
}

/// Open one URL: response, ICY headers, bounded download buffer, metadata reader, decoder.
async fn connect(
    client: &reqwest::Client,
    url: &Url,
    shared: &Arc<StreamShared>,
) -> Result<Opened, AppError> {
    let stream = HttpStream::<IcyClient>::new(IcyClient(client.clone()), url.clone())
        .await
        .map_err(|e| AppError::network("Could not open the station's stream", e))?;

    let mime = stream.content_type().as_ref().map(|ct| format!("{}/{}", ct.r#type, ct.subtype));
    if mime.as_deref().is_some_and(is_playlist_content_type) {
        return Ok(Opened::Playlist);
    }

    let icy = IcyHeaders::parse_from_headers(stream.headers());
    let facts = StationFacts {
        name: non_blank(icy.name()),
        genre: icy.genre().join(GENRE_SEPARATOR),
        homepage: non_blank(icy.station_url()),
        logo_url: non_blank(icy.logo_url()),
        codec: codec_from_mime(mime.as_deref()).to_owned(),
        bitrate: icy.bitrate().and_then(|kbps| i32::try_from(kbps).ok()).unwrap_or_default(),
        hls: false,
    };

    let storage = BoundedStorageProvider::new(MemoryStorageProvider, DOWNLOAD_BUFFER_BYTES);
    let settings = Settings::default().prefetch_bytes(prefetch_bytes(icy.bitrate()));

    let reader = StreamDownload::from_stream(stream, storage, settings)
        .await
        .map_err(|e| AppError::network("Could not buffer the station's stream", e))?;

    let titles = shared.clone();
    let reader: StreamReader =
        IcyMetadataReader::new(reader, icy.metadata_interval(), move |parsed| {
            titles.set_title(parsed.ok().and_then(|m| {
                m.stream_title().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
            }));
        });

    // **On the blocking pool, never on a worker.** Building the decoder probes the container by
    // *reading*, and `StreamDownload`'s reader is blocking: it parks the calling thread until its
    // downloader task delivers bytes. That task needs a worker to run on, and the runtime has two
    // — so a probe on a worker takes half the runtime hostage and two stations take all of it,
    // deadlocking the reads against the downloads that would satisfy them. The tell is not a
    // crash: the connect simply never returns, so nothing is staged, nothing is logged, and
    // shutdown's own `timeout` never fires either, no worker being left to park and own the timer.
    //
    // The mime handed over is what the server actually sent, in preference to the directory's
    // free-form `codec` string.
    let decoder = tokio::task::spawn_blocking(move || {
        StreamDecoder::open(Box::new(LiveSource(reader)), mime.as_deref())
    })
    .await
    .map_err(AppError::io_source)??;

    Ok(Opened::Audio(Box::new(OpenedStream {
        shape: decoder.shape(),
        decoder,
        facts,
    })))
}

/// A trimmed header value, or `None` where the server sent the field empty.
///
/// Icecast pads and blanks these freely, and an empty `name` handed on as `Some("")` would name a
/// station after nothing at all.
fn non_blank(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// Between the genre words Icecast lists separately. Matches the directory's own `tags` spelling,
/// so a hand-typed station's tags line reads like a browsed one's.
const GENRE_SEPARATOR: &str = ",";

/// A response content type as the short codec token the directory would have used.
///
/// Deliberately not exhaustive and deliberately not fallible: an unrecognised type is a station
/// whose codec line stays blank, which every surface already handles — `bitrate` is blank on a
/// large share of live stations for the same reason.
fn codec_from_mime(mime: Option<&str>) -> &'static str {
    match mime {
        Some("audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg") => "MP3",
        Some("audio/aac" | "audio/aacp" | "audio/x-aac" | "audio/mp4" | "audio/x-m4a") => "AAC",
        Some("audio/ogg" | "application/ogg" | "audio/vorbis") => "OGG",
        Some("audio/opus") => "OPUS",
        Some("audio/flac" | "audio/x-flac") => "FLAC",
        Some("audio/wav" | "audio/x-wav" | "audio/wave") => "WAV",
        _ => "",
    }
}

/// How much to buffer before the first read, from the bitrate the server states.
///
/// The directory's own `bitrate` column is not an input: it is `0` on a large share of live
/// stations, so the fallback would fire far more often than the calculation.
fn prefetch_bytes(bitrate_kbps: Option<u32>) -> u64 {
    let Some(kbps) = bitrate_kbps.filter(|k| *k > 0) else {
        return PREFETCH_FALLBACK_BYTES;
    };
    let bytes_per_second = u64::from(kbps) * 1_000 / 8;
    (bytes_per_second * PREFETCH_SECONDS).clamp(PREFETCH_MIN_BYTES, PREFETCH_MAX_BYTES)
}

/// Fetch a playlist and work out what it is: a station in its own right, or a pointer at one.
///
/// The HLS check comes first because the two overlap on the wire — a segment playlist is also a
/// valid Extended M3U, and read as a pointer its first segment opens, plays for a few seconds and
/// stops.
async fn follow_playlist(
    client: &reqwest::Client,
    url: &Url,
    shared: &Arc<StreamShared>,
) -> Result<Followed, AppError> {
    let body = fetch_playlist(client, url).await?;
    if hls::playlist::is_hls(&body) {
        return Ok(Followed::Segments(Box::new(open_segments(client, url, &body, shared).await?)));
    }

    let target = first_stream_url(&body)
        .ok_or_else(|| AppError::network_msg("Station playlist named no stream URL"))?;
    let mount = Url::parse(&target)
        .map_err(|e| AppError::network("Station playlist named an invalid URL", e))?;
    Ok(Followed::Mount(mount))
}

/// Read at most [`PLAYLIST_MAX_BYTES`] of `url` as text.
async fn fetch_playlist(client: &reqwest::Client, url: &Url) -> Result<String, AppError> {
    crate::services::net::get_capped_text(
        client,
        url,
        "Station playlist",
        PLAYLIST_TIMEOUT,
        PLAYLIST_MAX_BYTES,
    )
    .await
}

/// Does this URL's path end in a playlist extension? Query and fragment are stripped first, since
/// a mount routinely carries a session parameter after the name.
fn is_playlist_url(url: &Url) -> bool {
    let path = url.path();
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    PLAYLIST_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known))
}

/// Does this response content type name a playlist? Parameters (`; charset=…`) are already gone by
/// the time stream-download hands over a `ContentType`, but callers passing a raw header are
/// tolerated.
fn is_playlist_content_type(content_type: &str) -> bool {
    let bare = content_type.split(';').next().unwrap_or(content_type).trim();
    PLAYLIST_CONTENT_TYPES.iter().any(|known| bare.eq_ignore_ascii_case(known))
}

/// The first `http(s)` URL a playlist body names, across all three formats a station uses.
///
/// One pass rather than three parsers: `.pls` carries `File1=<url>` under a `[playlist]` header,
/// `.m3u` carries the URL on a line of its own with `#` comments around it, and `.asx` carries it
/// in an `href` attribute. Each line offers up to three readings and the first that is an absolute
/// HTTP URL wins, which is what keeps them from stepping on each other: a bare `.m3u` line whose
/// query string contains an `=` reads as a key/value pair under the `.pls` rule, and only falling
/// through to the whole line recovers it.
///
/// `library::playlist_files::m3u` is deliberately not reused: it is private to `library` (which
/// depends on this module's crate half, not the other way round) and it answers a different
/// question — track paths with BLAKE3 hashes and `#EXTINF` durations, none of which a stream
/// playlist carries, and neither of the other two formats at all.
fn first_stream_url(body: &str) -> Option<String> {
    let body = body.strip_prefix('\u{FEFF}').unwrap_or(body);
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let readings = [
            quoted_href(line),
            line.split_once('=').map(|(_, value)| value.trim()),
            Some(line),
        ];
        if let Some(url) =
            readings.into_iter().flatten().find(|c| crate::services::net::is_http_url(c))
        {
            return Some(url.to_owned());
        }
    }
    None
}

/// The value of an `href="…"` (or `href='…'`) attribute anywhere in `line`.
fn quoted_href(line: &str) -> Option<&str> {
    let lower = line.to_ascii_lowercase();
    let start = lower.find("href")? + "href".len();
    let rest = line.get(start..)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let inner = rest.get(quote.len_utf8()..)?;
    inner.split(quote).next()
}

/// How long to wait before reconnect attempt `attempt`, or `None` once the budget is spent.
///
/// Exponential from [`RECONNECT_BASE_SECS`] and capped, so a station that drops for a moment is
/// back almost immediately while one that has gone away is not hammered.
fn reconnect_delay(attempt: u32) -> Option<Duration> {
    if attempt >= RECONNECT_ATTEMPTS {
        return None;
    }
    let secs = RECONNECT_BASE_SECS.saturating_mul(2u64.saturating_pow(attempt));
    Some(Duration::from_secs(secs).min(RECONNECT_MAX_DELAY))
}

/// Everything the feed thread owns for the life of a station.
struct FeedContext {
    decoder: StreamDecoder,
    writer: RingWriter,
    shared: Arc<StreamShared>,
    client: reqwest::Client,
    /// Where a reconnect goes, not whatever the user originally typed: the audio mount for an
    /// ordinary stream, the segment playlist for a segmented one.
    url: Url,
    reopen: Reopen,
    shape: Shape,
    runtime: tokio::runtime::Handle,
}

fn spawn_feed(ctx: FeedContext) {
    let shared = ctx.shared.clone();
    // Spelled inline rather than lifted to a const: `services::tests`' thread-name walk matches a
    // literal after `.name(`, so a named constant here would be silently unmeasured.
    if let Err(e) =
        std::thread::Builder::new().name("radio-buffer".to_owned()).spawn(move || feed_loop(ctx))
    {
        log::error!("Could not start the radio buffer thread: {e}");
        // Nothing will ever fill the ring, so let the source end rather than play silence forever.
        shared.finish();
    }
}

/// Drain the decoder into the ring, reconnecting when it ends, until the source is dropped or the
/// attempt budget runs out.
fn feed_loop(mut ctx: FeedContext) {
    let mut attempt = 0;
    loop {
        let mut produced = false;
        for sample in ctx.decoder.by_ref() {
            produced = true;
            if !ctx.writer.push(sample) {
                return;
            }
        }
        if ctx.shared.is_abandoned() {
            return;
        }
        // Only a connection that actually delivered audio earns a fresh budget; otherwise a server
        // that accepts and immediately hangs up would be retried forever.
        if produced {
            attempt = 0;
        }

        let Some(delay) = reconnect_delay(attempt) else {
            log::warn!("Radio stream ended and could not be re-established; stopping");
            ctx.shared.finish();
            return;
        };
        attempt += 1;
        if !sleep_unless_abandoned(&ctx.shared, delay) {
            return;
        }

        match ctx.runtime.block_on(reopen(&ctx.client, &ctx.url, &ctx.shared, ctx.reopen)) {
            Ok(opened) if opened.shape == ctx.shape => {
                ctx.decoder = opened.decoder;
            }
            Ok(_) => {
                // The deck built its converter from this source's channel count and rate at the
                // append, and cannot rebuild it mid-source. Ending is what lets the station be
                // restarted cleanly from the top.
                log::warn!("Radio stream returned in a different audio format; stopping");
                ctx.shared.finish();
                return;
            }
            Err(e) => log::warn!("Radio reconnect attempt {attempt} failed: {}", describe(&e)),
        }
    }
}

/// Sleep for `delay`, waking early if the source is dropped. `false` means it was.
fn sleep_unless_abandoned(shared: &StreamShared, delay: Duration) -> bool {
    let mut left = delay;
    while !left.is_zero() {
        if shared.is_abandoned() {
            return false;
        }
        let slice = left.min(ABANDON_POLL);
        std::thread::sleep(slice);
        left -= slice;
    }
    !shared.is_abandoned()
}

#[cfg(test)]
#[path = "tests/stream_source_tests.rs"]
mod tests;
