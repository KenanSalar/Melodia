//! Tests for what each rung of the device negotiation asks for.
//!
//! Opening a stream needs a card, so the attempt loop is out of reach here. What each rung *asks
//! for*, and the order the ladder tries them in, are not: the block sizes, the rate list and the
//! ranking are pure functions, and each
//! carries an argument — the halved maximum, the ordering-free clamp, the staging floor that
//! deliberately does not follow the request down, the strict test that stops a standard rate
//! duplicating an endpoint — that nothing else in the tree can check.

use super::{Rungs, ladder, period_frames, rates_for, staging_samples, target_frames};
use melodia_audio::player::source::audio::SampleRate;

const RATE: u32 = 48_000;

/// 50 ms at 48 kHz.
const TARGET: cpal::FrameCount = 2_400;

fn config(
    channels: u16,
    rate: u32,
    buffer_size: cpal::SupportedBufferSize,
) -> cpal::SupportedStreamConfig {
    cpal::SupportedStreamConfig::new(channels, rate, buffer_size, cpal::SampleFormat::F32)
}

fn range(min: cpal::FrameCount, max: cpal::FrameCount) -> cpal::SupportedBufferSize {
    cpal::SupportedBufferSize::Range { min, max }
}

#[test]
fn the_target_is_fifty_milliseconds_of_frames_at_the_config_rate() {
    assert_eq!(target_frames(&config(2, RATE, range(0, u32::MAX))), TARGET);
    assert_eq!(target_frames(&config(2, 44_100, range(0, u32::MAX))), 2_205);
}

/// cpal turns a `Fixed` period into a request for twice as much buffer, and the range bounds the
/// buffer — so a period at the whole maximum lands on one period per buffer, with nothing to refill
/// from. The rung we want is the largest period the device can actually double-buffer.
#[test]
fn the_period_leaves_room_for_the_second_half_of_the_buffer() {
    let tight = config(2, RATE, range(64, 1_024));
    assert_eq!(period_frames(&tight), 512, "asking for the whole buffer leaves no room to refill");
}

#[test]
fn a_period_the_device_can_carry_whole_is_left_at_the_target() {
    assert_eq!(period_frames(&config(2, RATE, range(64, 65_536))), TARGET);
}

#[test]
fn a_floor_above_half_the_ceiling_still_wins() {
    // `min` is applied last because the two costs are not the same: cpal checks the request against
    // the whole reported range before any backend sees it, so a period under the floor loses the
    // rung outright, where one over half the ceiling only loses the second half of its buffer.
    assert_eq!(period_frames(&config(2, RATE, range(900, 1_024))), 900);
}

/// The pair comes straight off a driver. `clamp` asserts its bounds are ordered, so a device
/// reporting them backwards would panic the boot rather than fail one rung.
#[test]
fn a_range_reported_backwards_does_not_panic() {
    assert_eq!(period_frames(&config(2, RATE, range(8_192, 64))), 8_192);
}

#[test]
fn an_unknown_range_is_left_at_the_target() {
    assert_eq!(period_frames(&config(2, RATE, cpal::SupportedBufferSize::Unknown)), TARGET);
}

/// The multiply happens in `u128`, so even the widest rate a config can name divides back down
/// instead of wrapping to a tiny block. Nothing reaches the saturating fallback at this target — it
/// is there so that raising [`TARGET_BUFFER`] cannot quietly turn an overflow into a 3-frame period.
///
/// [`TARGET_BUFFER`]: super::TARGET_BUFFER
#[test]
fn the_widest_rate_a_config_can_name_does_not_wrap() {
    let absurd = config(2, u32::MAX, cpal::SupportedBufferSize::Unknown);
    assert_eq!(target_frames(&absurd), 214_748_364);
}

/// Staging is what the callback writes into before the block is converted out, so it is sized
/// against what a *host* may hand over. A period narrowed to what the device can double-buffer
/// cannot drag it down with it, or the host's own larger block allocates its way back up.
#[test]
fn staging_does_not_follow_the_period_down_a_tight_range() {
    let tight = config(2, RATE, range(64, 1_024));
    assert_eq!(period_frames(&tight), 512);
    assert_eq!(staging_samples(&tight), TARGET as usize * 2);
}

/// The other direction, which the floor reaches: a device that will not go below its own minimum
/// gets a period *over* the target, and staging that stayed at the target would leave the very first
/// callback resizing on the audio thread.
#[test]
fn staging_follows_a_period_the_devices_floor_pushed_above_the_target() {
    let floored = config(2, RATE, range(8_192, 65_536));
    assert_eq!(period_frames(&floored), 8_192);
    assert_eq!(staging_samples(&floored), 8_192 * 2);
}

#[test]
fn staging_covers_every_channel_of_the_block() {
    let surround = config(6, RATE, range(64, 65_536));
    assert_eq!(staging_samples(&surround), TARGET as usize * 6);
}

/// The top first, since a device reporting a range is likeliest to run at it, then 48 kHz ahead of
/// 44.1 — the order cpal's own `try_with_standard_sample_rate` walks, and the one its default
/// config now prefers.
#[test]
fn a_wide_range_tries_both_standard_rates_between_its_ends() {
    let rungs: Vec<_> = rates_for(8_000, 192_000).collect();
    assert_eq!(rungs, [192_000, cpal::SAMPLE_RATE_48K, cpal::SAMPLE_RATE_CD, 8_000]);
}

/// A rung costs a stream the driver may take its time refusing, so a standard rate only earns one where it falls *strictly* inside: on an endpoint it is already
/// the rung either side, and outside the range `try_with_sample_rate` would drop it anyway.
#[test]
fn a_standard_rate_only_earns_a_rung_strictly_inside_the_range() {
    assert_eq!(rates_for(44_100, 48_000).collect::<Vec<_>>(), [48_000, 44_100]);
    assert_eq!(rates_for(48_000, 48_000).collect::<Vec<_>>(), [48_000]);
    assert_eq!(rates_for(96_000, 192_000).collect::<Vec<_>>(), [192_000, 96_000]);
}

fn supported(min: u32, max: u32) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(
        2,
        min,
        max,
        cpal::SupportedBufferSize::Unknown,
        cpal::SampleFormat::F32,
    )
}

fn rates(configs: &[cpal::SupportedStreamConfig]) -> Vec<u32> {
    configs.iter().map(cpal::SupportedStreamConfig::sample_rate).collect()
}

/// A device that only runs 48 kHz, one that spans the CD rate, and one that tops out at it.
fn three_ranges() -> Vec<cpal::SupportedStreamConfigRange> {
    vec![supported(48_000, 48_000), supported(44_100, 192_000), supported(8_000, 44_100)]
}

/// Following the file means *not* taking the device's default, so every range that can run the
/// requested rate is tried at exactly that rate before anything else, in the order given.
#[test]
fn a_requested_rate_leads_with_every_range_that_can_run_it() {
    let rate = SampleRate::new(cpal::SAMPLE_RATE_CD);
    let Rungs { preferred, .. } = ladder(three_ranges(), rate);
    assert_eq!(rates(&preferred), [cpal::SAMPLE_RATE_CD, cpal::SAMPLE_RATE_CD]);
}

#[test]
fn a_rate_no_range_can_run_prefers_nothing() {
    let Rungs { preferred, .. } = ladder(three_ranges(), SampleRate::new(4_000));
    assert!(preferred.is_empty(), "{:?}", rates(&preferred));
}

/// The request only puts rungs in front. What follows is the walk a boot with no request takes, so
/// a refused rate still lands where the output would have opened anyway.
#[test]
fn a_requested_rate_leaves_the_fallback_walk_as_it_was() {
    let unrequested = ladder(three_ranges(), None);
    let requested = ladder(three_ranges(), SampleRate::new(cpal::SAMPLE_RATE_CD));

    assert!(unrequested.preferred.is_empty());
    assert_eq!(rates(&requested.fallback), rates(&unrequested.fallback));
    assert_eq!(
        rates(&unrequested.fallback),
        [48_000, 192_000, cpal::SAMPLE_RATE_48K, 44_100, 44_100, 8_000]
    );
}
