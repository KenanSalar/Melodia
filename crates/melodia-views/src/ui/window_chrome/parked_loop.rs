//! The Slint tick Win32's modal resize-and-move loop parks winit out of.
//!
//! Win32 pumps its own message loop for the whole of a resize or move drag, so winit sits inside
//! `DefWindowProcW` and never reaches the wait point that emits `NewEvents`, the one place the
//! backend calls `update_timers_and_animations`. Sizes and paints keep arriving, so the layout
//! tracks the pointer and only the *decisions* stall: every `Timer` and every `changed` handler,
//! which is the whole responsive layer (the miniplayer swap, `GridColumnsSync`'s column count, each
//! `changed width` mirror).
//!
//! [`pump`] runs that tick from the window events the modal loop still delivers, and on its own it
//! only lasts while events do. Slint's frame throttle is itself a `Timer`, so a paint landing before
//! the throttle's next deadline fires nothing and asks for no further paint: the pointer stops and
//! the swap stops with it, half faded. The heartbeat is what outlives the pointer. A thread sleeps
//! until the next timer is due and posts the tick back, and a posted event is one the modal loop
//! dispatches.
//!
//! [`LoopTicks`] is how a parked loop is told apart from the ordinary one, which runs `NewEvents`
//! ahead of every batch. Two pumps with none between them can only be inside a modal loop, and
//! nothing arms outside one, so a window idling or animating normally never wakes for this.

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use slint::winit_030::winit::event::StartCause;
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::{CustomApplicationHandler, EventResult};

/// The longest one heartbeat sleeps, a frame at 60 Hz. A paint-driven pump can start a timer due
/// sooner than the one the thread is already sleeping toward, and a sleeping thread can't be told.
const PARKED_TICK_CAP: Duration = Duration::from_millis(16);

thread_local! {
    /// `NewEvents` so far. Winit emits them on the thread that runs the pumps.
    static LOOP_TICKS: Cell<u64> = const { Cell::new(0) };
    static WATCH: RefCell<ParkedWatch> = const { RefCell::new(ParkedWatch::new()) };
    /// The heartbeat thread's inbox, spawned by the first parked pump. `None` if the spawn failed.
    static HEARTBEAT: OnceCell<Option<Sender<Duration>>> = const { OnceCell::new() };
}

/// What the pumps and the heartbeat remember between calls, as `NewEvents` counts.
///
/// Each method is one transition and answers what that transition decides, the way
/// `Iterator::next` both advances and answers: split into a query and a command, a caller could
/// ask and forget to record, which is a heartbeat armed twice or an arm never spent.
struct ParkedWatch {
    /// The count the previous window-event pump saw.
    pumped_at: Option<u64>,
    /// The count a pending heartbeat was armed at.
    armed_at: Option<u64>,
}

impl ParkedWatch {
    const fn new() -> Self {
        Self { pumped_at: None, armed_at: None }
    }

    /// Records a window-event pump at `ticks`, answering whether it should arm a heartbeat: none is
    /// pending, and no `NewEvents` ran since the previous pump, which only a modal loop allows.
    fn pumped(&mut self, ticks: u64) -> bool {
        let parked = self.pumped_at.replace(ticks) == Some(ticks);
        parked && self.armed_at.is_none()
    }

    fn armed(&mut self, ticks: u64) {
        self.armed_at = Some(ticks);
    }

    /// Spends the pending arm, answering whether the heartbeat should tick and re-arm: the loop is
    /// still parked where it was armed. One landing after `NewEvents` found the ordinary loop back.
    fn heartbeat(&mut self, ticks: u64) -> bool {
        self.armed_at.take() == Some(ticks)
    }
}

/// Counts the loop's `NewEvents`, installed on the backend by `main`.
pub struct LoopTicks;

impl CustomApplicationHandler for LoopTicks {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) -> EventResult {
        LOOP_TICKS.set(LOOP_TICKS.get().wrapping_add(1));
        EventResult::Propagate
    }
}

/// Runs the tick winit is parked out of, and arms the heartbeat once the loop is seen parked.
pub fn pump() {
    slint::platform::update_timers_and_animations();

    let ticks = LOOP_TICKS.get();
    if WATCH.with_borrow_mut(|watch| watch.pumped(ticks)) {
        arm_heartbeat(ticks);
    }
}

/// How long the heartbeat sleeps toward the next due timer, or `None` when there is nothing left
/// for it to fire.
fn heartbeat_delay(until_next_timer: Option<Duration>) -> Option<Duration> {
    until_next_timer.map(|until| until.min(PARKED_TICK_CAP))
}

fn arm_heartbeat(ticks: u64) {
    let Some(delay) = heartbeat_delay(slint::platform::duration_until_next_timer_update()) else {
        return;
    };
    let sent = HEARTBEAT.with(|inbox| {
        inbox.get_or_init(spawn_heartbeat).as_ref().is_some_and(|tx| tx.send(delay).is_ok())
    });
    if sent {
        WATCH.with_borrow_mut(|watch| watch.armed(ticks));
    }
}

/// The posted half, back on the UI thread.
fn on_heartbeat() {
    let ticks = LOOP_TICKS.get();
    if !WATCH.with_borrow_mut(|watch| watch.heartbeat(ticks)) {
        return;
    }
    slint::platform::update_timers_and_animations();
    arm_heartbeat(ticks);
}

fn spawn_heartbeat() -> Option<Sender<Duration>> {
    let (tx, rx) = mpsc::channel::<Duration>();
    let spawned = std::thread::Builder::new().name("melodia-parked".to_owned()).spawn(move || {
        for delay in rx {
            // `sleep` rather than a `recv_timeout`: std's sleep is high resolution on Windows, and a
            // channel timeout rounds to the system tick, a frame late.
            std::thread::sleep(delay);
            if slint::invoke_from_event_loop(on_heartbeat).is_err() {
                return;
            }
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            log::warn!("parked loop: heartbeat thread: {e}");
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/parked_loop_tests.rs"]
mod tests;
