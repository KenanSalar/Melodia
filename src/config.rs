use std::path::PathBuf;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
    pub view_state_path: PathBuf,
    pub queue_path: PathBuf,
    pub search_history_path: PathBuf,
    pub scrobble_credentials_path: PathBuf,
    pub scrobble_queue_path: PathBuf,
    pub artwork_dir: PathBuf,
    pub artists_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> AppResult<Self> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| AppError::Settings("could not resolve user data directory".into()))?
            .join("Melodia");
        std::fs::create_dir_all(&data_dir)?;

        let artwork_dir = data_dir.join("artwork");
        let artists_dir = data_dir.join("artists");
        std::fs::create_dir_all(&artwork_dir)?;
        std::fs::create_dir_all(&artists_dir)?;

        Ok(Self {
            db_path: data_dir.join("melodia.db"),
            settings_path: data_dir.join("settings.json"),
            view_state_path: data_dir.join("views.json"),
            queue_path: data_dir.join("queue.json"),
            search_history_path: data_dir.join("search_history.json"),
            scrobble_credentials_path: data_dir.join("scrobble_credentials.json"),
            scrobble_queue_path: data_dir.join("scrobble_queue.json"),
            artwork_dir,
            artists_dir,
            data_dir,
        })
    }
}
