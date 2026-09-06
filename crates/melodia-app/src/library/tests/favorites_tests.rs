//! Tests for the two favorite writers and the retroactive love backfill.
//!
//! A favorite is one of the four things a rescan cannot give back, and it is the only one that
//! also leaves the machine: both writers end in `sync_love`, so a toggle reaches a public
//! Last.fm or `ListenBrainz` profile. What is pinned here is therefore the order and the gates,
//! not the queue underneath, which `melodia-integrations` already covers.
//!
//! `ListenBrainz` is the provider every armed case uses. A stored token is its whole credential,
//! where Last.fm additionally needs app keys baked in at compile time, so a keyed build and a
//! keyless CI one would disagree about whether the test ran at all.

use std::sync::OnceLock;

use tempfile::TempDir;

use super::*;
use crate::state::fixtures::test_sinks;
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_engine::player::engine::fixtures::test_track;
use melodia_engine::player::engine::types::PlaybackSource;
use melodia_integrations::services::integrations::scrobble::{
    ListenBrainzCredentials, LoveItem, ScrobbleQueue,
};
use melodia_store::database::queries::fixtures::insert_test_track;

/// A `FavoriteWrite` over an in-memory library, with the scrobble service's files under a
/// throwaway root.
struct Fixture {
    write: FavoriteWrite,
    paths: Paths,
    _tmp: TempDir,
}

impl Fixture {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let paths = Paths::rooted_at(tmp.path().to_path_buf());
        paths.create_dirs()?;
        let scrobble =
            ScrobbleService::init(&paths, &ScrobbleFlags::default(), Arc::new(OnceLock::new()));

        let db = DbPool::test_pool().await?;
        queries::folder::insert_folder(&db, "/music", true).await?;

        Ok(Self {
            write: FavoriteWrite {
                db,
                player_state: Arc::new(PlayerStateHandle::default()),
                sinks: Arc::new(test_sinks()),
                library_changed: Signal::new(),
                scrobble: Arc::new(scrobble),
            },
            paths,
            _tmp: tmp,
        })
    }

    async fn track(&self, name: &str) -> Result<i64, AppError> {
        insert_test_track(&self.write.db, &format!("/music/{name}.mp3"), name, "Artist", "A", "R")
            .await
    }

    /// Give `id` the recording MBID `ListenBrainz` feedback keys on, so a love for it survives
    /// the provider's own gate.
    async fn tag(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("UPDATE tracks SET musicbrainz_track_id = 'rec-1' WHERE id = ?")
            .bind(id)
            .execute(self.write.db.write())
            .await?;
        Ok(())
    }

    async fn arm_listenbrainz(&self) -> Result<(), AppError> {
        self.write.scrobble.set_flags(ScrobbleFlags {
            listenbrainz_love_enabled: true,
            ..Default::default()
        });
        self.write
            .scrobble
            .set_listenbrainz_credentials(Some(ListenBrainzCredentials {
                token: "token".to_owned(),
                username: "listener".to_owned(),
            }))
            .await
    }

    /// Put `id` on the deck with the favorite flag the row is meant to disagree with.
    fn seat(&self, id: i64, favorite: bool) {
        let mut track = (*test_track("Song", Some("Artist"), None)).clone();
        track.id = id;
        track.is_favorite = favorite;
        lock_state(&self.write.player_state).source = Some(PlaybackSource::Track(Arc::new(track)));
    }

    fn deck_favorite(&self) -> Option<bool> {
        lock_state(&self.write.player_state).current_track().map(|t| t.is_favorite)
    }

    async fn row_favorite(&self, id: i64) -> Result<bool, AppError> {
        let flag: i64 = sqlx::query_scalar("SELECT is_favorite FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(self.write.db.read())
            .await?;
        Ok(flag != 0)
    }

    /// The loves as they reached disk, which is what a restart would replay.
    fn persisted_loves(&self) -> Result<Vec<LoveItem>, AppError> {
        Ok(ScrobbleQueue::load(&self.paths.scrobble_queue_path)?.loves.into_iter().collect())
    }
}

#[tokio::test]
async fn a_favorite_write_marks_every_row_it_was_given() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let first = fixture.track("first").await?;
    let second = fixture.track("second").await?;

    fixture.write.set(&[first, second], true).await?;

    assert!(fixture.row_favorite(first).await?);
    assert!(fixture.row_favorite(second).await?);
    Ok(())
}

#[tokio::test]
async fn a_favorite_write_wakes_the_library_subscribers() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    let changed = fixture.write.library_changed.subscribe();

    fixture.write.set(&[id], true).await?;

    assert_eq!(
        changed.has_changed().ok(),
        Some(true),
        "Favorites and the heart columns re-fetch off this tick and nothing else",
    );
    Ok(())
}

#[tokio::test]
async fn a_favorite_write_mirrors_the_flag_onto_the_playing_track() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.seat(id, false);

    fixture.write.set(&[id], true).await?;

    assert_eq!(
        fixture.deck_favorite(),
        Some(true),
        "the Now-Playing heart must not wait for the next track load",
    );
    Ok(())
}

#[tokio::test]
async fn a_favorite_write_leaves_a_playing_track_outside_the_set_alone() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let playing = fixture.track("playing").await?;
    let listed = fixture.track("listed").await?;
    fixture.seat(playing, false);

    fixture.write.set(&[listed], true).await?;

    assert_eq!(fixture.deck_favorite(), Some(false));
    assert!(fixture.row_favorite(listed).await?);
    Ok(())
}

#[tokio::test]
async fn an_armed_provider_gets_the_love_a_favorite_write_makes() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.tag(id).await?;
    fixture.arm_listenbrainz().await?;

    fixture.write.set(&[id], true).await?;

    let loves = fixture.persisted_loves()?;
    assert_eq!(loves.len(), 1);
    assert_eq!(loves.first().map(|l| l.loved), Some(true));
    Ok(())
}

#[tokio::test]
async fn an_unarmed_provider_gets_no_love() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.tag(id).await?;

    fixture.write.set(&[id], true).await?;

    assert_eq!(fixture.write.scrobble.queued_len(), 0);
    Ok(())
}

/// An un-favorite carries to the provider too: dropping it would leave a love standing on a
/// profile the user just cleared it from.
#[tokio::test]
async fn un_favoriting_queues_the_unlove() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.tag(id).await?;
    fixture.arm_listenbrainz().await?;

    fixture.write.set(&[id], false).await?;

    assert_eq!(fixture.persisted_loves()?.first().map(|l| l.loved), Some(false));
    Ok(())
}

#[tokio::test]
async fn toggling_with_nothing_on_the_deck_writes_nothing() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let changed = fixture.write.library_changed.subscribe();

    let toggled = fixture.write.toggle_current().await?;

    assert_eq!(toggled, None);
    assert_eq!(changed.has_changed().ok(), Some(false), "nothing changed, so nothing re-fetches");
    Ok(())
}

#[tokio::test]
async fn toggling_the_playing_track_flips_the_row_and_the_cached_copy() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.seat(id, false);

    let toggled = fixture.write.toggle_current().await?;

    assert_eq!(toggled, Some((id, true)));
    assert!(fixture.row_favorite(id).await?);
    assert_eq!(fixture.deck_favorite(), Some(true));
    Ok(())
}

#[tokio::test]
async fn toggling_an_already_favorited_track_clears_it() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.write.set(&[id], true).await?;
    fixture.seat(id, true);

    let toggled = fixture.write.toggle_current().await?;

    assert_eq!(toggled, Some((id, false)));
    assert!(!fixture.row_favorite(id).await?);
    Ok(())
}

#[tokio::test]
async fn the_backfill_reports_nothing_when_the_target_is_not_armed() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.write.set(&[id], true).await?;

    let queued =
        queue_favorite_loves(&fixture.write.db, &fixture.write.scrobble, LoveTarget::ListenBrainz)
            .await;

    assert_eq!(queued, None);
    Ok(())
}

#[tokio::test]
async fn the_backfill_reports_nothing_when_the_library_has_no_favorites() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    fixture.track("song").await?;
    fixture.arm_listenbrainz().await?;

    let queued =
        queue_favorite_loves(&fixture.write.db, &fixture.write.scrobble, LoveTarget::ListenBrainz)
            .await;

    assert_eq!(queued, None, "an empty pass has nothing to tell the user");
    Ok(())
}

#[tokio::test]
async fn the_backfill_counts_only_the_favorites_listenbrainz_can_key_on() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let tagged = fixture.track("tagged").await?;
    let untagged = fixture.track("untagged").await?;
    fixture.tag(tagged).await?;
    fixture.write.set(&[tagged, untagged], true).await?;
    fixture.arm_listenbrainz().await?;

    let queued =
        queue_favorite_loves(&fixture.write.db, &fixture.write.scrobble, LoveTarget::ListenBrainz)
            .await;

    assert_eq!(queued, Some(1));
    Ok(())
}

/// The zero that feeds the advice toast: armed, favorites present, and not one of them carries
/// the id `ListenBrainz` needs.
#[tokio::test]
async fn the_backfill_reports_a_zero_when_no_favorite_is_tagged() -> Result<(), AppError> {
    let fixture = Fixture::new().await?;
    let id = fixture.track("song").await?;
    fixture.write.set(&[id], true).await?;
    fixture.arm_listenbrainz().await?;

    let queued =
        queue_favorite_loves(&fixture.write.db, &fixture.write.scrobble, LoveTarget::ListenBrainz)
            .await;

    assert_eq!(queued, Some(0));
    Ok(())
}

#[test]
fn the_backfill_speaks_only_where_the_count_says_something() {
    assert_eq!(
        backfill_detail(3, LoveTarget::Lastfm).as_deref(),
        Some("Syncing 3 loved track(s) to Last.fm"),
    );
    assert_eq!(
        backfill_detail(3, LoveTarget::ListenBrainz).as_deref(),
        Some("Syncing 3 loved track(s) to ListenBrainz"),
    );
    assert_eq!(
        backfill_detail(0, LoveTarget::Lastfm),
        None,
        "Last.fm takes every favorite it was handed, so a zero there reports on nothing",
    );
    assert!(
        backfill_detail(0, LoveTarget::ListenBrainz)
            .is_some_and(|detail| detail.contains("MusicBrainz ID")),
        "a ListenBrainz zero is the untagged-library answer, not silence",
    );
}
