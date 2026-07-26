//! Per-provider clients: the Last.fm signed API and the `ListenBrainz` client.
//! Each exposes pure request/payload builders plus `async` network functions
//! that take a shared `&reqwest::Client` and return a provider-specific,
//! retry-classified error — the retry policy needs a classification
//! `AppError` can't carry. Called from the submitter in `super::submit`.

pub mod lastfm;
pub mod listenbrainz;
