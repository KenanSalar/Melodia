# ADR 11: AAC encoder delay is read out of the file, not switched on

**Status:** Accepted, 2026-09-02

An AAC encoder writes silence at the start of a track and pads the end, and playing those samples is
what puts a gap in a gapless album. Symphonia has a gapless option and it defaults on, which makes
this look like a setting rather than a feature. It is not: in 0.6.1 only the MP3 and Vorbis decoders
act on that flag, and the MP4 demuxer fills in neither the packet trims nor the track's delay and
padding fields, so nothing upstream takes an iTunes `.m4a`'s priming off.

Decision: read the delay out of the two places a file states it, the `iTunSMPB` tag and the edit
list, and hand back a head to skip and a playable length in decoded frames. It installs as a skip at
the open and a countdown in the packet loop, both above the shared cursor, and the seek adds the
head back because the demuxer's own timeline still opens on the priming.

Alternatives: trusting the gapless flag; deriving the length from the track duration; trimming every
format rather than AAC.

Trade: the two sources disagree, and which one wins had to be measured rather than reasoned.
`iTunSMPB` wins where both are present because it names the original sample count outright. An edit
list needs both of its numbers and the second is not interchangeable with the track duration:
deriving the length as duration minus media time left 106 frames of padding on the reference files
that exist to test exactly this. So the edit's own length wins where its timescale converts exactly,
the derived one is both the fallback and the ceiling, and the rule is divisibility rather than equal
timescales. That is a fiddly rule to carry, and it is the cost of not having a switch.

Scope is AAC alone. FLAC, ALAC, Vorbis and Opus either carry no encoder delay or have it handled
where the container states it, so widening this would add code paths with nothing to do.

It lives with the file decoder rather than in the shared decode path, and that is deliberate. The
contract there is that the file and stream decoders differ only in what they open, and an encoder
delay is part of what a file is: a live mount has no container header stating one and no gapless
transition to spoil. Putting it in the shared path would mean threading a `None` through the common
open for the stream case, where keeping it above means the stream path is untouched by construction.
