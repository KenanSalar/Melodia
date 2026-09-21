# Opus Support

Working doc. Delete when the feature ships; leave an ADR behind first.

Status: **in progress**, Phases 0 to 2 landed · Created: 2026-09-21

Issue [#35](https://github.com/KenanSalar/Melodia/issues/35), branch `feat/opus-decoding`.
The lofty bump in Phase 0 ships on this branch too, as its own commit, not a separate PR.

> Upstream facts below were verified **2026-09-20** against the pinned `symphonia 0.6.1` /
> `lofty 0.24.0` sources in the registry, the `lofty 0.25.4` and `opus-pure 0.2.1` crate tarballs,
> and this tree's `Cargo.lock`. The decode measurements were taken in-session against
> `ffmpeg`'s libopus; the protocol is in the decision section.
>
> Anything marked ⚠️ **re-verify** is expected to drift; check it again on the day rather than
> trusting this doc.

---

## The symptom

An `.opus` file in a watched folder is invisible. `utils::audio_ext::AUDIO_EXTENSIONS` has no entry
for it, so the library walk skips it, the watcher ignores it, drag-and-drop refuses it and Browse
will not show it. Reached anyway, `decode::open` gets as far as a track and then fails:
`symphonia-format-ogg` maps Ogg Opus completely (48 kHz, channels off the mapping family, the whole
`OpusHead` handed over as extra data) and then no decoder in symphonia 0.6.1 claims
`CODEC_ID_OPUS`.

The same gap shows on the radio side one step further along. `stream_source::codec_from_mime` maps
`audio/opus` to the label `"OPUS"`, so a station serving Opus paints a codec badge over a stream
that cannot decode.

## The decision: a pure-Rust decoder, not libopus

Both routes end at the same Symphonia `AudioDecoder` trait, so this is a dependency choice rather
than an architectural one.

`symphonia-adapter-libopus` 0.3.0 is 209 lines over `opusic-sys`, which vendors libopus and builds
it with CMake. It handles mono and stereo only, never reads the `OpusHead` output gain, ignores
`AudioDecoderOptions::gapless` where Vorbis honours it, and reallocates its `AudioBuffer` whenever a
packet's frame count changes. For a local file here that last one is an allocation on the cpal
callback thread. It is still the shape to copy, minus those three.

Two pure-Rust adapters exist and neither is the answer. `symphonia-adapter-oporus` 0.1.3 over
`oporus` 0.2.4 is **LGPL-2.1**, workable under AGPL-3.0 but the only copyleft audio dependency we
would be adding, and by `licenses/ATTRIBUTION.txt`'s own criterion it would owe an entry plus a
licence text in all five package formats. `symphonia-adapter-opus-rs` 0.1.1 over `ruopus` 0.1.2 is
permissive, and it is 0.1.x on both halves with unsafe SIMD kernels on the decode path.

`opus-pure` 0.2.1 (BSD-3-Clause, the same licence as libopus) has zero dependencies, no `build.rs`,
and ships an `OpusMSDecoder`. BSD-3 is permissive and the crate is unmodified, so by that same
criterion it owes no attribution entry. Measured rather than quoted:

| | |
|---|---|
| its own suite, `cargo test --release` on rustc 1.97.0 | **339 passed, 0 failed, 3 ignored** |
| SNR against ffmpeg's libopus, 12 s of 128 kb/s VBR stereo | **109.0 dB** |
| largest absolute difference | **1 LSB** at 16-bit, on 0.084% of samples |
| frame count | 576000, exactly 12.00 s, **delta 0** |
| throughput | 12.00 s of audio in 0.02 s wall, single-threaded |

The frame-count delta is the one that matters: pre-skip and end-trim land exactly where libopus puts
them. A first pass read 79.7 dB at 2 LSB, which turned out to be that crate's example WAV writer and
its `×32767` truncating cast, confirmed by pushing ffmpeg's own float output through the same rule
and reproducing the identical 2 LSB on 71% of samples.

Taking libopus would add CMake and a vendored C build to ten release slots, and a media C dependency
to a binary whose pitch is that it has none, in exchange for a decoder that is measurably no better
here and does less. The risk is that `opus-pure` is young and lightly adopted. What mitigates it is
owning the seam: `decode::CODECS` is our registry, so swapping the crate behind it is one line there
and nothing at either call site.

**Native Symphonia Opus is not the plan.** ⚠️ **re-verify:** `symphonia-codec-opus` does not exist on
crates.io and the facade's 0.6.1 feature list has no `opus` entry, both re-checked 2026-09-21, and
upstream's own status table still marks it as not started. The two open PRs both conflict and the further along of the two implements
SILK only, which is the wrong half for music. Both routes register into the same `CodecRegistry`
behind the same trait, so adopting a first-party decoder later is a dependency line and a
registration call.

---

## Structure

Where each piece lands, and what stays out of reach of what.

| what | where | note |
|---|---|---|
| the decoder | `crates/melodia-audio/src/player/source/opus.rs` | new module, beside `aac_trim.rs`, which is the precedent for a codec quirk the tiers below do not handle |
| registration | `decode::CODECS`, `decode.rs:45-49` | one line; nothing at either call site changes |
| `opus-pure` | `[workspace.dependencies]`, named by `melodia-audio` alone | the tier boundary in `.claude/rules/audio-stack.md` holds: `source` names the codec, `playback` never does |
| pre-skip, end-trim, output gain, surround | inside that module | **not** `file_decode`, which is where the AAC delay had to go. Only a decoder sees `packet.trim_end`, and only the identification packet states the priming in a way all three containers carry: Matroska fills no `Track::delay` and MP4 fills neither, so a reader above the codec would cover Ogg alone |
| R128 read | `media/ingest/metadata.rs`, beside the four `ReplayGain*` keys | one helper, no format branch |
| R128 storage | the four existing `replaygain_*` columns | no schema change, no DSP change |
| zero-channel guard | `metadata::read_tags`, on the resolved `FileType` | `sniff_file_type` only runs once the extension has resolved to nothing, so it never sees a file named `.opus` |

One duplication this deliberately does not create: **the gain is not folded into the ReplayGain
columns at scan**, which would be the cheaper edit and would silently gate a mandatory-to-apply gain
on a user toggle. `opus_pure` applies it inside the decoder through the `gain_q8` field libopus's
own `OPUS_SET_GAIN` exists for.

---

## Phase 0 — lofty 0.24.0 to 0.25.4 ✅ done

**First commit on this branch, ahead of the Opus work, and it ships with it.** Keep it a commit of
its own so the bump and the three unrelated fixes it carries stay bisectable, but the whole feature
lands as one PR.

Why it goes first: `ItemKey::R128TrackGain` and `R128AlbumGain` arrived in 0.25.0. Without them
R128 has to be read through the concrete `lofty::ogg::OpusFile`, because `ItemKey` is a closed enum
with no `Unknown` variant and `VorbisComments::split_tag` leaves an unmapped key in the remainder,
where the generic `Tag`, and therefore `TaggedFile`, never sees it. That would mean a second lofty
open per Opus file, bypassing the single-opener discipline `crates/melodia/tests/lofty_open.rs` pins.
**This bump deletes a module rather than enabling one.**

- [x] Bump `lofty` to `0.25.4` in `[workspace.dependencies]` (full `x.y.z`, per the dep convention).
- [x] Confirm the zero-line claim. Every breaking change in 0.25.x lands either on API we never name
      (`LoftyError`, `Mp4Codec`, `crate::ogg::VorbisComments`, `TagType::remove_from_path`) or on
      ground our own convention already covers: every call site is
      `.map_err(|e| AppError::metadata(msg, e))`, and `metadata` takes
      `impl Into<Box<dyn Error + Send + Sync>>`, which absorbs whatever concrete error type lofty now
      returns. MSRV moves to 1.89 against our 1.97 pin.
      **Held for production code and not for the suite:** two tests pinned the broken write half as
      an equality and failed, which is what an equality is for.
      `role_tags_tests::each_format_names_exactly_the_roles_it_has_no_key_for` and
      `tag_writer_tests::mp3_reports_the_roles_it_has_no_key_to_write` both listed six unwritable
      `ID3v2` roles where there is now one. `role_tags`' module doc needed a line too: it had
      arranger written on FLAC, Ogg and APE only, which was the write bug rather than the mapping.
- [x] Verify each of the three fixes against a real file, since all three fail silently today:
      - **MP3 role-credit writes fail.** `Tag::insert_text` returns `false` for
        `ItemKey::{Producer,Arranger,Engineer,MixDj,MixEngineer}` on an ID3v2 tag, because the
        support lookup misses the `TIPL` special-casing that the write conversion does handle. That
        is exactly `role_tags::push_values`, which contradicts its own doc comment.
        `tag_writer::apply_recording_id` already hand-works-around the MBID half of the same defect
        with `insert_unchecked`.
      - **MP3 movement number and total are dropped.** `insert_text` succeeds, so nothing reaches
        `UnsupportedFields` and the value simply never lands in the file.
      - **First-time M4A tagging can corrupt the file.** The `moov.stco` offsets are not updated when
        `udta`/`meta` have to be created, which is `apply_to_file`'s `insert_tag` branch, on the
        faststart layout ffmpeg and iTunes write.

**Gate:** the full static gate, plus a tag round-trip on an MP3 covering role credits and movement
number, and a first-time tag write to a previously untagged faststart M4A that still plays after.

Passed. `fmt` and clippy clean, 3508 tests green. The three fixes, measured rather than taken from
the changelog: the unwritable `ID3v2` role set went from six to one, with a producer credit now
reading back off a real MP3; movement number and total round-trip; and a first-time tag write onto a
faststart M4A whose `udta` had to be created leaves the decoded audio bit-identical, checked by
md5 either side rather than by the file still opening.

---

## Phase 0.5 — A tag edit that says what it could not write, and why ✅ done

Phase 0 left two loose ends, and they are the same end seen twice: a tag edit reports its failures
badly. One field lies about succeeding, and the toast that covers the rest names neither the field
nor the reason.

This is also the answer to the `performer` hole rather than a workaround for it. Performer stays
unsupported on MP3 and M4A by design: lofty removed `ItemKey::MusicianCredits` deliberately in
0.24.0 as an "ID3v2-specific field with a special format", and closed the issue asking for the
`TMCL` half having landed only `TIPL`, so there is nothing upstream to wait for. Reading it would
cost a second `Id3v2Tag` parse per MP3 on the scan path, and writing it without reading it is worse
than not writing it at all, because the re-extract after a save would blank the credit the user just
entered. What was actually wrong is that the app never said so.

### The `MusicBrainz` recording id stops lying

`apply_recording_id` exists only because `Tag::insert_text`'s support check had no `UFID` mapping
and refused the key, so the function reaches past it with `insert_unchecked`. On 0.25.4 the checked
insert is accepted and writes a correct `UFID` frame carrying the `http://musicbrainz.org` owner,
measured against a real MP3 rather than read off the changelog.

- [x] Delete `apply_recording_id` and its doc comment. It is `apply_string` with one difference that
      no longer exists, so the call site becomes an ordinary `apply_string` with
      `ItemKey::MusicBrainzRecordingId` and a field name.
- [x] Name the behaviour change in the commit: an unchecked insert always "succeeds", so the MBID is
      the one field that can never appear in `UnsupportedFields` today. After this it reports like
      every other field, which is the point, and `library::mbid`'s auto-tagging writes through the
      same path so a container that cannot take the id starts surfacing instead of silently dropping.

### The toast names the field and the container

Today the user gets `"{n} files have unsupported fields"`: a count, with no field and no reason. A
performer credit on an MP3 is honest and useless.

- [x] **Type the field name.** `apply_edit` passes 29 bare `&'static str` literals as the reported
      field, which is the magic-string shape that makes a translated label impossible. Replace them
      with a `TagField` enum whose string form is today's value, so the report carries something a
      label can be derived from. The ten roles are already typed through `CreditRole::as_db_str`, so
      they fold in as one `TagField::Credit(CreditRole)` rather than ten more variants.
- [x] **Carry the container.** The reason is a property of the format, and `TagEditReport.unsupported`
      is only `(path, fields)`. `apply_to_file` already resolves `primary_tag_type()`, so hand that
      back with the rest.
- [x] **Translated labels, under the `@tr` constraint.** `@tr` resolves literals at codegen and a
      `[string]` seeded from Rust renders whatever Rust pushed, so the labels are inline literal
      lists in `.slint` indexed by position. `tag-editor-body.slint`'s `role-labels` is that pattern
      already and its order matches `ROLES` exactly, so the credit half reuses it; the 29 non-credit
      fields owe a list of their own.
- [x] **Fold before phrasing.** Report the distinct container-and-field pairs rather than one line
      per file: a batch edit of one album is one format and one field repeated, so the useful message
      is a single "Performer isn't supported by MP3 files". Keep the existing count as the fallback
      where a batch genuinely spans several formats or fields, since a toast is one line.
- [x] **Pin the new list's order.** A positional label list mislabels silently when it drifts, which
      is exactly what `smart_criteria_tests` guards against by `include_str!`ing the `.slint` and
      asserting order and length. The new list owes the same, and `role-labels` has no such pin
      today despite its own comment warning that a label out of order files a conductor as a
      producer.
- [x] Every new string owes the same `msgid` in all six catalogues, which
      `translations.rs::every_translated_literal_has_a_msgid_in_every_catalogue` enforces.
- [x] Correct `role_tags`' module doc while there: it explains the performer hole as `split_tag`
      leaving `TMCL` in a `pub(crate)` companion tag, which is true but reads as though a newer lofty
      might close it. Say the `ItemKey` was removed on purpose.

**Gate:** clippy and tests. A performer credit saved onto an MP3, and an arranger onto an M4A, each
produce a message naming the field and the container rather than a count. A `MusicBrainz` id written
to a container that maps it still round-trips, and one written to a container that does not now
reports instead of vanishing.

Passed. `fmt` and clippy clean, 3510 tests green. The M4A half measured on the fixture rather than
reasoned about: an arranger credit saved beside a composer comes back `Some("M4A")` with the
arranger alone unwritten, the composer landed in the file, and a `label_index` pointing at the slot
`settings.slint` spells `"Arranger"`, so the sentence is "Arranger can't be saved to M4A files".

Two things the bullets above named and the first pass did not carry all the way. `format_name`'s
answer is an `Option`, because its fallback for a container lofty grows later is interpolated into a
translated sentence and any filler word would read as English inside whatever locale is up; a
container we cannot name puts the toast back on the count. And the backfill's half of the MBID
bullet is a log rather than a toast, `tasks::mbid_backfill` having no UI to raise one.

The two write paths log it differently, which is the half worth remembering. The backfill goes per
file through `WriteOutcome::log_unsupported`, which only suits it because its set is structurally
almost always empty: every primary tag type maps the recording id. A tag edit is a batch the user
sized, so it folds through `TagEditReport::log_unsupported`, one line per container-and-fields
group naming a path and the count behind it, and it sits in `write_tag_edit` rather than in
`apply_tag_edit` so the rating write-back is covered by construction and no later `?` can drop the
report before it is recorded. `tag_writer::describe_unsupported` is the clause both spell, so the
fallback for a container lofty grows later cannot drift between them. Nothing else records which
file lost a field.

---

## Phase 1 — The decoder ✅ done

New module `crates/melodia-audio/src/player/source/opus.rs`, holding an `AudioDecoder` and
`RegisterableAudioDecoder` for `CODEC_ID_OPUS` over `opus_pure`. **Phase 2 ships in the same
commit**: the extension const is the only gate on the scan, the watcher, import and Browse, so a
registered decoder without it is a commit nothing can validate.

- [x] `opus-pure = "0.2.1"` in `[workspace.dependencies]`, named by `melodia-audio` alone.
- [x] Register with one line in `decode::CODECS` (`decode.rs:45-49`), whose doc comment already names
      this case as the reason the registry is ours rather than `symphonia::default::get_codecs`.
      Nothing changes at either call site.
- [x] **Two fields off `extra_data`, at their RFC 7845 §5.1 offsets.** Not one parse:
      `opus_pure::OpusHead::parse` is `pub(crate)` and unreachable, and `symphonia_common`'s own
      `OpusHead` is not re-exported by the facade and keeps neither the gain nor the mapping table.
      The container hands the identification packet over whole, Symphonia has already validated it,
      and only the pre-skip and the gain are read back.
- [x] **Pre-skip.** Symphonia's Ogg Opus packet parser returns a zero discard on every path
      (`mappings/opus.rs::parse_next_packet_dur` returns `(dur, Duration::ZERO)`), so
      `packet.trim_start` is always zero for Opus, unlike Vorbis. The count sits on `Track::delay`,
      **which no decoder can reach**: `decode::open` takes the id, the timebase and the length off
      the `Track` and drops it, and `make_decoder` sees `AudioCodecParameters` alone. It is also
      Ogg-only, Matroska shifting the timestamp by `codec_delay` instead and MP4 filling nothing. So
      it is read off the header and added to `packet.trim_start` on the first packet, a one-shot
      spent afterwards and not re-armed by `reset`.
- [x] **End padding.** This half does arrive, on the last packet's `trim_end`, worked out by the Ogg
      reader from the granule position. Applied through `AudioBuffer::trim`, the same call Vorbis
      makes, and under `opts.gapless` the same way.
- [x] **Output gain.** Q7.8 dB, RFC 7845 §5.1, which asks that players apply it by default.
      Symphonia parses it into `OpusHead.gain` and the Ogg mapper discards it, so nothing downstream
      applies it. It belongs here and **not** in the ReplayGain path: it is mandatory to apply, so it
      must not be gated on the ReplayGain toggle. `opus_pure` exposes `gain_q8` for exactly this, so
      it is a field assignment rather than a multiply of ours.
- [x] Size both buffers once at open, to the longest packet Opus allows (120 ms at 48 kHz), so
      nothing on the decode path grows a `Vec`. This decoder is pulled inline on the cpal callback for
      a local file, and it is the point at which the C adapter reallocates. `opus_pure` still
      allocates two or three small `Vec`s per packet inside its own frame-range bookkeeping, bounded
      by the frame count; that is upstream's and it is not the PCM buffer.
- [x] Do **not** add a soft-clip stage. `opus_pure`'s float output is not bounded by ±1, since codec
      ringing carries slightly past it, matching libopus. The DSP chain clamps whenever anything is
      enabled. A nonlinearity on every Opus track, to correct an overshoot the device converts
      identically either way, is the wrong trade. Say so in the module doc so it is not re-proposed.

**Gate:** clippy and tests. A `.opus` file decodes through `FileDecoder` at the right rate and
channel count. Keep the module inside the 800-line cap; surround lands in Phase 4 on top of this.

Passed. `fmt` and clippy clean, 3510 tests green, the module at 224 lines. The decode measured
rather than assumed: `test-assets/silence.opus` states `pre_skip = 312` and a container duration of
1.0065 s, and comes out of `FileDecoder` at 48 kHz, two channels and **exactly 48000 frames**, which
is what ffmpeg's libopus writes out of the same file. Priming and padding both land where they
should.

**The skew that buys the other two containers, recorded once.** The Ogg mapper leaves
`absgp_to_ts` at identity, so the granule position arrives still counting the priming and the first
packet's PTS is zero rather than negative. A decoder that swallows the pre-skip therefore hands back
audio that runs `pre_skip` frames ahead of the demuxer's timeline: the 1.0065 s above against 1.000 s
of audio, so a stated duration is that much long and a seek lands that much early, 6.5 ms at the 312
frames RFC 7845 recommends. Under everything that reads those numbers here, and the alternative is a
head reader above the codec that would serve Ogg and neither of the others. Argued in the module
doc, with `Track::delay` named as where exactness would come from.

---

## Phase 2 — Ingest: the extension surface, and the panic it exposes ✅ done

One phase, because the second half is caused by the first, and one commit with Phase 1 for the
reason stated there.

**The guard.** `lofty::ogg::opus::properties` ends its channel validation with
`ChannelMask::from_opus_channels(properties.channels).expect("Channel count is valid")`. 0.25.x adds
a `channel_mapping_family > 1` guard ahead of that, so ambisonic Opus now errors rather than
panicking. It does **not** guard `channels == 0`, which still falls to `from_opus_channels`'s
`_ => None` arm and still panics. Read out of the 0.25.4 source, not taken from the changelog.
`panic = "abort"` is in `[profile.release]`, so there is no `catch_unwind` to reach for, and
`extract_or_filename_row`'s `Err` arm cannot help either: a crafted or truncated `.opus` in a watched
folder **aborts Melodia mid-scan**. It is unreachable today only because `.opus` is not scanned,
which makes it this phase's exposure to create rather than a pre-existing one to note and leave.

- [x] Refuse a zero-channel Opus header before lofty parses it, **in `read_tags` on the resolved
      `FileType`, not in `sniff_file_type`**: that one runs only once the extension has resolved to
      nothing, and lofty maps `"opus"` directly, so it never sees the file the panic is about. The
      check derives the packet's offset from the page's own segment count rather than assuming 37,
      so a packet laced into two segments cannot walk past it, and hands anything it cannot make
      sense of to lofty like every other malformed header.
- [ ] Report it upstream.
- [x] `"opus"` into `AUDIO_EXTENSIONS` (`crates/melodia-core/src/utils/audio_ext.rs`). That single
      const is the whole gate: the walk, the watcher, import, Browse and the argv filter all route
      through `is_audio_extension`. No sniff is needed for *identification*, since lofty maps
      `"opus" => FileType::Opus` directly, unlike `.oga`.
- [x] `test-assets/silence.opus`, generated with ffmpeg, plus a `*.opus binary` line in
      `.gitattributes` beside its siblings.
      `file_decode_tests::every_scanned_extension_reaches_a_decoder` walks the const and opens
      `silence.<ext>` for each; `scanner_tests::collects_all_supported_extensions` asserts the count.
- [x] Two `crates/melodia/wix/main.wxs` rows, `SupportedTypes` **and**
      `Capabilities\FileAssociations`, held by `packaging.rs::the_msi_offers_every_audio_extension`.
- [x] `audio/x-opus+ogg` in the four `.desktop` sources: `scripts/Melodia.desktop`,
      `assets/desktop/Melodia.desktop.tmpl`, and the heredocs in `scripts/build-appimage.sh` and
      `scripts/build-rpm.sh`. Held by
      `desktop_integration_tests::all_desktop_sources_agree_on_mime_and_wmclass`, whose list is
      deliberately hand-maintained rather than derived from the const.

**Gate:** `cargo test --locked --workspace`, where the two walks fail loudly if the fixture or a
packaging row is missing. A zero-channel `.opus` header reaching `read_tags` comes back an
`AppError` instead of taking the process down.

Passed. Both halves measured on a crafted file rather than reasoned about: `silence.opus` with its
channel byte zeroed panics inside `lofty-0.25.4/src/ogg/opus/properties.rs:132` on
`"Channel count is valid"` when handed straight to lofty, and comes back
`Metadata error: … states no audio channels` through `read_tags`. The unmodified fixture reads
normally either way.

**Registering the decoder also turns on Opus inside `.mka` and `.m4a`**, both already scanned:
`symphonia-format-mkv` maps `A_OPUS` and `symphonia-format-isomp4` parses `dOps` into the same
`extra_data`. Neither container states a delay, so those files keep their 6.5 ms of priming; the
skew argued in Phase 1 is the same one.

---

## Phase 3 — R128 loudness

Opus carries no `REPLAYGAIN_*`. It carries `R128_TRACK_GAIN` and `R128_ALBUM_GAIN` (RFC 7845 §5.2),
Q7.8 fixed-point dB against EBU R128's −23 LUFS reference. `extract` reads only the four
`ItemKey::ReplayGain*` keys today (`metadata.rs:316-327`), so **every Opus file would read as
untagged and play at unity while the rest of the library is normalised**, which is the failure mode
that looks like the feature working.

- [ ] Read `ItemKey::R128TrackGain` and `R128AlbumGain` beside the existing four. On 0.25.4 these are
      plain `ItemKey`s, so they go through the same `text(tag, key)` helper as everything else: no
      format branch, no second open.
- [ ] Convert `value / 256.0`, then **add 5 dB** for the distance to ReplayGain 2.0's −18 LUFS
      reference. Argue it at its definition with both reference levels named, since it is exactly the
      constant that reads as a bug to the next person.
- [ ] Store into the existing four columns. No schema change and no DSP change; album mode, the
      preamp and prevent-clipping all keep working untouched.
- [ ] `REPLAYGAIN_*` wins where both are present, that path being already reference-aligned and
      needing no offset.
- [ ] Peak stays `None`, R128 defining none. The prevent-clipping path already handles an unknown
      peak.
- [ ] Unit-test the conversion: a known Q7.8 value round-trips, and the −23 to −18 offset is applied
      exactly once.

**Gate:** an Opus track tagged by `rsgain` sits at the same loudness as the rest of the library
rather than 5 dB under it.

---

## Phase 4 — Surround

Channel mapping family 1 above two channels. The C adapter cannot do this at all.

- [ ] Route mono and stereo to `opus_pure::OpusDecoder`, and family 1 above two channels to
      `OpusMSDecoder::new(48_000, channels, mapping_family)`.
- [ ] The supported set is exactly family 0 and family 1, because Symphonia's own `OpusHead::read`
      refuses family 2 and above before we see it, and builds the positioned layout up to 8 channels
      itself.
- [ ] `ChannelLayout::surround` derives the stream and coupled counts from the channel count, so
      validate those against the header's own `stream_count` and `coupled_count`, and **refuse a file
      where they disagree** rather than decoding it into the wrong channels.

**Gate:** a 5.1 fixture (`ffmpeg -ac 6 -c:a libopus`) decodes to six channels, and a header with a
mismatched `stream_count` is refused.

---

## Phase 5 — Seek pre-roll

RFC 7845 §4.2 wants roughly 80 ms decoded and discarded ahead of a seek target, or the first frames
come off a cold decoder.

- [ ] `file_decode::try_seek` already trims from wherever the demuxer landed to the exact requested
      position, via `samples_before(required_ts, actual_ts)`, so those frames *are* pre-roll. What is
      missing is a guaranteed amount of it. Ask the demuxer for `target − pre_roll` and trim to
      `target`.
- [ ] Make the pre-roll a per-codec figure that is zero for every codec but Opus, so nothing else
      changes behaviour.

**Gate:** `a_seek_lands_on_the_frame_it_asked_for` still passes for every existing format, and an
Opus seek lands on the frame it asked for.

---

## Phase 6 — Radio

- [ ] Nothing to build. Registering the decoder is all an Opus station mount needs, since
      `stream_decode` hands `decode::open` a MIME hint and everything from the codec inward is
      shared. Confirm that `codec_from_mime`'s existing `"OPUS"` label now sits over a stream that
      plays.

**Gate:** an Opus station tunes, plays, and survives a reconnect.

---

## Phase 7 — Docs and exit

- [ ] `README.md` format list in the feature section.
- [ ] The `[package.metadata.deb] extended-description` in `crates/melodia/Cargo.toml`.
- [ ] Root `CLAUDE.md`: the Symphonia formats bullet, and the R128 +5 dB offset. The "pure-Rust
      backend" claim in the header survives this change, which is a direct consequence of the route
      chosen and worth stating in the PR rather than leaving silent.
- [ ] Call the three Phase 0 fixes out in the PR description. They ship inside a PR titled for Opus
      and none of them is about Opus, so a reader scanning the release for why their MP3 role credits
      started saving has nothing else to go on.
- [ ] Delete this file.

---

## Verification across the whole feature

- `cargo fmt --all --check`, then `cargo clippy --all-targets --locked --workspace -- -D warnings`.
- `cargo test --locked --workspace`.
- A decode cross-check against ffmpeg's libopus on the committed fixture, run the way the measurement
  in the decision section was run. **Identical frame count is the assertion**, because that is what
  catches a pre-skip or end-trim regression, which an SNR threshold alone would not.
- Manual, after the static gates and only on the go-ahead: an `.opus` file scans with the right
  duration, metadata and artwork; plays; seeks; survives a gapless transition into and out of an MP3
  neighbour; and crossfades.
