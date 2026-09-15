use super::*;

/// A zone mapped to the wrong direction resizes along an axis the cursor didn't show, which is the
/// disagreement the ring owning the press exists to end.
#[test]
fn every_grab_starts_the_resize_its_zone_names() {
    const GRABS: [(ResizeZone, ResizeDirection); 8] = [
        (ResizeZone::North, ResizeDirection::North),
        (ResizeZone::South, ResizeDirection::South),
        (ResizeZone::West, ResizeDirection::West),
        (ResizeZone::East, ResizeDirection::East),
        (ResizeZone::NorthWest, ResizeDirection::NorthWest),
        (ResizeZone::NorthEast, ResizeDirection::NorthEast),
        (ResizeZone::SouthWest, ResizeDirection::SouthWest),
        (ResizeZone::SouthEast, ResizeDirection::SouthEast),
    ];

    let started: Vec<Option<ResizeDirection>> =
        GRABS.iter().map(|(zone, _)| direction(*zone)).collect();

    let expected: Vec<Option<ResizeDirection>> = GRABS.iter().map(|(_, dir)| Some(*dir)).collect();
    assert_eq!(started, expected);
}

/// Anything but `None` off the grabs turns every click in the window into a resize.
#[test]
fn off_every_grab_a_press_starts_no_resize() {
    assert_eq!(direction(ResizeZone::None), None);
}
