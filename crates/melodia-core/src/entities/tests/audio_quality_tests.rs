//! Which of two encodes of one song the duplicate finder suggests keeping.

use super::*;

fn lossless(codec: &str, bit_depth: i32, sample_rate: i32) -> AudioFormat {
    AudioFormat {
        codec: Some(codec.to_owned()),
        bitrate: None,
        sample_rate: Some(sample_rate),
        bit_depth: Some(bit_depth),
    }
}

fn lossy(codec: &str, bitrate: Option<i32>) -> AudioFormat {
    AudioFormat {
        codec: Some(codec.to_owned()),
        bitrate,
        sample_rate: Some(44_100),
        bit_depth: None,
    }
}

#[test]
fn lossless_is_the_container_or_an_mp4_carrying_a_bit_depth() {
    let unstated = |codec: Option<&str>, bit_depth| AudioFormat {
        codec: codec.map(str::to_owned),
        bitrate: None,
        sample_rate: None,
        bit_depth,
    };
    let rows = [
        ("Flac", unstated(Some("Flac"), None), true),
        ("Wav", unstated(Some("Wav"), None), true),
        ("Aiff", unstated(Some("Aiff"), None), true),
        ("Ape", unstated(Some("Ape"), None), true),
        ("WavPack", unstated(Some("WavPack"), None), true),
        ("ALAC", unstated(Some("Mp4"), Some(16)), true),
        ("AAC", unstated(Some("Mp4"), None), false),
        ("a depth on MP3", unstated(Some("Mpeg"), Some(16)), false),
        ("Vorbis", unstated(Some("Vorbis"), None), false),
        ("Opus", unstated(Some("Opus"), None), false),
        ("no codec", unstated(None, Some(24)), false),
    ];
    for (label, format, expected) in rows {
        assert_eq!(format.is_lossless(), expected, "{label}");
    }
}

#[test]
fn the_better_encode_ranks_higher() {
    let rows = [
        (
            "a sparse FLAC over a 320 kbps MP3",
            lossless("Flac", 16, 44_100),
            lossy("Mpeg", Some(320)),
        ),
        ("depth before rate", lossless("Flac", 24, 44_100), lossless("Flac", 16, 96_000)),
        ("rate at one depth", lossless("Flac", 16, 96_000), lossless("Flac", 16, 44_100)),
        ("bitrate among lossy", lossy("Mpeg", Some(320)), lossy("Vorbis", Some(256))),
        ("an unknown bitrate last", lossy("Mpeg", Some(128)), lossy("Mpeg", None)),
    ];
    for (label, better, worse) in rows {
        assert!(better.quality() > worse.quality(), "{label}: {better:?} over {worse:?}");
    }
}
