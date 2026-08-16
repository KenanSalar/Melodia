#!/usr/bin/env python3
"""Retire superseded build artifacts from `target/` without forcing a full rebuild.

Cargo garbage-collects its *global* cache (`~/.cargo`) but never `target/`. Every
distinct unit hash — a feature-set, profile, RUSTFLAGS or lockfile change, or
simply `clippy` vs `build` vs `test` vs `llvm-cov` — mints a fresh generation and
orphans the previous one forever. On a workspace this size each generation is
gigabytes, so a few months of churn buries the disk.

Three scopes, differing in how they decide a generation is dead:

  incremental  rustc's incremental caches, retired by age. Safe by construction:
               a wrong guess costs one non-incremental compile of that crate and
               never touches a dependency. The default, and the biggest pool.
  binaries     superseded artifacts in `deps/`.
  build        superseded build-script directories and their output.

The last two are decided by asking cargo which artifacts the current resolve
actually uses (`--live-from`), never by age. Age looks like it should work and
does not: cargo touches nothing when a unit is already fresh, so a dependency
compiled months ago and still linked today sorts *below* an artifact from a
feature-set experiment that has since been abandoned. Retiring a live rlib
rebuilds that crate and everything downstream of it.

Deleting is opt-in: without `--apply` this only reports.

Usage:
    python3 scripts/prune-target.py                      # report, incremental only
    python3 scripts/prune-target.py --scope all --apply \\
        --live-from 'build,clippy --all-targets,test --no-run'
    python3 scripts/prune-target.py --help               # shell-wrapper snippets
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Cargo names artifacts `<stem>-<hash><ext>`, where the hash identifies one
# compilation. Files sharing a hash (.rlib + .rmeta + .d) are one unit and have to
# retire together, or cargo finds a fingerprint whose artifact is half gone.
# Greedy stem so a crate name containing a hash-shaped segment still splits at the
# last valid position.
DEPS_UNIT = re.compile(r"^(?P<stem>.+)-(?P<hash>[0-9a-f]{16})(?P<ext>\..*)?$")
# Incremental directories carry a base-36 hash and never an extension.
INCREMENTAL_UNIT = re.compile(r"^(?P<stem>.+)-(?P<hash>[0-9a-z]{10,})$")

STAMP_NAME = ".prune-stamp"

# The commands whose artifacts are worth keeping warm here: a plain build, the
# clippy gate over every target, and the test harnesses.
LIVE_FROM_SUGGESTION = "build,clippy --all-targets,test --no-run"


@dataclass
class Unit:
    """One compilation's artifacts, keyed by (stem, hash)."""

    stem: str
    kind: str
    digest: str = ""
    paths: list[Path] = field(default_factory=list)
    mtime: float = 0.0
    # Sizes keyed by (device, inode). rustc hardlinks freely between generations,
    # so summing bytes per path reports space that deleting would not actually
    # return — only an inode nothing else references is truly reclaimed.
    inodes: dict[tuple[int, int], int] = field(default_factory=dict)
    live: bool = False

    @property
    def size(self) -> int:
        return sum(self.inodes.values())


def human(size: float) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.1f} {unit}" if unit != "B" else f"{int(size)} B"
        size /= 1024
    return f"{size:.1f} GB"


def reclaimable(doomed: list[Unit], survivors: list[Unit]) -> int:
    """Bytes actually freed: inodes the doomed units hold and nobody else does."""
    kept = {ino for unit in survivors for ino in unit.inodes}
    freed = {
        ino: size
        for unit in doomed
        for ino, size in unit.inodes.items()
        if ino not in kept
    }
    return sum(freed.values())


def tree_inodes(path: Path) -> dict[tuple[int, int], int]:
    found: dict[tuple[int, int], int] = {}
    for dirpath, _, filenames in os.walk(path):
        for name in filenames:
            try:
                st = os.lstat(os.path.join(dirpath, name))
            except OSError:
                continue
            found[(st.st_dev, st.st_ino)] = st.st_size
    return found


def collect_deps(deps_dir: Path) -> list[Unit]:
    """One Unit per unit hash.

    The hash — not the filename stem — is what identifies a compilation: a single
    unit emits `libfoo-<hash>.rlib` alongside `foo-<hash>.d`, two different stems.
    They have to retire together, or cargo finds a fingerprint whose artifact is
    only half there.
    """
    if not deps_dir.is_dir():
        return []

    units: dict[str, Unit] = {}
    for entry in os.scandir(deps_dir):
        match = DEPS_UNIT.match(entry.name)
        if not match:
            continue
        try:
            st = entry.stat(follow_symlinks=False)
        except OSError:
            continue
        digest = match["hash"]
        unit = units.setdefault(
            digest, Unit(stem=match["stem"].removeprefix("lib"), kind="deps", digest=digest)
        )
        unit.paths.append(Path(entry.path))
        unit.mtime = max(unit.mtime, st.st_mtime)
        if entry.is_file(follow_symlinks=False):
            unit.inodes[(st.st_dev, st.st_ino)] = st.st_size
        else:
            unit.inodes |= tree_inodes(Path(entry.path))
        # Uplifted to `target/<profile>/`, so a second link means cargo hands this
        # one out. Only covers final artifacts — dependency rlibs are never
        # uplifted — so it is a backstop, not the liveness test.
        unit.live = unit.live or getattr(st, "st_nlink", 1) > 1

    return list(units.values())


def collect_incremental(inc_dir: Path) -> list[Unit]:
    units: list[Unit] = []
    if not inc_dir.is_dir():
        return units
    for entry in os.scandir(inc_dir):
        if not entry.is_dir(follow_symlinks=False):
            continue
        match = INCREMENTAL_UNIT.match(entry.name)
        if not match:
            continue
        path = Path(entry.path)
        units.append(
            Unit(
                stem=match["stem"],
                kind="incremental",
                paths=[path],
                mtime=entry.stat().st_mtime,
                inodes=tree_inodes(path),
            )
        )
    return units


def live_hashes(project: Path, commands: list[str], quiet: bool) -> set[str]:
    """Unit hashes the current resolve actually uses, straight from cargo.

    File mtimes cannot answer this: cargo touches nothing when a unit is already
    fresh, so a dependency compiled months ago and still linked today looks older
    than an artifact from a feature-set experiment that has since been abandoned.
    Age therefore ranks the live copy *below* the dead one. `--message-format=json`
    makes cargo name the artifacts itself, and it is a no-op when nothing changed.

    Only what these commands build counts as live — a command left out of the list
    rebuilds the next time it runs.

    Interactively cargo keeps its stderr: a command whose units aren't fresh
    compiles them first, and swallowing the progress makes that look like a hang.
    Under --quiet it goes to the void instead — that mode runs backgrounded from a
    shell wrapper, where stray output lands on top of the next prompt.
    """
    found: set[str] = set()
    for command in commands:
        if not quiet:
            print(f"  querying live artifacts: cargo {command}", file=sys.stderr)
        result = subprocess.run(
            ["cargo", *command.split(), "--message-format=json"],
            cwd=project,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL if quiet else None,
            text=True,
        )
        if result.returncode != 0:
            raise SystemExit(f"`cargo {command}` failed, refusing to guess")
        for line in result.stdout.splitlines():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            reason = message.get("reason")
            if reason == "compiler-artifact":
                reported = message.get("filenames") or []
            elif reason == "build-script-executed":
                # The compiled script and the output it produced live under
                # separate hashes; only this message names the second.
                reported = [message.get("out_dir") or ""]
            else:
                continue
            for name in reported:
                # `deps/libfoo-<hash>.rlib` carries it in the basename,
                # `build/foo-<hash>/out` in a parent, so check every component.
                # A stray match only spares something, which is the safe direction.
                found.update(m["hash"] for p in Path(name).parts if (m := DEPS_UNIT.match(p)))
    return found


def collect_build(build_dir: Path) -> list[Unit]:
    """Build-script directories, split by whether they hold a script or its output.

    A missing output re-runs the script, which for `melodia-ui` means regenerating
    the whole Slint tree — so this is decided by liveness, never by age.
    """
    units: list[Unit] = []
    if not build_dir.is_dir():
        return units
    for entry in os.scandir(build_dir):
        match = DEPS_UNIT.match(entry.name)
        if not match or not entry.is_dir(follow_symlinks=False):
            continue
        path = Path(entry.path)
        units.append(
            Unit(
                stem=match["stem"],
                kind="build",
                digest=match["hash"],
                paths=[path],
                mtime=entry.stat().st_mtime,
                inodes=tree_inodes(path),
            )
        )
    return units


def select_by_age(units: list[Unit], keep: int) -> list[Unit]:
    """Everything but the `keep` newest generations of each ladder.

    Only sound for incremental caches, where a wrong guess costs one
    non-incremental compile of that crate and never touches a dependency. A ladder
    is one crate's history under one target name — lib, bin and each test harness
    carry distinct stems, so they don't displace each other.
    """
    ladders: dict[tuple[str, str], list[Unit]] = {}
    for unit in units:
        ladders.setdefault((unit.kind, unit.stem), []).append(unit)

    stale: list[Unit] = []
    for generations in ladders.values():
        generations.sort(key=lambda u: u.mtime, reverse=True)
        stale.extend(generations[keep:])
    return stale


def select_by_liveness(units: list[Unit], live: set[str]) -> list[Unit]:
    """Every unit the current resolve doesn't name. Age plays no part.

    Deleting a `deps/` or `build/` artifact cargo still wants rebuilds that crate
    *and everything downstream of it*, so this needs cargo's answer rather than a
    heuristic — see `live_hashes`. The hardlink guard is belt and braces for a
    query that under-reports because a command was left out of `--live-from`.
    """
    return [unit for unit in units if unit.digest not in live and not unit.live]


def acquire_lock(profile_dir: Path):
    """Hold cargo's target lock for the run, or return None if a build owns it.

    rust-analyzer runs `cargo check` continuously in the background, so "my shell
    is idle" is no evidence that nothing is building. Holding the lock rather than
    probing it means a build started mid-prune waits instead of racing us.
    """
    lock_path = profile_dir / ".cargo-lock"
    if not lock_path.exists():
        return None
    try:
        import fcntl
    except ImportError:
        return None  # Windows: no portable advisory lock; --keep still protects.
    handle = open(lock_path, "a+b")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        raise SystemExit("a cargo build holds the target lock — nothing done")
    return handle


def throttled(target_dir: Path, hours: float) -> bool:
    """True if the last run was recent enough to skip. Mirrors cargo's own
    `cache.auto-clean-frequency`, so a per-build trigger costs process startup."""
    if hours <= 0:
        return False
    stamp = target_dir / STAMP_NAME
    try:
        return (time.time() - stamp.stat().st_mtime) < hours * 3600
    except OSError:
        return False


def remove(unit: Unit) -> None:
    for path in unit.paths:
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path, ignore_errors=True)
        else:
            try:
                path.unlink()
            except OSError:
                pass


EPILOG = """\
triggering it automatically

  The script is deliberately manual; wire your own trigger so nothing is imposed
  on contributors or CI. Cargo cannot hook `build` or `clippy` — aliases may not
  shadow built-ins — so this has to live in your shell.

  A shell wrapper is the right home rather than a scheduler: `target/` only grows
  when you build, and right after a build is also the one moment the tree is
  fresh, so the --live-from query costs nothing. A timer firing mid-build just
  hits the lock and skips.

  Each wrapper tests for the script before firing, so it stays inert in every
  other cargo project you build in.

  zsh / bash  (~/.zshrc, ~/.bashrc):
      cargo() {
        command cargo "$@"; local rc=$?
        case "$1" in build|b|run|r|clippy|test|t)
          [ -f %(script)s ] && (python3 %(script)s \\
            --scope all --throttle 12 --apply --quiet --live-from %(live)s &) ;;
        esac
        return $rc
      }

  fish  (~/.config/fish/functions/cargo.fish):
      function cargo
          command cargo $argv; set -l rc $status
          if contains -- $argv[1] build b run r clippy test t
              and test -f %(script)s
              python3 %(script)s \\
                  --scope all --throttle 12 --apply --quiet --live-from %(live)s &
              disown
          end
          return $rc
      end

  PowerShell  ($PROFILE):
      function cargo {
        & (Get-Command cargo.exe) @args; $rc = $LASTEXITCODE
        if ($args[0] -in 'build','b','run','r','clippy','test','t' -and
            (Test-Path '%(script)s')) {
          Start-Process -NoNewWindow python `
            -ArgumentList '%(script)s','--scope','all','--throttle','12','--apply','--quiet',`
             '--live-from','%(livebare)s'
        }
        return $rc
      }
""" % {
    "script": "scripts/prune-target.py",
    "live": f"'{LIVE_FROM_SUGGESTION}'",
    "livebare": LIVE_FROM_SUGGESTION,
}


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--scope",
        choices=("incremental", "binaries", "build", "all"),
        default="incremental",
        help="what to consider; 'incremental' (default) never rebuilds a dependency",
    )
    parser.add_argument(
        "--keep",
        type=int,
        default=2,
        metavar="N",
        help="generations to retain per unit (default: 2)",
    )
    parser.add_argument(
        "--profile",
        default="debug",
        help="target subdirectory to prune (default: debug)",
    )
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=ROOT / "target",
        help="cargo target directory (default: <repo>/target)",
    )
    parser.add_argument(
        "--throttle",
        type=float,
        default=0.0,
        metavar="HOURS",
        help="skip if a run finished less than HOURS ago; for per-build triggers",
    )
    parser.add_argument(
        "--project",
        type=Path,
        default=ROOT,
        help="directory to run the --live-from commands in (default: <repo>)",
    )
    parser.add_argument(
        "--live-from",
        metavar="CMDS",
        help="comma-separated cargo commands defining what counts as live, e.g. "
        f"{LIVE_FROM_SUGGESTION!r}. Required by --scope binaries/build/all",
    )
    parser.add_argument("--apply", action="store_true", help="delete (default: report only)")
    parser.add_argument("--quiet", action="store_true", help="only report what was freed")
    args = parser.parse_args()

    if os.environ.get("CI"):
        return 0

    if args.keep < 1:
        parser.error("--keep must be at least 1; 0 would retire the live generation")

    needs_liveness = args.scope in ("binaries", "build", "all")
    if needs_liveness and not args.live_from:
        parser.error(
            f"--scope {args.scope} needs --live-from. Deleting a deps/ or build/ artifact\n"
            "cargo still wants rebuilds that crate and everything downstream, and file age\n"
            "cannot tell the two apart — cargo touches nothing when a unit is already fresh.\n"
            f"Run the commands you care about staying fast, e.g.\n"
            f"    --live-from '{LIVE_FROM_SUGGESTION}'"
        )

    target_dir: Path = args.target_dir
    profile_dir = target_dir / args.profile
    if not profile_dir.is_dir():
        print(f"nothing to do: {profile_dir} does not exist", file=sys.stderr)
        return 0

    if throttled(target_dir, args.throttle):
        return 0

    # Ahead of the lock: the query runs cargo, which wants that same lock.
    live = (
        live_hashes(args.project, args.live_from.split(","), args.quiet)
        if needs_liveness
        else set()
    )

    lock = acquire_lock(profile_dir)
    try:
        stale: list[Unit] = []
        units: list[Unit] = []
        if args.scope in ("incremental", "all"):
            caches = collect_incremental(profile_dir / "incremental")
            units += caches
            stale += select_by_age(caches, args.keep)
        if args.scope in ("binaries", "all"):
            artifacts = collect_deps(profile_dir / "deps")
            units += artifacts
            stale += select_by_liveness(artifacts, live)
        if args.scope in ("build", "all"):
            scripts = collect_build(profile_dir / "build")
            units += scripts
            stale += select_by_liveness(scripts, live)

        doomed = set(map(id, stale))
        reclaimed = reclaimable(stale, [u for u in units if id(u) not in doomed])

        if not args.quiet:
            for unit in sorted(stale, key=lambda u: u.size, reverse=True)[:10]:
                print(f"  {human(unit.size):>10}  {unit.paths[0].name}")
            if len(stale) > 10:
                print(f"  … and {len(stale) - 10} more")

        if args.apply:
            for unit in stale:
                remove(unit)
            (target_dir / STAMP_NAME).touch()
            print(f"freed {human(reclaimed)} across {len(stale)} generations")
        else:
            print(
                f"would free {human(reclaimed)} across {len(stale)} generations "
                f"(re-run with --apply)"
            )
    finally:
        if lock is not None:
            lock.close()

    return 0


if __name__ == "__main__":
    sys.exit(main())
