# Translation Catalogs From a Template (issue #114)

Working doc. Delete when the feature ships.

Status: **in progress**, Phases 0 to 3 done · Created: 2026-10-07 · Branch: `refactor/translation-catalog-template`

> **Checked 2026-10-07** against `360dda24`. Read: the `slint-tr-extractor` 1.16.1 and `rspolib`
> 0.1.2 sources (rspolib is the extractor's PO writer, and 0.1.2 is still its latest release), and
> `i-slint-compiler` 1.16.1's `translations.rs` and `pathutils.rs`. Run: GNU gettext 0.26
> `msgmerge`, `msgcat` and `msgattrib` against scratch copies of all six catalogs. The extractor
> isn't installed, so extraction was **simulated** in Python; findings 3 and 4 are confirmed
> against the real binary in Phase 1 before anything touches the catalogs.
>
> **Phase 1 confirmed findings 3 to 5** against `slint-tr-extractor 1.16.1` on 2026-10-07, and
> turned up finding 12.

## What we see

- **Adding a string means six hand edits.** There is no template, so a new `@tr` means opening all
  six catalogs and adding the same entry in the same place in each. Each file holds 777 entries and
  51 hand-drawn `# ---- Section ----` banners, and the banners have to be kept in step too.
- **The six copies have drifted.** The `Critical update` note is in German only. The `Play {} Next`
  note is English in de and fr and half translated in el, es, it and tr. A Turkish plural note sits
  on `{n} folder in your library` in all six files. The files carry between 175 and 186 comment
  lines.
- **A fuzzy or empty translation ships in English and nothing says so.** The test only checks
  msgids. Slint skips an entry rspolib doesn't call `translated()`, which is any fuzzy, obsolete or
  empty one, and falls back to the msgid.

## What ships

Adding a string becomes: write the `@tr`, run `scripts/update-translations.sh`, fill in six
msgstrs. The script puts the empty entries where they belong. A note for translators lives once,
in the `.slint` source, as a `// Translators:` line. The test fails on a missing, extra, fuzzy or
empty entry, and on a translator note that never reached the template.

The build doesn't change and still needs no gettext. Only regenerating the catalogs does.

## Prior art

This is gettext's ordinary loop: extract to a `.pot`, `msgmerge` it into each `.po`. The only
Slint-specific parts are the extractor and its comment rule, which picks up the one `//` line
directly above the line holding the `@tr`, whatever it says. That is why the script keeps only
notes starting with `Translators:`.

## Findings

1. **The merge loses nothing.** Against the simulated template, every catalog keeps 777 translated
   entries, with 0 fuzzy and 0 obsolete. Each grows from about 2,630 lines to about 3,370 (+28%,
   measured on the first run), from the `#:` lines and from unwrapping. Four to six `msgstr`s per
   catalog are wrapped today, so the test's "every entry in these catalogues is on one line today"
   is already false; `--no-wrap` makes it true again.
2. **The timestamp churns every file.** The extractor stamps `POT-Creation-Date` from the clock on
   every run, and `msgmerge` copies that field into every catalog's header. Unless the script drops
   it, each run rewrites seven files even when no string moved.
3. **rspolib writes long comments without their prefix.** A comment line wider than 76 columns
   comes out as bare wrapped text with no `#.` in front, so the raw extractor output is not valid
   PO. 6 of the 32 code comments sitting directly above an `@tr` line are that wide today. Two
   consequences: the filter has to run on the raw text before any gettext tool reads it, and a
   `Translators:` note has to fit in 76 columns, counting from `Translators:`. The real binary
   keeps the prefix at 76 columns and drops it at 77.
4. **rspolib drops locations.** When a `#:` line overflows 78 columns, the writer skips the
   occurrence that didn't fit and repeats the earlier ones on the next line. In one run over every
   file, 121 of the 777 entries lose a whole file from their `#:` line (real binary).
   That undercuts the reason for deleting the notes that say where a string lives. Extracting **one
   file at a time** fixes it: the first occurrence in a file always survives, and `msgcat` takes
   the union across files.
5. **The first comment wins.** The extractor only attaches a comment to an entry that has none
   yet, and 167 msgids are used from more than one file. In a single run, an ordinary comment above
   an earlier occurrence would hide a `Translators:` note on a later one. Per-file extraction ends
   that across files. `msgcat` does cumulate the notes, but wraps each in a
   `#. #-#-#-#-#  <file>  #-#-#-#-#` marker whenever the msgid comes from more than one input, so
   the script strips the markers.
6. **Order inside a file depends on line numbers.** Against file-only locations, `--sort-by-file`
   orders a file's entries alphabetically. Against `file:line` it follows the source. So `msgmerge`
   reads the line-numbered template from the temp dir, and the checked-in `.pot` is that same
   template reduced to file-only locations. Both then follow source order, and a second run is
   byte-identical.
7. **Headers stay per catalog.** `msgmerge` leaves a catalog's header alone apart from
   `POT-Creation-Date`, so `Language` and `Plural-Forms` keep their values. `msgfmt -c` warns on
   all six today about the missing `PO-Revision-Date`, `Last-Translator` and `Language-Team`.
8. **Slint never reads the `.pot`.** `load_translations` looks for `LC_MESSAGES/melodia-ui.po`
   under each entry in `translations/` and skips anything without one. Because of
   `rerun-if-changed=translations`, a template change rebuilds `melodia-ui`, which is harmless.
9. **Windows paths stay forward-slash.** `clean_path` keeps whichever separator it sees first, so
   the `ui/…` paths Git Bash hands over come out as `ui/…` in the `#:` lines.
10. **Every `#` line in the catalogs is a translator comment**, either `# ` or a bare `#`. There
    are no flags, locations or extracted comments, so "delete every line starting with `#`" is
    exact. No gettext tool can strip translator comments: `msgmerge`, `msgcat` and `--compendium`
    all carry them through (tested).
11. **`msgattrib --no-wrap --no-obsolete`** round-trips a merged catalog byte-identical and drops
    `#~` blocks, so a string deleted from the source leaves nothing behind in six files.
12. **A note reaches every `@tr` on the line below it.** The extractor walks back from the string
    across every token on its line, so `[@tr("A"), @tr("B")]` or a ternary's two arms on one line
    share whatever comment sits above. A note needs a line where its `@tr` stands alone: `Key` is
    noted in the tag editor rather than in `settings.slint`'s five-per-line list, and `LIVE` was
    split off `Buffering…` in `player.slint`.

## Structure

- **`scripts/update-translations.sh`** owns regeneration from end to end: the tool and version
  check, the file list, extraction, the filter, the template and the merge. One bash script that
  runs under bash 3.2 (macOS) and Git Bash, with no PowerShell twin to drift from it.
- **`crates/melodia-ui/translations/melodia-ui.pot`** is generated and never edited by hand. Its
  header comment says so.
- **The six `.po` files** hold translations and per-language `#` notes, nothing else.
- **The `.slint` sources** hold the translator notes: a single `// Translators: …` line directly
  above the line holding the `@tr`, at most 76 columns from `Translators:` on, and one note per
  msgid.
- **`crates/melodia/tests/translations.rs`** swaps the line-scanning `collect_from_catalogue` for
  one entry parser that reads the template and the catalogs alike.

What collapses: six copies of every note become one in the source, the 306 banners go, and the
test's line scanner becomes one entry parser.

## Phases

Each phase starts only when it is asked for.

- [x] Phase 0: tools ✅
- [x] Phase 1: the script ✅
- [x] Phase 2: one-off catalog cleanup ✅
- [x] Phase 3: first run ✅
- [ ] Phase 4: stricter test
- [ ] Phase 5: docs
- [ ] Phase 6: Windows run

### Phase 0: tools ✅

Done on Kenan's Linux machine 2026-10-07: `slint-tr-extractor 1.16.1`, with gettext 0.26 already
installed. The Windows half waits for Phase 6.

- `cargo install slint-tr-extractor --version 1.16.1 --locked`, the same version as the
  workspace's `slint`.
- gettext: already on Fedora. Debian and Ubuntu need the `gettext` package, since `gettext-base`
  has no `msgmerge`. On Windows, `winget install MicheleLocati.GettextIconv`.

### Phase 1: `scripts/update-translations.sh` ✅

Done 2026-10-07. The two guards (a note past 76 columns, two notes on one msgid) were each made to
fire on a scratch copy and exit before writing anything. shellcheck is clean.

1. If `slint-tr-extractor`, `msgcat`, `msgmerge`, `msgattrib` or `msgfmt` is missing, fail and
   print the install line.
2. **Version guard.** Read `slint`'s version from the root `Cargo.toml`, compare it with
   `slint-tr-extractor --version`, and refuse to run on a mismatch, printing the exact
   `cargo install` line. That makes "bumped with slint" something the script enforces, and the
   1.18 bump planned in `SLINT_NATIVE_ADOPTION.md` will trip it.
3. `cd crates/melodia-ui`, then `find ui -name '*.slint' | LC_ALL=C sort`. The sort matters
   because `find` lists files in a different order on NTFS and on btrfs. Run the extractor once per
   file with `--no-default-translation-context` (matching `build.rs`), writing numbered parts into
   a `mktemp -d` that a `trap` removes.
4. **Filter each part (awk, on the raw text).** Drop the `POT-Creation-Date` line. Keep a `#.`
   line only if it starts with `#. Translators:`. Drop bare lines, which are rspolib's unprefixed
   wrap of a long comment. **Fail** on a bare line starting with `Translators:`, naming the file
   and the 76-column limit. The comment at the filter names both rspolib bugs.
5. `msgcat --no-wrap --sort-by-file --files-from=<sorted list of parts>` builds the line-numbered
   template. Strip msgcat's `#-#-#-#-#` marker lines. **Fail** on an entry carrying two
   `Translators:` notes, because a msgid gets one.
6. Write the checked-in template from it with
   `msgcat --no-wrap --add-location=file --sort-by-file`, carrying the "generated" header comment.
7. For each `translations/*/LC_MESSAGES/melodia-ui.po`, run `msgmerge -q -U --no-wrap
   --add-location=file --sort-by-file --backup=none` against the **line-numbered** template, then
   `msgattrib --no-wrap --no-obsolete` in place.
8. End with `msgfmt --statistics` for each catalog, so a run finishes by listing what still needs
   translating.

**Before Phase 3 touches the real catalogs**, run the extraction half into a scratch directory and
confirm findings 3 to 5 with the real binary. Check each entry's `#:` files against a per-file grep
for `@tr(`, and compare one run over every file with the per-file output.

### Phase 2: one-off catalog cleanup ✅

Done 2026-10-07: 29 notes in 13 files, and 15 per-language notes. Where it left the candidate list:
the lyrics note was stale (it named the Settings tab) and now sits on the two `… the Lyrics menu`
hints it is about; `Support me on Ko-fi` and `{} / {} files · {}` got notes of their own beside
their siblings; the multi-select note covers ten msgids, the two station ones saying "stations".
The ten Turkish plural notes now share one wording. The normalized comparison differed only by
the three header fields step 4 adds.

This is content only, done in the current layout, so its diff reads as deletions rather than as a
reshuffle.

1. **Write the surviving notes into the source first**, one Edit each: a single
   `// Translators:` line of at most 76 columns, directly above the line holding the `@tr`. A note
   survives if it changes what a translator writes: what a placeholder holds, a length budget, a
   word sense, a brand that stays, or a label it has to match. Candidates from the inventory:
   - `Remove "{}" from your library? …`: `{}` is the folder's absolute path
   - `Version {}`: the version number
   - `Made by {}`: the author's name, a proper noun
   - `Play {} Next` and its multi-select siblings: `{}` is the number of selected tracks
   - `{} / {} files`, `{} s`: what the placeholders hold
   - `Support Melodia`: "support" as in funding, not technical support; Ko-fi stays as it is
   - `Key`: a musical key, as in A minor
   - `Media`: the physical medium, CD or vinyl
   - `LIVE`: a short pill
   - `Paused`: a state, not the Pause button's verb
   - `Station`: a heading as short as "Up Next"
   - `Show`: a narrow control slot
   - `Expand`: names the action, not the row
   - `Number`: the second box of "Track 3 / Total 12"
   - `Play something to see its lyrics`: "Turn it on…" has to match the Settings tab's label
   - `Welcome to Melodia`: the product name stays

   Everything else goes: notes that say where a string lives, which code wires it, or how its
   neighbours are ordered. The `#:` lines cover the first, and the rest is for code readers.
2. **Strip.** One command deletes every line starting with `#` from the six catalogs and changes
   nothing else. This is an exception to the no-sed rule that Kenan approved on 2026-10-07, for
   this step only. The check below is what makes it safe. The blank lines it leaves behind go away
   in Phase 3.
3. **Put back the per-language notes**, one Edit each, in their own catalog only:
   - **tr**: the plural notes on `{n} track`, `{n} artist`, `{n} disc`, `{n} play`,
     `{n} favorite`, `{n} station`, `{n} vote`, `{n} station added`,
     `{n} folder in your library` and `Looking for music… {n} file found`
   - **el**: `{n} track`, `{n} favorite`, `Tonal Spot`, `Support Melodia`
   - **de**: `{n} favorite`

   The Turkish note that sits in all six files today ends up in `tr` alone.
4. **Headers**, one Edit per catalog: `PO-Revision-Date: <date of the cleanup>`,
   `Last-Translator: Kenan Salar`, `Language-Team: none`.

**Verify.** For each catalog, run `msgcat --no-wrap --no-location --sort-output` before and after,
drop the `#` lines, and the two outputs are byte-identical, so no msgid or msgstr moved.
`msgfmt -c --statistics` reports 777 translated messages and no warnings.
`cargo test --locked --workspace` stays green.

### Phase 3: first run ✅

Done 2026-10-07, landing in one commit with Phases 1 and 2. Every check below held; the second
run was compared against a copy of the first rather than against `git status`.

Run the script and check in `melodia-ui.pot`. The catalogs gain their `#:` lines and the
`#. Translators:` notes, re-sort by file and unwrap.

**Verify.** The same msgcat comparison against the Phase 2 state shows no change.
`msgfmt --statistics` reports 777 translated, 0 fuzzy and 0 untranslated in every catalog. **A
second run leaves `git status` clean.** `cargo test` is green. This lands as its own commit: a large
diff that the comparison proves is mechanical.

### Phase 4: stricter test

In `crates/melodia/tests/translations.rs`:

- **One entry parser** over blank-line-separated blocks, giving each entry's notes (`#.`), flags
  (`#,`), msgid, msgid_plural and msgstrs. It keeps `read_po_value`'s continuation handling (PO
  editors wrap even though the script doesn't), skips the header and `#~` blocks, and serves the
  template and the catalogs alike.
- **Checks:**
  1. The template's entries **equal** the `.slint` tree's. A stale template means someone skipped
     the script.
  2. Each catalog's entries **equal** the template's. This replaces today's superset check.
  3. No catalog entry is fuzzy, and none has an empty `msgstr` or `msgstr[n]`. The failure lists
     each one and says it shows in English.
  4. Every `// Translators:` line in the tree sits directly above a line holding exactly one
     `@tr(` (finding 12), and its text is that msgid's `#.` note in the template. This catches a
     note split over two lines (the extractor keeps only the last), a note hidden by an earlier
     comment in the same file, and a template that wasn't regenerated.
- The notes walk gets a vacuity floor, and unreadable paths are collected rather than skipped, as
  the other corpus walks do.
- Fix the stale prose in the same pass: the one-line claim in `read_po_value`'s doc, and the
  `every_unknown_field_fallback_is_translated` message, which says to add the msgid to all six
  catalogues instead of running the script.
- Show each new check failing on a scratch mutation: an entry marked fuzzy, an emptied msgstr, a
  note split over two lines, a hand-edited `.pot`.

### Phase 5: docs

- `CONTRIBUTING.md`: the string paragraph under "Pull requests" covers the workflow (write the
  `@tr`, run the script, fill in six msgstrs) and the note rule. A short block under "Getting set
  up" says what a contributor has to install before running the script, and that only adding,
  changing or removing an `@tr` string needs it. Building, testing and editing a translation need
  nothing extra:
  - `cargo install slint-tr-extractor --version <the workspace's slint version> --locked`, plus
    the note that it moves with `slint` and that the script names the exact version on a mismatch
  - gettext: `sudo dnf install gettext` on Fedora; `sudo apt install gettext` on Debian and
    Ubuntu, since the default `gettext-base` has no `msgmerge`; `brew install gettext` on macOS;
    `winget install MicheleLocati.GettextIconv` on Windows
  - Windows runs the script from Git Bash, which comes with Git for Windows
- `CLAUDE.md`: the melodia-ui module-map bullet (the catalogs and the generated `.pot`), the i18n
  "Bundled translations" bullet, and a rewrite of "A new string means the same `msgid` in every
  shipped `.po`".
- `.github/pull_request_template.md`: the translation checklist line.
- Root `Cargo.toml`: the comment beside `slint-build` says the script checks the extractor's
  version against this pin.

### Phase 6: Windows run (Kenan's machine)

Run the script from Git Bash after a pull. `git status` has to stay clean, which proves the output
matches Linux byte for byte. Watch for CRLF from the Windows gettext build; if it shows up, the
script normalizes line endings after each gettext call. Confirm that Git Bash picks up MSYS `find`
and `sort`, and note how long the 251 extractor runs take.

## Cross-cutting

- **Build, CI and packaging don't change.** The test reads the files without needing the tools. A
  change to the `.pot` alone still runs the PR gate, since its path filter only skips `**/*.md`.
- **The extractor is a dev tool.** It never ships, so nothing goes into `licenses/`.
- **The trade, stated plainly:** catalogs about a third longer, grouping by file instead of
  hand-picked sections, and gettext needed to add a string.
- **A possible follow-up, not a phase:** report both rspolib bugs upstream. If they get fixed, the
  76-column limit and the per-file loop could go.

## Open questions

None blocking. Which notes survive is decided in Phase 2 by the rule above, and the candidate list
is a starting point rather than a verdict.
