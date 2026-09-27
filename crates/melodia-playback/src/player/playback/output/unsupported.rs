//! The exclusive backend where there is none: nothing to list, and every claim refused, so the
//! output plays shared and the panel says why.

use melodia_audio::player::source::audio::{Shape, SourceFormat};

use super::claim::ClaimError;
use super::device::Feed;
use super::{Negotiated, OutputDevice};

pub(super) const SUPPORTED: bool = false;

pub(super) enum Claim {}

pub(super) enum ExclusiveStream {}

impl ExclusiveStream {
    pub(super) fn negotiated(&self) -> Negotiated {
        match *self {}
    }

    pub(super) fn into_claim(self) -> Option<Claim> {
        match self {}
    }
}

pub(super) fn devices() -> Vec<OutputDevice> {
    Vec::new()
}

pub(super) fn open(
    _: Option<&str>,
    _: Shape,
    _: SourceFormat,
    _: &Feed,
    _: Option<Claim>,
) -> Result<ExclusiveStream, ClaimError> {
    Err(ClaimError::Unsupported)
}
