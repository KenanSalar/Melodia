# ADR 19: The updater's trust boundary is the GitHub repo

**Status:** Accepted, 2026-05-25

Melodia replaces its own binary on machines it has no other relationship with. That makes the
updater the most dangerous code in the tree by a wide margin: everything else can corrupt a library,
and this can install anything, anywhere, on every install at once. So the question that has to be
answered first is not how to make it safe but what exactly is being trusted, because a threat model
that is not written down is one that quietly expands until it covers nothing.

Decision: the trust boundary is the GitHub repository. A `latest.json` carrying the publisher's
minisign signature is trusted content, and so is every artifact signed with the same key. The client
verifies the manifest signature before it parses a byte of it, and verifies each artifact before
installing it. The threat model covers transport and integrity and stops there.

Alternatives: relying on TLS alone; treating the manifest as attacker-controlled and defending
against a hostile publisher; having no in-app updater and delegating entirely to package managers.

Trade: TLS alone trusts whoever controls the host, the certificate path and every redirect in a
release asset URL, and it gives a client no way to tell a substituted asset from a real one. Signing
moves all of that onto one key, and verifying before parse rather than after download is what keeps
a malformed or swapped manifest from reaching the parser at all.

Defending against a hostile manifest is the alternative that sounds strictly better and is not.
Every mitigation in that direction, refusing an asset whose target does not match, cross-checking
one artifact against another, is a defence against the publisher, and if the publisher's key is
compromised the attacker signs whatever those checks demand. So they buy nothing real and cost
complexity in the one code path that has to stay readable enough to audit. The line is drawn where
it actually holds.

What this costs, and it is not small: everyone who installs Melodia is trusting one key and the
account security behind it. There is no second signer, no threshold, no transparency log the client
consults, and no way for a user to notice a manifest served only to them. Build provenance is
attested through Sigstore and can be verified out of band, which is the closest thing to an
independent check, but nothing in the client looks at it. Dropping the updater entirely would remove
all of this and hand the problem to package managers, which is the right answer for the formats that
have one, and it leaves the tarball and AppImage users with nothing.

Two things carry the residual risk rather than the threat model. A new binary is smoke-tested by
running it with `--version` before the old one is discarded, with rollback if it does not answer,
which is why that flag is a forward-compatibility contract older clients depend on. And the manifest
carries a schema version, so a client that does not understand a future manifest declines it rather
than guessing.

This ADR was written in September 2026, reconstructed from `.claude/rules/updater.md` and the code.
The updater predates the repository's first commit.
