//! [`audio`] is the interface the chain is written against, and imports nothing. [`decode`] is the
//! one Symphonia, [`file_decode`] and [`stream_decode`] handing it only a different `MediaSource`
//! and `Hint`, and [`opus`] the one codec its registry carries itself. [`stream_source`],
//! [`prebuffer`] and [`hls`] are the network's end, the ring keeping a blocking socket read off the
//! audio callback thread.

pub mod aac_config;
pub mod aac_trim;
pub mod audio;
pub mod decode;
pub mod file_decode;
pub mod hls;
pub mod mkv_trim;
pub mod opus;
pub mod prebuffer;
mod stall_watch;
pub mod stream_decode;
pub mod stream_source;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers;
}
