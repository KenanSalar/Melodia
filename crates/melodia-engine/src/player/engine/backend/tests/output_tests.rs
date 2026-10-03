//! Tests for where the output choice sends a shared stream, and at which rate, both of which differ
//! by platform.

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

/// Windows shared mode converts every stream to the mix format, so asking for the file's rate
/// changes nothing the device runs at and costs a reopen at every track start. The engine keeps it
/// off whatever the settings file says.
#[cfg(target_os = "windows")]
#[tokio::test]
async fn a_shared_stream_on_windows_never_asks_for_the_files_rate()
-> Result<(), melodia_core::error::AppError> {
    use std::num::NonZero;

    use melodia_audio::player::source::audio::{Shape, SourceFormat};
    use melodia_playback::player::playback::decks::DECK_COUNT;
    use melodia_playback::player::playback::output::mixer;

    use super::PlaybackEngine;

    let hi_res = Shape {
        channels: NonZero::new(2).unwrap_or(NonZero::<u16>::MIN),
        rate: NonZero::new(96_000).unwrap_or(NonZero::<u32>::MIN),
    };
    let (mixer, _pull) = mixer::pair(DECK_COUNT, hi_res);
    let engine = PlaybackEngine::new(&mixer, tokio::runtime::Handle::current())?;
    engine.set_follow_rate(true);

    let request = engine.wanted_request(hi_res, SourceFormat::F32);

    assert_eq!(request, OutputRequest::Shared { rate: None, device: None });
    Ok(())
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
