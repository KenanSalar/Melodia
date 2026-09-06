use crate::state::AppState;
use melodia_core::entities::search::SearchResults;
use melodia_core::error::AppError;
use melodia_store::database::queries;

/// A blank query answers four empty lists, and the guard for that is
/// `queries::search::search_all`'s own: it trims and returns before it builds a MATCH, so a copy
/// here would be a second statement of one argument rather than a second layer of safety.
pub async fn search_all(state: &AppState, query: String) -> Result<SearchResults, AppError> {
    queries::search::search_all(&state.db, &query).await
}

pub fn get_search_history(state: &AppState) -> Result<Vec<String>, AppError> {
    Ok(state.search_history.get())
}

pub async fn add_search_history(state: &AppState, query: String) -> Result<Vec<String>, AppError> {
    state.search_history.add(&query).await
}

pub async fn remove_search_history(
    state: &AppState,
    query: String,
) -> Result<Vec<String>, AppError> {
    state.search_history.remove(&query).await
}

pub async fn clear_search_history(state: &AppState) -> Result<Vec<String>, AppError> {
    state.search_history.clear().await
}
