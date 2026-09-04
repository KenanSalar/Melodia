# ADR 31: Melodia ships its own packages, in five formats

**Status:** Accepted, 2026-08-12

A Rust desktop application with no distribution packager behind it reaches nobody by default. The
choice is to publish artifacts people can install directly, or to publish source and wait for
somebody downstream to package it.

Decision: every release builds five formats from one pipeline: a tarball, an AppImage, an RPM, a DEB
and an MSI, each signed, with a manifest the in-app updater reads
([ADR 19](0019-updater-trust-boundary-is-the-repo.md)).

Alternatives: a tarball only, leaving the rest to distributions; two formats covering the common
cases; source releases and no binaries at all.

Trade: waiting for downstream packagers is the option with the lowest ongoing cost and it does not
work for a project nobody has heard of yet. Somebody has to be able to install this before anyone
will package it. A tarball alone reaches the people who already know how to install a tarball, which
is the audience that needed the least help.

The cost is five toolchains and it is not amortised, because they share almost nothing. Four of the
five cannot be built on the machine this is developed on, and the MSI cannot be built on a Linux
runner at all, so most of the packaging surface is only ever exercised in CI. That has a specific
consequence: a format that quietly stops doing something correct fails nowhere until a user or a
packager reports it. The obligations that every artifact carries, the licence text most of all, are
therefore held by tests that walk the packaging inputs rather than by anybody reviewing a workflow
file, and the tool versions in that pipeline are pinned by exact version because their undocumented
behaviour is load-bearing in at least one place.

The second cost is that shipping our own packages is what makes an in-app updater necessary, which
is a whole trust boundary that would not otherwise exist. Where a format has a package manager
behind it the updater steps aside and says so, but the tarball and AppImage users have nothing else.

This ADR was written in September 2026 from `.claude/rules/ci-packaging.md` and the packaging tests.
