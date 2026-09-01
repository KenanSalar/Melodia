//! Tests for the block sizes the device negotiation asks for.
//!
//! Opening a stream needs a card, so the ladder and the attempt loop are out of reach here. The two
//! functions that decide *what* each rung asks for are not: they are pure functions of a config, and
//! each carries an argument — the halved maximum, the ordering-free clamp, the staging floor that
//! deliberately does not follow the request down — that nothing else in the tree can check.

use super::{period_frames, staging_samples, target_frames};

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
