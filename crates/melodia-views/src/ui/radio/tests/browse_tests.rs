//! Which browsed station a card's selection key names.
//!
//! A browsed row has no id, so a selection names it by its place in the answer: [`select_key_at`]
//! writes the key into the card and [`resolve_keys`] reads it back for every batch action and copy.
//! Nothing on screen reports the two disagreeing by one; the menu just stars or copies the station
//! beside the one picked.

use super::*;

/// One directory station, named after its uuid so a failure says which one resolved.
fn directory(uuid: &str) -> DirectoryStation {
    DirectoryStation {
        station_uuid: uuid.to_owned(),
        name: uuid.to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        state: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        votes: 0,
        click_count: 0,
        last_check_ok: true,
    }
}

fn page(uuids: &[&str], has_more: bool) -> StationPage {
    StationPage { stations: uuids.iter().copied().map(directory).collect(), has_more }
}

/// A Browse cache holding one landed page of `uuids`.
fn browsing(uuids: &[&str]) -> RadioUi {
    let radio_ui = RadioUi::new(false, None);
    radio_ui.browse.lock().stations = page(uuids, false).stations;
    radio_ui
}

/// Asks for a page and lands `answer` as what came back, reporting whether it was taken.
fn land(browse: &mut BrowseState, answer: StationPage, append: bool) -> bool {
    let Some((_, generation)) = browse.begin(append) else { return false };
    browse.finish(generation, answer, append)
}

/// The uuids `resolve_keys` hands back, in the order it hands them.
fn resolved(radio_ui: &RadioUi, keys: &[i32]) -> Vec<String> {
    resolve_keys(radio_ui, keys.iter().copied())
        .into_iter()
        .map(|(station, _)| station.station_uuid)
        .collect()
}

/// The key `apply` stamps on a card and the station `resolve_keys` answers with are worked out in
/// two places, so this is what holds them to the same numbering. Handed back in the order asked,
/// since that is the order a Copy lists them in.
#[test]
fn the_key_a_card_is_stamped_with_names_that_cards_station() {
    let radio_ui = browsing(&["first", "second", "third"]);

    assert_eq!(resolved(&radio_ui, &[select_key_at(2), select_key_at(0)]), ["third", "first"]);
}

#[test]
fn the_first_and_last_places_both_resolve() {
    let radio_ui = browsing(&["first", "second", "third"]);

    assert_eq!(resolved(&radio_ui, &[1, 3]), ["first", "third"]);
}

/// `0` is the key a selection refuses and so the one no card may carry. One past the end is a key
/// left over from a longer answer.
#[test]
fn a_key_outside_the_answer_resolves_to_nothing() {
    let radio_ui = browsing(&["first", "second", "third"]);

    assert!(resolved(&radio_ui, &[0, 4, -1, i32::MIN]).is_empty());
}

/// What lets a Browse selection outlive a Load More, where a fresh search has to drop it.
#[test]
fn a_page_appended_keeps_every_key_the_first_page_handed_out() {
    let radio_ui = RadioUi::new(false, None);
    let landed = {
        let mut browse = radio_ui.browse.lock();
        [
            land(&mut browse, page(&["first", "second"], true), false),
            land(&mut browse, page(&["third"], false), true),
        ]
    };
    assert_eq!(landed, [true, true], "both pages answer the current query");

    assert_eq!(resolved(&radio_ui, &[select_key_at(1)]), ["second"]);
}
