//! Tests for the ALSA backend's pure half: how each rung is spelled to the card, the order a claim
//! asks in, which refusals an open names, and the handshake a granted release waits on. The one
//! reach past it is `configure` against alsa-lib's `null` device, which takes any request and so
//! shows exactly what a claim asks for. Finding, opening and writing to a card need one, and are
//! tested by hand.

use std::sync::Arc;
use std::time::Duration;

use alsa::Direction;
use alsa::pcm::PCM;
use rustix::io::Errno;

use super::{WriterControl, alsa_format, candidates, configure, is_busy, open_error};
use crate::player::playback::output::claim::{ClaimSource, FallbackReason};
use crate::player::playback::output::encode::DeviceFormat;
use crate::player::playback::output::rates::{LADDER, RateSet};
use crate::player::playback::output::{ExclusiveRequest, ExclusiveTuning, RateFallback};
use crate::player::playback::tests::helpers::shape;
use melodia_audio::player::source::audio::{Shape, SourceFormat};

const CARD: &str = "hw:CARD=Test,DEV=0";

const S16_SOURCE: SourceFormat = SourceFormat { bits: 16, float: false, lossy: false };

/// Long past any scheduler delay, so only a release that never sees the close runs it out.
const PATIENCE: Duration = Duration::from_secs(10);

fn open_refused(errno: Errno) -> alsa::Error {
    alsa::Error::new("snd_pcm_open", errno.raw_os_error())
}

/// ALSA's name for each rung, which the panel shows too since it is what `hw_params` prints for
/// the card, and the bits a sample takes on the card, which `encode` has to fill exactly: a width
/// one byte off has the card drain the stream at the wrong rate, fast and garbled.
#[test]
fn each_rung_opens_in_the_layout_encode_writes_and_the_panel_names() {
    let rows = [
        (DeviceFormat::S16, "S16_LE", 16, 2),
        (DeviceFormat::S24Packed, "S24_3LE", 24, 3),
        (DeviceFormat::S24Low, "S24_LE", 32, 4),
        (DeviceFormat::S32, "S32_LE", 32, 4),
        (DeviceFormat::F32, "FLOAT_LE", 32, 4),
    ];
    for (format, name, card_bits, encoded_bytes) in rows {
        let on_card =
            alsa_format(format).map(|alsa| (alsa.to_string(), alsa.physical_width().ok()));
        let ours = (format.to_string(), format.bytes_per_sample());

        assert_eq!(
            on_card,
            Some((name.to_owned(), Some(card_bits))),
            "{format} as the card opens it"
        );
        assert_eq!(ours, (name.to_owned(), encoded_bytes), "{format} as encode writes it");
    }
}

/// The 17 to 24-bit partition with a step either side of it. Packed is asked first because it is
/// what many USB DACs offer alone, and the MSB-aligned rung never, ALSA reading `S24_LE` as the
/// low three bytes of the word, which is the one layout that rung isn't.
#[test]
fn a_claim_asks_for_each_source_through_the_rungs_alsa_spells() {
    use DeviceFormat::{F32, S16, S24Low, S24Packed, S32};
    let rows: [(SourceFormat, &[DeviceFormat]); 5] = [
        (SourceFormat { bits: 16, float: false, lossy: false }, &[S16, S32, S24Packed, S24Low]),
        (SourceFormat { bits: 17, float: false, lossy: false }, &[S24Packed, S32, S24Low]),
        (SourceFormat { bits: 24, float: false, lossy: false }, &[S24Packed, S32, S24Low]),
        (SourceFormat { bits: 25, float: false, lossy: false }, &[S32, F32]),
        (SourceFormat::F32, &[S32, F32, S24Packed, S24Low, S16]),
    ];
    for (source, expected) in rows {
        let asked: Vec<DeviceFormat> = candidates(source).map(|(format, _)| format).collect();

        assert_eq!(asked, expected, "{source}");
    }
}

/// A card unplugged has to read as not connected, the one reason the reclaim poll watches for and
/// the only thing that brings the claim back with the card. A busy one is asked again at every
/// track start, and anything else is a fault in the open itself.
#[test]
fn an_open_names_a_busy_or_vanished_card_and_anything_else_is_io() {
    let rows = [
        (Errno::BUSY, FallbackReason::Busy),
        (Errno::NOENT, FallbackReason::NotConnected),
        (Errno::NODEV, FallbackReason::NotConnected),
        (Errno::ACCESS, FallbackReason::Io),
        (Errno::INVAL, FallbackReason::Io),
    ];
    for (errno, expected) in rows {
        let refusal = open_error(CARD, open_refused(errno));

        assert_eq!(refusal.reason(), expected, "{errno:?}");
    }
}

/// Only a busy card is waited out, since the sound server closes one a moment after letting it go.
/// The wait runs under the decks lock on a reopen, so waiting on a card that has gone would hold
/// every transport op for the whole of it and fail anyway.
#[test]
fn only_a_busy_card_is_worth_waiting_for() {
    let rows =
        [(Errno::BUSY, true), (Errno::NOENT, false), (Errno::NODEV, false), (Errno::AGAIN, false)];
    for (errno, expected) in rows {
        assert_eq!(is_busy(&open_refused(errno)), expected, "{errno:?}");
    }
}

/// A release answered before the card is closed sends the asker into the `EBUSY` the reservation
/// exists to avoid. The thread stands in for `alsa-out`, which closes the card once told to stop.
#[test]
fn a_release_is_granted_once_the_writer_has_closed_the_card() {
    let control = Arc::new(WriterControl::default());
    let writer = std::thread::spawn({
        let control = Arc::clone(&control);
        move || {
            while !control.stopping() {
                std::thread::yield_now();
            }
            control.mark_closed();
        }
    });

    let closed = control.stop_and_wait(PATIENCE);

    // Ahead of the join, which would wait out a writer that was never told to stop.
    assert!(closed, "the release gave up waiting for the writer to close the card");
    assert!(writer.join().is_ok(), "the stand-in writer panicked");
}

/// A writer that never closes the card costs the asker the wait and a no, the card still being
/// open. It is told to stop all the same, so it lets go as soon as it can.
#[test]
fn a_writer_that_never_closes_the_card_is_refused_but_still_stopped() {
    let control = WriterControl::default();

    let closed = control.stop_and_wait(Duration::ZERO);

    assert_eq!((closed, control.stopping()), (false, true), "(closed, told to stop)");
}

/// `null` takes any shape, format and period, so what a claim lands on there is what it asked for:
/// the source's own channel count, the first rung ALSA spells on its ladder, and the tuned period
/// at the source's rate.
#[test]
fn a_card_that_takes_anything_opens_at_exactly_what_the_claim_asks_for() -> Result<(), ClaimSource>
{
    let rows = [
        (
            "mono 16-bit at 44.1 kHz",
            shape(1, 44_100),
            SourceFormat { bits: 16, float: false, lossy: false },
            (1, DeviceFormat::S16, 882, 882),
        ),
        (
            "stereo 24-bit at 96 kHz",
            shape(2, 96_000),
            SourceFormat { bits: 24, float: false, lossy: false },
            (2, DeviceFormat::S24Packed, 1_920, 1_920),
        ),
        (
            "5.1 float at 48 kHz",
            shape(6, 48_000),
            SourceFormat::F32,
            (6, DeviceFormat::S32, 960, 960),
        ),
    ];
    for (what, shape, format, expected) in rows {
        let pcm = PCM::new("null", Direction::Playback, false)?;

        let config = configure(&pcm, &claim(shape, format, RateFallback::Shared))?;

        let landed =
            (config.channels.get(), config.format, config.requested_period, config.period_frames);
        assert_eq!(landed, expected, "{what}: (channels, format, period asked, period taken)");
    }
    Ok(())
}

/// A claim on any card, at `shape` and `format`, with `rate_fallback` as the rate policy.
fn claim(shape: Shape, format: SourceFormat, rate_fallback: RateFallback) -> ExclusiveRequest {
    ExclusiveRequest {
        device: None,
        shape,
        format,
        tuning: ExclusiveTuning::default(),
        hardware_volume: false,
        rate_fallback,
    }
}

/// Under Resample a claim keeps the ladder rates the card offers, so a later track can ask
/// whether a fresh claim would land where this one runs. `null` offers every rate, so the whole
/// ladder comes back. Playing through the system mixer converts nothing, so nothing is asked.
#[test]
fn a_claim_that_may_resample_keeps_every_ladder_rate_the_card_offers() -> Result<(), ClaimSource> {
    let rows = [
        (RateFallback::Resample, Some(LADDER.into_iter().collect::<RateSet>())),
        (RateFallback::Shared, None),
    ];
    for (rate_fallback, expected) in rows {
        let pcm = PCM::new("null", Direction::Playback, false)?;

        let config = configure(&pcm, &claim(shape(2, 44_100), S16_SOURCE, rate_fallback))?;

        assert_eq!(config.offered, expected, "{rate_fallback:?}");
    }
    Ok(())
}

/// A rate off the ladder has no bit in the set, so a claim asks the card about the source's own
/// rate directly. Read off the set alone, a 24 kHz track would be converted on a card that runs it.
#[test]
fn a_source_rate_off_the_ladder_runs_where_the_card_takes_it() -> Result<(), ClaimSource> {
    let pcm = PCM::new("null", Direction::Playback, false)?;

    let config = configure(&pcm, &claim(shape(2, 24_000), S16_SOURCE, RateFallback::Resample))?;

    assert_eq!(config.rate.get(), 24_000);
    Ok(())
}
