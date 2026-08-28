# Releasing

`main` is the integration branch and a pushed `v*.*.*` tag is the release. Merging
changes nothing on its own; tagging is the deliberate act.

## Cutting a release

**1. Bump the version on `main`.** `[workspace.package] version` in `Cargo.toml` is
the one line, and `Cargo.lock` carries it too, so run `cargo build` (or
`cargo update -w`) to refresh both entries. Every build slot compiles `--locked`
and a stale lockfile fails all ten. Land it through a PR like anything else.

**2. Tag the merge commit and push the tag.**

```bash
git switch main && git pull
git tag -a v0.12.0 -m "Melodia v0.12.0"
git push origin v0.12.0
```

The tag must be `v` plus the version you just set. `prepare` compares the two and
fails the run in seconds rather than an hour into the matrix, because the binary,
all five package formats and `latest.json` take their version from `Cargo.toml`
and would otherwise ship under a name the tag contradicts. Recovering from that
mismatch means correcting the version on `main` and re-pointing the tag, which is
an admin bypass of the `v*.*.*` ruleset even though nothing has been built yet.

**3. Wait for the draft.** `release.yml` holds on a RustSec advisory, then runs ten
build slots, signs every artifact with minisign, attests them through Sigstore and
collates the lot into a draft release with `latest.json` beside it.

**4. QA the draft and write the release notes.** The draft body starts as GitHub's
auto-generated text; the updater panel shows what you leave there.

**5. Publish.**

```bash
gh release edit v0.12.0 --draft=false --latest
```

Drafts aren't served by `/releases/latest/download/`, so this flip is the moment
installed clients can see the release. `refresh-manifest.yml` fires on publish and
re-signs `latest.json` from the final body, which is why step 4 comes first.

## When the draft is wrong

Bump to the next patch, tag again, delete the old draft. Re-pointing the tag is the
other option and needs an admin bypass of the `v*.*.*` ruleset, which is deliberate: a
published tag is referenced by every download URL and by `latest.json`, so moving
one breaks links for everybody already on that version.

## Re-running a build

`workflow_dispatch` runs from a branch ref, where there is no tag to check, so
`prepare` falls back to `Cargo.toml` alone and rebuilds into the existing draft in
place. That is the recovery path for a slot that failed on infrastructure rather
than on code.

## Patching a version `main` has moved past

Nothing automatic covers this: `latest.json` carries a single entry and the updater
has no notion of a supported-version window. If it ever comes up, cut the branch
that doesn't exist yet and tag on it:

```bash
git switch -c release/0.11 v0.11.0
```

The tag trigger doesn't care which branch a tag points into.
