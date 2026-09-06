use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;

#[test]
fn add_query_inserts_at_front() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "hello");
    add_query(&mut history, "world");
    assert_eq!(history.queries, vec!["world", "hello"]);
}

#[test]
fn add_query_deduplicates() {
    let mut history = SearchHistory {
        queries: vec!["a".into(), "b".into(), "c".into()],
    };
    add_query(&mut history, "b");
    assert_eq!(history.queries, vec!["b", "a", "c"]);
}

#[test]
fn add_query_caps_at_max() {
    let mut history = SearchHistory {
        queries: (1..=10).map(|i| format!("q{i}")).collect(),
    };
    assert_eq!(history.queries.len(), 10);
    add_query(&mut history, "new");
    assert_eq!(history.queries.len(), 10);
    assert_eq!(history.queries[0], "new");
    assert_eq!(history.queries[9], "q9");
}

#[test]
fn add_query_ignores_empty() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "");
    add_query(&mut history, "   ");
    assert!(history.queries.is_empty());
}

#[test]
fn add_query_trims_whitespace() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "  hello  ");
    assert_eq!(history.queries, vec!["hello"]);
}

// ---- the half that reaches disk ----

/// A store rooted in `tmp`, which is all `init` needs: it takes `&Paths` rather than an
/// `&AppState`.
async fn store(tmp: &TempDir) -> SearchHistoryState {
    SearchHistoryState::init(&Paths::rooted_at(tmp.path().to_path_buf())).await
}

/// The whole reason the history is a file. Every mutator flushes through the same
/// `spawn_blocking` write, so a store that only moved its in-memory `Vec` looks identical for
/// the rest of the session and is empty on the next launch.
#[tokio::test]
async fn a_history_written_this_session_is_there_on_the_next_one() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    let queries = store(&tmp).await.add("radiohead").await?;
    assert_eq!(queries, vec!["radiohead"]);

    assert_eq!(store(&tmp).await.get(), vec!["radiohead"]);
    Ok(())
}

/// First launch, and every launch after a cleared profile. An unreadable or absent file is the
/// same answer, since a history is a convenience and no part of it is worth failing a boot over.
#[tokio::test]
async fn an_absent_history_file_loads_as_an_empty_one() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    assert!(store(&tmp).await.get().is_empty());
    Ok(())
}

/// Removing one entry is the mutator most easily written as a memory-only edit, and the symptom
/// of that is a dismissed suggestion coming back at the next launch.
#[tokio::test]
async fn removing_a_query_takes_it_off_disk_too() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    let history = store(&tmp).await;
    history.add("portishead").await?;
    history.add("massive attack").await?;
    let left = history.remove("portishead").await?;
    assert_eq!(left, vec!["massive attack"]);

    assert_eq!(store(&tmp).await.get(), vec!["massive attack"]);
    Ok(())
}

/// Clear writes an empty history rather than deleting the file, so the next launch reads an
/// empty list instead of falling back to one.
#[tokio::test]
async fn clearing_writes_an_empty_history_rather_than_removing_the_file() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("search_history.json");

    let history = store(&tmp).await;
    history.add("boards of canada").await?;
    assert_eq!(history.clear().await?, Vec::<String>::new());

    assert!(path.exists(), "the file is the record that the history is empty");
    assert!(store(&tmp).await.get().is_empty());
    Ok(())
}

/// `add_query`'s cap has its own case; this is the one that says the capped list is what lands
/// on disk. A store that flushed the pre-cap snapshot would grow the file without bound and
/// re-load more than ten entries on the next launch.
#[tokio::test]
async fn the_cap_and_the_ordering_are_what_reach_disk() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    let history = store(&tmp).await;
    for i in 1..=MAX_ENTRIES + 1 {
        history.add(&format!("q{i}")).await?;
    }

    let reloaded = store(&tmp).await.get();
    assert_eq!(reloaded.len(), MAX_ENTRIES);
    assert_eq!(reloaded.first().map(String::as_str), Some("q11"));
    assert_eq!(reloaded.last().map(String::as_str), Some("q2"));
    Ok(())
}
