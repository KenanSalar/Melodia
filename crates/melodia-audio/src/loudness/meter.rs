//! The meters a decode is fed through: its track's, and its album's for the layout it decodes to.

use ebur128::{EbuR128, Error, Mode};
use melodia_core::entities::loudness::REPLAYGAIN_REFERENCE_LUFS;
use symphonia::core::audio::Position;

use super::Gain;
use super::channels::channel_map;
use crate::player::source::audio::{AudioSource, Shape};
use crate::player::source::file_decode::FileDecoder;

/// What a meter is built against.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Layout {
    shape: Shape,
    positions: Option<Position>,
}

impl Layout {
    pub(super) fn of(decoder: &FileDecoder) -> Self {
        Self { shape: decoder.shape(), positions: decoder.channel_positions() }
    }
}

pub(super) struct MemberMeters<'a> {
    track: TrackMeter,
    album: Option<&'a mut AlbumMeter>,
}

impl<'a> MemberMeters<'a> {
    /// Fails on a channel count or rate the crate refuses.
    pub(super) fn new(layout: Layout, album: Option<&'a mut AlbumMeters>) -> Result<Self, Error> {
        let track = TrackMeter(meter(layout, Mode::I | Mode::SAMPLE_PEAK)?);
        let album = album.map(|meters| meters.meter_for(layout)).transpose()?;
        Ok(Self { track, album })
    }

    /// `frames` interleaved, whole frames only.
    pub(super) fn add(&mut self, frames: &[f32]) -> Result<(), Error> {
        self.track.0.add_frames_f32(frames)?;
        if let Some(album) = self.album.as_deref_mut() {
            album.0.add_frames_f32(frames)?;
        }
        Ok(())
    }

    pub(super) fn into_track(self) -> TrackMeter {
        self.track
    }
}

/// Exact rather than histogram: its block history is bounded by the one file.
pub(super) struct TrackMeter(EbuR128);

impl TrackMeter {
    pub(super) fn gain(&self) -> Option<Gain> {
        levelled(self.0.loudness_global().ok()?, self.peak())
    }

    pub(super) fn peak(&self) -> f64 {
        (0..self.0.channels())
            .filter_map(|channel| self.0.sample_peak(channel).ok())
            .fold(0.0, f64::max)
    }
}

/// An album's meters, one per layout, fed every member in disc and track order so the blocks across
/// a join hear the album played through. The relative gate runs over the whole album's blocks,
/// which the crate never hands out for a later sum, and histogram mode holds each meter to a fixed
/// size however long the album runs.
#[derive(Default)]
pub(super) struct AlbumMeters {
    meters: Vec<(Layout, AlbumMeter)>,
}

pub(super) struct AlbumMeter(EbuR128);

impl AlbumMeters {
    fn meter_for(&mut self, layout: Layout) -> Result<&mut AlbumMeter, Error> {
        if let Some(index) = self.meters.iter().position(|(held, _)| *held == layout) {
            return Ok(&mut self.meters[index].1);
        }
        let album = AlbumMeter(meter(layout, Mode::I | Mode::HISTOGRAM)?);
        let (_, album) = self.meters.push_mut((layout, album));
        Ok(album)
    }

    /// The album's gain, `peak` being its loudest member's.
    pub(super) fn gain(&self, peak: f64) -> Option<Gain> {
        let meters = self.meters.iter().map(|(_, album)| &album.0);
        levelled(EbuR128::loudness_global_multiple(meters).ok()?, peak)
    }
}

fn meter(layout: Layout, mode: Mode) -> Result<EbuR128, Error> {
    let channels = layout.shape.channels.get();
    let mut meter = EbuR128::new(u32::from(channels), layout.shape.rate.get(), mode)?;
    if let Some(map) = channel_map(usize::from(channels), layout.positions) {
        meter.set_channel_map(&map)?;
    }
    Ok(meter)
}

/// `None` for a loudness that isn't finite: silence and audio under one gating block answer
/// `-inf`, and NaN input answers NaN.
fn levelled(loudness: f64, peak: f64) -> Option<Gain> {
    if !loudness.is_finite() {
        return None;
    }
    Some(Gain { db: REPLAYGAIN_REFERENCE_LUFS - loudness, peak })
}
