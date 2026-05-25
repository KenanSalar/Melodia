use super::metadata::{format_channels, format_sample_rate};

#[test]
fn sample_rate_drops_trailing_zero() {
    assert_eq!(format_sample_rate(44_100), "44.1 kHz");
    assert_eq!(format_sample_rate(48_000), "48 kHz");
    assert_eq!(format_sample_rate(96_000), "96 kHz");
    assert_eq!(format_sample_rate(88_200), "88.2 kHz");
}

#[test]
fn channels_have_friendly_names() {
    assert_eq!(format_channels(1), "Mono");
    assert_eq!(format_channels(2), "Stereo");
    assert_eq!(format_channels(6), "6 channels");
}
