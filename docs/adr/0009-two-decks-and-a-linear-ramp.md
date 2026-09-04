# ADR 9: Crossfade on two decks, with the ramp innermost and the curve linear

**Status:** Accepted, 2026-07-11

A voice sequences its sources: appending schedules a source to start when the previous one ends,
which is exactly what makes gapless work and exactly what makes an overlap impossible. Crossfade
needs two tracks audible at once, so it needs something the playback layer did not have.

Decision: two decks, each holding its own voice on the one mixer, alternating roles for the life of
the process. The fade gain lives inside `EqSource`, innermost in the chain, and the curve is
complementary linear so the two gains always sum to one.

Alternatives: a ready-made crossfade combinator over the two sources; driving the deck's own volume
from a ticker; an equal-power sine and cosine curve.

Trade: a combinator that yields only the overlap window cannot drive an end-of-track fade without
re-decoding the outgoing track's tail seeked to its end, because the copy already playing cannot be
retroactively spliced, and seeking dies inside the window. Two decks cost two voices resident for
the life of the mixer, and an idle one contributes nothing rather than being fed silence to stay
attached.

Putting the ramp innermost rather than on the deck's volume buys four things and each one is a bug
avoided. It is sample-accurate, with no quantization from a periodic tick and no scheduler jitter.
It counts media samples, the same clock the remaining time is measured in, so the fade still lands
on the track end at any playback speed, where a volume ramp is wall-clock and desyncs the moment
speed is not 1.0. It leaves the volume control meaning the user's volume instead of racing a ramp
against live changes to it. And it lands after the chain's own clamp, which is what makes the curve
safe: the mixer sums its voices with no clamping at all, deliberately, so two decks that have each
already clamped and whose gains sum to one cannot push the sum past unity.

The curve is where the real trade is. Equal-power holds perceived loudness across the overlap, which
is the better-sounding choice for uncorrelated material, and its amplitude sum peaks at the square
root of two. Under an unclamped mixer that is clipping. Linear pays for that with the familiar dip
at the midpoint, and it is the price of the invariant that keeps the sum bounded without a clamp in
the hot path.

That invariant only holds while both ramps advance together, and they advance as their own decks are
pulled, so a fade armed between two whole-block renders leaves the outgoing deck at full gain for a
block. `output::mixer::LOCKSTEP_FRAMES` is what bounds that, and `crates/melodia/tests/crossfade.rs`
derives its tolerance from the constant under a compile-time assertion rather than guessing one.

This decision was made against a playback layer that has since been replaced
([ADR 7](0007-symphonia-and-cpal-not-rodio.md)). The mechanism moved and the shape did not: two
decks, ramp innermost, gains summing to one.
