//! The press half of the drag regions: which one the pointer is over, and what a left press there
//! does. The winit filter hands every such press to the OS as a window move, so Slint never sees
//! two in a row and no `double-clicked` on a region can fire. The double press is read here
//! instead, by the same rule Slint's own double click keeps.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, LogicalPosition};

use melodia_ui::{AppWindow, DragRegion, WindowChrome};

/// Slint's `Platform::click_interval`, which its winit backend doesn't override, so the titlebar's
/// double press is timed like every other double click in the app.
const DOUBLE_PRESS_INTERVAL: Duration = Duration::from_millis(500);

/// How far from the first press the second may land, the radius Slint's `ClickState` allows.
const DOUBLE_PRESS_SLOP: f32 = 10.0;

/// Registers the regions' hover callback and returns the cell it keeps up to date.
pub(super) fn wire(app: &AppWindow) -> Rc<Cell<DragRegion>> {
    let region = Rc::new(Cell::new(DragRegion::None));
    let writer = Rc::clone(&region);
    app.global::<WindowChrome>().on_drag_region_changed(move |hovered| writer.set(hovered));
    region
}

/// What a left press on a drag region does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PressAction {
    Move,
    ToggleMaximize,
}

/// The titlebar press waiting for a second one to make it a double.
#[derive(Default)]
pub(super) struct DoublePress {
    first: Option<Press>,
}

impl DoublePress {
    /// Answers what a left press on `region` does, and keeps it as the first half of the next
    /// double press. A completed pair starts over, so a third press moves the window again.
    ///
    /// Only the titlebar's pair maximizes: the miniplayer is a window size of its own, and
    /// maximizing it would only hand the window to the full player.
    pub(super) fn press(
        &mut self,
        region: DragRegion,
        at: Instant,
        position: LogicalPosition,
    ) -> PressAction {
        if region != DragRegion::Titlebar {
            self.first = None;
            return PressAction::Move;
        }
        let press = Press { at, position };
        if self.first.is_some_and(|first| first.is_doubled_by(press)) {
            self.first = None;
            PressAction::ToggleMaximize
        } else {
            self.first = Some(press);
            PressAction::Move
        }
    }

    /// Forgets the press the window moved under. That press was a drag, and the pointer rides
    /// along with the window, so a click straight after it would otherwise land within the slop.
    pub(super) fn moved(&mut self) {
        self.first = None;
    }
}

#[derive(Debug, Clone, Copy)]
struct Press {
    at: Instant,
    position: LogicalPosition,
}

impl Press {
    fn is_doubled_by(self, next: Self) -> bool {
        let dx = next.position.x - self.position.x;
        let dy = next.position.y - self.position.y;
        next.at.duration_since(self.at) < DOUBLE_PRESS_INTERVAL
            && dx * dx + dy * dy < DOUBLE_PRESS_SLOP * DOUBLE_PRESS_SLOP
    }
}

#[cfg(test)]
#[path = "tests/drag_region_tests.rs"]
mod tests;
