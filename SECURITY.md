# Security policy

## Reporting a vulnerability

Report it privately through
[GitHub's vulnerability reporting](https://github.com/KenanSalar/Melodia/security/advisories/new)
rather than in an issue, a discussion or a pull request, so it stays out of public view until a
fix ships.

Include the Melodia version (Settings → About), your OS and install method, and the steps or
input that trigger it.

## Supported versions

Only the latest release gets security fixes.

## Scope

The updater trusts anything signed with the release key, so a report that starts from a stolen
key or a hostile publisher falls outside its threat model.
[ADR 19](docs/adr/0019-updater-trust-boundary-is-the-repo.md) argues where that line sits.
