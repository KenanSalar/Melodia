# ADR 7: Decode through Symphonia and cpal directly, not rodio

**Status:** Accepted, 2026-08-20

A station serving MPEG-2 AAC never started playing. No error, no timeout, just silence. Symphonia
0.5 resolves a container by matching a two-byte marker and discards the hint it is handed, with
scoring left as a `TODO` in its own source, and the only ADTS marker it registers is `0xFFF1`. An
MPEG-2 stream opens with `0xFFF9`, so it matched nothing, the search wandered into the audio, and
the first byte pair that looked like MP3 handed the stream to the MP3 demuxer, which then hunted
unbounded for a frame that was never coming. Symphonia 0.6 scores its candidates and reads those
streams. rodio pins 0.5 and cannot reach 0.6, so the radio path went direct, and the tree was left
compiling two majors of the same library with two different probes: the same bytes could get two
answers depending on whether they arrived off a socket or off disk, and an MPEG-2 `.aac` file
sitting in someone's library hit exactly what radio had just escaped.

Decision: Symphonia decodes and cpal outputs, and the mixer, the rate and channel conversion, the
playback speed and the clock between them are ours. One version of Symphonia, one probe, one
answer for the same bytes.

Alternatives: staying on rodio and living with the two majors, waiting for rodio to bump its own
pin, and a media framework underneath everything instead.

Trade: rodio was never an alternative decoder, which is what made this smaller than it looked.
Every rodio feature the tree enabled was a `symphonia-*` one, so it was a wrapper around the same
library, and dropping it meant owning what the wrapper did rather than swapping decoding engines.
What it genuinely owned was the layer underneath: the mixer, the queues, the device stream and the
sample rate conversion. Waiting was the cheapest option on paper and the tracking issue for that
pin is open, unassigned and tangled in a larger rewrite, so it is worth watching rather than
planning around. A media framework would have bought the widest format support for free at the
cost of a heavy non-Rust dependency and a second threading model, which is the shape ADR 2 exists
to keep out.

What it costs is that the mixer and the clock are ours to get right, and the bill arrived in
pieces. A deck that renders a whole device block opened a crossfade overshoot that the previous
per-sample pull had bounded, which is why `output::mixer::LOCKSTEP_FRAMES` exists and why
`crates/melodia/tests/crossfade.rs` derives its tolerance from that constant rather than guessing
one. Symphonia 0.6 refuses HE-AAC where the container declares it, so those files stopped playing
until the decoder was handed a plain AAC-LC config built from values it had already parsed. Its
PCM decoder added a width check that the CAF demuxer contradicts, so every A-law `.caf` failed to
build a decoder until that was worked around, and it was a fixture walk rather than review that
found it. Rate conversion is linear interpolation rather than a resampling library, because the
obvious library's adapter traits require `unsafe` and this workspace denies it outside platform
FFI.

Two things it bought beyond the bug. Owning the decode path means owning the codec registry, so
adding a decoder is one registration call rather than a dependency's feature flag. Owning the
output is most of what a bit-perfect mode needs, and `docs/plans/BIT_PERFECT.md` was largely a
list of workarounds for a mixer that is no longer in the way.

It went in two phases on purpose. Decode first, shippable on its own, which collapsed the two
majors and left the output layer small enough to understand completely before replacing it, rather
than changing the decoder and the output in one release in the part of the app with the least
visible failure modes.
