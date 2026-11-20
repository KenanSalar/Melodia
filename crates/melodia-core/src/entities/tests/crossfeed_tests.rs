use super::super::crossfeed::CrossfeedStrength;

/// The DSP maps a strength to an amount with no check of its own, so a value past either end, or
/// one that isn't a number, has to stop here.
#[test]
fn a_strength_past_either_end_of_the_slider_is_refused() {
    let cases = [
        ("just under subtle", -0.01, false),
        ("subtle", 0.0, true),
        ("strong", 1.0, true),
        ("just past strong", 1.01, false),
        ("not a number", f32::NAN, false),
    ];
    for (what, strength, accepted) in cases {
        assert_eq!(CrossfeedStrength::try_from(strength).is_ok(), accepted, "{what}");
    }
}
