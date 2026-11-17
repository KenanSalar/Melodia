//! The headphone correction, run per source after the input gains and before the ten bands, so the
//! limiter catches its boosts as it does theirs.

pub(crate) mod bank;
pub(crate) mod response;
pub mod shared;
