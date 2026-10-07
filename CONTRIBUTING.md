# Contributing

Thanks for taking a look. Bug reports, fixes, features and translations are all
welcome, and a small PR needs no ceremony.

## Getting set up

[Building from Source](README.md#building-from-source) has the prerequisites and the
build. The toolchain is pinned by `rust-toolchain.toml`, so cargo picks the right one
on its own.

A build from this repository keeps its library, settings and database in a separate
`Melodia-dev` data directory, so you can break things without touching an installed
copy. Set `MELODIA_DATA_DIR` if you want a scratch directory of your own.

Run this once per clone, so the tree-wide formatting commits stay out of `git blame`:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

Adding, changing or removing an `@tr(...)` string means regenerating the translation
catalogues, which needs two tools that building, testing and translating don't. The first
is the string extractor, at the `slint` version in the root `Cargo.toml`:

```bash
cargo install slint-tr-extractor --version <slint version> --locked
```

It moves with `slint`, and `scripts/update-translations.sh` prints the exact line to run
when it is missing or out of step. The second is GNU gettext:

- Fedora: `sudo dnf install gettext`
- Debian and Ubuntu: `sudo apt install gettext`, since the default `gettext-base` has no
  `msgmerge`
- macOS: `brew install gettext`
- Windows: `winget install mlocati.GetText`, then run the script from Git Bash, which comes
  with Git for Windows

## Reporting a bug

Open an issue with the bug report template. `Melodia --logs` prints the log directory
on Linux and macOS; on Windows it is `%APPDATA%\Melodia\logs\`. Attaching the tail of
the log helps a lot, and it holds no credentials by design.

## Before you build something big

For a fix or a small improvement, just open a PR. For anything with real scope, open
an issue first so we can agree on the shape. Melodia is opinionated about where things
live, and a conversation up front is cheaper than a rewrite after review.

Before proposing a different library, a different threading shape or a different boundary
between crates, read [`docs/adr/`](docs/adr/). Each record is one page and says what was
chosen, what was not, and what the choice costs, so you can see whether your idea was
already weighed and on what grounds. If it was and the grounds have changed, that is a good
issue to open. A change that moves one of those seams should name the ADR it follows, or add
one.

## Pull requests

- **Target `main`**, the only long-lived branch. Merging ships nothing on its own:
  releases are pushed tags.
- **One PR, one logical change**, with commit messages in the
  [Conventional Commits](https://www.conventionalcommits.org/) style the history uses.
- **Link the issue from the PR's Development sidebar** rather than writing `Fixes #N`.
  The link is set once on the branch and survives every reword of the description.
- **Say how you tested it.** For UI changes that means running the app, not just
  `cargo test`.

Before pushing:

```bash
cargo fmt --all
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

The lint configuration is strict on purpose, and `unwrap()` is denied everywhere,
tests included. Reach for `?`, for `expect()` with the invariant in the message, or
for `let ... else`.

If you add, change or remove a user-facing string, wrap it in `@tr(...)` and run
`scripts/update-translations.sh`. It rebuilds the template,
`crates/melodia-ui/translations/melodia-ui.pot`, which nobody edits by hand. In every
catalogue beside it, each new or changed string comes back as an empty `msgstr`, or as a
`#, fuzzy` guess where gettext found a near match, for you to finish. A note for
translators goes in the `.slint` file as one `// Translators: …` line directly above the line
holding the `@tr`, with no second `@tr` on that line and at most 76 columns from
`Translators:` on. `cargo test` fails on a catalogue that is out of step with the source or
still has a fuzzy or empty entry. Slint shows either in English without a word, and a plural
counts as empty while any one of its forms is.

## What CI runs

Every pull request runs **PR Validation**: a `cargo audit` advisory scan, a `cargo fmt`
check, `clippy` with `-D warnings`, and the test suite on both Linux and Windows — the
Windows job skips the one integration test that needs an audio device. The aggregate
`pr-validation` check has to be green before a merge. Documentation-only changes skip
all six.

The Windows job is the one that can go red on a change you tested green locally, and
the usual cause is a path: build them with `Path::join` or `MAIN_SEPARATOR_STR` rather
than spelling a separator, in fixtures as much as in code.

Coverage is a separate manual run (**Actions → Deploy Coverage → Run workflow**),
published to [kenansalar.github.io/Melodia](https://kenansalar.github.io/Melodia/).

## On AI

Write it by hand or with an assistant, whichever you work best with. Review treats
both the same, which also means the bar is the same: you are responsible for what you
submit, so understand it well enough to explain it and fix it, and test it for real
rather than trusting generated tests. Please write issue and PR comments yourself.
