//! The Last.fm and `ListenBrainz` clients, each failing with its own error because the retry
//! policy needs a classification `AppError` can't carry.

pub mod lastfm;
pub mod listenbrainz;
