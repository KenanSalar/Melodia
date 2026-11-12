//! The pass's own threads, at lowered priority so the decode-ahead, the output and the UI take the
//! CPU first. Dedicated rather than borrowed: a priority set on a blocking-pool thread outlives the
//! job and slows whatever runs there next, and `ScanPool` can take every core.

use melodia_core::error::{AppError, AppResult};
use melodia_platform::services::platform::background_priority;

/// Jobs decoding at once. Each holds a decoder and its meters, and each past the first takes a
/// core the UI or a scan may want.
pub(super) const MEASURE_WORKERS: usize = 2;

/// Built at a pass's first job and dropped with the pass, so neither a finished backlog nor a wake
/// whose work is all out of reach leaves a thread behind.
#[derive(Default)]
pub(super) struct Workers(Option<rayon::ThreadPool>);

impl Workers {
    pub(super) fn spawn(&mut self, job: impl FnOnce() + Send + 'static) -> AppResult<()> {
        let pool = match &mut self.0 {
            Some(pool) => pool,
            unbuilt @ None => unbuilt.insert(build()?),
        };
        pool.spawn(job);
        Ok(())
    }
}

fn build() -> AppResult<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(MEASURE_WORKERS)
        .thread_name(|i| format!("loudness-{i}"))
        .start_handler(|_| background_priority::lower_current_thread())
        .build()
        .map_err(|e| AppError::io("Starting the loudness threads", e))
}
