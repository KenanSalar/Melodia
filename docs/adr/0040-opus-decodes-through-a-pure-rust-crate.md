# ADR 40: Opus decodes through a pure-Rust crate, not libopus

**Status:** Accepted, 2026-09-21

An `.opus` file in a watched folder was invisible. It never appeared after a scan, adding the folder
again did nothing, dropping the file on the window was refused, and Browse would not list it, so a
folder of Opus files looked like an empty folder with no error to go looking for. A station
streaming Opus was the same gap one step further along: the badge named the format and the stream
never played, which reads as a station that is down rather than one the player cannot open.
Everything needed to reach the audio was already in place. Symphonia demuxes Ogg Opus completely,
down to handing the whole identification packet over as extra data, and then no decoder in the tree
claims the codec.

Decision: Opus decodes through `opus-pure`, a BSD-3-Clause crate with no dependencies and no build
script, registered into our own codec registry as one line.

Alternatives: the maintained adapter over a vendored libopus, two other pure-Rust adapters, and
waiting for a first-party Symphonia decoder.

Trade: the crate is young and lightly adopted, so this rests on measurement rather than on a track
record. Against the reference implementation, 12 seconds of 128 kb/s stereo came back at 109 dB
signal-to-noise, with a largest difference of one unit at 16 bits on 0.084% of samples, and a frame
count of 576000, which is exactly 12.00 seconds and not one frame either side. The frame count is
the one that mattered. Opus states its own priming and padding, and a decoder that mishandles either
still sounds right and still scores well, so an accuracy threshold on its own would have passed the
failure this was picked to avoid. What it costs is that nobody else has found the bugs first.

The first measurement was wrong, and finding that out was most of the work. A first pass read
79.7 dB at two units of difference on 71% of samples, which looks like a decoder with a rounding
problem. It was the crate's own example WAV writer and its truncating cast to 16-bit integers;
pushing the reference implementation's float output through the same rule reproduced the identical
error, at which point the harness rather than the decoder was the thing to fix. A measurement that
has not been validated against a known-good input is not evidence, and this one very nearly became
the reason to reject the crate.

libopus was the obvious route and would have cost more than it bought. The adapter is thin, but
underneath it is a vendored C library built with CMake, which lands in every release slot and puts a
media C dependency in a binary whose case for existing is that it has none
([ADR 2](0002-native-rust-desktop-app.md)). It also does less: mono and stereo only, no reading of
the header's playback gain, no honouring of the gapless option that the Vorbis decoder beside it
honours, and a buffer reallocated whenever a packet's frame count changes, which for a local file is
an allocation on the audio callback thread. Paying a build-system dependency for a decoder that is
measurably no better here and handles fewer files is the wrong way round.

Of the two other pure-Rust adapters, one is LGPL-2.1. That is workable under AGPL, but it would be
the only copyleft audio dependency in the tree, and by the criterion
[ADR 6](0006-agpl-and-what-artifacts-owe.md) sets it would owe an attribution entry and a licence
text in all five package formats. The other is permissive and sits at 0.1.x on both halves of its
stack, with hand-written SIMD on the decode path, which this workspace denies outside platform FFI
([ADR 28](0028-no-unsafe-outside-ffi-no-unwrap-anywhere.md)). BSD-3-Clause is the same licence
libopus itself ships under, and an unmodified permissive crate owes no attribution entry, so the
packaging side of this decision is empty.

Waiting was not an option with a date on it. There is no first-party Opus decoder published, the
facade offers no feature that would enable one, upstream's own status table still marks it as not
started, and of the two open contributions the further along implements only the speech half of the
format, which is the wrong half for music. That is a reason to keep watching rather than to plan
around, and switching later is cheap for the same reason the crate choice is cheap to reverse.

What makes the youth of the crate survivable is owning the seam, which is what
[ADR 7](0007-symphonia-and-cpal-not-rodio.md) bought. The codec registry is ours rather than the
library's fixed feature set, so the decoder is registered by one line in it, and replacing the crate
behind that line changes nothing at either call site. The alternative routes all register into the
same registry behind the same trait, so adopting a first-party decoder later is a dependency line
and a registration call.

Three things had to live inside the decoder that an adapter would have left undone, and each is
there because no layer above the codec can reach it. The priming comes off here rather than in the
file decoder, because only the identification packet states it in a form all three containers carry:
Matroska fills no track delay and MP4 fills neither, so a reader above the codec would have covered
Ogg alone. The header's playback gain is mandatory to apply, so it is applied unconditionally in the
decoder rather than folded into the loudness columns at scan, where a user toggle would have gated
it. And surround is channel mapping family 1, which the libopus adapter cannot do at all; it carries
a plane reorder nothing above the codec could perform, the decoder emitting the format's own channel
order while a buffer's planes run in ascending channel position, which puts the low-frequency
channel fourth where the format puts it last.

Registering the decoder also turned on Opus inside Matroska and MP4, both already scanned, because
all three containers hand the same identification packet over and the priming comes off in the same
place for each. That was free, and it is the shape of win that owning the registry keeps producing.

The decision is held by a frame count rather than by an accuracy figure. `player::source::opus`'s
tests assert the frames a fixture decodes to against what the reference implementation reads out of
the same file, which is the only assertion a priming or padding regression moves, and the surround
fixture gives each channel a tone in a window of its own so the plane order is checked rather than
assumed.

This ADR was written in September 2026 from the Opus working doc, deleted when that feature shipped.
The measurements above were taken during that work and are reproduced from it rather than re-run.
