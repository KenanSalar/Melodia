//! Turning a station's URL into something the decks can play.
//!
//! Three jobs, in the order a station meets them: follow a playlist URL to the audio behind it,
//! open that as a seekable reader Symphonia will accept, and keep it fed from a thread of its own.
//! The ring that thread writes into is [`super::prebuffer`], which argues why the audio callback
//! must never see a socket.
//!
//! **Reconnect lives here rather than in the playback monitor.** The feed thread already holds the
//! URL, the client and the ring, so when its decoder ends it re-opens and keeps filling the *same*
//! ring: the rodio source never ends, the deck never blinks, and the state machine needs no
//! reconnect path at all. Only once the attempt budget is spent, or a server comes back with a
//! format the already-appended source cannot carry, does the thread give up and let the deck
//! drain — which the monitor reads as the end of the station.
//!
//! Nothing here logs a stream URL. They routinely carry a session token in the query string, and
//! `services::diagnostics` puts the log tail in front of a public GitHub issue.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use icy_metadata::{IcyHeaders, IcyMetadataReader, RequestIcyMetadata};
use reqwest::Url;
use rodio::{ChannelCount, Decoder, SampleRate, Source};
use stream_download::http::{Client as StreamClient, HttpStream, format_range_header_bytes};
use stream_download::storage::bounded::BoundedStorageProvider;
use stream_download::storage::memory::MemoryStorageProvider;
use stream_download::{Settings, StreamDownload};

use crate::error::AppError;
use crate::services::describe;

use super::prebuffer::{PrebufferSource, RingWriter, StreamShared};

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
/// Granularity at which a backoff sleep notices the source was dropped.
const ABANDON_POLL: Duration = Duration::from_millis(100);

/// Whole-request ceiling for fetching a playlist file, which is a small text document nobody
/// should be waiting on. The audio request itself is deliberately unbounded — it never ends.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of a playlist body to read before refusing it. A `.pls` or `.m3u` pointing at a
/// station is a few hundred bytes; anything past this is not the document we were promised.
const PLAYLIST_MAX_BYTES: usize = 64 * 1024;
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
    decoder: Decoder<StreamReader>,
    channels: ChannelCount,
    sample_rate: SampleRate,
}

/// What was actually behind a URL. A mount with no extension can only be told apart from a pointer
/// at it by what the server says it is, so [`connect`] reports that rather than guessing from the
/// shape of a failure.
enum Opened {
    Audio(Box<OpenedStream>),
    Playlist,
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
    let (opened, resolved) = connect_following_playlist(client, url, &shared).await?;

    let (source, writer) =
        PrebufferSource::new(shared.clone(), opened.channels, opened.sample_rate);
    spawn_feed(FeedContext {
        decoder: opened.decoder,
        writer,
        shared: shared.clone(),
        client: client.clone(),
        url: resolved,
        channels: opened.channels,
        sample_rate: opened.sample_rate,
        runtime: tokio::runtime::Handle::current(),
    });

    Ok(PreparedStream { source, shared })
}

/// Open `url`, and if what came back is a playlist rather than audio, follow it once.
///
/// Returns the resolved URL alongside the stream so the feed thread reconnects to the audio mount
/// rather than re-fetching the pointer.
async fn connect_following_playlist(
    client: &reqwest::Client,
    url: Url,
    shared: &Arc<StreamShared>,
) -> Result<(OpenedStream, Url), AppError> {
    // Extension first: a hand-typed `.pls` is worth spotting before opening a stream we would only
    // throw away, and it is the shape most custom stations arrive in.
    let url = if is_playlist_url(&url) {
        follow_playlist(client, &url).await?
    } else {
        url
    };

    match connect(client, &url, shared).await? {
        Opened::Audio(opened) => Ok((*opened, url)),
        // An extensionless mount that turned out to be a pointer. Depth stays at one: what it
        // names is opened as audio or not at all.
        Opened::Playlist => {
            let followed = follow_playlist(client, &url).await?;
            match connect(client, &followed, shared).await? {
                Opened::Audio(opened) => Ok((*opened, followed)),
                Opened::Playlist => {
                    Err(AppError::network_msg("Station playlist points at another playlist"))
                }
            }
        }
    }
}

/// [`connect`] for a URL already known to be audio, which is every reconnect: the feed thread only
/// ever re-opens the mount the first connect resolved to.
async fn connect_audio(
    client: &reqwest::Client,
    url: &Url,
    shared: &Arc<StreamShared>,
) -> Result<OpenedStream, AppError> {
    match connect(client, url, shared).await? {
        Opened::Audio(opened) => Ok(*opened),
        Opened::Playlist => Err(AppError::network_msg("Station stream URL now returns a playlist")),
    }
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
    let storage = BoundedStorageProvider::new(MemoryStorageProvider, DOWNLOAD_BUFFER_BYTES);
    let settings = Settings::default().prefetch_bytes(prefetch_bytes(icy.bitrate()));

    let reader = StreamDownload::from_stream(stream, storage, settings)
        .await
        .map_err(|e| AppError::network("Could not buffer the station's stream", e))?;

    let titles = shared.clone();
    let reader = IcyMetadataReader::new(reader, icy.metadata_interval(), move |parsed| {
        titles.set_title(parsed.ok().and_then(|m| {
            m.stream_title().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
        }));
    });

    let mut builder = Decoder::builder().with_data(reader);
    // What the server actually sent, in preference to the directory's free-form `codec` string.
    // Deliberately no `with_byte_len`: it sets `is_seekable` as a side effect, and a live mount
    // has neither a length nor a way back.
    if let Some(mime) = &mime {
        builder = builder.with_mime_type(mime);
    }
    let decoder = builder
        .with_seekable(false)
        .build()
        .map_err(|e| AppError::Player(format!("Cannot decode the station's stream: {e}")))?;

    Ok(Opened::Audio(Box::new(OpenedStream {
        channels: decoder.channels(),
        sample_rate: decoder.sample_rate(),
        decoder,
    })))
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

/// Fetch a playlist and return the first stream URL it names.
async fn follow_playlist(client: &reqwest::Client, url: &Url) -> Result<Url, AppError> {
    let body = fetch_capped(client, url).await?;
    let target = first_stream_url(&body)
        .ok_or_else(|| AppError::network_msg("Station playlist named no stream URL"))?;
    Url::parse(&target).map_err(|e| AppError::network("Station playlist named an invalid URL", e))
}

/// Read at most [`PLAYLIST_MAX_BYTES`] of `url` as text.
///
/// Streamed rather than `bytes()`-ed so a server that lies about (or omits) its content length
/// cannot make this allocate without bound.
async fn fetch_capped(client: &reqwest::Client, url: &Url) -> Result<String, AppError> {
    let response = client
        .get(url.clone())
        .timeout(PLAYLIST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network("Could not fetch the station's playlist", e))?;
    if !response.status().is_success() {
        return Err(AppError::network_msg(format!(
            "Station playlist request returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response.content_length().is_some_and(|len| len > PLAYLIST_MAX_BYTES as u64) {
        return Err(AppError::network_msg("Station playlist is larger than a playlist should be"));
    }

    let mut body = Vec::with_capacity(1024);
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|e| AppError::network("Could not read the station's playlist", e))?;
        if body.len() + chunk.len() > PLAYLIST_MAX_BYTES {
            return Err(AppError::network_msg(
                "Station playlist is larger than a playlist should be",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Does this URL's path end in a playlist extension? Query and fragment are stripped first, since
/// a mount routinely carries a session parameter after the name.
pub fn is_playlist_url(url: &Url) -> bool {
    let path = url.path();
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    PLAYLIST_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known))
}

/// Does this response content type name a playlist? Parameters (`; charset=…`) are already gone by
/// the time stream-download hands over a `ContentType`, but callers passing a raw header are
/// tolerated.
pub fn is_playlist_content_type(content_type: &str) -> bool {
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
pub fn first_stream_url(body: &str) -> Option<String> {
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
        if let Some(url) = readings.into_iter().flatten().find(|c| is_http_url(c)) {
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

fn is_http_url(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// How long to wait before reconnect attempt `attempt`, or `None` once the budget is spent.
///
/// Exponential from [`RECONNECT_BASE_SECS`] and capped, so a station that drops for a moment is
/// back almost immediately while one that has gone away is not hammered.
pub fn reconnect_delay(attempt: u32) -> Option<Duration> {
    if attempt >= RECONNECT_ATTEMPTS {
        return None;
    }
    let secs = RECONNECT_BASE_SECS.saturating_mul(2u64.saturating_pow(attempt));
    Some(Duration::from_secs(secs).min(RECONNECT_MAX_DELAY))
}

/// Everything the feed thread owns for the life of a station.
struct FeedContext {
    decoder: Decoder<StreamReader>,
    writer: RingWriter,
    shared: Arc<StreamShared>,
    client: reqwest::Client,
    /// The audio mount, already followed past any playlist. Reconnects go here, not to whatever
    /// the user originally typed.
    url: Url,
    channels: ChannelCount,
    sample_rate: SampleRate,
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

        match ctx.runtime.block_on(connect_audio(&ctx.client, &ctx.url, &ctx.shared)) {
            Ok(opened)
                if opened.channels == ctx.channels && opened.sample_rate == ctx.sample_rate =>
            {
                ctx.decoder = opened.decoder;
            }
            Ok(_) => {
                // The deck was told this source's channel count and rate when it was appended, and
                // rodio has no way to renegotiate mid-source. Ending is what lets the station be
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
