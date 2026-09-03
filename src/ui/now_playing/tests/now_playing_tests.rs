use std::sync::Arc;

use super::metadata::{format_channels, format_sample_rate};
use super::{NowPlayingSource, SourceKey};
use crate::player::engine::now_playing::SourceId;
use crate::player::tests::helpers::{test_station, test_track, test_view_model};

#[test]
fn sample_rate_drops_trailing_zero() {
    assert_eq!(format_sample_rate(44_100), "44.1 kHz");
    assert_eq!(format_sample_rate(48_000), "48 kHz");
    assert_eq!(format_sample_rate(96_000), "96 kHz");
    assert_eq!(format_sample_rate(88_200), "88.2 kHz");
}

#[test]
fn channels_have_friendly_names() {
    assert_eq!(format_channels(1), "Mono");
    assert_eq!(format_channels(2), "Stereo");
    assert_eq!(format_channels(6), "6 channels");
}

/// The key the subscriber would have built for `id`, which is what `describes` answers without
/// building. Spelled here so the tests below assert the equivalence rather than restate the match.
fn key_for(id: SourceId<'_>) -> SourceKey {
    match id {
        SourceId::Track(track_id) => SourceKey::Track(track_id),
        SourceId::Station(stream_url) => SourceKey::Station(stream_url.to_owned()),
    }
}

#[test]
fn describes_answers_the_key_compare_it_replaced() {
    // The subscriber runs this on every player emit in place of building a `SourceKey` and
    // comparing it, so the two have to agree over the whole cross product or a source change is
    // either missed or re-applied every tick.
    let ids = [
        None,
        Some(SourceId::Track(1)),
        Some(SourceId::Track(2)),
        Some(SourceId::Station("http://example.test/a.mp3")),
        Some(SourceId::Station("http://example.test/b.mp3")),
    ];

    for held in ids {
        let held = held.map(key_for);
        for id in ids {
            assert_eq!(
                SourceKey::describes(held.as_ref(), id),
                held == id.map(key_for),
                "held {held:?} against {id:?}"
            );
        }
    }
}

#[test]
fn a_station_hands_the_chips_no_row_of_its_own() {
    // The chips come off an eight-column projection of a `tracks` row, and a stream has none — so
    // the row has to follow the arm the key came from rather than be read off `vm` beside it.
    // Both halves set is unreachable through the state machine, which is why nothing else catches
    // a projection that asks `current_track` independently.
    let station = test_station("Night Radio");
    let stream_url = station.stream_url.clone();
    let mut station = Arc::unwrap_or_clone(station);
    station.artwork_path = Some("logo.png".to_owned());

    let vm = test_view_model(
        Some(test_track("Nocturne", Some("Field"), Some("Airs"))),
        Some(Arc::new(station)),
        200_000,
    );
    let projected = NowPlayingSource::from_vm(&vm).map(|s| (s.key, s.track, s.artwork_path));

    assert_eq!(
        projected,
        Some((SourceKey::Station(stream_url), None, Some("logo.png".to_owned())))
    );
}
