//! `AutoEq`'s list as last fetched, kept on disk so the picker opens on it at once and rechecks it
//! behind. Held in memory only by the picker that asked.

use std::io::ErrorKind;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::autoeq_client;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::entities::correction::AutoEqEntry;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::atomic_file;
use melodia_net::services::net::autoeq::{self, ListFetch};

/// How long a cached list stands before the picker asks whether it moved.
const LIST_RECHECK_AFTER: Duration = Duration::from_hours(24);

/// The list as the picker first shows it.
pub struct CachedList {
    /// Empty where nothing on disk reads.
    pub entries: Vec<AutoEqEntry>,
    pub check: ListCheck,
}

/// What a recheck needs of the cache it may replace.
pub struct ListCheck {
    held: Held,
    checked_at: SystemTime,
}

enum Held {
    Nothing,
    /// A file that parses to no entry, which the picker names apart from no list at all.
    Unreadable,
    Usable {
        etag: Option<String>,
    },
}

/// What the picker shows once a recheck answers.
pub enum ListAnswer {
    /// What the cache showed stands.
    Unchanged,
    Fresh(Vec<AutoEqEntry>),
    /// Nothing cached, and `AutoEq` didn't answer.
    Unreachable,
    /// Nothing that reads, cached or fetched.
    Unreadable,
}

/// The list's text has a file of its own, so this stays small enough to rewrite after every check.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ListMeta {
    etag: Option<String>,
    checked_at_secs: u64,
}

impl ListCheck {
    /// A list that can't be shown is due at once.
    pub fn is_due(&self, now: SystemTime) -> bool {
        let Held::Usable { .. } = self.held else {
            return true;
        };
        now.duration_since(self.checked_at).ok().is_none_or(|age| age >= LIST_RECHECK_AFTER)
    }

    fn etag(&self) -> Option<&str> {
        match &self.held {
            Held::Usable { etag } => etag.as_deref(),
            Held::Nothing | Held::Unreadable => None,
        }
    }
}

impl Held {
    /// The answer when the fetch brought no list that reads.
    fn unread_answer(&self) -> ListAnswer {
        match self {
            Self::Usable { .. } => ListAnswer::Unchanged,
            Self::Nothing | Self::Unreadable => ListAnswer::Unreadable,
        }
    }

    /// The answer when `AutoEq` couldn't be asked or didn't answer.
    fn unanswered(&self) -> ListAnswer {
        match self {
            Self::Usable { .. } => ListAnswer::Unchanged,
            Self::Nothing => ListAnswer::Unreachable,
            Self::Unreadable => ListAnswer::Unreadable,
        }
    }
}

/// Blocking.
pub fn cached_list(paths: &Paths) -> CachedList {
    let meta: ListMeta = atomic_file::load_json_or_default_sync(&paths.autoeq_list_meta_path)
        .unwrap_or_else(|e| {
            log::warn!("AutoEq list check: {}", describe(&e));
            ListMeta::default()
        });
    // A time past what `SystemTime` holds reads as never checked.
    let checked_at =
        UNIX_EPOCH.checked_add(Duration::from_secs(meta.checked_at_secs)).unwrap_or(UNIX_EPOCH);
    let (entries, held) = match std::fs::read_to_string(&paths.autoeq_list_path) {
        Ok(text) => {
            let entries = autoeq::parse_list(&text);
            let held = if entries.is_empty() {
                Held::Unreadable
            } else {
                Held::Usable { etag: meta.etag }
            };
            (entries, held)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => (Vec::new(), Held::Nothing),
        Err(e) => {
            log::warn!("AutoEq list: {}", describe(&e));
            (Vec::new(), Held::Unreadable)
        }
    };
    CachedList { entries, check: ListCheck { held, checked_at } }
}

/// Asks `AutoEq` whether the list moved. A fresh copy replaces the cache only if it parses to an
/// entry, so a moved or broken upstream file never overwrites a list that works.
pub async fn recheck_list(state: &AppState, check: ListCheck) -> ListAnswer {
    let now = SystemTime::now();
    match fetch(state, check.etag()).await {
        Ok(ListFetch::Fresh { text, etag }) => {
            let entries = autoeq::parse_list(&text);
            if entries.is_empty() {
                log::warn!("AutoEq's list arrived with no entry Melodia can read");
                return check.held.unread_answer();
            }
            let meta = ListMeta::checked(etag, now);
            // The text first, so a meta never carries a tag for a list that didn't land.
            store(state, move |paths| {
                atomic_file::write_text_sync(&paths.autoeq_list_path, &text)?;
                atomic_file::write_json_sync(&paths.autoeq_list_meta_path, &meta)
            })
            .await;
            ListAnswer::Fresh(entries)
        }
        Ok(ListFetch::NotModified) => match check.held {
            Held::Usable { etag } => {
                let meta = ListMeta::checked(etag, now);
                store(state, move |paths| {
                    atomic_file::write_json_sync(&paths.autoeq_list_meta_path, &meta)
                })
                .await;
                ListAnswer::Unchanged
            }
            // A `304` to a request that sent no tag says nothing about a list we don't hold.
            held @ (Held::Nothing | Held::Unreadable) => held.unanswered(),
        },
        Err(e) => {
            log::warn!("AutoEq list: {}", describe(&e));
            check.held.unanswered()
        }
    }
}

async fn fetch(state: &AppState, etag: Option<&str>) -> Result<ListFetch, AppError> {
    autoeq::fetch_list(autoeq_client(state)?, etag).await
}

impl ListMeta {
    fn checked(etag: Option<String>, now: SystemTime) -> Self {
        let checked_at_secs = now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Self { etag, checked_at_secs }
    }
}

/// Logged rather than returned: the answer in hand stands whether or not the cache took it.
async fn store(
    state: &AppState,
    write: impl FnOnce(&Paths) -> Result<(), AppError> + Send + 'static,
) {
    let paths = Arc::clone(&state.paths);
    let stored = state
        .runtime
        .spawn_blocking(move || write(&paths))
        .await
        .map_err(|e| AppError::io("AutoEq list write task failed", e))
        .and_then(|written| written);
    if let Err(e) = stored {
        log::warn!("AutoEq list cache: {}", describe(&e));
    }
}

#[cfg(test)]
#[path = "tests/list_tests.rs"]
mod tests;
