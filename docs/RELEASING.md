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
an admin bypass of the `v*.*.*` ruleset even though nothing has been built yet. Where
the typo names a version above the one you meant, pushing the tag you meant is cheaper
and needs neither: the check fails in `prepare`'s first step, so nothing exists under
the mistyped tag, not even a draft, and the correct tag is still free. The cost is that
number, which the ruleset keeps alive until a later cycle has to skip past it.

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

Bump to the next patch, tag again, delete the old draft. Deleting a draft leaves its tag
behind and the ruleset blocks that deletion too, so the abandoned tag stays, pointing at
a commit that never shipped. Re-pointing the tag is the other option and needs an admin
bypass of the `v*.*.*` ruleset, which is deliberate: a published tag is referenced by
every download URL and by `latest.json`, so moving one breaks links for everybody
already on that version.

## Re-running a build

Dispatch **on the tag**, not on a branch: the Run workflow dropdown lists tags as well
as branches, or `gh workflow run release.yml --ref v0.12.0`. That rebuilds the commit
the draft was signed from, and every release run shares one concurrency group, so a
re-run started while the first is still going queues instead of racing it into the same
draft. The version check runs and passes, which is why it is worth leaving live.

A branch ref is for a build nobody has tagged yet. From `main` after the next bump has
landed it resolves *that* version's tag and opens a different draft, which is not a
re-run of anything. Neither path survives publication: `prepare` short-circuits on a
published tag, so the recovery window is the draft.

## Patching a version `main` has moved past

Nothing automatic covers this: `latest.json` carries a single entry and the updater
has no notion of a supported-version window. If it ever comes up, cut the branch
that doesn't exist yet and tag on it:

```bash
git switch -c release/0.11 v0.11.0
```

The tag trigger doesn't care which branch a tag points into.
