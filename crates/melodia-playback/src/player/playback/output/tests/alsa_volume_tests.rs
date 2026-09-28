//! Tests for the card's volume element: which one a claim trusts, and the curve that has `alsamixer`
//! and Melodia's slider read the same percentage. Opening a mixer, and the device-0 rule in front
//! of it, need a card, and are tested by hand.

use super::{DB_GAIN_MUTE, Scale, pick, whole};

/// Inside half a percent, the two read the same whole percentage.
const SAME_PERCENT: f32 = 0.005;

fn elements(names: &[&str]) -> Vec<(String, u32)> {
    names.iter().map(|name| ((*name).to_owned(), 0)).collect()
}

/// `PCM` on an HD Audio card is usually a softvol that does nothing on `hw:`, and the elements past
/// `Master` each reach one output, so with no `Master` only a lone element is trusted.
#[test]
fn a_claim_takes_master_or_else_the_only_element() {
    let rows: [(&str, &[&str], Option<&str>); 5] = [
        ("an HD Audio card", &["Master", "Headphone", "PCM", "Front"], Some("Master")),
        ("Master listed late", &["Headphone", "Master"], Some("Master")),
        ("a USB DAC", &["PCM"], Some("PCM")),
        ("outputs with no Master", &["Headphone", "Speaker"], None),
        ("no volume element", &[], None),
    ];
    for (what, names, expected) in rows {
        let candidates = elements(names);

        let picked = pick(&candidates).map(|(name, _)| name.as_str());

        assert_eq!(picked, expected, "{what}");
    }
}

/// alsa-utils' rule: linear in dB up to 24 dB of range, the taper past it, and the raw steps where
/// the element states no dB range at all.
#[test]
fn the_scale_follows_the_range_the_element_states() {
    let rows = [
        ("no dB range", (0, 0), (0, 128), Scale::Steps { min: 0, max: 128 }),
        ("an inverted dB range", (100, 100), (0, 10), Scale::Steps { min: 0, max: 10 }),
        ("exactly 24 dB", (-2400, 0), (0, 64), Scale::Decibels { min: -2400, max: 0 }),
        ("just past 24 dB", (-2401, 0), (0, 64), Scale::Taper { min: -2401, max: 0 }),
        ("the ALC897's Master", (-6525, 0), (0, 87), Scale::Taper { min: -6525, max: 0 }),
    ];
    for (what, decibels, steps, expected) in rows {
        assert_eq!(Scale::from_ranges(decibels, steps), expected, "{what}");
    }
}

/// The percentages are what `amixer -M` printed for these cards: 71 % and 34 % for the PCM2902's
/// `PCM` at -9 and -28 dB, and 38 % for the ALC897's `Master` at -21.75 dB.
#[test]
fn a_reading_lands_on_the_percentage_alsamixer_shows() {
    let pcm2902 = Scale::Taper { min: -12800, max: 0 };
    let alc897 = Scale::Taper { min: -6525, max: 0 };
    let rows = [
        ("PCM2902 at -9 dB", pcm2902, -900, 0.71),
        ("PCM2902 at -28 dB", pcm2902, -2800, 0.34),
        ("ALC897 at -21.75 dB", alc897, -2175, 0.38),
        ("the top", alc897, 0, 1.0),
        ("the floor", alc897, -6525, 0.0),
        ("a silent floor", Scale::Taper { min: DB_GAIN_MUTE, max: 0 }, DB_GAIN_MUTE, 0.0),
        ("half on a silent floor", Scale::Taper { min: DB_GAIN_MUTE, max: 0 }, -1806, 0.5),
        ("linear in dB", Scale::Decibels { min: -2400, max: 0 }, -1200, 0.5),
        ("raw steps", Scale::Steps { min: 0, max: 128 }, 64, 0.5),
        ("a single step", Scale::Steps { min: 5, max: 5 }, 5, 0.0),
    ];
    for (what, scale, reading, expected) in rows {
        let level = scale.level_of(reading);

        assert!((level - expected).abs() < SAME_PERCENT, "{what}: {level} against {expected}");
    }
}

/// A level set and read back has to come back as itself, or Melodia's slider and `alsamixer` would
/// drift apart after every move.
#[test]
fn a_level_set_reads_back_as_itself() {
    let scales = [
        ("the PCM2902", Scale::Taper { min: -12800, max: 0 }),
        ("the ALC897", Scale::Taper { min: -6525, max: 0 }),
        ("a silent floor", Scale::Taper { min: DB_GAIN_MUTE, max: 0 }),
        ("linear in dB", Scale::Decibels { min: -2400, max: 0 }),
        ("fine raw steps", Scale::Steps { min: 0, max: 1000 }),
    ];
    for (what, scale) in scales {
        for level in [0.01, 0.1, 0.38, 0.5, 0.71, 1.0] {
            let reading = whole(scale.reading_for(level));

            let back = scale.level_of(reading);

            assert!((back - level).abs() < 0.001, "{what} at {level}: read back {back}");
        }
    }
}
