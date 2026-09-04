<!--
Pull requests target `main`. Releases are pushed tags, not merges, so nothing
here ships until a `v*.*.*` tag says so.
-->

## Summary

<!-- What changes, and why. -->

## Related issue

<!--
Link the issue from the sidebar ("Development"), not with a closing keyword.
The link is set once on the branch and survives every reword of this body, and
it is what closes the issue on merge. Reference it here for context instead:
-->

Part of #

## Checklist

- [ ] `cargo clippy --all-targets --locked -- -D warnings` passes
- [ ] `cargo test --locked` passes
- [ ] Docs updated where behaviour or conventions changed (`CLAUDE.md`, the relevant `.claude/rules/*.md`, `README.md`)
- [ ] A change that moves an architectural seam names the ADR it follows, or adds one under `docs/adr/`
- [ ] New user-facing strings are wrapped in `@tr(...)` and added to every `crates/melodia-ui/translations/*/LC_MESSAGES/melodia-ui.po`

## Notes for review

<!-- Anything worth a second look: a trade-off, a known gap, something you're unsure about. -->
