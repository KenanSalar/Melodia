//! What a cover cache remembers about one path's decode, failures included.

use std::path::Path;

use melodia_core::utils::missing_artwork;

/// A cached decode. Failures are remembered as well as successes, so a surface re-running its
/// bindings doesn't re-open a file that already failed, and how it failed decides whether that
/// answer can go stale.
#[derive(Clone)]
pub enum Decoded<T> {
    Ready(T),
    /// On disk and undecodable. Settled: asking again would fail the same way.
    Broken,
    /// Gone from disk. The library puts a stored cover back by extracting it again, usually under
    /// the same content-addressed name, so this answer lapses once the file reappears.
    Missing,
}

impl<T> Decoded<T> {
    /// Classifies a decode of `path` that produced nothing, reporting a missing file to the
    /// artwork restore. Reported here rather than by each cache so no cache can forget to.
    pub fn failed(path: &Path) -> Self {
        if path.exists() {
            return Self::Broken;
        }
        missing_artwork::report();
        Self::Missing
    }

    /// Whether this answer still holds for `path`. Only a missing file that is back says no, so
    /// the `stat` is paid by entries that are already failures.
    pub fn is_current(&self, path: &Path) -> bool {
        !matches!(self, Self::Missing) || !path.exists()
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Broken | Self::Missing => None,
        }
    }
}
