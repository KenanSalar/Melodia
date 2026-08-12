<!--
Pull requests target `dev`, not `main`. PRs into `main` come only from `dev`
or a `hotfix/*` branch, and CI enforces it.
-->

## Summary

<!-- What changes, and why. -->

## Related issue

<!--
Link the issue from the sidebar ("Development"), not with a closing keyword.
GitHub only interprets `Fixes #N` on PRs targeting the default branch, so it
does nothing on a PR into `dev`. Reference it here for context instead:
-->

Part of #

## Checklist

- [ ] `cargo clippy --all-targets --locked -- -D warnings` passes
- [ ] `cargo test --locked` passes
- [ ] Docs updated where behaviour or conventions changed (`CLAUDE.md`, the relevant `.claude/rules/*.md`, `README.md`)
- [ ] New user-facing strings are wrapped in `@tr(...)` and added to every `melodia-ui/translations/*/LC_MESSAGES/melodia-ui.po`

## Notes for review

<!-- Anything worth a second look: a trade-off, a known gap, something you're unsure about. -->
