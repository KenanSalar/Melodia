//! The coordinator's decision layer: what one wake does with the appearance snapshot, the OS
//! signal and whatever is on the deck.
//!
//! Nothing here needs Slint. `repaint_tx` carries a `SystemColorState` to a subscriber installed
//! somewhere else entirely, which is what lets the whole decision be a free function — and what
//! makes its two failure modes silent from the outside: a variant string that resolves the wrong
//! way paints the app in the opposite polarity, and a gate that stops holding republishes a
//! palette on every position tick.

use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;
use melodia_engine::player::engine::fixtures::{test_track, test_view_model};

/// The style every case here generates under. Any of the seven would do; `None` is the off
/// switch and has its own case.
const STYLE: &str = "tonal_spot";

/// The page background and the accent, which is as much of a generated palette as a decision test
/// has any business knowing. `Palette` is 16 packed colours with no `PartialEq`, and what
/// separates a dark scheme from a light one is `base`.
type Look = (u32, u32);

/// One coordinator's worth of state, arranged so `tick` is the whole act.
struct Coordinator {
    os_state: Arc<RwLock<SystemColorState>>,
    seeds: SeedCache,
    last: LastApplied,
    appearance: AppearanceSnapshot,
    /// Held only so the receiver below has a live sender behind it.
    _view_model_tx: watch::Sender<Option<PlayerViewModelLight>>,
    view_model_rx: watch::Receiver<Option<PlayerViewModelLight>>,
    repaint_tx: watch::Sender<SystemColorState>,
    repaint_rx: watch::Receiver<SystemColorState>,
    thumbs: Arc<CoverThumbs>,
    cover: PathBuf,
    /// Held only so the cover above outlives the coordinator reading it.
    _cover_dir: TempDir,
}

impl Coordinator {
    /// A coordinator with a real cover on the deck. Real because the generate arm decodes it and
    /// the clear arm asks whether it is still there.
    fn new(theme_id: &str, style: &str, variant: &str, os_theme: &str) -> Result<Self, AppError> {
        let (cover_dir, cover) = melodia_testkit::write_test_png(600)
            .map_err(|e| AppError::Validation(format!("write cover: {e}")))?;
        let mut os = SystemColorState::unknown();
        os.theme = os_theme.to_owned();
        let (repaint_tx, repaint_rx) = watch::channel(os.clone());

        let mut track = Arc::unwrap_or_clone(test_track("Song", None, None));
        track.artwork_path = Some(cover.to_string_lossy().into_owned());
        let (view_model_tx, view_model_rx) =
            watch::channel(Some(test_view_model(Some(Arc::new(track)), None, 200_000)));

        Ok(Self {
            os_state: Arc::new(RwLock::new(os)),
            seeds: SeedCache::new(),
            last: LastApplied::default(),
            appearance: AppearanceSnapshot {
                theme_id: theme_id.to_owned(),
                dynamic_color_style: style.to_owned(),
                theme_variant: variant.to_owned(),
            },
            _view_model_tx: view_model_tx,
            view_model_rx,
            repaint_tx,
            repaint_rx,
            thumbs: Arc::new(CoverThumbs::new()),
            cover,
            _cover_dir: cover_dir,
        })
    }

    /// Unlink the cover while the row still names it, which is what a sweep past the grace window
    /// does to a store the library has moved on from.
    fn retire_cover(&self) -> Result<(), AppError> {
        std::fs::remove_file(&self.cover)?;
        Ok(())
    }

    async fn tick(&mut self) {
        react(
            &self.os_state,
            &mut self.seeds,
            &mut self.last,
            Some(&self.appearance),
            &self.view_model_rx,
            &self.repaint_tx,
            &self.thumbs,
        )
        .await;
    }

    fn look(&self) -> Option<Look> {
        self.os_state.read().material_you.map(|(palette, accent)| (palette.base, accent))
    }

    /// Whether the last tick published, resetting so the next question is about the next tick.
    fn published(&mut self) -> bool {
        let published = self.repaint_rx.has_changed().unwrap_or(false);
        let _ = self.repaint_rx.borrow_and_update();
        published
    }
}

/// The look one wake settles on for a given variant string over a given OS signal.
async fn look_for(variant: &str, os_theme: &str) -> Result<Option<Look>, AppError> {
    let mut coordinator = Coordinator::new("material3", STYLE, variant, os_theme)?;
    coordinator.tick().await;
    Ok(coordinator.look())
}

/// The variant is a string out of `settings.json`, so the arm that matters is the one neither
/// literal matches: a file written by a build with a variant this one has retired lands there,
/// and the OS is the only sensible thing left to ask. Falling to a literal instead would paint
/// every such install in the wrong polarity with nothing to say why.
#[tokio::test]
async fn the_variant_string_resolves_three_ways() -> Result<(), AppError> {
    let dark = look_for("dark", "light").await?;
    let light = look_for("light", "dark").await?;
    assert!(dark.is_some(), "a cover under material3 has to generate something");
    assert_ne!(dark, light, "the two polarities must not agree, or nothing below means anything");

    for (variant, os_theme, expected) in [
        ("dark", "light", dark),
        ("light", "dark", light),
        ("system", "light", light),
        ("system", "dark", dark),
        ("a variant id this build has never heard of", "light", light),
        ("a variant id this build has never heard of", "dark", dark),
    ] {
        assert_eq!(
            look_for(variant, os_theme).await?,
            expected,
            "variant {variant:?} over an OS reporting {os_theme:?}"
        );
    }
    Ok(())
}

/// The gate, and the reason it is not an optimisation. This runs on every view-model wake, which
/// is every volume step, seek and position tick — republishing there is a full theme re-apply
/// several times a second for a track that has not changed.
#[tokio::test]
async fn a_second_wake_over_the_same_state_publishes_nothing() -> Result<(), AppError> {
    let mut coordinator = Coordinator::new("material3", STYLE, "dark", "dark")?;

    coordinator.tick().await;
    assert!(coordinator.published(), "the first wake has a palette to publish");

    coordinator.tick().await;
    assert!(!coordinator.published(), "nothing relevant moved, so nothing may be republished");
    Ok(())
}

/// The three ways a wake decides there should be no generated palette. Two are the user's own
/// pick and the third is not: the sweep can retire a cover out from under a live row, and a
/// palette left standing over a file that is gone is one nothing can regenerate or account for.
/// A deck with nothing on it at all takes the same arm.
#[tokio::test]
async fn a_wake_with_nothing_to_generate_from_clears_the_palette() -> Result<(), AppError> {
    for (theme_id, style, cover_retired) in [
        ("mocha", STYLE, false),
        ("material3", "none", false),
        ("material3", STYLE, true),
    ] {
        let mut coordinator = Coordinator::new("material3", STYLE, "dark", "dark")?;
        coordinator.tick().await;
        assert!(coordinator.look().is_some(), "arrange: a palette to lose");

        coordinator.appearance.theme_id = theme_id.to_owned();
        coordinator.appearance.dynamic_color_style = style.to_owned();
        if cover_retired {
            coordinator.retire_cover()?;
        }
        coordinator.tick().await;

        assert_eq!(
            coordinator.look(),
            None,
            "theme {theme_id:?}, style {style:?}, cover retired: {cover_retired}"
        );
    }
    Ok(())
}

/// The guard beside the clear. A library with no artwork at all wakes this task on every playback
/// emit and every kick, and each one would otherwise be a repaint of a palette that is already
/// unset.
#[tokio::test]
async fn a_repeated_clear_does_not_republish() -> Result<(), AppError> {
    let mut coordinator = Coordinator::new("mocha", STYLE, "dark", "dark")?;

    coordinator.tick().await;
    let _ = coordinator.published();

    coordinator.tick().await;
    assert!(!coordinator.published(), "an already-clear state has nothing to say twice");
    Ok(())
}

/// The startup drive, in substance: a restored queue seeds the view model before this task is
/// spawned, so the very first wake is what decides whether the window opens in the user's colours
/// or grey until the next track change.
#[tokio::test]
async fn the_first_wake_with_a_persisted_style_publishes_a_palette() -> Result<(), AppError> {
    let mut coordinator = Coordinator::new("material3", STYLE, "dark", "dark")?;

    coordinator.tick().await;

    assert!(coordinator.look().is_some(), "the restored cover has to reach the first wake");
    assert!(coordinator.published(), "and the subscriber has to be told");
    Ok(())
}
