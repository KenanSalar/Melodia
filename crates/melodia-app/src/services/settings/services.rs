//! The flag structs behind Settings ▸ Radio and ▸ Lyrics. Scrobbling and Discord, the Integrations
//! category, keep theirs in `melodia_core::entities::integrations`.

use serde::{Deserialize, Serialize};

/// Radio ships off, as every online feature does, so an upgrade stays silent for an install that
/// never asked for the network. Read live through the shadows it seeds on
/// [`crate::state::AppState`] at boot. The `Default` is hand-written for `radio_send_clicks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent settings.json keys under one Settings card; splitting them would invent a container for the lint rather than one describing something"
)]
pub struct RadioFlags {
    pub radio_enabled: bool,
    /// Drops segmented stations from directory results. Off, since they play, if seconds slower
    /// to start than a direct mount. **A new key rather than a new default**: the one it replaced
    /// shipped `true`, and a default reaches no file already spelling the key.
    pub radio_hide_segmented: bool,
    /// Tells the directory a station was played. Opt-out: clicks build the popularity order
    /// everyone browses by, and carry no identity beyond the request.
    pub radio_send_clicks: bool,
    /// Scrobbles a song heard on a station. Opt-in: an announcement is the station's word rather
    /// than a file's tags, and scrobbling a library is not asking for it.
    pub radio_scrobble: bool,
}

impl Default for RadioFlags {
    fn default() -> Self {
        Self {
            radio_enabled: false,
            radio_hide_segmented: false,
            radio_send_clicks: true,
            radio_scrobble: false,
        }
    }
}

/// Lyrics, their online lookup and romanization: **three independent switches, since they sell
/// different things.** Off, the feature reads, fetches and holds nothing. The lookup is traffic,
/// and without it a stored sheet stays readable. Romanization only changes how a line is drawn.
///
/// The first two ship off, a panel nobody asked for not taking the Up Next column on upgrade.
/// Romanization ships on, drawing nothing for a Latin sheet. All three live in `settings.json`
/// like `viz_enabled`, the other switch on Now Playing's header, a `views.json` flag never being a
/// bool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsFlags {
    /// Read under its old name too: v0.13.0 shipped it as the column preference it grew out of.
    #[serde(alias = "lyrics_panel_shown")]
    pub lyrics_enabled: bool,
    pub lyrics_online_enabled: bool,
    pub lyrics_romanization_shown: bool,
}

impl Default for LyricsFlags {
    fn default() -> Self {
        Self {
            lyrics_enabled: false,
            lyrics_online_enabled: false,
            lyrics_romanization_shown: true,
        }
    }
}
