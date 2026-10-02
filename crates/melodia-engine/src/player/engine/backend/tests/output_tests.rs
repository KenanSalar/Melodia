//! Tests for where the output choice sends a shared stream on Windows.

use melodia_audio::player::source::audio::SampleRate;
use melodia_playback::player::playback::output::OutputRequest;

use super::OutputChoice;

const ENDPOINT: &str = "{0.0.0.00000000}.{a-test-endpoint}";

/// Windows' endpoint ids are the ones a shared stream opens by, so the device picked on the Output
/// card is where shared output plays too, and none picked follows the system default.
#[test]
fn a_shared_stream_on_windows_plays_through_the_chosen_device() {
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
