#!/usr/bin/env python3
"""Build the `latest.json` manifest for the in-app auto-updater.

Reads every signed artifact in `--artifacts <dir>`, inlines the matching
`.minisig` text (full multi-line, `\\n`s preserved through `json.dumps`)
into the `signature` field per `platforms{}` entry, computes file sizes,
and emits `latest.json` to `--out`.

`linux-x86_64-rpm` and `linux-x86_64-deb` entries are surfaced so the
in-app updater can drive `pkexec dnf install` / `pkexec apt install`
on systems where `/usr/bin/melodia` is owned by `rpm` or `dpkg`
respectively. Client-side detection lives in
`src/services/updater/linux_pkg.rs`.

Usage (from the release workflow):
    python3 scripts/build-latest-json.py \\
        --version "$VERSION" \\
        --tag "$TAG" \\
        --artifacts artifacts/ \\
        --out latest.json
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path


# Filename pattern → `platforms{}` key. Matched in order; first match
# wins. The patterns intentionally pin both the format AND the arch so
# a future macOS dmg doesn't accidentally land in the wrong slot.
#
# aarch64 entries come BEFORE x86_64 — `melodia-vX-aarch64.AppImage`
# would also match the x86_64 AppImage pattern's `.AppImage$` suffix
# if it weren't for the explicit `-x86_64` token, but ordering matters
# for the cargo-deb arm64 fragment (`_arm64.deb`) which has no leading
# arch token to disambiguate against a hypothetical fragment overlap.
PLATFORM_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    # aarch64 (ARM64 — Apple Silicon on Linux, Snapdragon X laptops,
    # Raspberry Pi 4/5, AWS Graviton, Ampere Altra).
    (re.compile(r"-aarch64\.AppImage$"), "linux-aarch64-appimage"),
    (re.compile(r"-aarch64-linux\.tar\.gz$"), "linux-aarch64-tarball"),
    (re.compile(r"\.aarch64\.rpm$"), "linux-aarch64-rpm"),
    (re.compile(r"_arm64\.deb$"), "linux-aarch64-deb"),
    (re.compile(r"-aarch64-windows\.zip$"), "windows-aarch64-zip"),
    (re.compile(r"-aarch64\.msi$"), "windows-aarch64-msi"),
    # x86_64.
    (re.compile(r"-x86_64\.AppImage$"), "linux-x86_64-appimage"),
    (re.compile(r"-x86_64-linux\.tar\.gz$"), "linux-x86_64-tarball"),
    # build-rpm.sh emits melodia-<ver>-1.fc<rel>.x86_64.rpm; the suffix
    # is invariant across Fedora releases.
    (re.compile(r"\.x86_64\.rpm$"), "linux-x86_64-rpm"),
    # cargo-deb default: melodia_<ver>_amd64.deb (Debian uses amd64,
    # not x86_64).
    (re.compile(r"_amd64\.deb$"), "linux-x86_64-deb"),
    (re.compile(r"-x86_64-windows\.zip$"), "windows-x86_64-zip"),
    (re.compile(r"-x86_64\.msi$"), "windows-x86_64-msi"),
]


def classify(name: str) -> str | None:
    """Return the `platforms{}` key for an artifact filename, or `None`
    if the filename doesn't match any of the in-app-update formats
    (source tarballs and unknown formats are returned as `None`)."""
    for pattern, key in PLATFORM_PATTERNS:
        if pattern.search(name):
            return key
    return None


# Markdown → plain text. Notes shown in the in-app dialog (`Updater.notes-short`)
# render in Slint Text, which has no Markdown widget. The notes shown on the
# GitHub release page keep the full Markdown; this strips just enough to
# produce a clean plain-text preview.
#
# Prefers `markdown-it-py` (robust AST walk — handles fenced code blocks,
# tables, nested formatting, auto-links) when available; falls back to
# the regex pipeline when it's not (no-pip environments, isolated runs).
# CI installs the dep via `pip install markdown-it-py` before invoking
# this script so production manifests get the high-quality stripper.

try:
    from markdown_it import MarkdownIt as _MarkdownIt
    _HAS_MD_IT = True
except ImportError:
    _HAS_MD_IT = False


_FALLBACK_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"```.*?```", re.DOTALL), ""),       # fenced code block → drop
    (re.compile(r"\*\*(.+?)\*\*"), r"\1"),           # **bold**
    (re.compile(r"__(.+?)__"), r"\1"),               # __bold__
    (re.compile(r"(?<!\*)\*(?!\*)(.+?)\*"), r"\1"),  # *italic*
    (re.compile(r"(?<!_)_(?!_)(.+?)_"), r"\1"),      # _italic_
    (re.compile(r"`([^`]+)`"), r"\1"),               # `code`
    (re.compile(r"\[([^\]]+)\]\([^)]+\)"), r"\1"),   # [text](url) → text
    (re.compile(r"^#{1,6}\s*", re.MULTILINE), ""),   # # heading → heading
    (re.compile(r"^\s*[-*+]\s+", re.MULTILINE), "• "),  # - bullet → • bullet
    (re.compile(r"\r\n"), "\n"),
]


def _strip_via_regex(text: str) -> str:
    out = text
    for pattern, replacement in _FALLBACK_PATTERNS:
        out = pattern.sub(replacement, out)
    return out


def _strip_via_md_it(text: str) -> str:
    """AST walk of the parsed Markdown → plain text. Whitelist the
    inline token types we want to render; skip the rest (HTML, images,
    raw HTML blocks). Block boundaries become single newlines; bullets
    become `• `; headings get a trailing newline so the next paragraph
    isn't glued on."""
    md = _MarkdownIt("commonmark", {"breaks": False, "html": False})
    tokens = md.parse(text)
    out: list[str] = []
    list_stack: list[str] = []  # tracks "bullet" vs "ordered" to format markers
    ordered_counter: list[int] = []

    def walk_inline(children: object) -> str:
        if not children:
            return ""
        buf: list[str] = []
        for tok in children:  # type: ignore[union-attr]
            t = tok.type
            if t in {"text", "code_inline"}:
                buf.append(tok.content)
            elif t == "softbreak":
                buf.append(" ")
            elif t == "hardbreak":
                buf.append("\n")
            elif t == "link_open":
                pass  # render the link's text children, drop the URL
            elif t == "link_close":
                pass
            elif t in {"strong_open", "strong_close", "em_open", "em_close"}:
                pass  # no markers in plain text
            elif t == "image":
                pass  # drop
            # anything else falls through silently
        return "".join(buf)

    for tok in tokens:
        t = tok.type
        if t == "heading_open":
            pass
        elif t == "heading_close":
            out.append("\n\n")
        elif t == "paragraph_open":
            pass
        elif t == "paragraph_close":
            out.append("\n\n")
        elif t == "inline":
            out.append(walk_inline(tok.children))
        elif t == "bullet_list_open":
            list_stack.append("bullet")
        elif t == "ordered_list_open":
            list_stack.append("ordered")
            ordered_counter.append(1)
        elif t in {"bullet_list_close", "ordered_list_close"}:
            if list_stack:
                kind = list_stack.pop()
                if kind == "ordered" and ordered_counter:
                    ordered_counter.pop()
            out.append("\n")
        elif t == "list_item_open":
            if list_stack and list_stack[-1] == "ordered" and ordered_counter:
                out.append(f"{ordered_counter[-1]}. ")
                ordered_counter[-1] += 1
            else:
                out.append("• ")
        elif t == "list_item_close":
            out.append("\n")
        elif t == "fence" or t == "code_block":
            pass  # drop fenced code blocks from the preview
        elif t == "hr":
            out.append("\n---\n")
        elif t == "blockquote_open":
            out.append("> ")
        elif t == "blockquote_close":
            out.append("\n")
    return "".join(out)


def strip_markdown(text: str) -> str:
    if _HAS_MD_IT:
        out = _strip_via_md_it(text)
    else:
        out = _strip_via_regex(text)
    # Collapse runs of >2 newlines down to 2 so paragraph breaks
    # survive but don't accumulate.
    out = re.sub(r"\n{3,}", "\n\n", out)
    return out.strip()


def build_manifest(
    *,
    version: str,
    pub_date: str,
    notes_short: str,
    artifacts_dir: Path,
    manifest_schema_version: int,
    critical: bool,
) -> dict[str, object]:
    platforms: dict[str, dict[str, object]] = {}

    for path in sorted(artifacts_dir.iterdir()):
        if not path.is_file():
            continue
        key = classify(path.name)
        if key is None:
            continue
        sig_path = path.with_suffix(path.suffix + ".minisig")
        if not sig_path.exists():
            print(f"SKIP {path.name}: missing .minisig", file=sys.stderr)
            continue
        signature = sig_path.read_text(encoding="utf-8").rstrip("\n")
        size = path.stat().st_size
        # The release URL pattern matches GitHub's
        # `/releases/download/<tag>/<filename>` layout. We don't write
        # the tag here — the GitHub release UI substitutes it at fetch
        # time via the `/latest/download/` redirect. Embedding the
        # filename keeps the manifest stable across release tag bumps.
        url = (
            "https://github.com/KenanSalar/Melodia/releases/latest/download/"
            f"{path.name}"
        )
        platforms[key] = {
            "url": url,
            "signature": signature,
            "size": size,
        }

    if not platforms:
        raise SystemExit(
            f"no recognised artifacts in {artifacts_dir} — "
            "expected at least one .AppImage / .tar.gz / .zip / .msi"
        )

    return {
        # Forward-compat gate. The Rust client refuses to install when
        # this value exceeds its compiled-in SUPPORTED_MANIFEST_SCHEMA.
        # Bump in lockstep with breaking changes to LatestManifest's
        # shape — adding `#[serde(default)]` fields is back-compat and
        # does NOT require a bump.
        "manifest_schema_version": manifest_schema_version,
        "version": version,
        # When true, the Settings → Updates panel hides the
        # "Skip this version" button. Set per-release at CI time; defaults
        # to false. Use for security-critical fixes only — the toast is
        # still per-session dismissable but re-appears at the next daily
        # check.
        "critical": critical,
        "pub_date": pub_date,
        "notes_short": notes_short,
        "platforms": platforms,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="release version (no 'v' prefix)")
    parser.add_argument("--tag", required=True, help="release tag (e.g. v0.2.0)")
    parser.add_argument(
        "--artifacts",
        required=True,
        type=Path,
        help="directory containing the signed artifact files + their .minisig siblings",
    )
    parser.add_argument(
        "--out",
        required=True,
        type=Path,
        help="output path for the generated latest.json",
    )
    parser.add_argument(
        "--notes-file",
        type=Path,
        help="optional path to release notes (Markdown) — stripped into notes_short",
    )
    parser.add_argument(
        "--manifest-schema-version",
        type=int,
        default=1,
        help=(
            "value to write into manifest_schema_version (default: 1). "
            "Bump only on a breaking change to LatestManifest's shape — "
            "adding optional fields is back-compat and does NOT need a bump."
        ),
    )
    parser.add_argument(
        "--pub-date",
        help=(
            "ISO-8601 publish timestamp to write into pub_date. When "
            "omitted, the current UTC time is used. The publish-time "
            "manifest refresh (refresh-manifest.yml) passes the original "
            "release's pub_date through so a rebuild doesn't bump it."
        ),
    )
    parser.add_argument(
        "--critical",
        action="store_true",
        help=(
            "mark this release as critical — the in-app updater hides the "
            "'Skip this version' button so users can't permanently defer it. "
            "Use for security-critical fixes only."
        ),
    )
    args = parser.parse_args()

    if not args.artifacts.is_dir():
        raise SystemExit(f"artifacts dir {args.artifacts} does not exist")

    notes_short = ""
    if args.notes_file is not None and args.notes_file.is_file():
        notes_md = args.notes_file.read_text(encoding="utf-8")
        notes_short = strip_markdown(notes_md)

    # `datetime.now(timezone.utc)` works on Python 3.2+; the previous
    # `datetime.now(_dt.UTC)` form required 3.11+ (the `UTC` alias was
    # added in 3.11). Keeping compatibility back to 3.8 avoids brittle
    # coupling to `ubuntu-latest`'s current Python (3.12) — a future
    # runner image bump shouldn't break the manifest build.
    #
    # `--pub-date` overrides it so a publish-time rebuild of an already
    # released manifest keeps the original timestamp instead of bumping
    # it to the rebuild time.
    if args.pub_date:
        pub_date = args.pub_date
    else:
        pub_date = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")

    manifest = build_manifest(
        version=args.version,
        pub_date=pub_date,
        notes_short=notes_short,
        artifacts_dir=args.artifacts,
        manifest_schema_version=args.manifest_schema_version,
        critical=args.critical,
    )

    args.out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.out} ({len(manifest['platforms'])} platform entries)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
