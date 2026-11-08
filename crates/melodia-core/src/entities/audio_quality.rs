//! How good an encode is, for choosing which of two copies of one song to keep.

/// lofty's container names, as `tracks.codec` stores them, that only ever hold lossless audio.
/// `Mp4` holds AAC as often as ALAC, so [`AudioFormat::is_lossless`] asks it apart.
const LOSSLESS_CONTAINERS: [&str; 5] = ["Flac", "Wav", "Aiff", "Ape", "WavPack"];

/// The technical columns an encode is judged by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
}

impl AudioFormat {
    /// Whether the encode is lossless. An MP4 is lossless only as ALAC or FLAC, the codecs lofty
    /// reports a bit depth for; AAC never has one.
    pub fn is_lossless(&self) -> bool {
        match self.codec.as_deref() {
            Some("Mp4") => self.bit_depth.is_some(),
            Some(codec) => LOSSLESS_CONTAINERS.contains(&codec),
            None => false,
        }
    }

    /// Where this encode ranks among copies of one song.
    pub fn quality(&self) -> AudioQuality {
        if self.is_lossless() {
            AudioQuality::Lossless {
                bit_depth: self.bit_depth.unwrap_or_default(),
                sample_rate: self.sample_rate.unwrap_or_default(),
            }
        } else {
            AudioQuality::Lossy { bitrate: self.bitrate.unwrap_or_default() }
        }
    }
}

/// An encode's rank, the greater the better, an unknown figure ranking as zero. Any lossless copy
/// outranks any lossy one, since bitrate alone puts a sparse FLAC below a 320 kbps MP3. The derive
/// is the rule: variant first, then the fields in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioQuality {
    Lossy { bitrate: i32 },
    Lossless { bit_depth: i32, sample_rate: i32 },
}

#[cfg(test)]
#[path = "tests/audio_quality_tests.rs"]
mod tests;
