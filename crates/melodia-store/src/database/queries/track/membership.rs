//! Which tracks a [`TrackListScope`] holds, as the `FROM` and `WHERE` a query over `tracks t`
//! starts from. The keysets order and filter it; the scope facts add it up.

use sqlx::{QueryBuilder, Sqlite};

use crate::database::DbPool;
use crate::database::queries::smart_playlist::push_positioned_membership;
use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::AppError;

/// Where a list's tracks come from.
pub(super) enum Membership {
    Library,
    Favorites,
    Album(i64),
    Artist(i64),
    Genre(i64),
    ManualPlaylist(i64),
    SmartPlaylist(SmartCriteria),
}

impl Membership {
    /// A playlist is the one scope that can't say where its tracks come from without asking: a
    /// smart one has no `playlist_items` rows and resolves through its stored criteria. One that
    /// no longer exists reads as an empty manual playlist.
    pub(super) async fn resolve(db: &DbPool, scope: TrackListScope) -> Result<Self, AppError> {
        Ok(match scope {
            TrackListScope::Library => Self::Library,
            TrackListScope::Favorites => Self::Favorites,
            TrackListScope::Album(id) => Self::Album(id),
            TrackListScope::Artist(id) => Self::Artist(id),
            TrackListScope::Genre(id) => Self::Genre(id),
            TrackListScope::Playlist(id) => {
                let verdict: Option<(bool, Option<String>)> =
                    sqlx::query_as("SELECT is_smart, smart_criteria FROM playlists WHERE id = ?")
                        .bind(id)
                        .fetch_optional(db.read())
                        .await?;
                match verdict {
                    Some((true, criteria)) => {
                        Self::SmartPlaylist(SmartCriteria::from_json_opt(criteria.as_deref()))
                    }
                    _ => Self::ManualPlaylist(id),
                }
            }
        })
    }

    /// Whether [`Self::push_source`] already opened a `WHERE`.
    fn narrows(&self) -> bool {
        !matches!(self, Self::Library | Self::SmartPlaylist(_))
    }

    /// The column holding the list's own order, where it has one.
    pub(super) fn position(&self) -> Option<&'static str> {
        match self {
            Self::ManualPlaylist(_) | Self::SmartPlaylist(_) => Some("m.position"),
            _ => None,
        }
    }

    /// Pushes the `FROM`, and the `WHERE` that narrows it to this membership where one does.
    ///
    /// An artist or genre is matched through its join table, every credit and every tag rather
    /// than the first, which is what its card's `track_count` counts: reading `tracks.genre_id`
    /// once listed a subset of the count above it, and nothing for a genre no track carries first.
    /// `IN` rather than a join, as a credit naming one artist twice ("X feat. X") would join its
    /// track in twice.
    pub(super) fn push_source(&self, qb: &mut QueryBuilder<Sqlite>) {
        match self {
            Self::Library => {
                qb.push("tracks t");
            }
            Self::Favorites => {
                qb.push("tracks t WHERE t.is_favorite = TRUE");
            }
            Self::Album(id) => {
                qb.push("tracks t WHERE t.album_id = ").push_bind(*id);
            }
            Self::Artist(id) => {
                qb.push(
                    "tracks t WHERE t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ",
                )
                .push_bind(*id)
                .push(")");
            }
            Self::Genre(id) => {
                qb.push(
                    "tracks t WHERE t.id IN (SELECT track_id FROM track_genres WHERE genre_id = ",
                )
                .push_bind(*id)
                .push(")");
            }
            Self::ManualPlaylist(id) => {
                qb.push(
                    "playlist_items m JOIN tracks t ON t.id = m.track_id WHERE m.playlist_id = ",
                )
                .push_bind(*id);
            }
            Self::SmartPlaylist(criteria) => {
                qb.push("(");
                push_positioned_membership(qb, criteria);
                qb.push(") m JOIN tracks t ON t.id = m.id");
            }
        }
    }

    /// Pushes ` AND ` or ` WHERE `, whichever the next condition after [`Self::push_source`] needs.
    pub(super) fn push_and(&self, qb: &mut QueryBuilder<Sqlite>) {
        qb.push(if self.narrows() { " AND " } else { " WHERE " });
    }
}
