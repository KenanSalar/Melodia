use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config::Paths;
use crate::error::AppResult;

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchHistory {
    pub queries: Vec<String>,
}

/// `parking_lot::Mutex` is correct here precisely because the lock is never
/// held across an `.await` — each method clones a `SearchHistory` snapshot
/// inside a sync block, drops the guard, then awaits the disk flush. The
/// lock therefore needs only short, uncontended critical sections (the
/// fast path is ~25 ns for `parking_lot`, vs ~200 ns for `tokio::sync::Mutex`
/// even uncontended). Concurrent flushes still race on the JSON file, but
/// `Vec<String>` snapshots are tiny and OS-level atomic-rename handles
/// ordering.
pub struct SearchHistoryState {
    inner: Mutex<SearchHistory>,
    path: PathBuf,
}

impl SearchHistoryState {
    /// Load from disk (or return empty) and return the managed handle.
    pub async fn init(paths: &Paths) -> Self {
        let path = paths.search_history_path.clone();
        let history: SearchHistory = crate::services::load_json_or_default(&path)
            .await
            .unwrap_or_default();
        Self {
            inner: Mutex::new(history),
            path,
        }
    }

    pub fn get(&self) -> Vec<String> {
        self.inner.lock().queries.clone()
    }

    pub async fn add(&self, query: &str) -> AppResult<Vec<String>> {
        let snapshot = {
            let mut history = self.inner.lock();
            add_query(&mut history, query);
            history.clone()
        };
        let queries = snapshot.queries.clone();
        self.flush(snapshot).await?;
        Ok(queries)
    }

    pub async fn remove(&self, query: &str) -> AppResult<Vec<String>> {
        let snapshot = {
            let mut history = self.inner.lock();
            history.queries.retain(|s| s != query);
            history.clone()
        };
        let queries = snapshot.queries.clone();
        self.flush(snapshot).await?;
        Ok(queries)
    }

    pub async fn clear(&self) -> AppResult<Vec<String>> {
        let snapshot = {
            let mut history = self.inner.lock();
            history.queries.clear();
            history.clone()
        };
        self.flush(snapshot).await?;
        Ok(vec![])
    }

    async fn flush(&self, snapshot: SearchHistory) -> AppResult<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            crate::services::write_json_atomic_sync(&path, &snapshot)
        })
        .await
        .map_err(crate::error::AppError::io_source)?
    }
}

pub fn add_query(history: &mut SearchHistory, query: &str) {
    let trimmed = query.trim().to_owned();
    if trimmed.is_empty() {
        return;
    }
    history.queries.retain(|s| s != &trimmed);
    history.queries.insert(0, trimmed);
    history.queries.truncate(MAX_ENTRIES);
}

#[cfg(test)]
#[path = "tests/search_history_tests.rs"]
mod tests;
