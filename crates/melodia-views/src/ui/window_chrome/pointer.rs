//! Where the mouse pointer rests in the window, as the winit filter last saw it. Slint hit-tests
//! only on input, so a row rebuilt under a still pointer stays unhovered until [`rehover`] runs.

use std::cell::Cell;

use slint::LogicalPosition;
use slint::platform::WindowEvent;

thread_local! {
    static RESTING: Cell<Option<LogicalPosition>> = const { Cell::new(None) };
}

pub(super) fn rest_at(position: LogicalPosition) {
    RESTING.set(Some(position));
}

/// Forgets the pointer once it leaves the window, or once a touch takes over from it.
pub(super) fn leave() {
    RESTING.set(None);
}

pub(super) fn resting() -> Option<LogicalPosition> {
    RESTING.get()
}

/// Hit-tests the resting pointer again, so whatever was rebuilt under it takes the hover. Call it
/// from a spawned future or a timer: inside Slint's own dispatch, the nested event would run on
/// the mouse state the outer one has taken out.
pub fn rehover(window: &slint::Window) {
    let Some(position) = resting() else { return };
    // Twice: a control mounted by the first move's hover is hit-tested only by the next event.
    for _ in 0..2 {
        window.dispatch_event(WindowEvent::PointerMoved { position });
    }
}
