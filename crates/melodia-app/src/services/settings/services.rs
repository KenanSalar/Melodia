//! The flag structs behind the Settings ▸ Services tab. Scrobbling and Discord keep theirs in
//! `melodia_core::entities::integrations`.

use serde::{Deserialize, Serialize};

/// The Radio section's master switch, off by default. An upgrade has to be silent
/// for an install that never asked for a network feature, which is the reason
/// `discord_rpc_enabled` ships off and what the shipped package description
/// promises of every online feature.
///
/// The live answers are the shadows on [`crate::state::AppState`] this seeds at
/// boot — every reader is either on a tokio worker or in the boot path, where a
/// settings read is disk I/O for one bool.
///
/// Click reporting is the one field the derive would get wrong, so the `Default`
/// below is written by hand: it describes what the feature does rather than
/// whether it runs, and `false` there ships a directory nobody's plays are
/// counted for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent settings.json keys under one Settings card; splitting them would invent a container for the lint rather than one describing something"
)]
pub struct RadioFlags {
    pub radio_enabled: bool,
    /// Whether to drop segmented stations from directory results.
    ///
    /// Off by default: they play. It survives its own obsolescence because a
    /// segment playlist still starts several seconds slower than a direct
    /// mount, which is a reason to skip them and not one to hide the choice.
    ///
    /// **Renamed rather than re-defaulted**: the key it replaced shipped `true`, and a new default
    /// reaches no file that already spells the old key.
    pub radio_hide_segmented: bool,
    /// Whether playing a station tells the directory so.
    ///
    /// Opt-out rather than opt-in: the click is what popularity ordering is
    /// built from, so a user who leaves it on is paying for the ordering every
    /// other user browses by. It carries no identity beyond the request itself.
    pub radio_send_clicks: bool,
    /// Whether a song heard on a station is scrobbled like a track.
    ///
    /// Opt-in: a station's announcement is the station's word for what is playing rather than a
    /// file's tags, and a user who scrobbles their library has not asked for that on their profile.
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

/// Whether lyrics run at all, whether they may be looked up online, and how a line is drawn.
///
/// **Three switches because they sell different things.** The first is the feature: off, the Now
/// Playing column is Up Next and nothing reads, fetches or holds a sheet. The lookup is traffic,
/// and turning it off leaves a sheet already in the store perfectly readable, which is what "no
/// traffic" means and what "no lyrics" would not. Romanization is neither: it is how a line the
/// panel already has is drawn, so it costs nothing and reaches nothing.
///
/// **The first two are off and the third is on**, which is why the `Default` is written out rather
/// than derived. A panel nobody asked for should not take the Up Next column on upgrade, and the
/// shipped package description promises that every online feature is a setting the user controls;
/// but a reader who has turned lyrics on and is looking at a script they cannot sound out wanted
/// this before they knew to ask. It draws nothing at all for a Latin sheet, so "on" costs the other
/// libraries nothing.
///
/// **The three are independent, and the third one especially.** Turning lyrics off and on again
/// says nothing about romanization, so a reader who switched it off gets it back off.
///
/// `settings.json` rather than `views.json` for the feature switch too, for
/// `VisualizerFlags::viz_enabled`'s reason: it is the other Now Playing preference flipped from that
/// view's own header, and a `views.json` flag may not be a bool.
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
