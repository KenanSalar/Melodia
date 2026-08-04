//! Public state enum the Updater global mirrors. Owned by Rust; pushed
//! into Slint via the `Updater.*` properties in `melodia-ui/ui/globals/updater.slint`.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdaterState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes_short: String,
        /// `true` when the manifest marks the release as critical;
        /// the UI suppresses the Skip button in that case.
        critical: bool,
    },
    Downloading {
        progress: u8,
    },
    Verifying,
    Installed,
    Error(String),
}
