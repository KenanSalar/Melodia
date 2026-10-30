---
paths:
  - .github/**
  - Cargo.toml
  - crates/*/Cargo.toml
---

# CI and packaging

Each workflow, composite and packaging script argues its own steps; this file holds what reaches
across them.

- **Every action is SHA-pinned** with a trailing `# vX.Y.Z`, each resolving to `node24` or a
  composite over one, so no `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`. Re-pin with
  `gh api repos/OWNER/REPO/commits/TAG -q .sha`.
- **Every compiling job installs through `.github/actions/setup-rust`**, and every Linux one,
  release slots included, provisions through `linux-system-deps`.
- **Before adding a member, grep the `release*.yml` and the packaging scripts for `melodia`.** A
  step naming something unambiguous with one member breaks deep in a matrix slot, after review.
