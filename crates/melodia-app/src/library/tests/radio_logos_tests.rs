//! What a stored answer says, and the schedule deciding when a URL is asked again.
//!
//! Both halves are pure and neither was reachable: the classifier is only ever called from a
//! function holding an `AppState`, and the backoff was an expression inside one.

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Hours in a day. Not [`LOGO_MISS_BACKOFF_HOURS`], which is the same number for another reason.
const HOURS_PER_DAY: i64 = 24;

/// The instant every case is measured against, and an hour either side of it. Written in the
/// `to_rfc3339` shape both sides of the comparison use, since the comparison is on the strings.
const NOW: &str = "2026-09-05T12:00:00+00:00";
const EARLIER: &str = "2026-09-05T11:00:00+00:00";
const LATER: &str = "2026-09-05T13:00:00+00:00";

/// One row of the answer table. The two fields are mutually exclusive on a row the schema wrote,
/// which is why one case here sets both.
fn answer(artwork_path: Option<&str>, retry_after: Option<&str>) -> radio::StoredLogoAnswer {
    radio::StoredLogoAnswer {
        favicon_url: "https://site.example.test/favicon.ico".to_owned(),
        artwork_path: artwork_path.map(str::to_owned),
        retry_after: retry_after.map(str::to_owned),
    }
}

/// A stored station carrying only the fields the seed reads. `RadioStation` has no `Default`, and
/// its two siblings under `library/tests/` spell their own for the same reason.
fn station(
    favicon_url: Option<&str>,
    homepage: Option<&str>,
    stream_url: &str,
) -> radio::RadioStation {
    radio::RadioStation {
        id: 1,
        station_uuid: None,
        name: "Example Radio".to_owned(),
        stream_url: stream_url.to_owned(),
        homepage: homepage.map(str::to_owned),
        local_homepage: None,
        favicon_url: favicon_url.map(str::to_owned),
        local_favicon_url: None,
        local_tags: None,
        local_country: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: true,
        sort_key: "example radio".to_owned(),
        date_added: "2026-08-21T00:00:00.000+00:00".to_owned(),
        last_played: None,
        play_count: 0,
    }
}

// ---- what one stored row says ----

/// Strictly greater, so the instant the schedule names is the first one that asks again rather than
/// the last one that does not.
#[test]
fn a_miss_suppresses_its_url_until_the_moment_it_names() {
    let pending = classify_logo_answer(&answer(None, Some(LATER)), NOW);
    assert!(matches!(pending, LogoAnswer::Suppressed), "a retry time still ahead suppresses");

    let due = classify_logo_answer(&answer(None, Some(NOW)), NOW);
    assert!(matches!(due, LogoAnswer::Unknown), "the moment it names is no longer suppression");

    let past = classify_logo_answer(&answer(None, Some(EARLIER)), NOW);
    assert!(matches!(past, LogoAnswer::Unknown), "and nothing behind it is either");
}

#[test]
fn a_hit_whose_file_is_still_there_is_the_answer() -> TestResult {
    let dir = tempfile::tempdir()?;
    let logo = dir.path().join("logo.png");
    std::fs::write(&logo, b"png")?;
    let path = logo.to_string_lossy().into_owned();

    let hit = classify_logo_answer(&answer(Some(&path), None), NOW);

    assert!(
        matches!(&hit, LogoAnswer::Hit(found) if *found == path),
        "a stored file is what lets a session draw the logo without asking"
    );
    Ok(())
}

/// The store is swept against the columns referencing it, so a logo kept under a data root this
/// build no longer opens is deleted with the row left pointing at it. Trust the column and the card
/// paints an empty tile forever, since nothing re-asks about a station that already has an answer.
#[test]
fn a_hit_whose_file_was_swept_is_asked_again_rather_than_drawn() -> TestResult {
    let dir = tempfile::tempdir()?;
    let swept = dir.path().join("gone.png").to_string_lossy().into_owned();

    let answered = classify_logo_answer(&answer(Some(&swept), None), NOW);

    assert!(matches!(answered, LogoAnswer::Unknown), "a path naming nothing is not a hit");
    Ok(())
}

#[test]
fn an_absent_or_empty_artwork_path_is_not_a_hit() {
    assert!(!artwork_is_present(None));
    assert!(!artwork_is_present(Some("")));

    let answered = classify_logo_answer(&answer(Some(""), None), NOW);
    assert!(matches!(answered, LogoAnswer::Unknown));
}

/// The arms are mutually exclusive on a row the schema wrote, a hit clearing whatever backoff the
/// URL had earned, so the order is decided in one place precisely so a second reader cannot decide
/// it differently. The browse page used to answer this inline, with the arms the other way round.
#[test]
fn suppression_outranks_a_path_on_the_same_row() -> TestResult {
    let dir = tempfile::tempdir()?;
    let logo = dir.path().join("logo.png");
    std::fs::write(&logo, b"png")?;

    let answered = classify_logo_answer(&answer(Some(&logo.to_string_lossy()), Some(LATER)), NOW);

    assert!(matches!(answered, LogoAnswer::Suppressed), "the backoff has the last word");
    Ok(())
}

// ---- when it is asked again ----

#[test]
fn the_backoff_grows_a_day_per_attempt_and_then_stops() {
    let cases = [(1, 24), (2, 48), (6, 144), (7, 168), (8, 168), (1_000, 168)];

    for (attempts, hours) in cases {
        assert_eq!(miss_backoff_hours(attempts), hours, "after {attempts} attempts");
    }
}

/// The claim `LOGO_MISS_MAX_AGE_DAYS` makes about itself. Raise the attempt ceiling past the
/// retention window and a miss is dropped while its own backoff is still holding, so the URL is
/// asked again at once and earns the whole schedule from scratch.
#[test]
fn nothing_is_pruned_while_it_still_suppresses_anything() {
    let longest_backoff = miss_backoff_hours(LOGO_MISS_MAX_ATTEMPTS);
    let retention = LOGO_MISS_MAX_AGE_DAYS * HOURS_PER_DAY;

    assert!(
        longest_backoff < retention,
        "a {longest_backoff} h backoff outlives a {retention} h retention window"
    );
}

// ---- what a heal pass settles in advance ----

/// Two per station and in the order `heal_station_logo` visits them, so one query settles exactly
/// the lookups the pass is about to make.
#[test]
fn every_url_a_heal_pass_can_name_in_advance_is_seeded() {
    let stations = [
        station(
            Some("https://a.example.test/icon.png"),
            Some("https://a.example.test/"),
            "https://stream.example.test/a",
        ),
        // No logo field and no homepage, so the stream's own host is all there is to name.
        station(None, None, "http://b.example.test:8000/live"),
    ];

    assert_eq!(
        heal_seed_urls(&stations),
        [
            "https://a.example.test/icon.png",
            "https://a.example.test/",
            "https://b.example.test/",
        ]
    );
}

/// The payoff of letting the user record a website for an entry the directory left blank: the site
/// they named is the one read for a `<link rel="icon">`. Folded into the directory's own column it
/// would be reverted wholesale by the next re-import.
#[test]
fn the_users_own_columns_are_what_a_heal_pass_asks_about() {
    let overridden = radio::RadioStation {
        local_favicon_url: Some("https://mine.example.test/logo.png".to_owned()),
        local_homepage: Some("https://mine.example.test/".to_owned()),
        ..station(
            Some("https://theirs.example.test/icon.png"),
            Some("https://theirs.example.test/"),
            "https://stream.example.test/x",
        )
    };

    assert_eq!(
        heal_seed_urls(&[overridden]),
        [
            "https://mine.example.test/logo.png",
            "https://mine.example.test/"
        ]
    );
}

#[test]
fn a_station_naming_no_host_at_all_seeds_nothing() {
    assert!(heal_seed_urls(&[station(None, None, "not a url")]).is_empty());
}

// ---- the loop between the schedule and the table ----

/// The one answer the table holds about `url`. A URL with no row is the miss this returns `None`
/// for, which is what an unasked URL looks like.
async fn answer_for(db: &DbPool, url: &str) -> Result<Option<radio::StoredLogoAnswer>, AppError> {
    let asked = [url.to_owned()];
    Ok(queries::radio::logo_answers(db, &asked).await?.into_iter().next())
}

const DEAD_HOST: &str = "https://dead.example/logo.png";

/// The read-increment-schedule loop, which neither side's own tests can see: the store takes the
/// attempt count from its caller and [`miss_backoff_hours`] is pinned over a number nobody feeds
/// it. A `note_logo_miss` that wrote `1` every time would leave a host that is gone asked once a
/// day forever, and both of those tests would stay green.
#[tokio::test]
async fn each_miss_counts_the_ones_before_it_and_schedules_further_out()
-> Result<(), Box<dyn std::error::Error>> {
    let db = DbPool::test_pool().await?;

    note_logo_miss(&db, DEAD_HOST).await?;
    let first = answer_for(&db, DEAD_HOST).await?.and_then(|answer| answer.retry_after);

    note_logo_miss(&db, DEAD_HOST).await?;
    note_logo_miss(&db, DEAD_HOST).await?;
    let third = answer_for(&db, DEAD_HOST).await?.and_then(|answer| answer.retry_after);

    assert_eq!(
        queries::radio::logo_miss_attempts(&db, DEAD_HOST).await?,
        Some(3),
        "the stored count is what the next schedule is derived from"
    );
    // Both stamps are `to_rfc3339` closing on a constant `+00:00`, so lexical order is
    // chronological — the ordering `get_recent_stations` reads on.
    assert!(
        first < third,
        "and a third miss must be asked later than a first, not on the same day"
    );
    Ok(())
}

/// A host that came back and then failed once more starts the ladder over rather than resuming a
/// week-long schedule it earned while it was down. The clear is the store's and the re-read is
/// this module's, so only the pair says it.
#[tokio::test]
async fn a_hit_between_two_misses_starts_the_ladder_again() -> Result<(), Box<dyn std::error::Error>>
{
    let db = DbPool::test_pool().await?;
    let logo = StoredLogo {
        path: "radio-logos/ab/abcd.png".to_owned(),
        bytes: 1_024,
    };

    note_logo_miss(&db, DEAD_HOST).await?;
    note_logo_miss(&db, DEAD_HOST).await?;
    note_logo_hit(&db, DEAD_HOST, &logo).await?;
    note_logo_miss(&db, DEAD_HOST).await?;

    assert_eq!(queries::radio::logo_miss_attempts(&db, DEAD_HOST).await?, Some(1));
    Ok(())
}

/// The dispatch, which is one `match` and invisible when it is wrong: a miss recorded as a hit
/// leaves the row naming a file nothing downloaded, and a hit recorded as a miss suppresses a logo
/// this session already has.
#[tokio::test]
async fn an_outcome_is_recorded_as_the_arm_it_was_given() -> Result<(), Box<dyn std::error::Error>>
{
    let db = DbPool::test_pool().await?;
    let logo = StoredLogo {
        path: "radio-logos/ab/abcd.png".to_owned(),
        bytes: 1_024,
    };

    record_outcome(&db, "https://live.example/logo.png", Some(&logo)).await;
    record_outcome(&db, DEAD_HOST, None).await;

    let hit = answer_for(&db, "https://live.example/logo.png").await?;
    assert!(matches!(&hit, Some(answer) if answer.artwork_path.as_deref() == Some(&logo.path)));
    assert!(
        matches!(hit, Some(answer) if answer.retry_after.is_none()),
        "a hit suppresses nothing"
    );

    let miss = answer_for(&db, DEAD_HOST).await?;
    assert!(matches!(&miss, Some(answer) if answer.retry_after.is_some()));
    assert!(matches!(miss, Some(answer) if answer.artwork_path.is_none()));
    Ok(())
}

/// `AnswerSeed` exists so a page of stations costs one query rather than one per card, and nothing
/// held that it is consulted at all. Ignoring it compiles, passes every case above, and turns a
/// browse page into an N+1 walk whose only symptom is that the page is slower than it was.
///
/// The table is left empty, so a hit can only have come from the seed. The second call is the
/// control that says so.
#[tokio::test]
async fn a_seeded_answer_is_taken_without_asking_the_table() -> TestResult {
    let dir = tempfile::tempdir()?;
    let logo = dir.path().join("logo.png");
    std::fs::write(&logo, b"png")?;
    let path = logo.to_string_lossy().into_owned();
    let url = "https://site.example.test/favicon.ico";

    let seeded = AnswerSeed(std::iter::once((url.to_owned(), answer(Some(&path), None))).collect());
    let db = DbPool::test_pool().await?;

    let found = stored_answer(&db, &seeded, url).await;

    assert!(matches!(&found, LogoAnswer::Hit(at) if *at == path), "the seed answered");
    assert!(
        matches!(stored_answer(&db, &AnswerSeed::unseeded(), url).await, LogoAnswer::Unknown),
        "and the table holds nothing, so it could not have"
    );
    Ok(())
}
