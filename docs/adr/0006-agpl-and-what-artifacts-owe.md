# ADR 6: AGPL-3.0-or-later, and what every artifact then owes

**Status:** Accepted, 2026-05-25

Melodia is published, packaged in five formats and installed by people who will never read its
source. Whatever licence it carries decides two separate things: what someone may do with a
modified copy, and what every artifact has to ship alongside the binary.

Decision: AGPL-3.0-or-later, with the licence text and every third-party notice shipped in
`licenses/` by all five package formats.

Alternatives: GPL-3.0-or-later, MPL-2.0, a permissive MIT and Apache-2.0 dual licence, and
AGPL-3.0-only.

Trade: the AGPL is the strongest copyleft available, and it is chosen as a statement about what
this project is for rather than as a mechanism this binary exercises. That distinction is worth
being honest about, because the network clause is the whole difference between the AGPL and the
GPL and a desktop music player does not interact with anyone over a network in the sense that
clause means. So the practical difference today is close to nothing, and the cost is not nothing:
the AGPL is on the exclusion list of a number of organisations, which narrows who can contribute
and who can ship it, and it forecloses a commercial fork in a way the maintainer wants foreclosed
but a permissive licence would not. It also constrains dependencies going forward. A decoder
carrying patent terms that keep it out of distribution repositories is the wrong trade under five
package formats whether or not the licence itself is compatible, and that argument only exists
because the packaging obligations below are real. "or-later" is there so a future revision can be
adopted without tracking down every contributor.

The obligation half is the part that costs ongoing work. Two fonts and the vendored winit fork
compile into the binary, so every artifact redistributes third-party work and owes its licence
text: Apache-2.0 section 4(a) requires it and the OFL FAQ recommends it for a bundled font. That
is five formats built by five toolchains, one of them an MSI that no Linux runner can produce, so
a format that quietly stops shipping the text fails nowhere until a packager files a bug. It is
held by tests that walk the packaging inputs rather than by review, for exactly that reason, and
`crates/melodia/tests/packaging.rs` is where they live.

This ADR was written in September 2026. The licence was chosen before the repository's first
commit and no argument for it exists in the tree; the obligations half is reconstructed from
`.claude/rules/ci-packaging.md` and the packaging tests, and the choice itself from the
maintainer's account.
