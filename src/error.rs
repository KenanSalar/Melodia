#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Metadata error: {0}")]
    Metadata(String),

    #[error("Scanner error: {0}")]
    Scanner(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Player error: {0}")]
    Player(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("Settings error: {0}")]
    Settings(String),

    #[error("Window error: {0}")]
    Window(String),

    #[error("Watcher error: {0}")]
    Watcher(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Validation error: {0}")]
    Validation(String),
}

impl AppError {
    pub fn not_found(entity: &str, id: i64) -> Self {
        Self::NotFound(format!("{entity} not found: {id}"))
    }

    pub fn io_other(msg: impl Into<String>) -> Self {
        Self::Io(std::io::Error::other(msg.into()))
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
#[path = "tests/error_tests.rs"]
mod tests;
