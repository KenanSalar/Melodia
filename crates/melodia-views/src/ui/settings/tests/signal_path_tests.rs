//! Tests for the signal path's words, whose order Rust and the Output card each spell.

use melodia_playback::player::playback::output::claim::FallbackReason;
use melodia_testkit::{binding_value, strip_line_comments};

use super::fallback_index;

const OUTPUT_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/output-section.slint");

/// The words `chain` lands on for `index`, as Slint evaluates it: the first arm testing that
/// index, or the closing default where none does.
fn words_for(chain: &str, index: i32) -> &str {
    let arm = format!("fallback-reason == {index} ");
    chain.lines().find(|line| line.contains(&arm)).or_else(|| chain.lines().last()).unwrap_or("")
}

/// `fallback_index` and the card's inline reason list are one order spelled twice, and nothing
/// fails when they drift: the card just names another reason. `NotAllowed` is the one to watch,
/// numbered after `Io`, which has no arm of its own, and produced only on Windows, where its words
/// point at the Sound control panel box that fixes it.
#[test]
fn every_fallback_reason_lands_on_its_own_words_in_the_output_card() {
    let src = strip_line_comments(OUTPUT_SECTION);
    let chain = binding_value(&src, "property <string> fallback-text:");
    let rows = [
        (FallbackReason::Busy, "in use by another app"),
        (FallbackReason::Reserved { by: String::new() }, "holds the device"),
        (FallbackReason::RateRefused, "sample rate"),
        (FallbackReason::ChannelsRefused, "many channels"),
        (FallbackReason::FormatRefused, "sample format"),
        (FallbackReason::NotConnected, "isn't connected"),
        (FallbackReason::Unsupported, "isn't available on this system"),
        (FallbackReason::Io, "couldn't be opened"),
        (FallbackReason::NotAllowed, "doesn't allow exclusive control"),
    ];
    for (reason, words) in rows {
        let landed = words_for(chain, fallback_index(&reason));

        assert!(landed.contains(words), "{reason:?} landed on {landed:?}");
    }
}
