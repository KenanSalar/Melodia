//! The weight each speaker a decoded file names takes, against BS.1770's.

use ebur128::Channel::{Center, DualMono, LeftSurround, RightSurround, Unused};
use symphonia::core::audio::Position;

use super::channel_map;

const FRONT: Position = Position::FRONT_LEFT.union(Position::FRONT_RIGHT);
const SURROUND_51: Position = FRONT.union(Position::FRONT_CENTER).union(Position::LFE1);
const SIDES: Position = Position::SIDE_LEFT.union(Position::SIDE_RIGHT);
const BACKS: Position = Position::REAR_LEFT.union(Position::REAR_RIGHT);

#[test]
fn mono_is_weighed_as_the_two_channels_it_plays_on() {
    for positions in [None, Some(Position::FRONT_CENTER)] {
        assert_eq!(channel_map(1, positions), Some(vec![DualMono]), "{positions:?}");
    }
}

/// The crate's map by count drops a 7.1 file's side pair, so a named layout weighs each speaker by
/// where it sits.
#[test]
fn each_named_speaker_takes_its_bs1770_weight() {
    let heights = Position::TOP_FRONT_LEFT | Position::TOP_FRONT_RIGHT;
    let cases = [
        ("stereo", FRONT, vec![Center, Center]),
        ("front of centre", FRONT | Position::FRONT_LEFT_CENTER, vec![Center, Center, Center]),
        ("heights", FRONT | heights, vec![Center, Center, Center, Center]),
        (
            "5.1 side",
            SURROUND_51 | SIDES,
            vec![Center, Center, Center, Unused, LeftSurround, RightSurround],
        ),
        (
            "5.1 back",
            SURROUND_51 | BACKS,
            vec![Center, Center, Center, Unused, LeftSurround, RightSurround],
        ),
        (
            "6.1",
            SURROUND_51 | Position::REAR_CENTER | SIDES,
            vec![Center, Center, Center, Unused, Center, LeftSurround, RightSurround],
        ),
        (
            "7.1",
            SURROUND_51 | BACKS | SIDES,
            vec![
                Center,
                Center,
                Center,
                Unused,
                LeftSurround,
                RightSurround,
                LeftSurround,
                RightSurround,
            ],
        ),
    ];

    for (layout, positions, expected) in cases {
        assert_eq!(channel_map(expected.len(), Some(positions)), Some(expected), "{layout}");
    }
}

#[test]
fn a_layout_that_names_no_speakers_or_the_wrong_number_takes_the_default() {
    for (channels, positions) in [(2, None), (6, Some(FRONT))] {
        assert_eq!(channel_map(channels, positions), None, "{channels} channels, {positions:?}");
    }
}
