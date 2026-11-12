//! A track's loudness, and an album's as one whole, stated as `ReplayGain` 2.0 tags state it.
//! Decoded through [`FileDecoder::open_track`], the decoder a deck plays from, so what is measured
//! is what plays: trimmed of priming and padding, a cue track's span alone, an Opus file's header
//! gain applied.

mod channels;
mod meter;

use std::path::Path;

use melodia_core::entities::track::TrackAudio;
use melodia_core::error::describe;

use self::meter::{AlbumMeters, Layout, MemberMeters};
use crate::player::source::audio::AudioSource;
use crate::player::source::decode::Ended;
use crate::player::source::file_decode::FileDecoder;

/// Frames fed to the meters between two stop checks, bounding how long a stop waits and the
/// buffer a measurement holds.
const BLOCK_FRAMES: usize = 4096;

/// A gain and the peak its clipping guard caps it by, as a gain tag and its peak tag state them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    /// Against `melodia_core`'s `REPLAYGAIN_REFERENCE_LUFS`.
    pub db: f64,
    /// The loudest sample over every channel, linear with 1.0 at full scale. A sample peak, not a
    /// true peak: the guard caps a boost at `1 / peak` to keep the samples Melodia produces under
    /// full scale, which a sample peak bounds exactly.
    pub peak: f64,
}

/// What a track's measurement leaves to record. [`Self::OutOfReach`] alone records nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackOutcome {
    Measured(Gain),
    /// Nothing to level: silent, under one gating block, or a file that is there but would not
    /// open or read through, which would fail the same way next time.
    NoResult,
    /// The decode failed on a file that isn't there afterwards, as on an unmounted drive, which may
    /// come back.
    OutOfReach,
}

/// Each member's outcome, and the album's gain metered over all of them as one whole.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumOutcome {
    /// One per member, in the order given.
    pub tracks: Vec<TrackOutcome>,
    /// Only where every member was read to its end and some block rose above the gate.
    pub album: Option<Gain>,
}

/// A measurement stopped before its end, leaving nothing partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

/// Measures `members` as one album, fed in the order given, which the caller makes disc and track
/// order. `stop` is asked between blocks.
pub fn measure_album(
    members: &[TrackAudio],
    stop: impl Fn() -> bool,
) -> Result<AlbumOutcome, Stopped> {
    let mut album_meters = AlbumMeters::default();
    let mut tracks = Vec::with_capacity(members.len());
    let mut read_through = true;
    let mut album_peak = 0.0_f64;
    for member in members {
        // A member short of its end already rules the album gain out.
        let read = read(member, read_through.then_some(&mut album_meters), &stop)?;
        match read {
            Read::Through { peak, .. } => album_peak = album_peak.max(peak),
            Read::Failed | Read::OutOfReach => read_through = false,
        }
        tracks.push(read.outcome());
    }
    let album = if read_through { album_meters.gain(album_peak) } else { None };
    Ok(AlbumOutcome { tracks, album })
}

/// Measures a track that belongs to no album. `stop` is asked between blocks.
pub fn measure_track(track: &TrackAudio, stop: impl Fn() -> bool) -> Result<TrackOutcome, Stopped> {
    Ok(read(track, None, &stop)?.outcome())
}

/// How far one member's decode got. A summary rather than its meter, so a measurement holds one
/// track's history at a time however long the album.
#[derive(Clone, Copy)]
enum Read {
    /// To its end: its gain where it has one, and its peak for the album's.
    Through { gain: Option<Gain>, peak: f64 },
    /// Short of its end on a file that is still there.
    Failed,
    /// Short of its end on a file that isn't.
    OutOfReach,
}

impl Read {
    fn outcome(self) -> TrackOutcome {
        match self {
            Self::Through { gain, .. } => {
                gain.map_or(TrackOutcome::NoResult, TrackOutcome::Measured)
            }
            Self::Failed => TrackOutcome::NoResult,
            Self::OutOfReach => TrackOutcome::OutOfReach,
        }
    }
}

fn read(
    track: &TrackAudio,
    album: Option<&mut AlbumMeters>,
    stop: &impl Fn() -> bool,
) -> Result<Read, Stopped> {
    let path = Path::new(&track.path);
    let mut decoder = match FileDecoder::open_track(track) {
        Ok(decoder) => decoder,
        Err(e) => {
            log::debug!("Loudness not measured: {}", describe(&e));
            return Ok(short_of_end(path));
        }
    };
    let mut meters = match MemberMeters::new(Layout::of(&decoder), album) {
        Ok(meters) => meters,
        Err(e) => return Ok(meter_refused(path, e)),
    };

    let mut block = Vec::new();
    loop {
        if stop() {
            return Err(Stopped);
        }
        fill_block(&mut decoder, &mut block);
        if block.is_empty() {
            break;
        }
        if let Err(e) = meters.add(&block) {
            return Ok(meter_refused(path, e));
        }
    }

    let ended = decoder.ended();
    let short = match ended {
        Some(Ended::Finished) => {
            let track = meters.into_track();
            return Ok(Read::Through { gain: track.gain(), peak: track.peak() });
        }
        Some(Ended::Failed) => short_of_end(path),
        Some(Ended::ShapeChanged) | None => Read::Failed,
    };
    log::debug!("Loudness not measured: {} stopped short ({ended:?})", path.display());
    Ok(short)
}

fn meter_refused(path: &Path, e: ebur128::Error) -> Read {
    log::debug!("Loudness not measured: the meter refused {}: {}", path.display(), describe(&e));
    Read::Failed
}

/// Leaves `block` empty once the decoder has ended.
fn fill_block(decoder: &mut FileDecoder, block: &mut Vec<f32>) {
    let channels = usize::from(decoder.channels().get());
    block.clear();
    block.extend(decoder.by_ref().take(BLOCK_FRAMES * channels));
    // The meters take whole frames only.
    block.truncate(block.len() - block.len() % channels);
}

/// Out of reach only where the file has gone: one still there would fail the same way at every
/// launch, so it is recorded instead.
fn short_of_end(path: &Path) -> Read {
    if matches!(path.try_exists(), Ok(true)) { Read::Failed } else { Read::OutOfReach }
}
