use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use melodia_core::config::Paths;
use melodia_core::error::AppResult;

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchHistory {
    pub queries: Vec<String>,
}

/// The list in memory, written to disk after every change.
pub struct SearchHistoryState {
    shared: Arc<Shared>,
    path: PathBuf,
}

/// What a change edits and a write saves, the write running on the blocking pool.
struct Shared {
    history: Mutex<SearchHistory>,
    /// Held across a write, which snapshots the list under it. Two blocking tasks have no order of
    /// their own, so a snapshot taken by the change could land after a newer one; taken here, the
    /// last write to run always saves the newest list.
    writer: Mutex<()>,
}

impl SearchHistoryState {
    /// Starts empty if the file is missing or unreadable.
    pub async fn init(paths: &Paths) -> Self {
        let path = paths.search_history_path.clone();
        let history: SearchHistory =
            melodia_core::utils::atomic_file::load_json_or_default(&path).await.unwrap_or_default();
        let shared = Arc::new(Shared { history: Mutex::new(history), writer: Mutex::new(()) });
        Self { shared, path }
    }

    pub fn get(&self) -> Vec<String> {
        self.shared.history.lock().queries.clone()
    }

    pub async fn add(&self, query: &str) -> AppResult<Vec<String>> {
        self.change(|history| add_query(history, query)).await
    }

    pub async fn remove(&self, query: &str) -> AppResult<Vec<String>> {
        self.change(|history| history.queries.retain(|s| s != query)).await
    }

    /// Hands back what it took, which is what an undo puts back.
    pub async fn clear(&self) -> AppResult<Vec<String>> {
        let cleared = std::mem::take(&mut self.shared.history.lock().queries);
        self.flush().await?;
        Ok(cleared)
    }

    pub async fn restore(&self, cleared: Vec<String>) -> AppResult<Vec<String>> {
        self.change(|history| restore_queries(history, cleared)).await
    }

    /// Answers the list as `edit` left it.
    async fn change(&self, edit: impl FnOnce(&mut SearchHistory)) -> AppResult<Vec<String>> {
        let queries = {
            let mut history = self.shared.history.lock();
            edit(&mut history);
            history.queries.clone()
        };
        self.flush().await?;
        Ok(queries)
    }

    /// Saves the list as it stands when the write runs, which can be newer than the change that
    /// asked for it.
    async fn flush(&self) -> AppResult<()> {
        let (shared, path) = (Arc::clone(&self.shared), self.path.clone());
        tokio::task::spawn_blocking(move || {
            let _writer = shared.writer.lock();
            let snapshot = shared.history.lock().clone();
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
