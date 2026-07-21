//! Per-provider clients: the Last.fm signed API and the `ListenBrainz` client.
//! Each exposes pure request/payload builders plus `async` network functions
//! that take a shared `&reqwest::Client` and return a provider-specific,
//! retry-classified error. They stay unwired until the Phase 2 detector and
//! submitter tasks call them.

pub mod lastfm;
pub mod listenbrainz;
