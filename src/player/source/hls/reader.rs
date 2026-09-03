//! The segment scheduler, and the reader that hides it from the decoder.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use tokio::sync::mpsc;

use crate::error::AppError;
use crate::error::describe;
use crate::player::source::prebuffer::StreamShared;
use crate::player::source::stream_source::ABANDON_POLL;

use super::playlist::{self, MediaPlaylist, Playlist};
use super::segment::SegmentReader;

/// How far back from the live edge to start.
///
/// Three is what the spec recommends a client hold, and it is the whole latency budget: fewer and
/// a stall reaches the ring before the network recovers, more and the station starts audibly late.
const LIVE_EDGE_SEGMENTS: usize = 3;

/// Segments buffered ahead of the decoder.
///
/// This is the network cushion, the counterpart to `stream_source`'s byte-sized one, and the
/// sender blocking on a full queue is what paces fetching to playback.
const SEGMENT_QUEUE_DEPTH: usize = 4;

/// Ceiling on one segment. Generous next to the few seconds of audio a station sends, because the
/// point is to refuse a mount serving something else entirely rather than to size a buffer.
const SEGMENT_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SEGMENT_TIMEOUT: Duration = Duration::from_secs(20);

/// Ceiling on one manifest, and how long we wait for it.
///
/// **Deliberately its own cap rather than `stream_source`'s**, which bounds a `.pls` or `.m3u`
/// *pointer* — a few hundred bytes naming one mount. A media playlist is a live document listing
/// every segment still in the window and re-fetched twice a target duration, so the two answer
/// different questions and only looked like one number. The wait beside it lands on the same
/// figure, a manifest and a pointer being one request over one connection either way.
const MANIFEST_MAX_BYTES: u64 = 256 * 1024;
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Consecutive playlist failures before the stream is given up on.
///
/// Ending here rather than retrying forever hands the station to the feed thread's own reconnect,
/// which has the backoff and the budget that decide when to tell the user.
const PLAYLIST_FAILURE_BUDGET: u32 = 5;

/// Consecutive reloads bringing nothing new before the stream is given up on.
///
/// A playlist that answers but stops advancing is invisible to [`PLAYLIST_FAILURE_BUDGET`], and it
/// costs more than a dead one: the reader parks on an empty queue, the ring dries, and nothing ends
/// the source, so the reconnect that would recover it never runs. Counted in the half-period
/// reloads a caught-up client is already making, which puts this at six target durations with
/// nothing new; [`LIVE_EDGE_SEGMENTS`] is still playing through the first of them.
const SEGMENT_STALL_BUDGET: u32 = 12;

/// An opened HLS stream, and what its playlist said about itself.
pub struct HlsStream {
    pub reader: HlsReader,
    /// `AAC` or `MP3` off the first segment, empty where it was neither.
    pub codec: &'static str,
    /// From the chosen variant's `BANDWIDTH`, `0` where the playlist named none.
    pub bitrate_kbps: i32,
}

/// One unbroken byte stream, assembled from segments the scheduler fetches behind it.
///
/// Blocking by design: it is read from the feed thread, which already parks on a full ring, and
/// never from a runtime worker.
pub struct HlsReader {
    chunks: mpsc::Receiver<Vec<u8>>,
    held: Vec<u8>,
    offset: usize,
    position: u64,
    /// Read only to end the wait below. The scheduler cannot do it: it learns the listener is gone
    /// from `chunks.closed()`, which fires when this receiver drops, which needs this read to
    /// return, so parking here unconditionally leaves the two waiting on each other.
    shared: Arc<StreamShared>,
}

impl Read for HlsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.offset >= self.held.len() {
            match self.chunks.try_recv() {
                Ok(chunk) => {
                    self.held = chunk;
                    self.offset = 0;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(0),
                // Polled rather than parked, so a stopped station releases this thread, the
                // decoder and the ring at once instead of after the scheduler has spent its
                // stall budget, which on a playlist that answers but has stopped advancing runs to
                // a couple of minutes. Costs a wake-up granularity the segment cadence cannot feel.
                Err(mpsc::error::TryRecvError::Empty) => {
                    if self.shared.is_abandoned() {
                        return Ok(0);
                    }
                    std::thread::sleep(ABANDON_POLL);
                }
            }
        }

        let take = (self.held.len() - self.offset).min(buf.len());
        let end = self.offset + take;
        buf[..take].copy_from_slice(&self.held[self.offset..end]);
        self.offset = end;
        self.position = self.position.saturating_add(take as u64);
        Ok(take)
    }
}

impl Seek for HlsReader {
    /// The source wrapping this reports itself unseekable, so the only call that arrives is the
    /// no-op asking where the reader has got to.
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if matches!(pos, SeekFrom::Current(0)) {
            return Ok(self.position);
        }
        Err(io::Error::new(io::ErrorKind::Unsupported, "a live stream cannot seek"))
    }
}

/// Open the stream `body` describes, priming it with a segment so the caller learns the codec and
/// a dead station fails here rather than at the first read.
///
/// `body` is the already-fetched playlist at `url`, which the caller read to recognise it as HLS.
///
/// `shared` is taken for one field of it, and [`HlsReader::shared`] says which and why.
pub async fn open(
    client: &reqwest::Client,
    url: &Url,
    body: &str,
    shared: Arc<StreamShared>,
) -> Result<HlsStream, AppError> {
    let resolved = resolve(client, url, body).await?;

    // The live edge, minus the cushion. Anything older is history nobody tuning in wants to hear
    // before today's audio.
    let skipped = resolved.playlist.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS);
    let first = resolved
        .playlist
        .segments
        .get(skipped)
        .ok_or_else(|| AppError::network_msg("Station playlist named no segments"))?;

    let mut segments = SegmentReader::default();
    let mut primed = Vec::new();
    // Ahead of the first segment and only here: a fragmented stream's `ftyp`/`moov` is what the
    // demuxer probes, and the fragments behind it are unreadable on their own. A reconnect runs
    // this same path, so it arrives again with the fresh scheduler that needs it, which is also
    // the only thing that would recover a station rotating its `EXT-X-MAP` mid-stream.
    if let Some(init) = &resolved.playlist.init_segment {
        segments.append(&fetch_segment(client, init).await?, &mut primed);
    }
    segments.append(&fetch_segment(client, first).await?, &mut primed);
    if primed.is_empty() {
        return Err(AppError::network_msg("The station's stream carries no audio"));
    }
    let codec = segments.codec();

    let (sender, chunks) = mpsc::channel(SEGMENT_QUEUE_DEPTH);
    sender
        .send(primed)
        .await
        .map_err(|_| AppError::network_msg("Could not stage the station's stream"))?;

    tokio::spawn(
        Scheduler {
            client: client.clone(),
            playlist_url: resolved.media_url,
            segments,
            next_sequence: resolved
                .playlist
                .media_sequence
                .saturating_add(u64::try_from(skipped).unwrap_or(0))
                .saturating_add(1),
            refresh: resolved.playlist.target_duration,
            stalled: 0,
            chunks: sender,
        }
        .run(),
    );

    Ok(HlsStream {
        reader: HlsReader {
            chunks,
            held: Vec::new(),
            offset: 0,
            position: 0,
            shared,
        },
        codec,
        bitrate_kbps: resolved.bitrate_kbps,
    })
}

/// The media playlist to poll, wherever the URL the station named led.
struct Resolved {
    media_url: Url,
    playlist: MediaPlaylist,
    bitrate_kbps: i32,
}

/// Follow a master playlist to the rendition to play, or take a media playlist as it stands.
async fn resolve(client: &reqwest::Client, url: &Url, body: &str) -> Result<Resolved, AppError> {
    let variants = match playlist::parse(body, url)? {
        Playlist::Media(playlist) => {
            return Ok(Resolved {
                media_url: url.clone(),
                playlist,
                bitrate_kbps: 0,
            });
        }
        Playlist::Master(variants) => variants,
    };

    let variant = playlist::pick_variant(variants)
        .ok_or_else(|| AppError::network_msg("Station playlist named no stream"))?;
    // A picture's bits ride in `BANDWIDTH` too, so a simulcast's rung states nothing about its
    // audio. Blank is what every surface already draws for a server that named no bitrate.
    let bitrate_kbps = if variant.has_video {
        0
    } else {
        i32::try_from(variant.bandwidth / 1_000).unwrap_or(0)
    };
    let body = fetch_manifest(client, &variant.url).await?;
    Ok(Resolved {
        playlist: media_playlist(&body, &variant.url)?,
        bitrate_kbps,
        media_url: variant.url,
    })
}

fn media_playlist(body: &str, url: &Url) -> Result<MediaPlaylist, AppError> {
    match playlist::parse(body, url)? {
        Playlist::Media(playlist) => Ok(playlist),
        Playlist::Master(_) => {
            Err(AppError::network_msg("Station playlist points at another playlist"))
        }
    }
}

/// Everything the scheduler owns for the life of a station.
struct Scheduler {
    client: reqwest::Client,
    playlist_url: Url,
    segments: SegmentReader,
    /// The sequence number of the next segment worth fetching. A playlist that has moved past it
    /// is a stream we fell behind, and the gap is skipped rather than caught up on.
    next_sequence: u64,
    refresh: Duration,
    /// Reloads since the last one that brought audio, against [`SEGMENT_STALL_BUDGET`].
    stalled: u32,
    chunks: mpsc::Sender<Vec<u8>>,
}

impl Scheduler {
    /// Reload the playlist, fetch what is new, and hand it on until nobody is listening.
    async fn run(mut self) {
        let mut failures: u32 = 0;
        loop {
            let refresh = self.refresh;
            tokio::select! {
                () = tokio::time::sleep(refresh) => {}
                () = self.chunks.closed() => return,
            }

            let reloaded = fetch_manifest(&self.client, &self.playlist_url)
                .await
                .and_then(|body| media_playlist(&body, &self.playlist_url));
            let playlist = match reloaded {
                Ok(playlist) => {
                    failures = 0;
                    playlist
                }
                Err(e) => {
                    failures += 1;
                    if failures >= PLAYLIST_FAILURE_BUDGET {
                        log::warn!(
                            "radio: the station's playlist stopped answering: {}",
                            describe(&e)
                        );
                        return;
                    }
                    continue;
                }
            };

            if !self.drain(&playlist).await {
                return;
            }
            if self.stalled >= SEGMENT_STALL_BUDGET {
                log::warn!("radio: the station stopped sending audio");
                return;
            }
            if playlist.ended {
                return;
            }
        }
    }

    /// Fetch and forward everything in `playlist` we have not already played. `false` means the
    /// listener went away.
    async fn drain(&mut self, playlist: &MediaPlaylist) -> bool {
        let mut appended = false;
        let mut sequence = playlist.media_sequence;

        for url in &playlist.segments {
            if sequence >= self.next_sequence {
                // Bound the fetch's borrow before touching the reader, which the same `self` owns.
                let fetched = fetch_segment(&self.client, url).await;
                match fetched {
                    Ok(bytes) => {
                        let mut out = Vec::new();
                        self.segments.append(&bytes, &mut out);
                        if !out.is_empty() {
                            if self.chunks.send(out).await.is_err() {
                                return false;
                            }
                            appended = true;
                        }
                    }
                    // One segment is a gap the demuxer resyncs past; the next usually arrives.
                    Err(e) => log::debug!("radio: skipped a segment: {}", describe(&e)),
                }
                self.next_sequence = sequence.saturating_add(1);
            }
            sequence = sequence.saturating_add(1);
        }

        if appended {
            self.stalled = 0;
            self.refresh = playlist.target_duration;
        } else {
            self.stalled += 1;
            // Half a period, which is what the spec asks of a client that has caught up with a
            // playlist still being written.
            self.refresh = playlist.target_duration / 2;
        }
        true
    }
}

/// Read at most [`MANIFEST_MAX_BYTES`] of one playlist, as text.
async fn fetch_manifest(client: &reqwest::Client, url: &Url) -> Result<String, AppError> {
    crate::services::net::get_capped_text(
        client,
        url,
        "Station playlist",
        MANIFEST_TIMEOUT,
        MANIFEST_MAX_BYTES,
    )
    .await
}

/// Read at most [`SEGMENT_MAX_BYTES`] of one segment.
async fn fetch_segment(client: &reqwest::Client, url: &Url) -> Result<Vec<u8>, AppError> {
    crate::services::net::get_capped(
        client,
        url,
        "Station segment",
        SEGMENT_TIMEOUT,
        SEGMENT_MAX_BYTES,
    )
    .await
}
