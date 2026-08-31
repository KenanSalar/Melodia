//! Shared fixtures for the player's DSP tests.
//!
//! Five things were being written out more than once, so each lives here:
//! [`TestSource`] and its `NonZero` wrappers, which the equalizer and
//! visualizer suites both drain to compare a source's output against its input;
//! the float comparisons, which four suites need to check a value that is exact
//! in principle but rides through float maths; [`fill_sine`], which the
//! spectrum and waveform suites both feed a known tone; [`test_station`], which
//! the four transport suites tune to, as does the now-playing ladder's; and
//! [`test_track`] beside [`test_view_model`], the deck that ladder's two suites
//! — one here, one under `ui/` — both hand a source to.
//!
//! The crossfade suite deliberately takes none of it: it exercises pure
//! predicates and the ramp cell, so it needs no source, and it holds a tighter
//! tolerance than [`approx_eq`] can — its gains are plain arithmetic on a
//! counter rather than the output of a filter.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use crate::entities::track::TrackSummary;
use crate::player::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError};
use crate::player::state::PlayerViewModelLight;
use crate::player::types::RadioNowPlaying;

pub(crate) fn nz_u16(v: u16) -> ChannelCount {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u16>::MIN,
    }
}

pub(crate) fn nz_u32(v: u32) -> SampleRate {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u32>::MIN,
    }
}

/// Scalar near-equality — avoids `clippy::float_cmp` on checks that are exact
/// in principle (clamp bounds, preset values) but ride through float maths.
pub(crate) fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// [`approx_eq`] as an assertion, reporting both sides on failure.
pub(crate) fn assert_approx(a: f32, b: f32) {
    assert!(approx_eq(a, b), "expected {b}, got {a}");
}

/// Bit pattern of each sample — lets a test assert *bit-identical* passthrough
/// (and divergence) without a float `==`.
pub(crate) fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// Fill `buf` with a sine of `freq_hz` at `sample_rate`, scaled to `amplitude`.
pub(crate) fn fill_sine(buf: &mut [f32], freq_hz: f32, sample_rate: f32, amplitude: f32) {
    for (i, sample) in buf.iter_mut().enumerate() {
        let phase = 2.0 * std::f32::consts::PI * freq_hz * crate::player::dsp::index_to_f32(i)
            / sample_rate;
        *sample = amplitude * phase.sin();
    }
}

/// In-memory source. `try_seek` rewinds to the start, like a decoder seeking to
/// 0, so a post-seek run can be compared against a fresh one.
pub(crate) struct TestSource {
    data: Vec<f32>,
    pos: usize,
    channels: u16,
    sample_rate: u32,
}

impl TestSource {
    pub(crate) fn new(data: Vec<f32>, channels: u16, sample_rate: u32) -> Self {
        Self {
            data,
            pos: 0,
            channels,
            sample_rate,
        }
    }
}

impl Iterator for TestSource {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let s = self.data.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl AudioSource for TestSource {
    fn channels(&self) -> ChannelCount {
        nz_u16(self.channels)
    }
    fn sample_rate(&self) -> SampleRate {
        nz_u32(self.sample_rate)
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        self.pos = 0;
        Ok(())
    }
}

/// A station for the transport tests, with only what they assert on filled in.
///
/// Four suites were spelling this out — three under `player/` and one under `library/` — so a
/// field added to `RadioNowPlaying` broke all four the same way. The display facts are left empty
/// deliberately: nothing below the UI layer reads them, so a fixture carrying them would suggest
/// the transport cares.
pub fn test_station(name: &str) -> Arc<RadioNowPlaying> {
    Arc::new(RadioNowPlaying {
        station_id: 42,
        station_uuid: None,
        name: name.to_owned(),
        stream_url: "http://example.test/live.mp3".to_owned(),
        artwork_path: None,
        live_title: None,
        buffering: false,
        country: None,
        tags: None,
        homepage: None,
        codec: None,
        bitrate: 0,
        play_count: 0,
    })
}

/// A track for the suites that need one on the deck rather than a decodable file, with only the
/// tagged fields varying. Shares [`test_station`]'s reason for existing: `TrackSummary` has
/// seventeen columns and the two suites reading this one care about four.
pub fn test_track(title: &str, artist: Option<&str>, album: Option<&str>) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id: 7,
        file_path: String::new(),
        file_name: String::new(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album: album.map(str::to_owned),
        duration_ms: 200_000,
        artwork_path: Some("cover.jpg".to_owned()),
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// A published view model carrying `current_track`, `radio` and the player's own `duration_ms`,
/// which is the trio anything asking what is on the deck reads. Everything else is left at a
/// resting value: a test that wants one sets it on the returned struct, so a field added to
/// `PlayerViewModelLight` lands here rather than in every suite that builds one.
pub fn test_view_model(
    current_track: Option<Arc<TrackSummary>>,
    radio: Option<Arc<RadioNowPlaying>>,
    duration_ms: u64,
) -> PlayerViewModelLight {
    PlayerViewModelLight {
        status: "playing",
        current_track,
        position_ms: 0,
        duration_ms,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        radio,
        has_next: false,
        has_previous: false,
    }
}
