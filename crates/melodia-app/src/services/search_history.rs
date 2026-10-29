use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use melodia_core::config::Paths;
use melodia_core::error::AppResult;

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchHistory {
    pub queries: Vec<String>,
}

/// Each change snapshots under the lock and flushes after it, so concurrent flushes race on the
/// file, harmlessly: every write is a whole snapshot, atomically renamed.
pub struct SearchHistoryState {
    inner: Mutex<SearchHistory>,
    path: PathBuf,
}

impl SearchHistoryState {
    /// Starts empty if the file is missing or unreadable.
    pub async fn init(paths: &Paths) -> Self {
        let path = paths.search_history_path.clone();
        let history: SearchHistory =
            melodia_core::utils::atomic_file::load_json_or_default(&path).await.unwrap_or_default();
        Self { inner: Mutex::new(history), path }
    }

    pub fn get(&self) -> Vec<String> {
        self.inner.lock().queries.clone()
    }

    pub async fn add(&self, query: &str) -> AppResult<Vec<String>> {
        self.change(|history| add_query(history, query)).await
    }

    pub async fn remove(&self, query: &str) -> AppResult<Vec<String>> {
        self.change(|history| history.queries.retain(|s| s != query)).await
    }

    /// Hands back what it took, which is what an undo puts back.
    pub async fn clear(&self) -> AppResult<Vec<String>> {
        let cleared = std::mem::take(&mut self.inner.lock().queries);
        self.flush(SearchHistory::default()).await?;
        Ok(cleared)
    }

    pub async fn restore(&self, cleared: Vec<String>) -> AppResult<Vec<String>> {
        self.change(|history| restore_queries(history, cleared)).await
    }

    /// Answers the list as `edit` left it.
    async fn change(&self, edit: impl FnOnce(&mut SearchHistory)) -> AppResult<Vec<String>> {
        let snapshot = {
            let mut history = self.inner.lock();
            edit(&mut history);
            history.clone()
        };
        let queries = snapshot.queries.clone();
        self.flush(snapshot).await?;
        Ok(queries)
    }

    async fn flush(&self, snapshot: SearchHistory) -> AppResult<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            melodia_core::utils::atomic_file::write_json_sync(&path, &snapshot)
        })
        .await
        .map_err(melodia_core::error::AppError::io_source)?
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

/// Puts a cleared list back behind whatever was searched since, which is newer.
fn restore_queries(history: &mut SearchHistory, cleared: Vec<String>) {
    for query in cleared {
        if !history.queries.contains(&query) {
            history.queries.push(query);
        }
    }
    history.queries.truncate(MAX_ENTRIES);
}

#[cfg(test)]
#[path = "tests/search_history_tests.rs"]
mod tests;
