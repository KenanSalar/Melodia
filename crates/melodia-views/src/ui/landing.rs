//! Work run on the runtime whose answer lands on the UI thread.

use std::future::Future;

use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use melodia_core::error::describe;

/// Runs `work` on the runtime and hands its answer to `land` on the UI thread, `None` for a task
/// that didn't finish. Spawned rather than awaited on the event loop, where every stretch between
/// its awaits would run on the thread that draws. Answers its abort handle if the landing spawned.
pub fn spawn_landing<T: Send + 'static>(
    runtime: &Handle,
    label: &'static str,
    work: impl Future<Output = T> + Send + 'static,
    land: impl FnOnce(Option<T>) + 'static,
) -> Option<AbortHandle> {
    let answer = runtime.spawn(work);
    let abort = answer.abort_handle();
    let spawned = slint::spawn_local(async_compat::Compat::new(async move {
        let answer = match answer.await {
            Ok(answer) => Some(answer),
            Err(e) => {
                if !e.is_cancelled() {
                    log::warn!("{label}: task failed: {}", describe(&e));
                }
                None
            }
        };
        land(answer);
    }));
    if let Err(e) = &spawned {
        log::warn!("{label}: not spawned: {}", describe(e));
    }
    spawned.is_ok().then_some(abort)
}
