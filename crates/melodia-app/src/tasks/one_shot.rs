//! The shape a once-per-install sweep runs in: a pass no rescan can reach, gated on a
//! `settings.json` marker rather than inferred, walking its work list a page at a time.
//! [`OnFailure`] is the one knob, being what the sweeps genuinely differ on.

use std::path::Path;

use crate::services;
use crate::services::settings::LibraryFlags;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_core::utils::reach;
use melodia_store::media::ingest::scan_pool::ScanPool;
use tokio_util::sync::CancellationToken;

/// Whether a pass that failed part way still records its marker.
#[derive(Clone, Copy)]
pub enum OnFailure {
    /// Mark anyway. For a pass whose partial result is still correct and which the next scan
    /// repairs, where retrying the same failure every launch buys nothing.
    Mark,
    /// Leave the marker down. For a pass nothing else repairs, where recording one would put
    /// what it was after out of reach for the life of the install.
    Retry,
}

#[derive(Clone, Copy)]
pub struct Sweep {
    /// Names the pass in the two lines below. Prose rather than the flag name, these landing in
    /// the log tail a bug report carries.
    pub label: &'static str,
    /// The `settings.json` flag [`mark`](Self::mark) raises, for `persist_blocking`'s own line.
    pub marker: &'static str,
    pub done: fn(&LibraryFlags) -> bool,
    pub mark: fn(&mut LibraryFlags),
    pub on_failure: OnFailure,
}

impl Sweep {
    /// A marker over a failure puts what the pass was after out of reach for the install's life;
    /// one withheld over a success spends the pass again every launch.
    fn records_marker(self, passed: bool) -> bool {
        passed || matches!(self.on_failure, OnFailure::Mark)
    }
}

/// Runs `pass` unless this install already had one. The marker goes down through
/// `mutate_settings`: the pass takes minutes, and writing back the snapshot read before it would
/// revert every setting changed meanwhile.
pub fn spawn<F, Fut>(spawner: &TaskSpawner, state: &AppState, sweep: Sweep, pass: F)
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = AppResult<()>> + Send,
{
    let state = state.clone();
    spawner.spawn(async move {
        match services::settings::read_settings(&state.paths) {
            Ok(settings) if (sweep.done)(&settings.library) => return,
            Ok(_) => {}
            Err(e) => {
                log::warn!("{} skipped: {}", sweep.label, describe(&e));
                return;
            }
        }

        let passed = match pass(state.clone()).await {
            Ok(()) => true,
            Err(e) => {
                log::warn!("{} failed: {}", sweep.label, describe(&e));
                false
            }
        };
        if !sweep.records_marker(passed) {
            return;
        }

        state.persist_blocking(sweep.marker, move |paths| {
            services::settings::mutate_settings(paths, |settings| {
                (sweep.mark)(&mut settings.library);
            })
        });
    });
}

/// Rows a sweep holds and reads per page, which keeps a library-sized pass off the RSS budget.
pub const PAGE_ROWS: i64 = 2_000;

/// A keyset walk over a sweep's `(id, path)` work list, each page starting past the last id the
/// page before it held.
#[derive(Default)]
pub struct Pages {
    after_id: i64,
}

impl Pages {
    /// The page `fetch` answers for the id this walk has reached, `None` once the list runs out.
    pub async fn next_page<F, Fut>(&mut self, fetch: F) -> AppResult<Option<Vec<(i64, String)>>>
    where
        F: FnOnce(i64) -> Fut,
        Fut: Future<Output = AppResult<Vec<(i64, String)>>>,
    {
        let page = fetch(self.after_id).await?;
        let Some(&(last_id, _)) = page.last() else {
            return Ok(None);
        };
        self.after_id = last_id;
        Ok(Some(page))
    }
}

/// Runs `read` over a page's files on a scan pool sized to it, off the runtime's workers.
pub async fn read_page<T, F>(page: Vec<(i64, String)>, read: F) -> AppResult<T>
where
    F: FnOnce(&[(i64, String)]) -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || ScanPool::for_files(page.len()).install(|| read(&page)))
        .await
        .map_err(|e| AppError::scanner("A sweep's page read panicked", e))
}

/// What [`read_files`] made of a page.
pub struct FileReads<T> {
    pub read: Vec<(i64, T)>,
    /// Files gone or held by another program, which a later launch may reach.
    pub out_of_reach: usize,
}

/// **Blocking**: `read` over each file of `rows`, fanned out on the calling pool. A file that is
/// there but won't read is left out, since it would fail the same way at every launch.
pub fn read_files<T: Send>(
    rows: &[(i64, String)],
    read: impl Fn(&Path) -> AppResult<T> + Sync,
) -> FileReads<T> {
    use rayon::prelude::*;

    let reached: Vec<Reached<T>> =
        rows.par_iter().filter_map(|(id, path)| reach_file(*id, Path::new(path), &read)).collect();
    let mut reads = FileReads { read: Vec::with_capacity(reached.len()), out_of_reach: 0 };
    for file in reached {
        match file {
            Reached::Read(id, value) => reads.read.push((id, value)),
            Reached::OutOfReach => reads.out_of_reach += 1,
        }
    }
    reads
}

enum Reached<T> {
    Read(i64, T),
    OutOfReach,
}

fn reach_file<T>(id: i64, path: &Path, read: impl Fn(&Path) -> AppResult<T>) -> Option<Reached<T>> {
    match read(path) {
        Ok(value) => Some(Reached::Read(id, value)),
        Err(e) if reach::is_out_of_reach(path, &e) => Some(Reached::OutOfReach),
        Err(e) => {
            log::debug!("A sweep left out a file that won't read: {}", describe(&e));
            None
        }
    }
}

/// What a rewriting sweep did with one file.
pub enum Rewrite {
    Written,
    Failed,
    Skipped,
}

/// A rewriting sweep's count, across its pages.
#[derive(Default)]
pub struct Rewrites {
    pub written: usize,
    pub failed: usize,
    pub out_of_reach: usize,
}

impl Rewrites {
    /// Runs `rewrite` over the files `reads` reached one at a time, so a quit stops the pass
    /// between whole-file rewrites rather than inside one the exit would cut short.
    pub async fn rewrite_each<T, F, Fut>(
        &mut self,
        shutdown: &CancellationToken,
        reads: FileReads<T>,
        mut rewrite: F,
    ) -> AppResult<()>
    where
        F: FnMut(i64, T) -> Fut,
        Fut: Future<Output = AppResult<Rewrite>>,
    {
        self.out_of_reach += reads.out_of_reach;
        for (id, file) in reads.read {
            if shutdown.is_cancelled() {
                return Err(AppError::scanner_msg("stopped by the quit, finished next launch"));
            }
            match rewrite(id, file).await? {
                Rewrite::Written => self.written += 1,
                Rewrite::Failed => self.failed += 1,
                Rewrite::Skipped => {}
            }
        }
        Ok(())
    }
}

/// Fails a pass that left `out_of_reach` of its `files` unread, so an [`OnFailure::Retry`] sweep
/// runs again next launch.
pub fn require_all_reached(out_of_reach: usize, files: &str) -> AppResult<()> {
    (out_of_reach == 0).ok_or_else(|| {
        AppError::scanner_msg(format!(
            "{out_of_reach} {files} were out of reach, read again next launch"
        ))
    })
}

#[cfg(test)]
#[path = "tests/one_shot_tests.rs"]
mod tests;
