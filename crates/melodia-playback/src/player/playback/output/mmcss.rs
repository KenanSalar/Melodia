//! Multimedia Class Scheduler registration for the exclusive writer thread: Windows' counterpart
//! to `realtime.rs`.
//!
//! MMCSS rather than a raised thread priority, because the service is what keeps a registered
//! audio thread ahead of everything else under load and is what an exclusive stream is expected
//! to run under. Best-effort throughout: a writer it refuses still plays, and only underruns
//! sooner.

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows_sys::w;

/// The calling thread's registration, handed back when dropped. Neither `Send` nor `Sync`, since
/// it has to be reverted on the thread that made it.
pub(super) struct Registration(HANDLE);

/// Register the calling thread as a "Pro Audio" task, the class for low-latency output. `None`
/// where the service refuses, which is logged and costs nothing else.
#[allow(
    unsafe_code,
    reason = "FFI to AvSetMmThreadCharacteristicsW. The task name is a static NUL-terminated UTF-16 literal and the task index a stack-local u32; the API retains neither pointer."
)]
pub(super) fn register_current_thread() -> Option<Registration> {
    let mut task_index = 0_u32;
    // SAFETY: `w!` yields a pointer to a NUL-terminated UTF-16 literal with a static lifetime, and
    // `task_index` is a live stack local the call writes one `u32` through. Neither is retained.
    let handle = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) };
    if handle.is_null() {
        log::debug!(
            "audio: output thread stays at normal priority: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }
    Some(Registration(handle))
}

impl Drop for Registration {
    #[allow(
        unsafe_code,
        reason = "FFI to AvRevertMmThreadCharacteristics, handed only the handle the registration on this same thread returned."
    )]
    fn drop(&mut self) {
        // SAFETY: the handle is the non-null one `AvSetMmThreadCharacteristicsW` returned, and a
        // `Registration` can't leave the thread that made it, so it is reverted where it was set.
        unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}
