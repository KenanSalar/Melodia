# ADR 29: A vendored winit fork, checked in, with a stated retirement

**Status:** Accepted, 2026-05-25

Dragging files from a file manager onto the window does not work on Wayland with the released
windowing crate. The fix exists as unmerged commits upstream. Dropping files onto a music player
is not a nicety, and waiting for a merge is waiting on someone else's schedule.

Decision: the fork is checked into the repository at `winit/` and wired in by a patch entry, so a
fresh clone and CI both build with no setup at all.

Alternatives: waiting for the upstream merge and shipping without Wayland drag and drop; a git
dependency pointing at a fork hosted elsewhere; patching at build time.

Trade: a git dependency is the lighter-looking option and it moves the problem rather than solving
it. It makes the build depend on a second host staying up and on a revision staying reachable, and
it means a fresh clone cannot build offline. Checked in, the fork is as available as the rest of
the tree and its diff is reviewable in place.

What it costs is the whole of a windowing crate sitting in the repository, showing up in every
clone and every search, carrying its own licence obligation into all five package formats (ADR 6),
and needing a rebase whenever the base version moves. It also means the vendored copy can drift
from what the UI toolkit expects without anything saying so.

One trap is worth recording because it looks like a tidy-up. The geometry types crate underneath is
deliberately not vendored: vendoring by path creates a second instance that cannot unify with the
one another dependency pulls from the registry, and the clash only appears on Windows.

The retirement condition is specific rather than aspirational, and both halves are required: the
upstream release that ships the merged drag-and-drop API, and a toolkit release that surfaces
external drops as file paths. Until both land the fork stays, and when they do it goes by deleting
the directory and the patch entry, along with renaming the two version-pinned feature and module
names that ride on the current major.

This ADR was written in September 2026. The fork predates the repository's first commit.
