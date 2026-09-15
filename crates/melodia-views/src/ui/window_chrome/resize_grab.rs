//! The press half of `ResizeRing`: the zone its hover names, kept as the OS resize a left press
//! there starts. The ring owns the geometry and the winit filter owns the press, so neither
//! restates the other and the cursor can't promise a resize the press doesn't start.

use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::winit_030::winit::window::ResizeDirection;

use melodia_ui::{AppWindow, ResizeZone, WindowChrome};

/// Registers the ring's zone callback and returns the cell it keeps up to date.
///
/// An `Rc<Cell>` rather than the drag region's atomic: the callback and the winit filter both run
/// on the event-loop thread, and the value is `Copy`.
pub(super) fn wire(app: &AppWindow) -> Rc<Cell<Option<ResizeDirection>>> {
    let grab = Rc::new(Cell::new(None));
    let writer = Rc::clone(&grab);
    app.global::<WindowChrome>().on_resize_zone_changed(move |zone| writer.set(direction(zone)));
    grab
}

/// The OS resize a press in `zone` starts, or `None` off every grab.
fn direction(zone: ResizeZone) -> Option<ResizeDirection> {
    match zone {
        ResizeZone::None => None,
        ResizeZone::North => Some(ResizeDirection::North),
        ResizeZone::South => Some(ResizeDirection::South),
        ResizeZone::West => Some(ResizeDirection::West),
        ResizeZone::East => Some(ResizeDirection::East),
        ResizeZone::NorthWest => Some(ResizeDirection::NorthWest),
        ResizeZone::NorthEast => Some(ResizeDirection::NorthEast),
        ResizeZone::SouthWest => Some(ResizeDirection::SouthWest),
        ResizeZone::SouthEast => Some(ResizeDirection::SouthEast),
    }
}

#[cfg(test)]
#[path = "tests/resize_grab_tests.rs"]
mod tests;
