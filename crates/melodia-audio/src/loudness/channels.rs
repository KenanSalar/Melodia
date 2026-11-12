//! The BS.1770 weight each of a decoded file's channels takes.

use ebur128::Channel;
use symphonia::core::audio::Position;

const LFE: Position = Position::LFE1.union(Position::LFE2);

/// Side and back alike take the ±110° surround weight: a 5.1 file labels its surround pair either
/// way. The crate's ±135° and 180° are BS.1770's rear, heard at unity, so neither pair maps there.
const SURROUND_LEFT: Position = Position::SIDE_LEFT.union(Position::REAR_LEFT);
const SURROUND_RIGHT: Position = Position::SIDE_RIGHT.union(Position::REAR_RIGHT);

/// The map a meter over `channels` channels takes, or `None` for the crate's default by count,
/// which is all a layout naming no positions has to go on.
///
/// Mono is dual mono: the output copies a mono source to both front channels at unity, so it is
/// heard as two, and measured as one it would play 3 dB hotter than the same audio in stereo.
pub(super) fn channel_map(channels: usize, positions: Option<Position>) -> Option<Vec<Channel>> {
    if channels == 1 {
        return Some(vec![Channel::DualMono]);
    }
    let map: Vec<Channel> = positions?.iter().map(weighted).collect();
    (map.len() == channels).then_some(map)
}

/// The crate reads a label only for its weight, so every channel heard at unity takes `Center`.
fn weighted(position: Position) -> Channel {
    if position.intersects(LFE) {
        Channel::Unused
    } else if position.intersects(SURROUND_LEFT) {
        Channel::LeftSurround
    } else if position.intersects(SURROUND_RIGHT) {
        Channel::RightSurround
    } else {
        Channel::Center
    }
}
