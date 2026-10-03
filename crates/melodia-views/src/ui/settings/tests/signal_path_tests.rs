//! Tests for the signal path's words, whose order Rust and the Output card each spell.

use melodia_engine::player::engine::signal_path::Verdict;
use melodia_playback::player::playback::output::claim::FallbackReason;
use melodia_testkit::{binding_value, strip_line_comments};

use super::{fallback_index, verdict_index};

const OUTPUT_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/output-section.slint");

const SIGNAL_PATH: &str = include_str!("../../../../../melodia-ui/ui/globals/signal-path.slint");

/// The words `chain` lands on for `property` at `index`, as Slint evaluates it: the first arm
/// testing that index, or the closing default where none does.
fn words_for<'a>(chain: &'a str, property: &str, index: i32) -> &'a str {
    let arm = format!("{property} == {index} ");
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
        let landed = words_for(chain, "fallback-reason", fallback_index(&reason));

        assert!(landed.contains(words), "{reason:?} landed on {landed:?}");
    }
}

/// `verdict_index` and the global's two verdict chains are one order spelled three times. Drifted,
/// the panel and the quality chip call a fallback lossy or a converted path bit-perfect. Converted
/// is the closing default in both, so it is the one a missing arm lands on silently.
#[test]
fn every_verdict_lands_on_its_own_words_in_the_panel_and_the_chip() {
    let src = strip_line_comments(SIGNAL_PATH);
    let text = binding_value(&src, "out property <string> verdict-text:");
    let label = binding_value(&src, "out property <string> verdict-label:");
    let rows = [
        (Verdict::BitPerfect, "Bit-perfect"),
        (Verdict::Enhanced, "Enhanced"),
        (Verdict::Converted, "Converted"),
        (Verdict::Fallback, "Fallback"),
        (Verdict::Lossy, "Lossy"),
    ];
    for (verdict, words) in rows {
        let index = verdict_index(verdict);

        let landed = (words_for(text, "verdict", index), words_for(label, "verdict", index));

        assert!(
            landed.0.contains(words) && landed.1.contains(words),
            "{verdict:?} landed on {landed:?}"
        );
    }
}
