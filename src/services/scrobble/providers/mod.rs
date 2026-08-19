//! Per-provider clients: the signed Last.fm API and the `ListenBrainz` one. Each
//! exposes pure request/payload builders plus `async` network functions taking a
//! shared `&reqwest::Client` and returning a provider-specific, retry-classified
//! error — the retry policy needs a classification `AppError` can't carry.

pub mod lastfm;
pub mod listenbrainz;
