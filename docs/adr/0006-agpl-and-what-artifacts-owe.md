# ADR 6: AGPL-3.0-or-later, and what every artifact then owes

**Status:** Accepted, 2026-05-25

Melodia is published, packaged in five formats and installed by people who will never read its
source. Whatever licence it carries decides two separate things: what someone may do with a modified
copy, and what every artifact has to ship alongside the binary.

Decision: AGPL-3.0-or-later, with the licence text and every third-party notice shipped in
`licenses/` by all five package formats.

Alternatives: GPL-3.0-or-later, MPL-2.0, a permissive MIT and Apache-2.0 dual licence, and
AGPL-3.0-only.

Trade: the AGPL is the strongest copyleft available, and it is chosen as a statement about what this
project is for rather than as a mechanism this binary exercises. The distinction matters, because
the network clause is the whole difference between the AGPL and the GPL and a desktop music player
does not interact with anyone over a network in the sense that clause means. So the practical
difference today is close to nothing, and the cost is not nothing: the AGPL is on the exclusion list
of a number of organisations, which narrows who can contribute and who can ship it, and it
forecloses a commercial fork in a way the maintainer wants foreclosed but a permissive licence would
not. It also constrains dependencies going forward. A decoder carrying patent terms that keep it out
of distribution repositories is the wrong trade under five package formats whether or not the
licence itself is compatible, and that argument only exists because the packaging obligations below
are real. "or-later" is there so a future revision can be adopted without tracking down every
contributor.

The obligation half is the part that costs ongoing work. Two fonts and the vendored winit fork
compile into the binary, so every artifact redistributes third-party work and owes its licence text:
Apache-2.0 section 4(a) requires it and the OFL FAQ recommends it for a bundled font. That is five
formats built by five toolchains, one of them an MSI that no Linux runner can produce, so a format
that quietly stops shipping the text fails nowhere until a packager files a bug. It is held by tests
that walk the packaging inputs rather than by review, for exactly that reason, and
`crates/melodia/tests/packaging.rs` is where they live.

**Amendment, 2026-09-08:** the constraint named above has bound. `kakasi`, the Japanese
romanization engine [ADR 39](0039-romanization-runs-three-engines.md) chose, is GPL-3.0 with no
permissive arm, which an AGPL-3.0-or-later work may link under GPLv3 section 13 with the combination
staying AGPL. Auditing that claim showed it was not the first: Slint is tri-licensed and an AGPL
project takes its GPL-3.0 arm, and Symphonia's sixteen decoder crates are MPL-2.0. So the
obligation had been outstanding for longer than the dependency that surfaced it. `licenses/` now
carries both texts, `ATTRIBUTION.txt` names the four crates they cover, and the rule it states has
moved from "what this repository modifies or carries" to that plus "what it links under copyleft".

**Amendment, 2026-09-09:** the rule grew a fourth clause, and this one is not about copyleft.
`uroman`, the general romanization engine beside `kakasi`, is Apache-2.0 and ships a NOTICE file,
which section 4(d) asks be reproduced wherever the work is redistributed. Nothing about permissive
terms decides that: the obligation is the NOTICE's existence, so an entry is owed for a crate whose
licence would otherwise leave it among the hundreds that are not listed. `ATTRIBUTION.txt` now names
five crates, four for their licence and one for its NOTICE, and states the rule as what this
repository modifies, carries a copy of, links under copyleft, or ships a NOTICE for.

This ADR was written in September 2026. The licence was chosen before the repository's first commit
and no argument for it exists in the tree; the obligations half is reconstructed from
`.claude/rules/ci-packaging.md` and the packaging tests, and the choice itself from the maintainer's
account.
