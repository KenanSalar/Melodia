//! Tells a resize the pointer has let go of apart from one it is still holding.
//!
//! The OS holds the pointer for the whole of a resize drag, so the release never reaches the window
//! as such. What does is the pointer coming back: winit posts a left-button release when its own
//! Win32 drag ends, and X11, macOS and Wayland hand the window pointer input again once the grab is
//! over. None of it can arrive while the drag goes on.
//!
//! **On Wayland the pointer coming back is not yet the end.** xdg-shell ends a resize with a
//! configure dropping the resizing state, and `KWin` sends it a loop pass after handing the pointer
//! back. A size the window picks before acknowledging that configure is one `KWin` doesn't adopt, so
//! the configure puts the drag's size straight back: a snap taken on the pointer flashes and undoes
//! itself. The release there is the next size reading, which winit emits for that configure even
//! when the size is unchanged, the state set having changed.
//!
//! [`ResizeRelease`] decides; [`ResizeWatch`] is what the winit filter drives, carrying each
//! decision to the UI.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::TimerMode;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::dpi::PhysicalSize;
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use melodia_ui::AppWindow;

/// How long a Wayland window waits for the configure ending a resize before taking the pointer's
/// return as the release after all. Headroom for a compositor that sent the configure first rather
/// than a wait `KWin` reaches, which sends it a loop pass behind the pointer.
const CONFIGURE_GRACE: Duration = Duration::from_millis(250);

/// Which window system the window is on, the one answer the release waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowServer {
    Wayland,
    Other,
}

/// What a reading means for the resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Transition {
    None,
    /// The first size reading since the pointer last reached the window, taken before the window
    /// applies it.
    Began,
    /// The pointer is back on a Wayland window, whose release is the configure still to come.
    AwaitingConfigure,
    Released,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Resizing,
    AwaitingConfigure,
}

/// Where the window's latest resize stands.
///
/// Each method is one transition and answers what that transition decides, `ParkedWatch`'s shape:
/// split into a query and a command, a caller could ask and forget to record.
#[derive(Debug, Default)]
pub(super) struct ResizeRelease {
    phase: Phase,
    size: Option<PhysicalSize<u32>>,
}

impl ResizeRelease {
    /// Records a size reading.
    ///
    /// A Wayland reading that repeats the size mid-resize is the ending configure with the pointer
    /// still away, which is all a drag let go off the window hands back.
    pub(super) fn resized(&mut self, size: PhysicalSize<u32>, server: WindowServer) -> Transition {
        let repeated = self.size.replace(size) == Some(size);
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Resizing;
                Transition::Began
            }
            Phase::Resizing if repeated && server == WindowServer::Wayland => self.release(),
            Phase::Resizing => Transition::None,
            Phase::AwaitingConfigure => self.release(),
        }
    }

    /// Records pointer input.
    pub(super) fn pointer(&mut self, server: WindowServer) -> Transition {
        if self.phase != Phase::Resizing {
            return Transition::None;
        }
        match server {
            WindowServer::Wayland => {
                self.phase = Phase::AwaitingConfigure;
                Transition::AwaitingConfigure
            }
            WindowServer::Other => self.release(),
        }
    }

    /// Gives up on a configure that is not coming, the compositor having sent it ahead of the
    /// pointer instead.
    pub(super) fn configure_overdue(&mut self) -> Transition {
        if self.phase != Phase::AwaitingConfigure {
            return Transition::None;
        }
        self.release()
    }

    fn release(&mut self) -> Transition {
        self.phase = Phase::Idle;
        Transition::Released
    }
}

/// [`ResizeRelease`] fed from the winit filter, telling the window when a resize begins and when it
/// is let go.
pub(super) struct ResizeWatch {
    weak: slint::Weak<AppWindow>,
    release: Rc<RefCell<ResizeRelease>>,
    configure_wait: slint::Timer,
    server: OnceCell<WindowServer>,
}

impl ResizeWatch {
    pub(super) fn new(weak: slint::Weak<AppWindow>) -> Self {
        Self {
            weak,
            release: Rc::default(),
            configure_wait: slint::Timer::default(),
            server: OnceCell::new(),
        }
    }

    pub(super) fn resized(&self, window: &slint::Window, size: PhysicalSize<u32>) {
        let transition = self.release.borrow_mut().resized(size, self.server(window));
        self.follow(transition);
    }

    pub(super) fn pointer(&self, window: &slint::Window) {
        let transition = self.release.borrow_mut().pointer(self.server(window));
        self.follow(transition);
    }

    // Asked of the window an event came from, so the winit window exists and the answer is final.
    fn server(&self, window: &slint::Window) -> WindowServer {
        *self.server.get_or_init(|| {
            let wayland = window.with_winit_window(|ww| {
                ww.window_handle()
                    .is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)))
            });
            if wayland == Some(true) { WindowServer::Wayland } else { WindowServer::Other }
        })
    }

    fn follow(&self, transition: Transition) {
        match transition {
            Transition::None => {}
            // Synchronous: the filter runs ahead of Slint, which still holds the size from before.
            Transition::Began => {
                if let Some(ui) = self.weak.upgrade() {
                    ui.invoke_resize_began();
                }
            }
            Transition::AwaitingConfigure => {
                let weak = self.weak.clone();
                let release = Rc::clone(&self.release);
                self.configure_wait.start(TimerMode::SingleShot, CONFIGURE_GRACE, move || {
                    if release.borrow_mut().configure_overdue() == Transition::Released {
                        post_release(&weak);
                    }
                });
            }
            Transition::Released => {
                self.configure_wait.stop();
                post_release(&self.weak);
            }
        }
    }
}

/// Posted, so the window settles against the size Slint has applied rather than the one the event
/// is about to hand it, and a resize it asks for lands outside the dispatch.
fn post_release(weak: &slint::Weak<AppWindow>) {
    let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_resize_released());
}
