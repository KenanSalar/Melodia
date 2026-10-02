//! Tests for the Output card's pickers: what a pick changes, what a listing of the cards shows, and
//! the chip orders Rust and the card each spell. Applying a pick opens a device, and is tested by
//! hand.

use std::time::Duration;

use slint::SharedString;

use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::{
    Drive, ExclusiveTuning, OutputDevice, OutputMode,
};
use melodia_testkit::{array_body, binding_value, strip_line_comments};

use super::{PERIOD_PRESETS, Picked, mode_from_index, mode_index, period_index};

const OUTPUT_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/output-section.slint");
const SETTINGS: &str = include_str!("../../../../../melodia-ui/ui/settings.slint");

fn card(name: &str) -> OutputDevice {
    OutputDevice { id: format!("hw:CARD={name},DEV=0"), name: name.to_owned() }
}

fn cards(names: &[&str]) -> Vec<OutputDevice> {
    names.iter().map(|&name| card(name)).collect()
}

/// The picker with `saved` as the card last chosen, before any listing has landed.
fn picked(saved: Option<&str>) -> Picked {
    Picked {
        choice: OutputChoice { device: saved.map(|name| card(name).id), ..OutputChoice::default() },
        devices: Vec::new(),
        default_label: None,
        listing_asked: 0,
        listing_applied: 0,
    }
}

fn options_named(names: &[&str]) -> Vec<SharedString> {
    names.iter().map(|&name| SharedString::from(name)).collect()
}

/// The arguments of each `@tr(…)` in the inline `options` list of the chip group whose selection
/// is `index_property`, in the order the card shows them.
fn chip_options(src: &str, index_property: &str) -> Vec<String> {
    let binding = format!("selected-index <=> {index_property};");
    let head = src.split_once(binding.as_str()).map_or("", |(head, _)| head);
    let list = head.rfind("options: [").and_then(|open| array_body(&head[open..], "options: ["));
    list.map_or_else(Vec::new, |body| {
        body.split("@tr(")
            .skip(1)
            .filter_map(|call| call.split_once(')'))
            .map(|(args, _)| args.to_owned())
            .collect()
    })
}

/// A listing answered after a newer one would put back cards the picker has already moved past.
#[test]
fn a_listing_older_than_the_one_on_screen_leaves_the_picker_alone() {
    let mut picked = picked(None);
    picked.take_listing(cards(&["Newer"]), None, 2);

    let late = picked.take_listing(cards(&["Older"]), None, 1);

    assert!(late.is_none(), "an older listing reached the picker");
}

/// The row clicked is the row on screen, so a listing arriving out of order can't retarget a pick
/// at a card the user never saw there.
#[test]
fn a_device_pick_resolves_against_the_listing_on_screen() {
    let mut picked = picked(None);
    picked.take_listing(cards(&["USB DAC", "HDA Intel"]), None, 2);
    picked.take_listing(cards(&["HDA Intel", "USB DAC"]), None, 1);

    picked.pick_device(0);

    assert_eq!(picked.choice.device, Some(card("USB DAC").id));
}

/// The picker lists every couple of seconds while it is on screen, and a fresh model each time
/// would rebuild every row under an open popup for nothing. Only a change in the cards is news.
#[test]
fn the_options_are_rebuilt_only_when_the_cards_change() {
    let mut picked = picked(None);

    let first = picked.take_listing(cards(&["HDA Intel"]), None, 1).and_then(|shown| shown.names);
    let unchanged =
        picked.take_listing(cards(&["HDA Intel"]), None, 2).and_then(|shown| shown.names);
    let plugged_in = picked
        .take_listing(cards(&["HDA Intel", "USB DAC"]), None, 3)
        .and_then(|shown| shown.names);

    assert_eq!(first, Some(options_named(&["HDA Intel"])), "the first listing");
    assert_eq!(unchanged, None, "the same cards rebuilt the options");
    assert_eq!(plugged_in, Some(options_named(&["HDA Intel", "USB DAC"])), "a card plugged in");
}

/// The picker shows the card a claim would take. A saved card that isn't connected selects nothing
/// and says so, since the claim won't find it; with none saved the claim takes the first card
/// listed, so that is the row lit.
#[test]
fn the_picker_lights_the_card_a_claim_would_take() {
    let rows = [
        (
            "the saved card, listed second",
            Some("USB DAC"),
            cards(&["HDA Intel", "USB DAC"]),
            (1, false),
        ),
        ("a saved card unplugged", Some("USB DAC"), cards(&["HDA Intel"]), (-1, true)),
        ("a saved card and no cards", Some("USB DAC"), Vec::new(), (-1, true)),
        ("none saved", None, cards(&["HDA Intel", "USB DAC"]), (0, false)),
        ("none saved and no cards", None, Vec::new(), (-1, false)),
    ];
    for (what, saved, listed, expected) in rows {
        let mut picked = picked(saved);

        let shown = picked.take_listing(listed, None, 1);

        let lit = shown.map(|shown| (shown.selected, shown.missing));
        assert_eq!(lit, Some(expected), "{what}: (row lit, not connected)");
    }
}

/// A period chip changes the period and nothing else about the pacing.
#[test]
fn a_period_pick_keeps_the_writer_polling() {
    let mut picked = picked(None);
    picked.pick_polling(true);

    picked.pick_period(4);

    let expected = ExclusiveTuning::new(Duration::from_millis(100), Drive::Polling);
    assert_eq!(picked.choice.tuning, expected);
}

/// Each end of the chip row and a step past either. An index no chip has leaves the period where it
/// was rather than resetting it.
#[test]
fn each_period_chip_picks_its_preset_and_no_other_index_moves_the_period() {
    let rows = [
        (0, Duration::from_millis(5)),
        (4, Duration::from_millis(100)),
        (-1, ExclusiveTuning::DEFAULT_PERIOD),
        (5, ExclusiveTuning::DEFAULT_PERIOD),
    ];
    for (idx, expected) in rows {
        let mut picked = picked(None);

        picked.pick_period(idx);

        assert_eq!(picked.choice.tuning.period, expected, "index {idx}");
    }
}

/// A period hand-edited to one no chip offers lights none, rather than a neighbour that would claim
/// a pacing the writer isn't using.
#[test]
fn a_saved_period_lights_its_own_chip_or_none() {
    let rows = [(5, 0), (10, 1), (20, 2), (50, 3), (100, 4), (7, -1), (2, -1)];
    for (millis, expected) in rows {
        assert_eq!(period_index(Duration::from_millis(millis)), expected, "{millis} ms");
    }
}

/// `PERIOD_PRESETS` and the card's inline chip list are one order spelled twice, which the card
/// says in a comment and nothing else checks. Drifted, a chip picks a period it doesn't show.
#[test]
fn each_period_chip_shows_the_preset_it_picks() {
    let src = strip_line_comments(OUTPUT_SECTION);

    let shown = chip_options(&src, "Settings.output-period-idx");

    let presets: Vec<String> = PERIOD_PRESETS
        .iter()
        .map(|period| format!("\"{{}} ms\", {}", period.as_millis()))
        .collect();
    assert_eq!(shown, presets);
}

/// Only the Exclusive chip takes the card from every other app on the system, so an index no chip
/// has falls to shared.
#[test]
fn only_the_exclusive_chip_claims_the_card() {
    let rows = [
        (0, OutputMode::Shared),
        (1, OutputMode::Exclusive),
        (-1, OutputMode::Shared),
        (2, OutputMode::Shared),
    ];
    for (idx, expected) in rows {
        assert_eq!(mode_from_index(idx), expected, "index {idx}");
    }
}

/// `mode_index` and the card's chip list are one order spelled twice. Drifted, the card lights the
/// other chip for the saved mode, and picking Shared takes the card.
#[test]
fn each_mode_chip_names_the_mode_at_its_index() {
    let src = strip_line_comments(OUTPUT_SECTION);
    let chips = chip_options(&src, "Settings.output-mode-idx");
    for (mode, label) in
        [(OutputMode::Shared, "\"Shared\""), (OutputMode::Exclusive, "\"Exclusive\"")]
    {
        let chip = usize::try_from(mode_index(mode)).ok().and_then(|i| chips.get(i));

        assert_eq!(chip.map(String::as_str), Some(label), "{mode:?}");
    }
}

/// The card's exclusive rows hang off `Settings.exclusive-chosen`, which reads the mode chip's index
/// itself, so it has to name the index Rust gives Exclusive or the rows answer the other chip.
#[test]
fn the_settings_global_reads_the_exclusive_chip_as_exclusive() {
    let src = strip_line_comments(SETTINGS);
    let chosen = binding_value(&src, "out property <bool> exclusive-chosen:");

    let reads = format!("root.output-mode-idx == {}", mode_index(OutputMode::Exclusive));

    assert!(chosen.contains(&reads), "exclusive-chosen reads {chosen:?}");
}
