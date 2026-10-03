//! Real-time scheduling for an exclusive writer thread, asked of `RealtimeKit`.
//!
//! rtkit rather than `SCHED_FIFO` directly: a desktop session has neither `CAP_SYS_NICE` nor an
//! `RLIMIT_RTPRIO`, so the direct call fails for exactly the users this is for, and rtkit is what
//! grants it to a session process instead. The price it asks is a capped `RLIMIT_RTTIME`, so a
//! thread that spins at RT priority is killed rather than locking up the machine.
//!
//! Best-effort throughout. A writer at normal priority still plays; it only underruns sooner
//! under load, and the health counters are where that shows.

use melodia_core::error::describe;
use rustix::process::{Resource, Rlimit};

use super::claim::ClaimSource;

const RTKIT: &str = "org.freedesktop.RealtimeKit1";
const RTKIT_PATH: &str = "/org/freedesktop/RealtimeKit1";

/// Low in the RT range: above every normal thread, below the sound server's own, which has a
/// deadline this one doesn't.
const PRIORITY: u32 = 10;

/// Ask for RT scheduling on the calling thread, logging rather than failing when refused.
pub(super) fn promote_current_thread() {
    match promote() {
        Ok(()) => log::debug!("audio: output thread running at real-time priority"),
        Err(e) => log::debug!("audio: output thread stays at normal priority: {}", describe(&*e)),
    }
}

fn promote() -> Result<(), ClaimSource> {
    let bus = zbus::blocking::Connection::system()?;
    let rtkit = super::reserve::uncached_proxy(&bus, RTKIT, RTKIT_PATH, RTKIT)?;
    let max_priority: i32 = rtkit.get_property("MaxRealtimePriority")?;
    let rttime_max: i64 = rtkit.get_property("RTTimeUSecMax")?;

    let limit = u64::try_from(rttime_max)?;
    let held = rustix::process::getrlimit(Resource::Rttime);
    if held.maximum.is_none_or(|maximum| maximum > limit) {
        rustix::process::setrlimit(
            Resource::Rttime,
            Rlimit { current: Some(limit), maximum: Some(limit) },
        )?;
    }

    let priority = PRIORITY.min(u32::try_from(max_priority)?);
    let thread = u64::try_from(rustix::thread::gettid().as_raw_nonzero().get())?;
    rtkit.call_method("MakeThreadRealtime", &(thread, priority))?;
    Ok(())
}
