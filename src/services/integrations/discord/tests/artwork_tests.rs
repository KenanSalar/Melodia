use std::future::ready;

use super::{Lookup, run_lookup};
use crate::error::AppError;

/// The seam the whole cache rule rests on: only a `Lookup::Miss` may be written to
/// the LRU as "this album has no cover", and only `Ok(None)` produces one.
///
/// Unpinned until now, which is how `media::fetch::itunes` came to fold a non-success HTTP
/// status into `Ok(None)` — every rate-limited iTunes reply was a definitive miss, and
/// blanked that album's cover for the rest of the session once Deezer had missed too.
/// A provider that wants to say "I don't know" says it with an `Err`.
#[tokio::test]
async fn only_an_empty_result_set_is_a_definitive_miss() {
    let found = run_lookup(ready(Ok(Some("https://cdn.example/cover.jpg".to_owned()))), "t").await;
    assert!(matches!(found, Lookup::Found(ref url) if url == "https://cdn.example/cover.jpg"));

    let miss = run_lookup(ready(Ok(None)), "t").await;
    assert!(matches!(miss, Lookup::Miss));

    // Every failure shape a provider can report — a refused status, a transport
    // error, a body that didn't decode — arrives as one of `AppError`'s network
    // variants, and none of them says the album has no cover.
    let refused =
        run_lookup(ready(Err(AppError::network_msg("iTunes album search returned HTTP 403"))), "t")
            .await;
    assert!(matches!(refused, Lookup::Unavailable));
}

/// The per-provider budget is the second way a lookup fails to answer, and it has to
/// land in the same arm as an error — a hop that ran out of time knows nothing about
/// the album either.
///
/// `start_paused` so the budget is spent instantly: the clock auto-advances to the
/// nearest deadline once everything is idle, which is the timeout rather than the
/// sleep behind it.
#[tokio::test(start_paused = true)]
async fn a_timed_out_provider_is_not_a_miss() {
    let timed_out = run_lookup(
        async {
            tokio::time::sleep(super::PROVIDER_TIMEOUT * 2).await;
            Ok(None)
        },
        "t",
    )
    .await;
    assert!(matches!(timed_out, Lookup::Unavailable));
}
