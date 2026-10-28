//! Turning a station's URL into something the decks can play: follow a playlist URL to the audio
//! behind it, open that as a reader Symphonia accepts, and keep it fed from a thread of its own
//! into [`super::prebuffer`]'s ring.
//!
//! **Reconnect lives in the feed thread** (`docs/adr/0014-no-network-on-the-audio-callback.md`): it
//! re-opens into the same ring, so the deck never blinks, and gives up only once the attempt budget
//! is spent or a server returns a format the appended source can't carry. A quiet connection ends
//! its decoder through `stall_watch`. **Nothing here logs a stream URL**, which can carry a session
//! token into a log tail users attach to public issues.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use icy_metadata::error::MetadataParseError;
use icy_metadata::{IcyHeaders, IcyMetadata, IcyMetadataReader, RequestIcyMetadata};
use reqwest::Url;
use stream_download::StreamDownload;
use stream_download::http::{Client as StreamClient, HttpStream, format_range_header_bytes};
use stream_download::storage::bounded::BoundedStorageProvider;
use stream_download::storage::memory::MemoryStorageProvider;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3,
    CODEC_ID_OPUS, CODEC_ID_VORBIS,
};

use melodia_core::error::AppError;
use melodia_core::error::describe;

use super::audio::{Shape, SourceFormat};
use super::hls;
use super::prebuffer::{PrebufferSource, RingWriter, StreamShared};
use super::stall_watch::StallWatch;
use super::stream_decode::{LiveSource, StreamDecoder};

/// The *network* cushion, in compressed bytes because that is what the layer takes. A byte budget
/// buys less audio at a higher bitrate, which is the right way round: that station's stall costs
/// most to ride out.
const DOWNLOAD_BUFFER_BYTES: NonZeroUsize = match NonZeroUsize::new(512 * 1024) {
    Some(bytes) => bytes,
    None => NonZeroUsize::MIN,
};

/// Pulled down before the decoder may read, derived per station from its stated bitrate and
/// clamped: too little and the first seconds stutter, too much and the station is slow to start.
const PREFETCH_MIN_BYTES: u64 = 32 * 1024;
const PREFETCH_MAX_BYTES: u64 = 128 * 1024;
const PREFETCH_FALLBACK_BYTES: u64 = 64 * 1024;
/// How many seconds of audio the prefetch aims at, where the bitrate is known.
const PREFETCH_SECONDS: u64 = 2;

/// Bounded, because a station that stopped broadcasting looks like a flaky network, and the honest
/// answer to it is to say so and stop.
const RECONNECT_ATTEMPTS: u32 = 5;
/// The first backoff step; each attempt doubles it up to [`RECONNECT_MAX_DELAY`].
const RECONNECT_BASE_SECS: u64 = 1;
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(16);
/// How often a wait checks whether the source was dropped, here, in [`super::hls`] and in
/// `stall_watch`.
pub(super) const ABANDON_POLL: Duration = Duration::from_millis(100);

/// For a small text document; the audio request is deliberately unbounded, never ending.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(15);
/// A `.pls` or `.m3u` naming a station is a few hundred bytes.
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

/// Metadata blocks stripped out of a bounded circular download buffer over an HTTP response.
type StreamReader =
    IcyMetadataReader<StreamDownload<BoundedStorageProvider<MemoryStorageProvider>>>;

/// The shared client, wearing the `Icy-MetaData: 1` header that makes a server interleave titles.
/// stream-download's [`StreamClient::get`] takes no `RequestBuilder`, and a second client would
/// mean a second pool and losing the `Melodia/<version>` User-Agent some Icecast servers gate on.
struct IcyClient(reqwest::Client);

impl StreamClient for IcyClient {
    type Url = Url;
    type Headers = reqwest::header::HeaderMap;
    type Response = reqwest::Response;
    type Error = reqwest::Error;

    /// Reachable only through the convenience constructors this tree never calls, which
    /// `nothing_reaches_the_convenience_constructors` pins.
    fn create() -> Self {
        Self(<reqwest::Client as StreamClient>::create())
    }

    async fn get(&self, url: &Self::Url) -> Result<Self::Response, Self::Error> {
        self.0.get(url.clone()).request_icy_metadata().send().await
    }

    /// Asked only by stream-download's reconnect and a seek, and a live mount gets neither. The
    /// header stays so turning that reconnect on can't quietly cost a station its track names.
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

/// What a server said about itself while opening, kept rather than thrown away so [`probe`] can
/// describe a hand-typed URL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationFacts {
    pub name: Option<String>,
    pub genre: String,
    pub homepage: Option<String>,
    pub logo_url: Option<String>,
    /// A display token in the directory's spelling, off the decoded container where
    /// [`codec_token`] names it and off the content type otherwise.
    pub codec: String,
    /// Advertised kbps, `0` where the server does not say.
    pub bitrate: i32,
    /// A segment playlist, recorded as the directory's own `hls` column states it.
    pub hls: bool,
}

/// What was behind a URL. An extensionless mount can only be told from a pointer at one by what
/// the server says it is, so [`connect`] reports that rather than guessing from a failure.
enum Opened {
    Audio(Box<OpenedStream>),
    Playlist,
}

/// What a playlist body turned out to name.
enum Followed {
    /// A segment playlist, already opened: the playlist *is* the station.
    Segments(Box<OpenedStream>),
    Mount(Url),
}

/// How the feed thread gets back to a station whose stream ended.
#[derive(Clone, Copy)]
enum Reopen {
    Mount,
    /// A segmented station has no single mount, so its playlist URL is what gets carried.
    Segments,
}

/// A station opened and ready to be fed, with what a reconnect needs to find it again.
struct Resolved {
    opened: OpenedStream,
    url: Url,
    reopen: Reopen,
}

/// Its feed thread is already running, so dropping one unplayed is how a superseded station
/// cancels.
pub struct PreparedStream {
    source: PrebufferSource,
    shared: Arc<StreamShared>,
    codec: String,
}

impl PreparedStream {
    /// The source the deck plays, and the cell everyone else watches.
    pub fn into_parts(self) -> (PrebufferSource, Arc<StreamShared>) {
        (self.source, self.shared)
    }

    /// [`StationFacts::codec`] for the mount that just opened, so the station's row can record
    /// what it actually serves.
    pub fn codec(&self) -> &str {
        &self.codec
    }
}

/// A prepared stream with no socket, for `melodia-engine`'s generation tests, a `cfg(test)` item
/// being unable to cross crates. The `Weak` is the observation: every strong reference lives in the
/// returned value, so an upgrade failing is the stage having been dropped.
#[doc(hidden)]
pub fn prepared_stream_for_test(shape: Shape) -> (PreparedStream, std::sync::Weak<StreamShared>) {
    let shared = StreamShared::new();
    let watching = Arc::downgrade(&shared);
    let (source, _writer) = PrebufferSource::new(Arc::clone(&shared), shape, SourceFormat::F32);
    (PreparedStream { source, shared, codec: String::new() }, watching)
}

/// Opens `url` and starts feeding it, following one level of playlist indirection, which a
/// hand-typed URL or the odd directory row pointing at a `.pls` needs.
pub async fn open(client: &reqwest::Client, url: &str) -> Result<PreparedStream, AppError> {
    let url = Url::parse(url).map_err(|e| AppError::network("Invalid station URL", e))?;
    let shared = StreamShared::new();
    let Resolved { opened, url, reopen } = connect_following_playlist(client, url, &shared).await?;

    let (source, writer) =
        PrebufferSource::new(shared.clone(), opened.shape, opened.decoder.format());
    let codec = opened.facts.codec;
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

    Ok(PreparedStream { source, shared, codec })
}

/// [`open`] minus the ring and the thread, so a web page, an encrypted playlist or a codec with no
/// decoder is refused while the user is looking at a dialog rather than at play. The station's own
/// headers come back so a hand-typed URL can name itself.
pub async fn probe(client: &reqwest::Client, url: &str) -> Result<StationFacts, AppError> {
    let url = Url::parse(url).map_err(|e| AppError::network("Invalid station URL", e))?;
    // Throwaway: the title callback writes into it and nothing reads it back.
    let shared = StreamShared::new();
    Ok(connect_following_playlist(client, url, &shared).await?.opened.facts)
}

/// Opens `url`, following it once if what came back is a playlist.
async fn connect_following_playlist(
    client: &reqwest::Client,
    url: Url,
    shared: &Arc<StreamShared>,
) -> Result<Resolved, AppError> {
    // Extension first, so a hand-typed `.pls` is spotted before opening a stream to throw away.
    let url = if is_playlist_url(&url) {
        match follow_playlist(client, &url, shared).await? {
            Followed::Segments(opened) => {
                return Ok(Resolved { opened: *opened, url, reopen: Reopen::Segments });
            }
            Followed::Mount(mount) => mount,
        }
    } else {
        url
    };

    match connect(client, &url, shared).await? {
        Opened::Audio(opened) => Ok(Resolved { opened: *opened, url, reopen: Reopen::Mount }),
        // An extensionless pointer. Depth stays at one.
        Opened::Playlist => match follow_playlist(client, &url, shared).await? {
            Followed::Segments(opened) => {
                Ok(Resolved { opened: *opened, url, reopen: Reopen::Segments })
            }
            Followed::Mount(mount) => match connect(client, &mount, shared).await? {
                Opened::Audio(opened) => {
                    Ok(Resolved { opened: *opened, url: mount, reopen: Reopen::Mount })
                }
                Opened::Playlist => {
                    Err(AppError::network_msg("Station playlist points at another playlist"))
                }
            },
        },
    }
}

/// Re-opens a station the feed thread has lost, by whichever route it arrived on.
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

/// `shared` reaches the reader for its abandon flag alone, HLS carrying no ICY titles.
async fn open_segments(
    client: &reqwest::Client,
    url: &Url,
    body: &str,
    shared: &Arc<StreamShared>,
) -> Result<OpenedStream, AppError> {
    let stream = hls::open(client, url, body, Arc::clone(shared)).await?;
    let mut facts = StationFacts {
        codec: stream.codec.to_owned(),
        bitrate: stream.bitrate_kbps,
        hls: true,
        ..StationFacts::default()
    };

    // On the blocking pool for `connect`'s reason: this reader parks until the scheduler delivers.
    let decoder = tokio::task::spawn_blocking(move || {
        StreamDecoder::open(Box::new(LiveSource(stream.reader)), None)
    })
    .await
    .map_err(AppError::io_source)??;

    if let Some(token) = codec_token(decoder.codec()) {
        token.clone_into(&mut facts.codec);
    }

    Ok(OpenedStream { shape: decoder.shape(), decoder, facts })
}

/// Opens one URL: response, ICY headers, bounded download buffer, metadata reader, decoder.
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
    let mut facts = StationFacts {
        name: non_blank(icy.name()),
        genre: icy.genre().join(GENRE_SEPARATOR),
        homepage: non_blank(icy.station_url()),
        logo_url: non_blank(icy.logo_url()),
        codec: codec_from_mime(mime.as_deref()).to_owned(),
        bitrate: icy.bitrate().and_then(|kbps| i32::try_from(kbps).ok()).unwrap_or_default(),
        hls: false,
    };

    let storage = BoundedStorageProvider::new(MemoryStorageProvider, DOWNLOAD_BUFFER_BYTES);
    let watch = StallWatch::new();
    let settings = watch.settings(prefetch_bytes(icy.bitrate()));

    let reader = StreamDownload::from_stream(stream, storage, settings)
        .await
        .map_err(|e| AppError::network("Could not buffer the station's stream", e))?;
    watch.spawn(reader.cancellation_token(), Arc::clone(shared));

    let titles = shared.clone();
    let reader: StreamReader =
        IcyMetadataReader::new(reader, icy.metadata_interval(), move |parsed| {
            let Some(metadata) = readable_metadata(parsed) else { return };
            // A block carrying only `StreamUrl` says nothing about the song; read as a cleared
            // title, it ends the song mid-play and the repeat after it counts as a new one.
            let Some(title) = metadata.stream_title() else { return };
            titles.set_title(Some(title.trim()).filter(|t| !t.is_empty()).map(str::to_owned));
        });

    // **On the blocking pool, never a worker.** The probe reads, and `StreamDownload`'s reader
    // parks its thread until the downloader task delivers, which needs a worker of its own: two
    // stations probing on the runtime's two workers deadlock, nothing staged and nothing logged.
    // The mime is what the server sent, preferred to the directory's free-form `codec`.
    let decoder = tokio::task::spawn_blocking(move || {
        StreamDecoder::open(Box::new(LiveSource(reader)), mime.as_deref())
    })
    .await
    .map_err(AppError::io_source)??;

    if let Some(token) = codec_token(decoder.codec()) {
        token.clone_into(&mut facts.codec);
    }

    Ok(Opened::Audio(Box::new(OpenedStream { shape: decoder.shape(), decoder, facts })))
}

/// A block that isn't UTF-8 is read as Windows-1252, what older servers send, rather than dropped
/// and leaving the previous title up. An empty block is skipped, not read as a cleared title that
/// would make the next block look like a new song.
fn readable_metadata(parsed: Result<IcyMetadata, MetadataParseError>) -> Option<IcyMetadata> {
    match parsed {
        Ok(metadata) => Some(metadata),
        Err(MetadataParseError::InvalidUtf8(e)) => {
            let decoded: String = e.as_bytes().iter().copied().map(windows_1252).collect();
            decoded.trim_end_matches('\0').parse().ok()
        }
        Err(MetadataParseError::Empty(_)) => None,
    }
}

/// Windows-1252's 0x80–0x9F, where it parts from Latin-1 with the curly apostrophe and dashes
/// song titles are full of. Its five undefined bytes keep their Latin-1 reading, as WHATWG's
/// decoder does.
const WINDOWS_1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn windows_1252(byte: u8) -> char {
    match byte {
        0x80..=0x9F => WINDOWS_1252_HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// Icecast pads and blanks these freely, and an empty `name` would name a station after nothing.
fn non_blank(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// The directory's own `tags` spelling, so a hand-typed station's tags read like a browsed one's.
const GENRE_SEPARATOR: &str = ",";

/// A content type as the directory's codec token. Not exhaustive and not fallible: an unrecognised
/// type leaves the codec line blank, which every surface already handles.
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

/// The decoded codec in the same spelling, which wins wherever it has one: a content type names
/// only the container, so every Ogg mount says `audio/ogg` whatever it carries. `None` leaves the
/// content type's answer standing, which for PCM's dozen ids is the `WAV` a listener recognises.
fn codec_token(codec: AudioCodecId) -> Option<&'static str> {
    match codec {
        CODEC_ID_MP1 | CODEC_ID_MP2 | CODEC_ID_MP3 => Some("MP3"),
        CODEC_ID_AAC => Some("AAC"),
        CODEC_ID_VORBIS => Some("VORBIS"),
        CODEC_ID_OPUS => Some("OPUS"),
        CODEC_ID_FLAC => Some("FLAC"),
        CODEC_ID_ALAC => Some("ALAC"),
        _ => None,
    }
}

/// From the bitrate the server states, never the directory's column, which is `0` on a large share
/// of live stations.
fn prefetch_bytes(bitrate_kbps: Option<u32>) -> u64 {
    let Some(kbps) = bitrate_kbps.filter(|k| *k > 0) else {
        return PREFETCH_FALLBACK_BYTES;
    };
    let bytes_per_second = u64::from(kbps) * 1_000 / 8;
    (bytes_per_second * PREFETCH_SECONDS).clamp(PREFETCH_MIN_BYTES, PREFETCH_MAX_BYTES)
}

/// The HLS check comes first: a segment playlist is also a valid Extended M3U, and read as a
/// pointer its first segment plays for a few seconds and stops.
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

async fn fetch_playlist(client: &reqwest::Client, url: &Url) -> Result<String, AppError> {
    melodia_net::services::net::get_capped_text(
        client,
        url,
        "Station playlist",
        PLAYLIST_TIMEOUT,
        PLAYLIST_MAX_BYTES,
    )
    .await
}

/// Off the path alone, a mount routinely carrying a session parameter after the name.
fn is_playlist_url(url: &Url) -> bool {
    let path = url.path();
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    PLAYLIST_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known))
}

/// Tolerates a raw header with parameters, stream-download's `ContentType` having none.
fn is_playlist_content_type(content_type: &str) -> bool {
    let bare = content_type.split(';').next().unwrap_or(content_type).trim();
    PLAYLIST_CONTENT_TYPES.iter().any(|known| bare.eq_ignore_ascii_case(known))
}

/// The first `http(s)` URL a playlist names, across `.pls`'s `File1=`, `.m3u`'s bare line and
/// `.asx`'s `href`. Each line offers all three readings and the first absolute URL wins, so an
/// `.m3u` line whose query holds an `=` still reaches the whole-line reading. Not
/// `library::playlist_files::m3u`, which reads track paths and hashes from a layer above.
fn first_stream_url(body: &str) -> Option<String> {
    let body = body.strip_prefix('\u{FEFF}').unwrap_or(body);
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let readings =
            [quoted_href(line), line.split_once('=').map(|(_, value)| value.trim()), Some(line)];
        if let Some(url) =
            readings.into_iter().flatten().find(|c| melodia_net::services::net::is_http_url(c))
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

/// `None` once the budget is spent. Exponential and capped, so a blip is back almost at once and a
/// station that has gone away is not hammered.
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
    /// Where a reconnect goes: the audio mount, or the segment playlist for a segmented station.
    url: Url,
    reopen: Reopen,
    shape: Shape,
    runtime: tokio::runtime::Handle,
}

fn spawn_feed(ctx: FeedContext) {
    let shared = ctx.shared.clone();
    // Inline rather than a const: `crates/melodia/tests/packaging.rs`'s thread-name walk matches a
    // literal after `.name(`.
    if let Err(e) =
        std::thread::Builder::new().name("radio-buffer".to_owned()).spawn(move || feed_loop(ctx))
    {
        log::error!("Could not start the radio buffer thread: {}", describe(&e));
        // Nothing will ever fill the ring, so let the source end rather than play silence forever.
        shared.finish();
    }
}

/// Drains the decoder into the ring, reconnecting when it ends, until the source is dropped or the
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
        // Only a connection that delivered audio earns a fresh budget, or a server that accepts and
        // hangs up at once would be retried forever.
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

        let reopened = ctx.runtime.block_on(async {
            tokio::select! {
                opened = reopen(&ctx.client, &ctx.url, &ctx.shared, ctx.reopen) => Some(opened),
                () = abandoned(&ctx.shared) => None,
            }
        });
        // Stopped mid-request, which a dead network can hold open a long time; dropping it closes
        // the socket.
        let Some(reopened) = reopened else { return };
        match reopened {
            Ok(opened) if opened.shape == ctx.shape => {
                log::info!("Radio stream re-established");
                ctx.decoder = opened.decoder;
            }
            Ok(_) => {
                // The deck's converter is fixed to this source's shape, so ending is what lets the
                // station be restarted cleanly.
                log::warn!("Radio stream returned in a different audio format; stopping");
                ctx.shared.finish();
                return;
            }
            Err(e) => log::warn!("Radio reconnect attempt {attempt} failed: {}", describe(&e)),
        }
    }
}

/// Sleeps for `delay`, waking early if the source is dropped. `false` means it was.
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

/// Resolves once the source is dropped.
async fn abandoned(shared: &StreamShared) {
    while !shared.is_abandoned() {
        tokio::time::sleep(ABANDON_POLL).await;
    }
}

#[cfg(test)]
#[path = "tests/stream_source_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/stream_probe_tests.rs"]
mod probe_tests;
