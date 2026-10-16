use super::ShownPosition;

/// An equal stamp is the same publish arriving on the other channel, and an older one is a tick
/// read before a stop or a seek, which would put the position it moved off back on screen.
#[test]
fn a_position_is_shown_only_when_its_stamp_is_newer() {
    let shown = ShownPosition::default();

    let admitted = [shown.admits(5), shown.admits(5), shown.admits(4), shown.admits(6)];

    assert_eq!(admitted, [true, false, false, true], "stamps 5, 5, 4, 6 in turn");
}

/// The view model and the position ticks are written by two subscribers, and ordering them only
/// works if both hold the same record rather than a copy each.
#[test]
fn the_two_subscribers_share_one_record() {
    let ticks = ShownPosition::default();
    let view_model = ticks.clone();

    assert!(ticks.admits(7));

    assert!(!view_model.admits(6), "a clone kept a record of its own");
}
