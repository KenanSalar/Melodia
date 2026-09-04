# ADR 32: A release is a pushed tag, not a merge

**Status:** Accepted, 2026-08-28

The repository ran a permanent integration branch that a release merged into the default branch.
That left the default branch, which is what a clone gets and what the project's front page shows,
an entire release behind for most of every cycle: over a hundred commits at a time, on files that
had been rewritten in the meantime, so a first contribution rebased into conflicts that had
nothing to do with it.

Decision: the default branch is the integration branch, pull requests target it, and a release is
a pushed version tag that triggers the build. Merging ships nothing on its own.

Alternatives: keeping the integration branch; cutting a release branch per version and tagging on
it, which is what larger projects do.

Trade: the integration branch looked like a checkpoint and was not one. Nothing ran against it: the
gate runs per pull request, against each one's merge commit in isolation, so a batch of a hundred
commits was never collectively exercised by anything. What it actually provided was a name for the
batch being tested locally before a release, and the default branch provides that just as well.

Release branches lose for a reason specific to this project rather than on principle. They exist to
support more than one version at once, and this supports exactly one: the update manifest carries a
single entry and the updater has no notion of a supported-version window. The day a patch has to
ship for a version the default branch has moved past, the branch can be cut from the tag and it
exists. Nothing here forecloses that, which is the argument for not paying for it now.

What this trades away is real. The default branch stops being the last released state, so its
history no longer reads as a release log and "what shipped" lives in the tags instead. That is the
one honest argument for leaving things as they were.

A pushed tag being the trigger makes the tag load-bearing: every download URL and the update
manifest name it, so a tag that moves or disappears breaks links for everyone already on that
version. Deleting and force-updating are blocked by rule, and the cost of that is that a blocker
found during release QA costs a patch version rather than a re-pointed tag.

This ADR was written in September 2026; `docs/RELEASING.md` is the procedure.
