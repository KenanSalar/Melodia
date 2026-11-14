//! What a measurement answers for a track and an album, against tones whose loudness BS.1770 fixes
//! by hand: a 1 kHz sine peaking at `A` on both front channels reads `20·log10(A)` LUFS (EBU Tech
//! 3341, case 1), so one peaking 24 dB below full scale is levelled by +6 dB.

use std::f64::consts::TAU;
use std::path::Path;

use tempfile::TempDir;

use super::{AlbumOutcome, Gain, Stopped, TrackOutcome, measure_album, measure_track};
use crate::player::source::tests::helpers::pcm_wav;
use melodia_core::entities::cue::{CueFrames, CueSpan};
use melodia_core::entities::track::TrackAudio;
use melodia_core::error::AppError;

/// EBU Tech 3341's tolerance on an integrated loudness, so on a gain, in dB.
const TOLERANCE_DB: f64 = 0.1;

const RATE: u32 = 48_000;

/// Whole cycles in every whole second at [`RATE`], so a tone joined to another stays in phase.
const TONE_HZ: f64 = 1_000.0;

const MONO: u16 = 1;
const STEREO: u16 = 2;

/// Each tone's length: well past the 400 ms gating block.
const SECONDS: u32 = 3;

const SILENT: f64 = f64::NEG_INFINITY;

/// A file of a tone per entry of `levels`, each peaking that many dB below full scale for
/// [`SECONDS`] on every one of `channels`.
fn tones(dir: &TempDir, name: &str, channels: u16, levels: &[f64]) -> Result<TrackAudio, AppError> {
    let mut samples = Vec::new();
    let mut frame = 0_u32;
    for level in levels {
        let amplitude = 10_f64.powf(level / 20.0);
        for _ in 0..SECONDS * RATE {
            let phase = TAU * TONE_HZ * f64::from(frame) / f64::from(RATE);
            let sample = sixteen_bit(amplitude * phase.sin());
            samples.extend(std::iter::repeat_n(sample, usize::from(channels)));
            frame += 1;
        }
    }
    let path = dir.path().join(name);
    pcm_wav(&path, channels, RATE, &samples)?;
    Ok(whole(&path))
}

#[expect(clippy::cast_possible_truncation, reason = "a tone's sample sits inside full scale")]
fn sixteen_bit(sample: f64) -> i16 {
    (sample * f64::from(i16::MAX)).round() as i16
}

fn whole(path: &Path) -> TrackAudio {
    TrackAudio { path: path.to_string_lossy().into_owned(), span: None }
}

fn unreadable(dir: &TempDir) -> Result<TrackAudio, AppError> {
    let path = dir.path().join("unreadable.wav");
    std::fs::write(&path, b"not audio")?;
    Ok(whole(&path))
}

fn gone(dir: &TempDir) -> TrackAudio {
    whole(&dir.path().join("gone.wav"))
}

/// The cue span from `start` to `end`, in seconds.
fn span(start: u32, end: Option<u32>) -> Result<CueSpan, AppError> {
    let frames = |seconds: u32| CueFrames::try_from(75 * i64::from(seconds));
    frames(start)
        .and_then(|start| CueSpan::new(start, end.map(frames).transpose()?))
        .map_err(|e| AppError::player("the test's cue span", e))
}

fn track_outcome(track: &TrackAudio) -> Result<TrackOutcome, AppError> {
    measure_track(track, || false).map_err(stopped_unasked)
}

fn album_outcome(members: &[&TrackAudio]) -> Result<AlbumOutcome, AppError> {
    measure_album(members.iter().copied(), || false).map_err(stopped_unasked)
}

fn stopped_unasked(_: Stopped) -> AppError {
    AppError::validation_msg("a measurement nothing stopped answered Stopped")
}

fn gain_of(outcome: TrackOutcome) -> Option<Gain> {
    match outcome {
        TrackOutcome::Measured(gain) => Some(gain),
        TrackOutcome::NoResult | TrackOutcome::OutOfReach => None,
    }
}

fn assert_near(what: &str, actual: Option<f64>, expected: f64, tolerance: f64) {
    let near = actual.is_some_and(|actual| (actual - expected).abs() <= tolerance);
    assert!(near, "{what}: {actual:?}, expected {expected} ±{tolerance}");
}

fn assert_levelled_by(what: &str, gain: Option<Gain>, expected_db: f64) {
    assert_near(what, gain.map(|gain| gain.db), expected_db, TOLERANCE_DB);
}

#[test]
fn a_track_is_levelled_to_the_replaygain_reference() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let track = tones(&dir, "tone.wav", STEREO, &[-24.0])?;

    let outcome = track_outcome(&track)?;

    assert_levelled_by("the track", gain_of(outcome), 6.0);
    Ok(())
}

#[test]
fn mono_is_measured_as_the_two_channels_it_plays_on() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let track = tones(&dir, "tone.wav", MONO, &[-24.0])?;

    let outcome = track_outcome(&track)?;

    assert_levelled_by("the mono track", gain_of(outcome), 6.0);
    Ok(())
}

/// The energy mean of −24 and −14 LUFS, equally long, is −16.60 LUFS, so −1.40 dB, where the mean
/// of the two tracks' gains would be +1.0 dB.
#[test]
fn an_album_is_measured_as_one_whole() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let quiet = tones(&dir, "quiet.wav", STEREO, &[-24.0])?;
    let loud = tones(&dir, "loud.wav", STEREO, &[-14.0])?;

    let outcome = album_outcome(&[&quiet, &loud])?;

    assert_levelled_by("the album", outcome.album, -1.40);
    Ok(())
}

#[test]
fn an_album_s_peak_is_its_loudest_member_s() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let quiet = tones(&dir, "quiet.wav", STEREO, &[-24.0])?;
    let loud = tones(&dir, "loud.wav", STEREO, &[-14.0])?;

    let outcome = album_outcome(&[&quiet, &loud])?;

    // −14 dBFS, to well within the half step 16 bits leave.
    assert_near("the album's peak", outcome.album.map(|album| album.peak), 0.199_5, 1e-4);
    Ok(())
}

/// A silent member is read to its end, so the album keeps its gain, and its blocks fall under the
/// absolute gate. The three blocks straddling the join hold ¾, ½ and ¼ of the tone, so the 30 above
/// the gate average 0.95 of −24 LUFS: −24.22 LUFS, levelled by +6.22 dB.
#[test]
fn a_silent_member_leaves_the_album_levelled_by_the_rest() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let tone = tones(&dir, "tone.wav", STEREO, &[-24.0])?;
    let silent = tones(&dir, "silent.wav", STEREO, &[SILENT])?;

    let outcome = album_outcome(&[&tone, &silent])?;

    assert_levelled_by("the album", outcome.album, 6.22);
    Ok(())
}

#[test]
fn a_track_with_no_level_or_no_file_answers_which() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let cases = [
        ("silent", tones(&dir, "silent.wav", STEREO, &[SILENT])?, TrackOutcome::NoResult),
        ("there but unreadable", unreadable(&dir)?, TrackOutcome::NoResult),
        ("gone", gone(&dir), TrackOutcome::OutOfReach),
    ];

    for (file, track, expected) in cases {
        assert_eq!(track_outcome(&track)?, expected, "{file}");
    }
    Ok(())
}

/// The lock lifts, so the track is left for a later launch rather than recorded as having nothing
/// to level, though the file is still there.
#[cfg(target_os = "windows")]
#[test]
fn a_track_held_open_without_sharing_is_left_for_a_later_launch() -> Result<(), AppError> {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = TempDir::new()?;
    let track = tones(&dir, "locked.wav", STEREO, &[-24.0])?;
    let _lock = std::fs::File::options().read(true).share_mode(0).open(&track.path)?;

    let outcome = track_outcome(&track)?;

    assert_eq!(outcome, TrackOutcome::OutOfReach);
    Ok(())
}

#[test]
fn a_member_short_of_its_end_leaves_the_album_without_a_gain() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let tone = tones(&dir, "tone.wav", STEREO, &[-24.0])?;
    let cases = [
        ("there but unreadable", unreadable(&dir)?, TrackOutcome::NoResult),
        ("gone", gone(&dir), TrackOutcome::OutOfReach),
    ];

    for (file, member, expected) in cases {
        let outcome = album_outcome(&[&tone, &member])?;
        let short = (outcome.tracks.get(1).copied(), outcome.album);
        assert_eq!(short, (Some(expected), None), "{file}");
    }
    Ok(())
}

#[test]
fn a_stopped_measurement_answers_nothing_partial() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let tone = tones(&dir, "tone.wav", STEREO, &[-24.0])?;

    let outcome = measure_album([&tone], || true);

    assert_eq!(outcome, Err(Stopped));
    Ok(())
}

#[test]
fn a_cue_track_is_measured_over_its_span_alone() -> Result<(), AppError> {
    let dir = TempDir::new()?;
    let image = tones(&dir, "image.wav", STEREO, &[-24.0, -14.0])?;
    let cases = [("first", span(0, Some(SECONDS))?, 6.0), ("last", span(SECONDS, None)?, -4.0)];

    for (track, span, expected_db) in cases {
        let audio = TrackAudio { span: Some(span), ..image.clone() };
        let outcome = track_outcome(&audio)?;
        assert_levelled_by(&format!("the {track} track"), gain_of(outcome), expected_db);
    }
    Ok(())
}
