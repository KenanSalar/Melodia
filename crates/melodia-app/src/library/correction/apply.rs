//! Setting a device's correction: from a model on `AutoEq`'s list, or from a file or paste the user
//! reviews first.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::{CorrectionTarget, autoeq_client, commit, parametric};
use crate::state::AppState;
use melodia_core::entities::correction::{
    AutoEqEntry, Correction, CorrectionOrigin, ParsedProfile, ProfileRefusal,
};
use melodia_core::error::AppError;
use melodia_core::utils::text::from_utf8_lossy_owned;
use melodia_net::services::net::autoeq;

/// Far past a profile's size, leaving room for the comments and the graphic EQ line other tools
/// write beside the filters.
const IMPORT_MAX_BYTES: u64 = 64 * 1024;

/// A file or paste as read, for the user to review before [`apply`].
pub struct Imported {
    pub origin: CorrectionOrigin,
    pub profile: ParsedProfile,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Read(#[from] AppError),

    #[error("the profile is larger than {IMPORT_MAX_BYTES} bytes")]
    TooLarge,

    #[error(transparent)]
    Refused(#[from] ProfileRefusal),
}

/// Fetches the model's profile and sets it. One with any line left unused is refused rather than
/// reviewed: `AutoEq`'s layout isn't a promise, and a moved one must not apply half a curve.
pub async fn apply_from_list(
    state: &AppState,
    target: CorrectionTarget,
    entry: &AutoEqEntry,
) -> Result<(), AppError> {
    let text = autoeq::fetch_profile(autoeq_client(state)?, entry).await?;
    let profile = parametric::parse(&text)
        .map_err(|e| AppError::validation("AutoEq's profile was refused", e))?;
    if !profile.skipped.is_empty() {
        log::warn!("AutoEq profile for {:?} refused, unused: {:?}", entry.name, profile.skipped);
        return Err(AppError::validation_msg("AutoEq's profile holds lines Melodia can't read"));
    }
    let origin = CorrectionOrigin::AutoEq {
        model: entry.name.as_ref().into(),
        source: entry.source.as_ref().into(),
    };
    apply(state, target, Imported { origin, profile });
    Ok(())
}

/// Blocking.
pub fn import_file(path: &Path) -> Result<Imported, ImportError> {
    let text = read_import(path)?;
    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
    Ok(Imported { origin: CorrectionOrigin::File { name }, profile: parametric::parse(&text)? })
}

pub fn import_paste(text: &str) -> Result<Imported, ImportError> {
    if !fits_import_cap(text.len()) {
        return Err(ImportError::TooLarge);
    }
    Ok(Imported { origin: CorrectionOrigin::Paste, profile: parametric::parse(text)? })
}

/// Sets `imported`'s filters for `target`, the lines it couldn't use dropped.
pub fn apply(state: &AppState, target: CorrectionTarget, imported: Imported) {
    let correction = Correction {
        origin: imported.origin,
        device_name: target.device_name,
        filters: imported.profile.filters,
    };
    commit(state, |corrections| corrections.insert(target.device, correction));
}

fn read_import(path: &Path) -> Result<String, ImportError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(IMPORT_MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| AppError::io("the file could not be read", e))?;
    if !fits_import_cap(bytes.len()) {
        return Err(ImportError::TooLarge);
    }
    // Lossy, so a comment written in another codepage doesn't refuse the file.
    Ok(from_utf8_lossy_owned(bytes))
}

fn fits_import_cap(len: usize) -> bool {
    u64::try_from(len).is_ok_and(|len| len <= IMPORT_MAX_BYTES)
}
