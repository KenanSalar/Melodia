use std::sync::Arc;

use super::{NowPlayingSource, SourceKey, Surfaces};
use melodia_engine::player::engine::fixtures::{test_station, test_track, test_view_model};
use melodia_engine::player::engine::now_playing::SourceId;
use melodia_ui::MiniLayout;

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

const fn on_screen(
    open: bool,
    mini_visible: bool,
    mini_layout: MiniLayout,
    mini_backdrop: bool,
) -> Surfaces {
    Surfaces { open, mini_visible, mini_layout, mini_backdrop }
}

/// What is on screen beside what the two gates answer, `(surfaces, artwork, panel)`. The corners:
/// Now Playing alone and open under the strip, a hidden miniplayer whose mirrors still name a
/// layout, and every layout with its backdrop off and on.
const RENDERED: [(Surfaces, bool, bool); 9] = [
    (on_screen(true, false, MiniLayout::Strip, false), true, true),
    (on_screen(true, true, MiniLayout::Strip, false), true, true),
    (on_screen(false, false, MiniLayout::Column, true), false, false),
    (on_screen(false, true, MiniLayout::Strip, false), false, false),
    (on_screen(false, true, MiniLayout::Strip, true), true, false),
    (on_screen(false, true, MiniLayout::Card, false), true, false),
    (on_screen(false, true, MiniLayout::Card, true), true, false),
    (on_screen(false, true, MiniLayout::Column, false), true, true),
    (on_screen(false, true, MiniLayout::Column, true), true, true),
];

/// A `true` nothing paints spends a decode and a blur per track, and a `false` under a drawn layout
/// leaves the row tier's thumb, or no backdrop colours at all, until the next track.
#[test]
fn only_a_surface_drawing_the_artwork_asks_for_its_decode() {
    for (surfaces, artwork, _) in RENDERED {
        assert_eq!(surfaces.renders_artwork(), artwork, "{surfaces:?}");
    }
}

/// The strip on its backdrop is the row that tells this gate from the artwork one: it needs the
/// decode and has no slot for a sheet, which would otherwise cost a lookup per track.
#[test]
fn only_now_playing_and_the_column_mount_a_panel() {
    for (surfaces, _, panel) in RENDERED {
        assert_eq!(surfaces.renders_panel(), panel, "{surfaces:?}");
    }
}
