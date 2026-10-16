//! Tests for the Output card's pickers: what a pick changes, what a listing of the cards shows, the
//! chip orders Rust and the card each spell, and the lock that orders two picks in flight. Applying
//! a pick opens a device, and is tested by hand.

use std::time::Duration;

use slint::SharedString;

use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::convert::Resampler;
use melodia_playback::player::playback::output::{
    Drive, ExclusiveTuning, OutputDevice, OutputMode,
};
use melodia_testkit::{array_body, binding_value, block_body, strip_line_comments};

use super::{
    PERIOD_PRESETS, Picked, Shadowed, mode_from_index, mode_index, period_index,
    resampler_from_index, resampler_index,
};

const OUTPUT_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/output-section.slint");
const SETTINGS: &str = include_str!("../../../../../melodia-ui/ui/settings.slint");
const OUTPUT_SETTINGS_RS: &str = include_str!("../output_settings.rs");

fn card(name: &str) -> OutputDevice {
    OutputDevice { id: format!("hw:CARD={name},DEV=0"), name: name.to_owned() }
}

fn cards(names: &[&str]) -> Vec<OutputDevice> {
    names.iter().map(|&name| card(name)).collect()
}

/// The picker with `saved` as the card last chosen, before any listing has landed.
fn picked(saved: Option<&str>) -> Picked {
    Picked::new(OutputChoice { device: saved.map(|name| card(name).id), ..OutputChoice::default() })
}

fn options_named(names: &[&str]) -> Vec<SharedString> {
    names.iter().map(|&name| SharedString::from(name)).collect()
}

/// The row leading the picker where it chooses for shared output too, Windows, which saves no
/// device.
fn system_default() -> SharedString {
    SharedString::from("System Default")
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

/// With the System Default row leading, none saved lights that row whatever is listed, since the
/// output follows the system default, and every device sits one row lower than it would without
/// it.
#[test]
fn with_the_system_default_row_the_picker_lights_where_the_audio_goes() {
    let rows = [
        (
            "the saved device, listed second",
            Some("USB DAC"),
            cards(&["HDA Intel", "USB DAC"]),
            (2, false),
        ),
        ("a saved device unplugged", Some("USB DAC"), cards(&["HDA Intel"]), (-1, true)),
        ("none saved", None, cards(&["HDA Intel", "USB DAC"]), (0, false)),
        ("none saved and no devices", None, Vec::new(), (0, false)),
    ];
    for (what, saved, listed, expected) in rows {
        let mut picked = picked(saved);

        let shown = picked.take_listing(listed, Some(system_default()), 1);

        let lit = shown.map(|shown| (shown.selected, shown.missing));
        assert_eq!(lit, Some(expected), "{what}: (row lit, not connected)");
    }
}

#[test]
fn the_system_default_row_leads_the_devices_as_listed() {
    let mut picked = picked(None);

    let options = picked
        .take_listing(cards(&["HDA Intel", "USB DAC"]), Some(system_default()), 1)
        .and_then(|shown| shown.names);

    assert_eq!(options, Some(options_named(&["System Default", "HDA Intel", "USB DAC"])));
}

/// The row is translated, so a language switch rewords it while the same devices are listed, and
/// only rebuilt options carry the new words to the picker.
#[test]
fn the_options_are_rebuilt_when_the_system_default_row_is_reworded() {
    let mut picked = picked(None);
    picked.take_listing(cards(&["HDA Intel"]), Some(system_default()), 1);

    let reworded = picked
        .take_listing(cards(&["HDA Intel"]), Some(SharedString::from("Systemstandard")), 2)
        .and_then(|shown| shown.names);

    assert_eq!(reworded, Some(options_named(&["Systemstandard", "HDA Intel"])));
}

/// The System Default row saves no device, which is what lets a pick go back to following the
/// system default, and each row under it saves the device listed there. The saved device is one
/// no row names, so every row's pick is a change.
#[test]
fn a_pick_under_the_system_default_row_saves_the_device_listed_there() {
    let rows = [(0, None), (1, Some(card("HDA Intel").id)), (2, Some(card("USB DAC").id))];
    for (row, expected) in rows {
        let mut picked = picked(Some("Bluetooth Headset"));
        picked.take_listing(cards(&["HDA Intel", "USB DAC"]), Some(system_default()), 1);

        picked.pick_device(row);

        assert_eq!(picked.choice.device, expected, "row {row}");
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

/// The chip order Rust and the card each spell, as for the mode's. Drifted, the card lights Linear
/// for a saved kernel, and picking Band-Limited picks linear.
#[test]
fn each_resampler_chip_names_the_resampler_at_its_index() {
    let src = strip_line_comments(OUTPUT_SECTION);
    let chips = chip_options(&src, "Settings.output-resampler-idx");
    for (resampler, label) in
        [(Resampler::Linear, "\"Linear\""), (Resampler::BandLimited, "\"Band-Limited\"")]
    {
        let chip = usize::try_from(resampler_index(resampler)).ok().and_then(|i| chips.get(i));

        assert_eq!(chip.map(String::as_str), Some(label), "{resampler:?}");
    }
}

/// The card is seeded from the saved resampler and a click hands back an index, so the two
/// directions have to agree or a launch lights one chip and a click on it picks the other.
#[test]
fn each_saved_resampler_lights_the_chip_that_picks_it_back() {
    for resampler in [Resampler::Linear, Resampler::BandLimited] {
        assert_eq!(resampler_from_index(resampler_index(resampler)), resampler);
    }
}

/// The Signal Path's sample rate row says how the rate is converted off
/// `Settings.resampler-linear`, which reads the chip's index itself.
#[test]
fn the_settings_global_reads_the_linear_chip_as_linear() {
    let src = strip_line_comments(SETTINGS);
    let linear = binding_value(&src, "out property <bool> resampler-linear:");

    let reads = format!("root.output-resampler-idx == {}", resampler_index(Resampler::Linear));

    assert!(linear.contains(&reads), "resampler-linear reads {linear:?}");
}

/// Two picks in flight run on the pool in either order, and whichever takes the lock last has to
/// apply and persist the newest. That holds only while the lock spans the apply, the engine and
/// the settings file each taking a lock of their own.
#[test]
fn apply_latest_holds_the_apply_lock_until_the_apply_returns() {
    let shadow = Shadowed::new(0);

    let held = shadow.apply_latest(|picked| *picked, |_| shadow.applying.try_lock().is_none());

    assert!(held, "a second task could take the apply lock while the first was applying");
}

/// A click takes the pick's own lock, and the apply behind it can be a device open.
#[test]
fn a_pick_can_land_while_an_apply_is_in_flight() {
    let shadow = Shadowed::new(0);

    let free = shadow.apply_latest(|picked| *picked, |_| shadow.picked.try_lock().is_some());

    assert!(free, "a click would wait out the device open in flight");
}

/// A task that captured its value races the next commit's task on the pool, and either can land
/// last, leaving the engine and the file on a value the user only passed through.
#[test]
fn every_pick_applied_on_the_pool_reads_its_shadow() {
    let src = strip_line_comments(OUTPUT_SETTINGS_RS);
    let applies: Vec<(&str, &str)> = src
        .match_indices("persist_blocking(")
        .filter_map(|(at, _)| {
            let label = src[at..].split('"').nth(1)?;
            let open = at + src[at..].find('{')?;
            Some((label, block_body(&src, open)?))
        })
        .collect();

    let captured: Vec<&str> = applies
        .iter()
        .filter(|(_, body)| !body.contains("apply_latest("))
        .map(|(label, _)| *label)
        .collect();

    assert!(applies.len() >= 4, "the walk lost the card's pool applies: {applies:?}");
    assert_eq!(captured, Vec::<&str>::new());
}
