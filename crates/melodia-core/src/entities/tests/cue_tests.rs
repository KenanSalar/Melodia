//! The newtypes a cue sheet's numbers and times are held in, and the one conversion of a time to
//! the decoder's samples.

use std::num::NonZero;

use super::*;

/// A rate from a literal, the rows naming only rates above zero.
fn hz(rate: u32) -> NonZero<u32> {
    NonZero::new(rate).unwrap_or(NonZero::<u32>::MIN)
}

fn frames(count: u32) -> CueFrames {
    CueFrames(count)
}

#[test]
fn a_track_number_holds_zero_to_ninety_nine() {
    let rows =
        [(-1, false), (0, true), (1, true), (98, true), (99, true), (100, false), (256, false)];
    for (number, holds) in rows {
        assert_eq!(CueTrack::try_from(number).is_ok(), holds, "TRACK {number}");
    }
}

#[test]
fn a_refused_track_number_names_itself() {
    assert!(matches!(CueTrack::try_from(100), Err(CueError::Track(100))));
}

#[test]
fn the_hidden_track_is_number_zero() {
    assert_eq!(CueTrack::HIDDEN.number(), 0);
}

#[test]
fn a_frame_count_holds_zero_to_u32_max() {
    let max = i64::from(u32::MAX);
    let rows = [(-1, false), (0, true), (max, true), (max + 1, false)];
    for (count, holds) in rows {
        assert_eq!(CueFrames::try_from(count).is_ok(), holds, "{count} frames");
    }
}

#[test]
fn a_cue_frame_is_a_whole_number_of_samples_at_the_common_rates() {
    let rows = [
        (44_100, 588),
        (48_000, 640),
        (88_200, 1_176),
        (96_000, 1_280),
        (176_400, 2_352),
        (192_000, 2_560),
    ];
    for (rate, samples) in rows {
        assert_eq!(frames(1).to_frames(hz(rate)), samples, "{rate} Hz");
    }
}

#[test]
fn a_rate_that_does_not_divide_by_75_floors() {
    let rows = [(32_000, 426), (16_000, 213), (8_000, 106)];
    for (rate, samples) in rows {
        assert_eq!(frames(1).to_frames(hz(rate)), samples, "{rate} Hz");
    }
}

#[test]
fn the_largest_time_converts_at_192_khz_without_overflow() {
    assert_eq!(frames(u32::MAX).to_frames(hz(192_000)), 10_995_116_275_200);
}

#[test]
fn millis_round_down() {
    let rows = [(0, 0), (1, 13), (74, 986), (75, 1_000), (76, 1_013)];
    for (count, millis) in rows {
        assert_eq!(frames(count).millis(), millis, "{count} frames");
    }
}

#[test]
fn a_span_has_to_end_after_it_starts() {
    let rows = [(Some(9), false), (Some(10), false), (Some(11), true), (None, true)];
    for (end, holds) in rows {
        let span = CueSpan::new(frames(10), end.map(frames));
        assert_eq!(span.is_ok(), holds, "10..{end:?}");
    }
}

#[test]
fn a_refused_span_names_both_ends() {
    let refused = CueSpan::new(frames(10), Some(frames(10)));
    assert!(matches!(refused, Err(CueError::Span { start: 10, end: 10 })));
}

#[test]
fn a_span_played_to_the_end_of_the_file_keeps_its_start() -> Result<(), CueError> {
    let span = CueSpan::new(frames(10), Some(frames(20)))?;

    let to_end = span.to_end_of_file();

    assert_eq!((to_end.start(), to_end.end()), (frames(10), None));
    Ok(())
}
