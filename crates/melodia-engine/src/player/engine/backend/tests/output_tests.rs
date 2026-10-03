//! Tests for where the output choice sends a shared stream, which differs by platform.

use melodia_audio::player::source::audio::SampleRate;
use melodia_playback::player::playback::output::OutputRequest;

use super::OutputChoice;

/// Windows' endpoint ids are the ones a shared stream opens by, so the device picked on the Output
/// card is where shared output plays too, and none picked follows the system default.
#[cfg(target_os = "windows")]
#[test]
fn a_shared_stream_on_windows_plays_through_the_chosen_device() {
    const ENDPOINT: &str = "{0.0.0.00000000}.{a-test-endpoint}";
    for chosen in [Some(ENDPOINT), None] {
        let choice = OutputChoice { device: chosen.map(str::to_owned), ..OutputChoice::default() };

        let request = choice.shared_request(SampleRate::new(48_000));

        let wanted = OutputRequest::Shared {
            rate: SampleRate::new(48_000),
            device: chosen.map(str::to_owned),
        };
        assert_eq!(request, wanted, "{chosen:?} chosen");
    }
}

/// A card's `hw:` name would take a shared stream past the sound server, which routes shared
/// output itself, so the card picked for exclusive output never reaches a shared open.
#[cfg(target_os = "linux")]
#[test]
fn a_shared_stream_on_linux_never_names_the_exclusive_card() {
    for chosen in [Some("hw:CARD=CODEC,DEV=0"), None] {
        let choice = OutputChoice { device: chosen.map(str::to_owned), ..OutputChoice::default() };

        let request = choice.shared_request(SampleRate::new(48_000));

        let wanted = OutputRequest::Shared { rate: SampleRate::new(48_000), device: None };
        assert_eq!(request, wanted, "{chosen:?} chosen");
    }
}
