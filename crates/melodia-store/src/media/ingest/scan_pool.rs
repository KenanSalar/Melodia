//! The pool a bulk pass over library files runs on, built for the pass and dropped with it.
//!
//! Not rayon's global pool, which `main()` keeps at two threads for jpeg-decoder's own
//! parallelism. Rayon serves injected work last, so a scan there would hold every JPEG the UI
//! thread decodes inline until the pass drained, and a global pool wide enough for scans keeps its
//! threads, their stacks and their resizer scratch until quit.

use std::num::NonZero;
use std::sync::Arc;

/// A pass's pool. Clones share it, and the last one to drop ends its threads.
///
/// The default holds no pool, so a parallel pass under [`ScanPool::install`] runs on the global
/// one. That is what a pass over no files gets, and what a build failure falls back to.
#[derive(Clone, Default)]
pub struct ScanPool(Option<Arc<rayon::ThreadPool>>);

impl ScanPool {
    /// Returns a pool of one thread per file, up to one per core.
    ///
    /// Sized to the pass rather than to the machine, so a watcher batch of one file costs one
    /// thread for as long as it runs.
    #[must_use]
    pub fn for_files(files: usize) -> Self {
        let cores = std::thread::available_parallelism().map_or(1, NonZero::get);
        Self::for_files_capped(files, cores)
    }

    /// Returns a pool of one thread per file, up to `max_threads`.
    #[must_use]
    pub fn for_files_capped(files: usize, max_threads: usize) -> Self {
        let threads = files.min(max_threads);
        if threads == 0 {
            return Self::default();
        }
        match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("scan-{i}"))
            .build()
        {
            Ok(pool) => Self(Some(Arc::new(pool))),
            Err(e) => {
                log::warn!("scan pool build failed ({e}); using the global pool");
                Self::default()
            }
        }
    }

    /// Runs `op` on this pool, which is where every `par_iter` inside it then fans out.
    pub fn install<R: Send>(&self, op: impl FnOnce() -> R + Send) -> R {
        match &self.0 {
            Some(pool) => pool.install(op),
            None => op(),
        }
    }
}
