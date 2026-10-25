//! Where a file let go of over the window lands, against the rects the two targets publish.

use slint::LogicalPosition;

use super::{DropRoute, DropTargets, QueuePanel, route};
use melodia_ui::WindowRect;

/// The playlist open on the content panel.
const PLAYLIST: i64 = 7;

fn rect(x: f32, y: f32, width: f32, height: f32) -> WindowRect {
    WindowRect { x, y, width, height }
}

/// The content panel, 200 to 800 across.
fn content() -> WindowRect {
    rect(200.0, 40.0, 600.0, 600.0)
}

/// The docked panel, 900 to 1200 across: apart from the content, so each of its edges is its own.
fn docked() -> WindowRect {
    rect(900.0, 40.0, 300.0, 600.0)
}

/// The floating card, over the content panel's right half.
fn card() -> WindowRect {
    rect(500.0, 40.0, 300.0, 600.0)
}

fn at(x: f32, y: f32) -> LogicalPosition {
    LogicalPosition::new(x, y)
}

#[test]
fn a_drop_lands_in_the_target_whose_rect_holds_it() {
    let targets = DropTargets { queue: Some(docked()), playlist: Some((content(), PLAYLIST)) };
    let cases = [
        (at(1000.0, 300.0), DropRoute::Queue, "inside the queue"),
        (at(500.0, 300.0), DropRoute::Playlist(PLAYLIST), "inside the content panel"),
        (at(850.0, 300.0), DropRoute::Nowhere, "between the two"),
    ];
    for (point, expected, case) in cases {
        assert_eq!(route(point, &targets), expected, "{case}");
    }
}

/// A rect holds its left and top edges and not its right and bottom ones, so two rects meeting at
/// an edge share no point.
#[test]
fn a_rect_holds_its_left_and_top_edges_and_not_its_right_and_bottom_ones() {
    let targets = DropTargets { queue: Some(docked()), playlist: None };
    let cases = [
        (at(900.0, 300.0), DropRoute::Queue, "the left edge"),
        (at(1000.0, 40.0), DropRoute::Queue, "the top edge"),
        (at(1200.0, 300.0), DropRoute::Nowhere, "the right edge"),
        (at(1000.0, 640.0), DropRoute::Nowhere, "the bottom edge"),
    ];
    for (point, expected, case) in cases {
        assert_eq!(route(point, &targets), expected, "{case}");
    }
}

/// The floating card is drawn over the content panel, so it is what the user let go over.
#[test]
fn where_both_rects_hold_the_point_the_queue_takes_the_drop() {
    let targets = DropTargets { queue: Some(card()), playlist: Some((content(), PLAYLIST)) };

    assert_eq!(route(at(600.0, 300.0), &targets), DropRoute::Queue);
}

#[test]
fn with_no_playlist_on_screen_the_content_panel_takes_nothing() {
    let targets = DropTargets { queue: Some(docked()), playlist: None };

    assert_eq!(route(at(500.0, 300.0), &targets), DropRoute::Nowhere);
}

/// A hidden panel's rect is where it last drew, which may well be over the page.
#[test]
fn the_queue_panel_decides_which_targets_take_a_drop() {
    let cases = [
        (
            QueuePanel { shown: true, floating: false, rect: docked() },
            at(500.0, 300.0),
            DropRoute::Playlist(PLAYLIST),
            "the page beside the docked panel",
        ),
        (
            QueuePanel { shown: false, floating: false, rect: card() },
            at(600.0, 300.0),
            DropRoute::Playlist(PLAYLIST),
            "where a hidden panel last drew",
        ),
        (
            QueuePanel { shown: true, floating: true, rect: card() },
            at(600.0, 300.0),
            DropRoute::Queue,
            "the floating card",
        ),
        (
            QueuePanel { shown: true, floating: true, rect: card() },
            at(300.0, 300.0),
            DropRoute::Nowhere,
            "the page under the floating card",
        ),
    ];
    for (panel, point, expected, case) in cases {
        let targets = DropTargets::new(panel, Some((content(), PLAYLIST)));

        assert_eq!(route(point, &targets), expected, "{case}");
    }
}
