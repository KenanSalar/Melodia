#!/usr/bin/env python3
r"""Measures Melodia's memory, CPU and GPU footprint on Linux or Windows the same way every run.

Launches Melodia and samples its startup straight away, or attaches to a running one. Once it has
settled, it samples one window per scenario while you switch between scenarios by hand. Every
sample goes to samples.csv and the per-scenario figures to summary.md. Standard library only:

    python3 scripts/measure-footprint.py
    py scripts\measure-footprint.py --data-dir D:\melodia-bench --scenario Idle

Each OS reports its own counters, because the two count memory differently.

Linux

    Anonymous is the headline figure: anonymous memory in RAM plus what of it sits in swap, zram
    included. Linux overcommits and keeps no commit charge per process, so this is the nearest it
    has. It doesn't fall when memory pressure pushes Melodia into swap, and nothing another process
    does moves it, so it tracks what Melodia allocated. Chromium reports the same sum as its
    private footprint on Linux.

    PSS is KDE System Monitor's Memory column. It is exact on a kernel that keeps per-page
    mapcounts, and the report says whether this one does, but it splits every shared page between
    the processes mapping it: start another OpenGL application and Melodia's PSS drops while
    Melodia does nothing. It is recorded because it is the number people check a README against,
    not because two runs can be compared on it.

    USS is the pages no other process maps, what exiting would give back, and System Monitor's
    Private column. RSS counts shared pages in full. All four come from smaps_rollup, which walks
    the page tables, rather than status, whose counters the kernel updates asynchronously.

    GPU memory is what the driver charges the process: framebuffer memory from nvidia-smi pmon on
    NVIDIA, whose DRM file descriptors publish no usage, and the drm-resident-* keys in fdinfo on
    drivers that implement DRM usage stats. Those are the sources System Monitor's GPU columns
    read. smaps sees none of it, so report it beside the memory columns, never summed into them.

Windows

    Commit is the headline figure: the private memory Windows has promised the process, whether it
    sits in RAM or in the page file. It doesn't fall when the memory manager trims the working set
    under pressure, so it tracks what Melodia allocated rather than what Windows left resident.
    Task Manager calls it "Commit size", and it is the field Chromium reports as its private
    footprint on Windows.

    Private working set is Task Manager's default "Memory" column. It is recorded because it is the
    number people check a README against, but it only says what was in RAM at that moment.

    There is no PSS on Windows. The kernel keeps no proportional count, and one synthesized from
    QueryWorkingSetEx divides each shared page by ShareCount, a 3-bit field that saturates at 7. A
    system DLL mapped by two hundred processes is charged as if seven shared it, so the figure
    moves with whatever else happens to be running.

    GPU memory comes from the "GPU Process Memory" counters Task Manager reads, because FemtoVG's
    textures and framebuffers are driver allocations commit and working set don't describe.
    Dedicated is VRAM, shared is system RAM mapped for the GPU. Report them beside commit, never
    summed into it.

Both

    CPU is process CPU time over wall time, one full core being 100%. System Monitor's CPU column
    and Task Manager's Details tab divide by the logical processor count, which is the "all cores"
    column. GPU is the busiest engine's running time over wall time. NVIDIA on Linux publishes only
    a per-process utilization in whole percents each second, so there it is the average of those.

    Threads, file descriptors and memory maps (handles, GDI and USER objects on Windows) should
    hold flat from one prompted scenario to the next, Startup's sitting higher while its database
    connections are open. A climb is a leak.
"""

from __future__ import annotations

import argparse
import csv
import ctypes
import functools
import gzip
import json
import math
import os
import platform
import random
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from typing import Any

WINDOWS = sys.platform == "win32"
LINUX = sys.platform.startswith("linux")

if WINDOWS:
    import msvcrt
    import winreg
else:
    import termios

REPO_ROOT = Path(__file__).resolve().parent.parent

STARTUP_SCENARIO = "Startup"
# What each default scenario's prompt asks for, in the order they run. A --scenario of your own gets
# the generic prompt. Plain ASCII, since a redirected Windows console encodes for its code page.
SCENARIO_SETUP = {
    "Idle": "Leave a playlist's detail view showing, its hero banner in view, with nothing playing.",
    "Playing, list view": "Stay on the playlist's detail view and play the playlist from its first track.",
    "Playing, visualizer live": (
        "Keep the playlist playing and open Now Playing with the visualizer showing, in the "
        "Mirrored style."
    ),
    # The library-sized scenarios run last: the Songs tab keeps its list after you leave it and a
    # queued library stays queued, both of which any scenario after them would carry.
    "Playing, Songs tab open": (
        "Keep the playlist playing, without stopping it or starting anything else, close Now "
        "Playing and switch to My Library > Songs."
    ),
    "Playing, whole library queued": (
        "Stay on the Songs tab and double-click its first song, which queues the whole library and "
        "plays from there."
    ),
    "Playing, queue sheet open": "Keep it playing and open the queue sheet with Q.",
}
DEFAULT_SCENARIOS = list(SCENARIO_SETUP)
# Both past the database pool's reap, the first counted from the launch and the second from each
# prompt, so neither startup's reads nor a switch's reach a window. Every connection maps
# melodia.db, and the reaper, looking once a minute for ones idle over a minute, closes one up to
# two minutes after its last query (IDLE_TIMEOUT in crates/melodia-store/src/database/mod.rs).
DEFAULT_SETTLE_SECONDS = "150"
DEFAULT_SWITCH_SETTLE_SECONDS = DEFAULT_SETTLE_SECONDS
DEFAULT_DURATION_SECONDS = "60"
DEFAULT_INTERVAL_SECONDS = "1"
MAX_WAIT_SECONDS = 3600
MIN_DURATION_SECONDS = 1
MIN_INTERVAL_SECONDS = 0.1
MAX_INTERVAL_SECONDS = 60

MICROSECONDS_PER_SECOND = 1_000_000
BYTES_PER_MIB = 1024 * 1024
LOGICAL_PROCESSORS = os.cpu_count() or 1
PROGRESS_STEP_SECONDS = 0.25
PROGRESS_BAR_WIDTH = 24
FALLBACK_TERMINAL_COLUMNS = 80
VERSION_TIMEOUT_SECONDS = 5

COLOR_TITLE = "\033[36m"
COLOR_RESULT = "\033[32m"
COLOR_RESET = "\033[0m"


class Failure(Exception):
    """Ends the run with one line rather than a traceback."""


@dataclass
class Settings:
    exe: Path
    data_dir: str | None
    attach_pid: int | None
    scenarios: list[str]
    settle_seconds: float
    switch_settle_seconds: float
    duration_seconds: float
    interval_seconds: float
    out_dir: Path


EXAMPLES = r"""
examples:
  python3 scripts/measure-footprint.py
  python3 scripts/measure-footprint.py --data-dir ~/melodia-bench --scenario Idle --scenario 'Playing, list view'
  py scripts\measure-footprint.py --pid 1234 --scenario Idle
"""


def parse_settings(argv: list[str] | None) -> Settings:
    parser = argparse.ArgumentParser(
        prog="scripts/measure-footprint.py",
        description="Measures Melodia's memory, CPU and GPU footprint on Linux or Windows. The "
        "docstring at the top of the script explains every column.",
        epilog=EXAMPLES,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--exe",
        metavar="PATH",
        help="Binary to launch. Defaults to the release build: cargo build --release -p melodia",
    )
    parser.add_argument(
        "--data-dir",
        metavar="DIR",
        help="Data root to launch against, passed as MELODIA_DATA_DIR, so every run sees the "
        "same library.",
    )
    parser.add_argument(
        "--pid",
        help="Measure an already running Melodia instead of launching one. Its startup is past, "
        f'so there is no "{STARTUP_SCENARIO}" window.',
    )
    parser.add_argument(
        "--scenario",
        action="append",
        metavar="NAME",
        help="One measurement window per name, in order; repeat it for more. You are prompted "
        f"before each. Defaults to {', '.join(repr(name) for name in DEFAULT_SCENARIOS)}. A launch "
        f'samples "{STARTUP_SCENARIO}" before them, unprompted.',
    )
    parser.add_argument(
        "--settle-seconds",
        default=DEFAULT_SETTLE_SECONDS,
        metavar="N",
        help="Time from the launch, or the attach, to the first prompt, so startup work and the "
        "database connections it opens stay out of every prompted window. Default %(default)s.",
    )
    parser.add_argument(
        "--switch-settle-seconds",
        default=DEFAULT_SWITCH_SETTLE_SECONDS,
        metavar="N",
        help="Wait after each prompt, so the switch itself (a view being built, covers decoding, "
        "the database connections it opens) stays out of the window. Default %(default)s.",
    )
    parser.add_argument(
        "--duration-seconds",
        default=DEFAULT_DURATION_SECONDS,
        metavar="N",
        help="Length of each scenario's window. Default %(default)s.",
    )
    parser.add_argument(
        "--interval-seconds",
        default=DEFAULT_INTERVAL_SECONDS,
        metavar="N",
        help="Time between memory samples. Default %(default)s.",
    )
    parser.add_argument(
        "--out-dir",
        metavar="DIR",
        help="Where samples.csv and summary.md go. Defaults to a timestamped folder under "
        "target/footprint.",
    )
    args = parser.parse_args(argv)

    attach_pid = None
    if args.pid is not None:
        if args.exe is not None or args.data_dir is not None:
            raise Failure("--pid measures a running Melodia, so it takes neither --exe nor --data-dir.")
        if not re.fullmatch(r"[1-9][0-9]*", args.pid):
            raise Failure(f"--pid must be a process id, not {args.pid}.")
        attach_pid = int(args.pid)

    settle = seconds_option("--settle-seconds", args.settle_seconds, 0, MAX_WAIT_SECONDS)
    switch_settle = seconds_option(
        "--switch-settle-seconds", args.switch_settle_seconds, 0, MAX_WAIT_SECONDS
    )
    duration = seconds_option(
        "--duration-seconds", args.duration_seconds, MIN_DURATION_SECONDS, MAX_WAIT_SECONDS
    )
    interval = seconds_option(
        "--interval-seconds", args.interval_seconds, MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS
    )
    if to_microseconds(interval) > to_microseconds(duration):
        raise Failure("--interval-seconds must not exceed --duration-seconds.")

    binary = "Melodia.exe" if WINDOWS else "Melodia"
    exe = Path(args.exe) if args.exe else REPO_ROOT / "target" / "release" / binary
    out_dir = (
        Path(args.out_dir)
        if args.out_dir
        else REPO_ROOT / "target" / "footprint" / datetime.now().strftime("%Y%m%d-%H%M%S")
    )
    return Settings(
        exe=exe,
        data_dir=args.data_dir,
        attach_pid=attach_pid,
        scenarios=args.scenario or list(DEFAULT_SCENARIOS),
        settle_seconds=settle,
        switch_settle_seconds=switch_settle,
        duration_seconds=duration,
        interval_seconds=interval,
        out_dir=out_dir,
    )


def seconds_option(option: str, value: str, low: float, high: float) -> float:
    if not re.fullmatch(r"[0-9]+(\.[0-9]+)?", value):
        raise Failure(f"{option} must be a number of seconds, not {value}.")
    if not to_microseconds(low) <= to_microseconds(value) <= to_microseconds(high):
        raise Failure(f"{option} must be between {low:g} and {high:g}.")
    return float(value)


def to_microseconds(seconds: float | str) -> int:
    return round(float(seconds) * MICROSECONDS_PER_SECOND)


class Terminal:
    """The console a run talks through: titles and results on stdout, the countdown on stderr."""

    def __init__(self) -> None:
        self.progress_enabled = sys.stderr.isatty()
        self.progress_visible = False
        self.columns = terminal_columns()
        self.colors = sys.stdout.isatty() and (not WINDOWS or enable_windows_vt())
        self.saved_settings: list[Any] | None = None

    def stop_echoing_keys(self) -> None:
        # An Enter echoed during a countdown moves the progress bar down a line and leaves its last
        # frame behind. A Windows console echoes a key only when a read consumes it, and the
        # typeahead is discarded unread.
        if WINDOWS or not sys.stdin.isatty():
            return
        descriptor = sys.stdin.fileno()
        self.saved_settings = termios.tcgetattr(descriptor)
        quiet = termios.tcgetattr(descriptor)
        quiet[3] &= ~termios.ECHO
        termios.tcsetattr(descriptor, termios.TCSADRAIN, quiet)

    def restore(self) -> None:
        self.clear_progress()
        if self.saved_settings is not None:
            termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, self.saved_settings)
            self.saved_settings = None

    def title(self, text: str) -> None:
        self.clear_progress()
        print(f"\n{self.paint(COLOR_TITLE, text)}", flush=True)

    def result(self, text: str) -> None:
        print(self.paint(COLOR_RESULT, text), flush=True)

    def paint(self, color: str, text: str) -> str:
        return f"{color}{text}{COLOR_RESET}" if self.colors else text

    def progress(self, activity: str, elapsed: float, total: float) -> None:
        if not self.progress_enabled:
            return
        elapsed = min(elapsed, total)
        filled = int(elapsed * PROGRESS_BAR_WIDTH / total)
        bar = "#" * filled + "-" * (PROGRESS_BAR_WIDTH - filled)
        line = f"{activity} [{bar}] {math.ceil(total - elapsed)} s left"
        width = self.columns - 1
        sys.stderr.write("\r" + line[:width].ljust(width))
        sys.stderr.flush()
        self.progress_visible = True

    def clear_progress(self) -> None:
        if self.progress_visible:
            sys.stderr.write("\r" + " " * (self.columns - 1) + "\r")
            sys.stderr.flush()
            self.progress_visible = False

    def confirm(self, title: str, setup: str | None) -> None:
        self.title(title)
        if setup:
            print(f"  {setup}")
            print("  Keep Melodia's window visible and unminimized, then press Enter.", flush=True)
        else:
            print("  Set Melodia up for it, keep its window visible and unminimized, then press Enter.", flush=True)
        discard_typeahead()
        # A closed stdin reads end-of-input at once, which would sample a scenario nobody set up.
        if not sys.stdin.readline():
            raise Failure(
                "No input to confirm the scenario with. Run the script from an interactive terminal."
            )


def terminal_columns() -> int:
    try:
        return os.get_terminal_size(sys.stderr.fileno()).columns or FALLBACK_TERMINAL_COLUMNS
    except OSError:
        return FALLBACK_TERMINAL_COLUMNS


def discard_typeahead() -> None:
    # An Enter pressed before the prompt would confirm a scenario nobody has set up yet. A terminal
    # only: piped input is the confirmations themselves.
    if not sys.stdin.isatty():
        return
    if WINDOWS:
        while msvcrt.kbhit():
            msvcrt.getwch()
    else:
        termios.tcflush(sys.stdin.fileno(), termios.TCIFLUSH)


@dataclass
class Target:
    """The Melodia being measured, and when its clock started."""

    pid: int
    started: float
    origin: str
    popen: subprocess.Popen[bytes] | None = None

    @property
    def launched(self) -> bool:
        return self.popen is not None


def launch(settings: Settings) -> Target:
    exe = settings.exe
    if not exe.is_file() or not os.access(exe, os.X_OK):
        raise Failure(f"No Melodia binary at {exe}. Build one with: cargo build --release -p melodia")

    # A session of its own, so Ctrl-C here or closing the terminal leaves Melodia running. On
    # Windows that means no console at all, since a console build would otherwise share this one.
    # Its log is mirrored to stderr, which would scroll the progress bar away.
    detached: dict[str, Any] = (
        {"creationflags": subprocess.DETACHED_PROCESS} if WINDOWS else {"start_new_session": True}
    )
    popen = subprocess.Popen(
        [str(exe)],
        env=melodia_environment(settings),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        **detached,
    )
    origin = (
        f"launched with MELODIA_DATA_DIR={settings.data_dir}"
        if settings.data_dir
        else "launched against its default data root"
    )
    return Target(popen.pid, time.monotonic(), origin, popen)


def melodia_environment(settings: Settings) -> dict[str, str]:
    environment = dict(os.environ)
    if settings.data_dir:
        environment["MELODIA_DATA_DIR"] = str(Path(settings.data_dir).resolve())
    return environment


def logs_dir(settings: Settings) -> str | None:
    """Where the launched Melodia logs, as its --logs answers.

    Asked here rather than left to the user, since a Windows release build prints nothing to a
    console.
    """
    answer = run_quietly([str(settings.exe), "--logs"], env=melodia_environment(settings))
    lines = (answer or "").splitlines()
    return lines[0] if lines else None


def attach(pid: int) -> Target:
    return Target(pid, time.monotonic(), f"attached to pid {pid}")


class Countdown:
    def __init__(self, activity: str, seconds: float, started: float | None = None) -> None:
        self.activity = activity
        self.seconds = seconds
        self.started = time.monotonic() if started is None else started

    def elapsed(self) -> float:
        return time.monotonic() - self.started


@dataclass
class ScenarioResult:
    name: str
    samples: list[dict[str, Any]]
    summary: dict[str, Any]


class Run:
    """One Melodia measured from launch or attach to the report."""

    def __init__(
        self, settings: Settings, terminal: Terminal, target: Target, backend: LinuxBackend | WindowsBackend
    ) -> None:
        self.settings = settings
        self.terminal = terminal
        self.target = target
        self.backend = backend
        self.names: list[str] = []
        self.results: list[ScenarioResult] = []

    def measure(self) -> None:
        target = self.target
        print(f"Measuring Melodia (pid {target.pid}), {target.origin}.", flush=True)
        self.names = list(self.settings.scenarios)
        first_prompted = 0
        if target.launched:
            self.names.insert(0, STARTUP_SCENARIO)
            title = self.scenario_title(0)
            self.terminal.title(title)
            print("  Sampled from the launch. Leave Melodia alone until the next prompt.", flush=True)
            self.measure_scenario(0, title)
            first_prompted = 1

        # From the launch or the attach, so the startup window counts toward the settle.
        self.wait(Countdown("Waiting for Melodia to settle", self.settings.settle_seconds, target.started))

        for index in range(first_prompted, len(self.names)):
            title = self.scenario_title(index)
            self.terminal.confirm(title, SCENARIO_SETUP.get(self.names[index]))
            self.wait(Countdown(f"{title}, settling after the switch", self.settings.switch_settle_seconds))
            self.measure_scenario(index, title)

        self.save_report()
        print(f"Melodia (pid {target.pid}) is still running. Close it before the next run.")

    def scenario_title(self, index: int) -> str:
        return f"Scenario {index + 1}/{len(self.names)}: {self.names[index]}"

    def wait(self, countdown: Countdown) -> None:
        self.wait_until(countdown, countdown.seconds)
        self.terminal.clear_progress()

    def wait_until(self, countdown: Countdown, target: float) -> None:
        # Sleeps in short slices so the bar keeps moving and an exited Melodia is noticed promptly.
        while True:
            self.assert_running()
            remaining = target - countdown.elapsed()
            if remaining <= 0:
                return
            self.terminal.progress(countdown.activity, countdown.elapsed(), countdown.seconds)
            time.sleep(min(remaining, PROGRESS_STEP_SECONDS))

    def assert_running(self) -> None:
        popen = self.target.popen
        if popen is None:
            if not self.backend.running():
                raise Failure(f"Melodia (pid {self.target.pid}) exited.")
            return
        # poll() also reaps it: a launched Melodia that exits stays a zombie until then.
        code = popen.poll()
        if code is None:
            return
        # A forwarded launch exits cleanly, so a crash or a kill must not be blamed on one.
        if code == 0:
            raise Failure(
                "Melodia exited with code 0. Straight after a launch that means another instance "
                "holds this data root and the launch was forwarded to it: close that one, or "
                "measure it with --pid."
            )
        logs = logs_dir(self.settings)
        hint = f" Its log is in {logs}." if logs else ""
        raise Failure(f"Melodia exited with code {exit_status(code)}.{hint}")

    def measure_scenario(self, index: int, title: str) -> None:
        interval = to_microseconds(self.settings.interval_seconds)
        duration = to_microseconds(self.settings.duration_seconds)
        self.backend.begin_window()
        countdown = Countdown(f"{title}, measuring", self.settings.duration_seconds)

        samples = []
        tick = 0
        while tick * interval <= duration:
            self.wait_until(countdown, tick * interval / MICROSECONDS_PER_SECOND)
            samples.append(self.read_sample(countdown.elapsed()))
            tick += 1

        gpu_percent = self.backend.end_window(countdown.elapsed())
        self.terminal.clear_progress()
        summary = self.summarize(samples, gpu_percent)
        self.results.append(ScenarioResult(self.names[index], samples, summary))
        self.terminal.result(f"  {self.result_line(summary)}")

    def read_sample(self, seconds: float) -> dict[str, Any]:
        try:
            counters = self.backend.read_sample()
        except OSError as error:
            self.assert_running()
            raise Failure(f"Could not read Melodia's counters: {error}") from error
        return {"seconds": seconds, **counters}

    # Memory is the median sample, so one spike can't set a scenario's figure. CPU is the total over
    # the window, where a median of per-interval rates would drop a periodic burst.
    def summarize(self, samples: list[dict[str, Any]], gpu_percent: float | None) -> dict[str, Any]:
        first, final = samples[0], samples[-1]
        cpu_one_core = (
            (final["cpu_seconds"] - first["cpu_seconds"]) / (final["seconds"] - first["seconds"]) * 100
        )
        summary = self.backend.summarize(samples)
        summary["cpu_one_core"] = cpu_one_core
        summary["cpu_all_cores"] = cpu_one_core / LOGICAL_PROCESSORS
        summary["gpu"] = gpu_percent
        return summary

    def result_line(self, summary: dict[str, Any]) -> str:
        memory = ", ".join(
            f"{label} {format_mib(summary[key])}" for label, key in self.backend.HEADLINE
        )
        return (
            f"{memory}, CPU {format_percent(summary['cpu_one_core'])} of one core "
            f"({format_percent(summary['cpu_all_cores'])} of all cores), "
            f"GPU {format_percent(summary['gpu'])}"
        )

    def save_report(self) -> None:
        out_dir = self.settings.out_dir
        out_dir.mkdir(parents=True, exist_ok=True)
        columns = ["seconds", *self.backend.SAMPLE_COLUMNS]
        with open(out_dir / "samples.csv", "w", newline="", encoding="utf-8") as file:
            writer = csv.writer(file)
            writer.writerow(["scenario", *columns])
            for result in self.results:
                for sample in result.samples:
                    writer.writerow([result.name, *(csv_value(sample[column]) for column in columns)])

        report = self.format_report()
        (out_dir / "summary.md").write_text(report + "\n", encoding="utf-8")
        print(f"\n{report}\n")
        print(f"Written to {out_dir.resolve()}")

    def format_report(self) -> str:
        backend = self.backend
        lines = [
            f"# Melodia footprint, {datetime.now():%Y-%m-%d %H:%M}",
            "",
            f"- Binary: {self.describe_binary()}",
            f"- Process: {self.target.origin}",
            *(f"- {line}" for line in backend.system_lines()),
            f"- Method: {self.describe_method()}",
            "",
        ]
        lines += markdown_table(
            ["Scenario", *(label for label, _ in backend.MEMORY_COLUMNS), "CPU (1 core)", "CPU (all cores)", "GPU"],
            [
                [
                    result.name,
                    *(format_mib(result.summary[key]) for _, key in backend.MEMORY_COLUMNS),
                    format_percent(result.summary["cpu_one_core"]),
                    format_percent(result.summary["cpu_all_cores"]),
                    format_percent(result.summary["gpu"]),
                ]
                for result in self.results
            ],
        )
        lines.append("")
        lines += markdown_table(
            ["Scenario", *(label for label, _, _ in backend.COUNTER_COLUMNS)],
            [
                [result.name, *(form(result.summary[key]) for _, key, form in backend.COUNTER_COLUMNS)]
                for result in self.results
            ],
        )
        return "\n".join(lines)

    def describe_binary(self) -> str:
        try:
            shown, runnable = self.backend.executable()
            built = datetime.fromtimestamp(os.stat(runnable).st_mtime)
        except OSError:
            return "unknown"
        version = "unknown"
        try:
            answer = subprocess.run(
                [runnable, "--version"],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=VERSION_TIMEOUT_SECONDS,
            )
            first_line = answer.stdout.splitlines()[:1]
            if answer.returncode == 0 and first_line:
                version = first_line[0].removeprefix("Melodia ")
        except (OSError, subprocess.TimeoutExpired):
            pass
        return f"{shown}, version {version}, built {built:%Y-%m-%d %H:%M}"

    def describe_method(self) -> str:
        settings = self.settings
        settle = f"{settings.settle_seconds:g} s settle"
        if self.target.launched:
            settle = (
                f"{STARTUP_SCENARIO} sampled from the launch, {settings.settle_seconds:g} s from the "
                "launch to the first prompt"
            )
        return (
            f"{settle}, {settings.switch_settle_seconds:g} s after each switch, "
            f"{settings.duration_seconds:g} s per scenario sampled every "
            f"{settings.interval_seconds:g} s. Memory is the median sample, CPU and GPU the total "
            "over the window."
        )


def platform_backend() -> type[LinuxBackend] | type[WindowsBackend]:
    if LINUX:
        return LinuxBackend
    if WINDOWS:
        return WindowsBackend
    raise Failure("The footprint is measured on Linux and Windows only.")


def median(samples: list[dict[str, Any]], key: str) -> int | None:
    """The middle sample of `key`. A source that never reported leaves it empty, not zero."""
    values = sorted(sample[key] for sample in samples if sample[key] is not None)
    if not values:
        return None
    middle = len(values) // 2
    if len(values) % 2:
        return values[middle]
    return round((values[middle - 1] + values[middle]) / 2)


def last(samples: list[dict[str, Any]], key: str) -> Any:
    return samples[-1][key]


def sum_of_present(*values: int | None) -> int | None:
    present = [value for value in values if value is not None]
    return sum(present) if present else None


def busiest_engine_percent(
    at_start: dict[str, int], at_end: dict[str, int], window_seconds: float, ticks_per_second: int
) -> float:
    """The busiest GPU engine's running time over the window, as Task Manager charges a process.

    Engines are separate queues, and adding 3D, copy and video can pass 100%. An engine absent at
    the start was created inside the window, so its whole running time belongs to it.
    """
    window_ticks = window_seconds * ticks_per_second
    return max(
        [0.0, *((ticks - at_start.get(engine, 0)) / window_ticks * 100 for engine, ticks in at_end.items())]
    )


def larger_of(first: float | None, second: float | None) -> float | None:
    present = [value for value in (first, second) if value is not None]
    return max(present) if present else None


def format_mib(value: float | None) -> str:
    if value is None:
        return "n/a"
    tenths = (round(value) * 10 + BYTES_PER_MIB // 2) // BYTES_PER_MIB
    return f"{tenths // 10}.{tenths % 10} MiB"


def format_percent(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.2f}%"


def format_count(value: int | None) -> str:
    return "n/a" if value is None else str(value)


def exit_status(code: int) -> str:
    # A Windows exit code past the signed range is an NTSTATUS, spelled 0xC0000005 everywhere else.
    if WINDOWS and code > 0x7FFFFFFF:
        return f"0x{code:08X}"
    return str(code)


def csv_value(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, float):
        return f"{value:.3f}"
    return str(value)


def markdown_table(header: list[str], rows: list[list[str]]) -> list[str]:
    lines = ["| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
    lines += ["| " + " | ".join(row) + " |" for row in rows]
    return lines


def run_quietly(
    command: list[str], timeout: float = 10, env: dict[str, str] | None = None
) -> str | None:
    """`command`'s stdout, or None where it is missing, fails or hangs."""
    try:
        answer = subprocess.run(
            command, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=timeout, env=env
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return answer.stdout if answer.returncode == 0 else None


UNIT_BYTES = {"KiB": 1024, "MiB": 1024**2, "GiB": 1024**3}
NANOSECONDS_PER_SECOND = 1_000_000_000
# console.warn from a KWin script reaches the journal a moment after the script runs.
JOURNAL_POLL_ATTEMPTS = 20
JOURNAL_POLL_SECONDS = 0.1

KWIN_FIND_OUTPUT = """\
let output = "";
for (const candidate of workspace.windowList()) {{
    if (candidate.pid === {pid} && candidate.output) {{
        output = candidate.output.name;
        break;
    }}
}}
console.warn("{token}", output);
"""


@dataclass
class DrmUsage:
    memory: int
    engines: dict[str, int]


class LinuxBackend:
    """Reads /proc, the DRM fdinfo keys and nvidia-smi for one process."""

    SAMPLE_COLUMNS = (
        "anonymous_bytes", "swap_bytes", "pss_bytes", "uss_bytes", "rss_bytes",
        "pss_anon_bytes", "pss_file_bytes", "pss_shmem_bytes", "peak_rss_bytes", "cpu_seconds",
        "threads", "memory_maps", "nvidia_framebuffer_bytes", "drm_resident_bytes",
        "file_descriptors",
    )
    HEADLINE = (("Anonymous", "anonymous"), ("PSS", "pss"))
    MEMORY_COLUMNS = (
        ("Anonymous", "anonymous"), ("PSS", "pss"), ("USS", "uss"), ("RSS", "rss"),
        ("GPU memory", "gpu_memory"),
    )
    COUNTER_COLUMNS = (
        ("Peak RSS", "peak_rss", format_mib),
        ("Swap", "swap", format_mib),
        ("Threads", "threads", format_count),
        ("File descriptors", "file_descriptors", format_count),
        ("Memory maps", "memory_maps", format_count),
    )

    def __init__(self, pid: int) -> None:
        self.pid = pid
        self.proc = Path(f"/proc/{pid}")
        if not os.access(self.proc / "stat", os.R_OK):
            raise Failure(f"No process with pid {pid}.")
        self.clock_ticks = os.sysconf("SC_CLK_TCK")
        self.nvidia = NvidiaMonitor.start()
        self.engines_at_start: dict[str, int] = {}
        self.nvidia_rows_at_start = 0

    def close(self) -> None:
        if self.nvidia is not None:
            self.nvidia.close()

    def running(self) -> bool:
        # An exited process stays a zombie, /proc entry and all, until its parent reaps it.
        try:
            stat = (self.proc / "stat").read_text()
        except OSError:
            return False
        state = stat.rpartition(") ")[2].split(" ", 1)[0]
        return state not in ("Z", "X")

    def executable(self) -> tuple[str, str]:
        # /proc/<pid>/exe still opens the binary that is running after a rebuild has replaced it on
        # disk, and then reads "(deleted)".
        link = str(self.proc / "exe")
        return os.readlink(link), link

    def read_sample(self) -> dict[str, Any]:
        rollup: dict[str, int] = {}
        for line in (self.proc / "smaps_rollup").read_text().splitlines():
            field = line.split()
            if len(field) == 3 and field[2] == "kB":
                rollup[field[0].rstrip(":")] = int(field[1]) * 1024

        peak_rss = threads = None
        for line in (self.proc / "status").read_text().splitlines():
            key, _, value = line.partition(":")
            if key == "VmHWM":
                peak_rss = int(value.split()[0]) * 1024
            elif key == "Threads":
                threads = int(value)

        # comm sits in parentheses and may itself hold spaces or parentheses.
        stat = (self.proc / "stat").read_text().rpartition(") ")[2].split()
        cpu_seconds = (int(stat[11]) + int(stat[12])) / self.clock_ticks
        with open(self.proc / "maps", "rb") as maps:
            memory_maps = sum(1 for _ in maps)
        drm = read_drm_usage(self.proc)

        return {
            "anonymous_bytes": rollup.get("Anonymous", 0) + rollup.get("Swap", 0),
            "swap_bytes": rollup.get("Swap", 0),
            "pss_bytes": rollup.get("Pss", 0),
            "uss_bytes": rollup.get("Private_Clean", 0)
            + rollup.get("Private_Dirty", 0)
            + rollup.get("Private_Hugetlb", 0),
            "rss_bytes": rollup.get("Rss", 0),
            "pss_anon_bytes": rollup.get("Pss_Anon", 0),
            "pss_file_bytes": rollup.get("Pss_File", 0),
            "pss_shmem_bytes": rollup.get("Pss_Shmem", 0),
            "peak_rss_bytes": peak_rss,
            "cpu_seconds": cpu_seconds,
            "threads": threads,
            "memory_maps": memory_maps,
            "nvidia_framebuffer_bytes": self.nvidia.framebuffer(self.pid) if self.nvidia else None,
            "drm_resident_bytes": drm.memory if drm else None,
            "file_descriptors": len(os.listdir(self.proc / "fd")),
        }

    def begin_window(self) -> None:
        drm = read_drm_usage(self.proc)
        self.engines_at_start = drm.engines if drm else {}
        self.nvidia_rows_at_start = self.nvidia.row_count() if self.nvidia else 0

    def end_window(self, window_seconds: float) -> float | None:
        drm = read_drm_usage(self.proc)
        drm_percent = None
        if drm and drm.engines:
            drm_percent = busiest_engine_percent(
                self.engines_at_start, drm.engines, window_seconds, NANOSECONDS_PER_SECOND
            )
        nvidia_percent = None
        if self.nvidia is not None:
            nvidia_percent = self.nvidia.utilization(
                self.pid, self.nvidia_rows_at_start, self.nvidia.row_count()
            )
        return larger_of(drm_percent, nvidia_percent)

    def summarize(self, samples: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "anonymous": median(samples, "anonymous_bytes"),
            "pss": median(samples, "pss_bytes"),
            "uss": median(samples, "uss_bytes"),
            "rss": median(samples, "rss_bytes"),
            "gpu_memory": sum_of_present(
                median(samples, "nvidia_framebuffer_bytes"), median(samples, "drm_resident_bytes")
            ),
            "peak_rss": last(samples, "peak_rss_bytes"),
            "swap": median(samples, "swap_bytes"),
            "threads": last(samples, "threads"),
            "file_descriptors": last(samples, "file_descriptors"),
            "memory_maps": last(samples, "memory_maps"),
        }

    def system_lines(self) -> list[str]:
        return [
            f"OS: {os_name()}",
            f"Kernel: {describe_kernel()}",
            f"Session: {describe_session()}",
            f"CPU: {cpu_model()}, {LOGICAL_PROCESSORS} logical processors",
            f"GPU: {describe_gpus()}",
            f"Display: {self.describe_display()}",
        ]

    # Only KWin will say which output holds another client's window, and only to a script loaded
    # into it, whose console.warn lands in the user journal.
    def describe_display(self) -> str:
        if not kwin_is_reachable():
            return "unknown, the screen under the window is only found under KWin"
        output = self.find_window_output()
        if not output:
            return "unknown, the window was hidden"
        return describe_output(output)

    def find_window_output(self) -> str:
        token = f"melodia-footprint-{os.getpid()}-{random.randrange(1 << 32)}"
        with tempfile.TemporaryDirectory(prefix="melodia-footprint-") as scratch:
            script = Path(scratch) / f"{token}.js"
            script.write_text(KWIN_FIND_OUTPUT.format(pid=self.pid, token=token))
            since = int(time.time())
            reply = gdbus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.loadScript", str(script), token)
            script_id = (reply or "").strip().removeprefix("(").removesuffix(",)")
            if not script_id.isdigit():
                return ""
            try:
                if gdbus("org.kde.KWin", f"/Scripting/Script{script_id}", "org.kde.kwin.Script.run") is None:
                    return ""
                return read_journal_report(token, since)
            finally:
                gdbus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.unloadScript", token)


def read_drm_usage(proc: Path) -> DrmUsage | None:
    """Every DRM client the process holds, each counted once, or None when it holds none.

    A dup-ed descriptor repeats the counters of the client behind it, so clients are told apart by
    driver, device and client id rather than by descriptor.
    """
    try:
        descriptors = os.listdir(proc / "fdinfo")
    except OSError:
        return None
    counted: set[tuple[str, str, str]] = set()
    memory = 0
    engines: dict[str, int] = {}
    for descriptor in descriptors:
        try:
            text = (proc / "fdinfo" / descriptor).read_text()
        except OSError:
            # Closed after the listing.
            continue
        client = parse_drm_client(text)
        if client is None or client[0] in counted:
            continue
        key, client_memory, client_engines = client
        counted.add(key)
        memory += client_memory
        driver, pdev, _ = key
        for name, nanoseconds in client_engines.items():
            engine = f"{driver}/{pdev}/{name}"
            engines[engine] = engines.get(engine, 0) + nanoseconds
    if not counted:
        return None
    return DrmUsage(memory, engines)


def parse_drm_client(text: str) -> tuple[tuple[str, str, str], int, dict[str, int]] | None:
    driver = pdev = client_id = ""
    resident = legacy = 0
    has_resident = False
    engines: dict[str, int] = {}
    for line in text.splitlines():
        field = line.split()
        if len(field) < 2:
            continue
        key = field[0]
        try:
            if key == "drm-driver:":
                driver = field[1]
            elif key == "drm-pdev:":
                pdev = field[1]
            elif key == "drm-client-id:":
                client_id = field[1]
            elif key.startswith("drm-resident-"):
                resident += drm_bytes(field)
                has_resident = True
            elif key.startswith("drm-memory-"):
                legacy += drm_bytes(field)
            elif key.startswith("drm-engine-") and not key.startswith("drm-engine-capacity-"):
                engines[key.removeprefix("drm-engine-").removesuffix(":")] = int(field[1])
        except ValueError:
            continue
    if not driver:
        return None
    # Newer amdgpu prints drm-resident-* beside the legacy drm-memory-* it replaces.
    return (driver, pdev, client_id), resident if has_resident else legacy, engines


def drm_bytes(field: list[str]) -> int:
    unit = field[2] if len(field) > 2 else ""
    return round(float(field[1]) * UNIT_BYTES.get(unit, 1))


class NvidiaMonitor:
    """`nvidia-smi pmon` for the whole run, so a sample in a scenario's first second has a row."""

    def __init__(self, process: subprocess.Popen[str]) -> None:
        self.process = process
        self.rows: list[dict[str, str]] = []
        self.lock = threading.Lock()
        threading.Thread(target=self.read, daemon=True).start()

    @classmethod
    def start(cls) -> NvidiaMonitor | None:
        if shutil.which("nvidia-smi") is None:
            return None
        process = subprocess.Popen(
            ["nvidia-smi", "pmon", "--select", "um", "--delay", "1", "--options", "T"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        return cls(process)

    def read(self) -> None:
        assert self.process.stdout is not None
        columns: list[str] | None = None
        for line in self.process.stdout:
            # pmon names its columns in a header, and their order follows --select.
            if line.startswith("#"):
                names = line[1:].split()
                if "pid" in names:
                    columns = names
                continue
            if columns is not None:
                row = dict(zip(columns, line.split()))
                with self.lock:
                    self.rows.append(row)

    def close(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()

    def row_count(self) -> int:
        with self.lock:
            return len(self.rows)

    def framebuffer(self, pid: int) -> int | None:
        """The latest second's framebuffer rows for `pid`, summed across GPUs."""
        wanted = str(pid)
        latest = None
        total = 0
        with self.lock:
            for row in reversed(self.rows):
                if row.get("pid") != wanted:
                    continue
                if latest is None:
                    latest = row.get("Time")
                elif row.get("Time") != latest:
                    break
                framebuffer = row.get("fb", "")
                if framebuffer.isdigit():
                    total += int(framebuffer) * BYTES_PER_MIB
        return None if latest is None else total

    def utilization(self, pid: int, start: int, end: int) -> float | None:
        # pmon prints "-" for a second without a utilization sample, which is an idle one. Seconds
        # come from every process's rows, so a second with no Melodia row still counts, as an idle
        # one.
        wanted = str(pid)
        busiest: dict[str | None, int] = {}
        listed = False
        with self.lock:
            window = self.rows[start:end]
        for row in window:
            second = row.get("Time")
            busiest.setdefault(second, 0)
            if row.get("pid") != wanted:
                continue
            listed = True
            sm = row.get("sm", "")
            if sm.isdigit():
                busiest[second] = max(busiest[second], int(sm))
        if not listed:
            return None
        return sum(busiest.values()) / len(busiest)


def os_name() -> str:
    try:
        return platform.freedesktop_os_release().get("PRETTY_NAME", "unknown")
    except OSError:
        return "unknown"


def describe_kernel() -> str:
    # A kernel built with CONFIG_NO_PAGE_MAPCOUNT keeps no per-page mapcount for large folios, and
    # smaps then estimates their PSS from an average and can file private pages under shared.
    release = platform.release()
    config = kernel_config(release)
    if not config:
        return release
    if "CONFIG_NO_PAGE_MAPCOUNT=y" in config.splitlines():
        return f"{release}, built without per-page mapcounts, so PSS and USS are estimates"
    return f"{release}, keeping per-page mapcounts, so PSS and USS are exact"


def kernel_config(release: str) -> str | None:
    try:
        boot = Path(f"/boot/config-{release}")
        if os.access(boot, os.R_OK):
            return boot.read_text(errors="replace")
        packed = Path("/proc/config.gz")
        if os.access(packed, os.R_OK):
            return gzip.decompress(packed.read_bytes()).decode(errors="replace")
    except OSError:
        pass
    return None


def describe_session() -> str:
    desktop = os.environ.get("XDG_CURRENT_DESKTOP") or "unknown desktop"
    session_type = os.environ.get("XDG_SESSION_TYPE") or "unknown session"
    if desktop == "KDE":
        version = (run_quietly(["plasmashell", "--version"]) or "").splitlines()[:1]
        if version:
            desktop = f"KDE Plasma {version[0].split()[-1]}"
    session_type = {"wayland": "Wayland", "x11": "X11"}.get(session_type, session_type)
    return f"{desktop} on {session_type}"


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            key, _, value = line.partition(":")
            if key.strip() == "model name":
                return value.strip()
    except OSError:
        pass
    return "unknown"


def describe_gpus() -> str:
    gpus = []
    for card in sorted(Path("/sys/class/drm").glob("card*")):
        # cardN-<connector> entries are outputs, and a firmware framebuffer has no PCI vendor.
        if not re.fullmatch(r"card[0-9]+", card.name) or not os.access(card / "device" / "vendor", os.R_OK):
            continue
        device = (card / "device").resolve()
        driver = os.path.basename(os.readlink(device / "driver")) if (device / "driver").is_symlink() else ""
        version = ""
        module_version = Path(f"/sys/module/{driver}/version")
        if driver and os.access(module_version, os.R_OK):
            version = f" {module_version.read_text().strip()}"
        gpus.append(f"{pci_device_name(device.name)} ({driver}{version})")
    return "; ".join(gpus) or "unknown"


def pci_device_name(address: str) -> str:
    description = run_quietly(["lspci", "-s", address])
    if not description:
        return f"PCI device {address}"
    description = description.strip().partition(": ")[2]
    return re.sub(r" \(rev [^)]*\)$", "", description)


def gdbus(destination: str, object_path: str, method: str, *arguments: str) -> str | None:
    return run_quietly(
        ["gdbus", "call", "--session", "--dest", destination, "--object-path", object_path,
         "--method", method, *arguments]
    )


def kwin_is_reachable() -> bool:
    if shutil.which("gdbus") is None or shutil.which("journalctl") is None:
        return False
    reply = gdbus(
        "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.NameHasOwner",
        "org.kde.KWin",
    )
    return (reply or "").strip() == "(true,)"


def read_journal_report(token: str, since: int) -> str:
    """The word logged after `token`, empty when the script found no window."""
    for _ in range(JOURNAL_POLL_ATTEMPTS):
        journal = run_quietly(
            ["journalctl", "--user", "--since", f"@{since - 1}", "--output", "cat", "--no-pager"]
        )
        for line in (journal or "").splitlines():
            words = line.split()
            if token in words:
                after = words.index(token) + 1
                return words[after] if after < len(words) else ""
        time.sleep(JOURNAL_POLL_SECONDS)
    return ""


def describe_output(output: str) -> str:
    if shutil.which("kscreen-doctor") is None:
        return f"{output}, its mode unknown without kscreen-doctor"
    # The report is written after the last scenario, so JSON shaped differently than expected costs
    # this line rather than the whole summary.
    try:
        screens = json.loads(run_quietly(["kscreen-doctor", "--json"]) or "{}").get("outputs", [])
        for screen in screens:
            if screen.get("name") != output:
                continue
            for mode in screen.get("modes", []):
                if mode.get("id") == screen.get("currentModeId"):
                    size = mode["size"]
                    return (
                        f"{size['width']}x{size['height']} at {round(mode['refreshRate'])} Hz, scale "
                        f"{json.dumps(screen.get('scale'))}, the screen the window was on"
                    )
    except (ValueError, KeyError, TypeError, AttributeError):
        pass
    return f"{output}, its mode unknown, the screen the window was on"


DWORD = ctypes.c_uint32
WORD = ctypes.c_uint16
LONG = ctypes.c_int32
BOOL = ctypes.c_int
HANDLE = ctypes.c_void_p
WCHAR = ctypes.c_wchar

PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
PROCESS_VM_READ = 0x0010
SYNCHRONIZE = 0x00100000
ERROR_ACCESS_DENIED = 5
WAIT_OBJECT_0 = 0
TH32CS_SNAPTHREAD = 0x00000004
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
GR_GDIOBJECTS = 0
GR_USEROBJECTS = 1
GW_OWNER = 4
MONITOR_DEFAULTTONEAREST = 2
ENUM_CURRENT_SETTINGS = 0xFFFFFFFF
STD_OUTPUT_HANDLE = -11 & 0xFFFFFFFF
STD_ERROR_HANDLE = -12 & 0xFFFFFFFF
ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004
PDH_MORE_DATA = 0x800007D2
# "Running Time" is the raw value behind "Utilization Percentage", a Timer100Ns counter.
GPU_TICKS_PER_SECOND = 10_000_000
FILETIME_TICKS_PER_SECOND = 10_000_000
# Windows 11 kept the registry's "Windows 10" product name, so the build is what tells them apart.
WINDOWS_11_FIRST_BUILD = 22000


class ProcessMemoryCountersEx2(ctypes.Structure):
    # The first revision carrying PrivateWorkingSetSize, so it needs Windows 10 or 11 22H2 with the
    # September 2023 update or later.
    _fields_ = [
        ("cb", DWORD),
        ("PageFaultCount", DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
        ("PrivateWorkingSetSize", ctypes.c_size_t),
        ("SharedCommitUsage", ctypes.c_uint64),
    ]


class FileTime(ctypes.Structure):
    _fields_ = [("low", DWORD), ("high", DWORD)]

    @property
    def ticks(self) -> int:
        return self.high << 32 | self.low


class ThreadEntry32(ctypes.Structure):
    _fields_ = [
        ("dwSize", DWORD),
        ("cntUsage", DWORD),
        ("th32ThreadID", DWORD),
        ("th32OwnerProcessID", DWORD),
        ("tpBasePri", LONG),
        ("tpDeltaPri", LONG),
        ("dwFlags", DWORD),
    ]


class PdhRawCounter(ctypes.Structure):
    _fields_ = [
        ("CStatus", DWORD),
        ("TimeStamp", FileTime),
        ("FirstValue", ctypes.c_int64),
        ("SecondValue", ctypes.c_int64),
        ("MultiCount", DWORD),
    ]


class PdhRawCounterItem(ctypes.Structure):
    _fields_ = [("szName", ctypes.c_wchar_p), ("RawValue", PdhRawCounter)]


class Rect(ctypes.Structure):
    _fields_ = [("left", LONG), ("top", LONG), ("right", LONG), ("bottom", LONG)]


class MonitorInfoEx(ctypes.Structure):
    _fields_ = [
        ("cbSize", DWORD),
        ("rcMonitor", Rect),
        ("rcWork", Rect),
        ("dwFlags", DWORD),
        ("szDevice", WCHAR * 32),
    ]


class DevMode(ctypes.Structure):
    # DEVMODEW with the display arm of each union.
    _fields_ = [
        ("dmDeviceName", WCHAR * 32),
        ("dmSpecVersion", WORD),
        ("dmDriverVersion", WORD),
        ("dmSize", WORD),
        ("dmDriverExtra", WORD),
        ("dmFields", DWORD),
        ("dmPositionX", LONG),
        ("dmPositionY", LONG),
        ("dmDisplayOrientation", DWORD),
        ("dmDisplayFixedOutput", DWORD),
        ("dmColor", ctypes.c_short),
        ("dmDuplex", ctypes.c_short),
        ("dmYResolution", ctypes.c_short),
        ("dmTTOption", ctypes.c_short),
        ("dmCollate", ctypes.c_short),
        ("dmFormName", WCHAR * 32),
        ("dmLogPixels", WORD),
        ("dmBitsPerPel", DWORD),
        ("dmPelsWidth", DWORD),
        ("dmPelsHeight", DWORD),
        ("dmDisplayFlags", DWORD),
        ("dmDisplayFrequency", DWORD),
        ("dmICMMethod", DWORD),
        ("dmICMIntent", DWORD),
        ("dmMediaType", DWORD),
        ("dmDitherType", DWORD),
        ("dmReserved1", DWORD),
        ("dmReserved2", DWORD),
        ("dmPanningWidth", DWORD),
        ("dmPanningHeight", DWORD),
    ]


class DisplayDevice(ctypes.Structure):
    _fields_ = [
        ("cb", DWORD),
        ("DeviceName", WCHAR * 32),
        ("DeviceString", WCHAR * 128),
        ("StateFlags", DWORD),
        ("DeviceID", WCHAR * 128),
        ("DeviceKey", WCHAR * 128),
    ]


@functools.cache
def windows_api() -> SimpleNamespace:
    """The Win32 calls this script makes, prototypes declared so ctypes passes 64-bit handles whole."""

    def declare(function: Any, restype: Any, *argtypes: Any) -> Any:
        function.restype = restype
        function.argtypes = argtypes
        return function

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    pdh = ctypes.WinDLL("pdh")
    pointer = ctypes.POINTER
    window_callback = ctypes.WINFUNCTYPE(BOOL, HANDLE, ctypes.c_ssize_t)
    return SimpleNamespace(
        window_callback=window_callback,
        OpenProcess=declare(kernel32.OpenProcess, HANDLE, DWORD, BOOL, DWORD),
        CloseHandle=declare(kernel32.CloseHandle, BOOL, HANDLE),
        WaitForSingleObject=declare(kernel32.WaitForSingleObject, DWORD, HANDLE, DWORD),
        GetProcessTimes=declare(
            kernel32.GetProcessTimes, BOOL, HANDLE, *[pointer(FileTime)] * 4
        ),
        GetProcessHandleCount=declare(kernel32.GetProcessHandleCount, BOOL, HANDLE, pointer(DWORD)),
        QueryFullProcessImageNameW=declare(
            kernel32.QueryFullProcessImageNameW, BOOL, HANDLE, DWORD, ctypes.c_wchar_p, pointer(DWORD)
        ),
        CreateToolhelp32Snapshot=declare(kernel32.CreateToolhelp32Snapshot, HANDLE, DWORD, DWORD),
        Thread32First=declare(kernel32.Thread32First, BOOL, HANDLE, pointer(ThreadEntry32)),
        Thread32Next=declare(kernel32.Thread32Next, BOOL, HANDLE, pointer(ThreadEntry32)),
        GetStdHandle=declare(kernel32.GetStdHandle, HANDLE, DWORD),
        GetConsoleMode=declare(kernel32.GetConsoleMode, BOOL, HANDLE, pointer(DWORD)),
        SetConsoleMode=declare(kernel32.SetConsoleMode, BOOL, HANDLE, DWORD),
        GetProcessMemoryInfo=declare(
            psapi.GetProcessMemoryInfo, BOOL, HANDLE, pointer(ProcessMemoryCountersEx2), DWORD
        ),
        GetGuiResources=declare(user32.GetGuiResources, DWORD, HANDLE, DWORD),
        EnumWindows=declare(user32.EnumWindows, BOOL, window_callback, ctypes.c_ssize_t),
        GetWindowThreadProcessId=declare(user32.GetWindowThreadProcessId, DWORD, HANDLE, pointer(DWORD)),
        IsWindowVisible=declare(user32.IsWindowVisible, BOOL, HANDLE),
        GetWindow=declare(user32.GetWindow, HANDLE, HANDLE, ctypes.c_uint),
        MonitorFromWindow=declare(user32.MonitorFromWindow, HANDLE, HANDLE, DWORD),
        GetMonitorInfoW=declare(user32.GetMonitorInfoW, BOOL, HANDLE, pointer(MonitorInfoEx)),
        EnumDisplaySettingsW=declare(
            user32.EnumDisplaySettingsW, BOOL, ctypes.c_wchar_p, DWORD, pointer(DevMode)
        ),
        EnumDisplayDevicesW=declare(
            user32.EnumDisplayDevicesW, BOOL, ctypes.c_wchar_p, DWORD, pointer(DisplayDevice), DWORD
        ),
        PdhOpenQueryW=declare(pdh.PdhOpenQueryW, LONG, ctypes.c_wchar_p, ctypes.c_size_t, pointer(HANDLE)),
        PdhAddEnglishCounterW=declare(
            pdh.PdhAddEnglishCounterW, LONG, HANDLE, ctypes.c_wchar_p, ctypes.c_size_t, pointer(HANDLE)
        ),
        PdhCollectQueryData=declare(pdh.PdhCollectQueryData, LONG, HANDLE),
        PdhGetRawCounterArrayW=declare(
            pdh.PdhGetRawCounterArrayW, LONG, HANDLE, pointer(DWORD), pointer(DWORD), ctypes.c_void_p
        ),
        PdhCloseQuery=declare(pdh.PdhCloseQuery, LONG, HANDLE),
    )


def enable_windows_vt() -> bool:
    """Lets the console draw colours, which Windows Terminal does already and conhost needs asked."""
    api = windows_api()
    enabled = True
    for which in (STD_OUTPUT_HANDLE, STD_ERROR_HANDLE):
        handle = api.GetStdHandle(which)
        mode = DWORD()
        if not api.GetConsoleMode(handle, ctypes.byref(mode)):
            enabled = False
            continue
        if not api.SetConsoleMode(handle, mode.value | ENABLE_VIRTUAL_TERMINAL_PROCESSING):
            enabled = False
    return enabled


class GpuCounters:
    """The "GPU Process Memory" and "GPU Engine" counters Task Manager reads, through PDH.

    Instances are named pid_<pid>_luid_<adapter>_..., one per adapter or engine, and there are none
    until Melodia opens its GPU device, after Startup's first samples.
    """

    PATHS = {
        "dedicated": r"\GPU Process Memory(*)\Dedicated Usage",
        "shared": r"\GPU Process Memory(*)\Shared Usage",
        "engines": r"\GPU Engine(*)\Running Time",
    }

    def __init__(self, api: SimpleNamespace, pid: int) -> None:
        self.api = api
        self.prefix = f"pid_{pid}_"
        self.query = HANDLE()
        pdh_check(api.PdhOpenQueryW(None, 0, ctypes.byref(self.query)), "PdhOpenQueryW")
        self.counters: dict[str, HANDLE] = {}
        for name, path in self.PATHS.items():
            counter = HANDLE()
            pdh_check(api.PdhAddEnglishCounterW(self.query, path, 0, ctypes.byref(counter)), path)
            self.counters[name] = counter

    @classmethod
    def open(cls, api: SimpleNamespace, pid: int) -> GpuCounters | None:
        """None on a Windows without the GPU counters, which then reports GPU columns as n/a."""
        try:
            return cls(api, pid)
        except OSError:
            return None

    def close(self) -> None:
        self.api.PdhCloseQuery(self.query)

    def read(self) -> dict[str, dict[str, int]]:
        """Each counter's raw value per instance of this process, all collected in one pass."""
        pdh_check(self.api.PdhCollectQueryData(self.query), "PdhCollectQueryData")
        return {name: self.instances(counter) for name, counter in self.counters.items()}

    def instances(self, counter: HANDLE) -> dict[str, int]:
        size = DWORD(0)
        count = DWORD(0)
        status = self.api.PdhGetRawCounterArrayW(counter, ctypes.byref(size), ctypes.byref(count), None)
        if status & 0xFFFFFFFF != PDH_MORE_DATA:
            pdh_check(status, "PdhGetRawCounterArrayW")
            return {}
        buffer = ctypes.create_string_buffer(size.value)
        pdh_check(
            self.api.PdhGetRawCounterArrayW(counter, ctypes.byref(size), ctypes.byref(count), buffer),
            "PdhGetRawCounterArrayW",
        )
        items = ctypes.cast(buffer, ctypes.POINTER(PdhRawCounterItem))
        return {
            items[index].szName: items[index].RawValue.FirstValue
            for index in range(count.value)
            if items[index].szName and items[index].szName.startswith(self.prefix)
        }


def pdh_check(status: int, call: str) -> None:
    if status != 0:
        raise OSError(f"{call} failed with PDH status 0x{status & 0xFFFFFFFF:08X}")


class WindowsBackend:
    """Reads psapi, toolhelp, user32 and the PDH GPU counters for one process."""

    SAMPLE_COLUMNS = (
        "commit_bytes", "peak_commit_bytes", "private_ws_bytes", "working_set_bytes",
        "gpu_dedicated_bytes", "gpu_shared_bytes", "cpu_seconds", "handles", "threads",
        "gdi_objects", "user_objects",
    )
    HEADLINE = (("Commit", "commit"), ("private WS", "private_working_set"))
    MEMORY_COLUMNS = (
        ("Commit", "commit"), ("Private WS", "private_working_set"),
        ("Working set", "working_set"), ("GPU dedicated", "gpu_dedicated"),
        ("GPU shared", "gpu_shared"),
    )
    COUNTER_COLUMNS = (
        ("Peak commit", "peak_commit", format_mib),
        ("Handles", "handles", format_count),
        ("Threads", "threads", format_count),
        ("GDI objects", "gdi_objects", format_count),
        ("USER objects", "user_objects", format_count),
    )

    def __init__(self, pid: int) -> None:
        self.api = windows_api()
        self.pid = pid
        self.handle = self.api.OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ | SYNCHRONIZE, False, pid
        )
        if not self.handle:
            if ctypes.get_last_error() == ERROR_ACCESS_DENIED:
                raise Failure(
                    f"Access to pid {pid} was denied. If Melodia runs as administrator, run the "
                    "script as administrator too."
                )
            raise Failure(f"No process with pid {pid}.")
        self.gpu = GpuCounters.open(self.api, pid)
        self.engines_at_start: dict[str, int] = {}

    def close(self) -> None:
        if self.gpu is not None:
            self.gpu.close()
        self.api.CloseHandle(self.handle)

    def running(self) -> bool:
        return self.api.WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0

    def executable(self) -> tuple[str, str]:
        size = DWORD(32768)
        buffer = ctypes.create_unicode_buffer(size.value)
        if not self.api.QueryFullProcessImageNameW(self.handle, 0, buffer, ctypes.byref(size)):
            raise ctypes.WinError(ctypes.get_last_error())
        return buffer.value, buffer.value

    def read_sample(self) -> dict[str, Any]:
        api = self.api
        memory = ProcessMemoryCountersEx2(cb=ctypes.sizeof(ProcessMemoryCountersEx2))
        if not api.GetProcessMemoryInfo(self.handle, ctypes.byref(memory), memory.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        handles = DWORD()
        if not api.GetProcessHandleCount(self.handle, ctypes.byref(handles)):
            raise ctypes.WinError(ctypes.get_last_error())
        gpu = self.gpu.read() if self.gpu is not None else None

        return {
            # PagefileUsage and PrivateUsage are the same commit charge under two names.
            "commit_bytes": memory.PrivateUsage,
            "peak_commit_bytes": memory.PeakPagefileUsage,
            "private_ws_bytes": memory.PrivateWorkingSetSize,
            "working_set_bytes": memory.WorkingSetSize,
            "gpu_dedicated_bytes": sum(gpu["dedicated"].values()) if gpu else None,
            "gpu_shared_bytes": sum(gpu["shared"].values()) if gpu else None,
            "cpu_seconds": self.cpu_seconds(),
            "handles": handles.value,
            "threads": self.thread_count(),
            "gdi_objects": api.GetGuiResources(self.handle, GR_GDIOBJECTS),
            "user_objects": api.GetGuiResources(self.handle, GR_USEROBJECTS),
        }

    def cpu_seconds(self) -> float:
        created, exited, kernel, user = FileTime(), FileTime(), FileTime(), FileTime()
        if not self.api.GetProcessTimes(
            self.handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel), ctypes.byref(user)
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        return (kernel.ticks + user.ticks) / FILETIME_TICKS_PER_SECOND

    def thread_count(self) -> int:
        snapshot = self.api.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
        if snapshot in (None, INVALID_HANDLE_VALUE):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            entry = ThreadEntry32(dwSize=ctypes.sizeof(ThreadEntry32))
            count = 0
            more = self.api.Thread32First(snapshot, ctypes.byref(entry))
            while more:
                if entry.th32OwnerProcessID == self.pid:
                    count += 1
                more = self.api.Thread32Next(snapshot, ctypes.byref(entry))
            return count
        finally:
            self.api.CloseHandle(snapshot)

    def begin_window(self) -> None:
        self.engines_at_start = self.gpu.read()["engines"] if self.gpu is not None else {}

    def end_window(self, window_seconds: float) -> float | None:
        if self.gpu is None:
            return None
        engines = self.gpu.read()["engines"]
        return busiest_engine_percent(self.engines_at_start, engines, window_seconds, GPU_TICKS_PER_SECOND)

    def summarize(self, samples: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "commit": median(samples, "commit_bytes"),
            "private_working_set": median(samples, "private_ws_bytes"),
            "working_set": median(samples, "working_set_bytes"),
            "gpu_dedicated": median(samples, "gpu_dedicated_bytes"),
            "gpu_shared": median(samples, "gpu_shared_bytes"),
            "peak_commit": last(samples, "peak_commit_bytes"),
            "handles": last(samples, "handles"),
            "threads": last(samples, "threads"),
            "gdi_objects": last(samples, "gdi_objects"),
            "user_objects": last(samples, "user_objects"),
        }

    def system_lines(self) -> list[str]:
        return [
            f"OS: {windows_version()}",
            f"CPU: {processor_name()}, {LOGICAL_PROCESSORS} logical processors",
            f"GPU: {display_adapters()}",
            f"Display: {self.window_display()}",
        ]

    # WMI reports one refresh rate per adapter, so on a multi-monitor desktop it can name a screen
    # the window isn't on. Asking for the monitor under the window can't.
    def window_display(self) -> str:
        api = self.api
        window = self.main_window()
        if not window:
            return "unknown, the window was hidden"
        monitor = api.MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST)
        info = MonitorInfoEx(cbSize=ctypes.sizeof(MonitorInfoEx))
        if not api.GetMonitorInfoW(monitor, ctypes.byref(info)):
            return "unknown, no monitor under the window"
        mode = DevMode(dmSize=ctypes.sizeof(DevMode))
        if not api.EnumDisplaySettingsW(info.szDevice, ENUM_CURRENT_SETTINGS, ctypes.byref(mode)):
            return f"unknown, {info.szDevice} reported no mode"
        return f"{mode.dmPelsWidth}x{mode.dmPelsHeight} at {mode.dmDisplayFrequency} Hz, the screen the window was on"

    def main_window(self) -> int | None:
        """The process's visible, unowned top-level window, as .NET's MainWindowHandle picks it."""
        api = self.api
        found: list[int] = []

        def visit(window: int, _parameter: int) -> bool:
            owner = DWORD()
            api.GetWindowThreadProcessId(window, ctypes.byref(owner))
            if owner.value == self.pid and api.IsWindowVisible(window) and not api.GetWindow(window, GW_OWNER):
                found.append(window)
                return False
            return True

        api.EnumWindows(api.window_callback(visit), 0)
        return found[0] if found else None


def windows_version() -> str:
    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") as key:
            product = winreg.QueryValueEx(key, "ProductName")[0]
            build = winreg.QueryValueEx(key, "CurrentBuild")[0]
    except OSError:
        return "unknown"
    if build.isdigit() and int(build) >= WINDOWS_11_FIRST_BUILD:
        product = product.replace("Windows 10", "Windows 11")
    return f"Microsoft {product}, build {build}"


def processor_name() -> str:
    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as key:
            return winreg.QueryValueEx(key, "ProcessorNameString")[0].strip()
    except OSError:
        return "unknown"


def display_adapters() -> str:
    api = windows_api()
    names: list[str] = []
    index = 0
    while True:
        device = DisplayDevice(cb=ctypes.sizeof(DisplayDevice))
        if not api.EnumDisplayDevicesW(None, index, ctypes.byref(device), 0):
            break
        # One entry per output, so an adapter driving two screens is listed twice.
        if device.DeviceString and device.DeviceString not in names:
            names.append(device.DeviceString)
        index += 1
    return "; ".join(names) or "unknown"


def interrupt(_signal: int, _frame: Any) -> None:
    raise KeyboardInterrupt


def main(argv: list[str] | None = None) -> int:
    if not WINDOWS:
        signal.signal(signal.SIGTERM, interrupt)
    terminal = Terminal()
    backend = None
    try:
        settings = parse_settings(argv)
        backend_class = platform_backend()
        terminal.stop_echoing_keys()
        target = launch(settings) if settings.attach_pid is None else attach(settings.attach_pid)
        backend = backend_class(target.pid)
        Run(settings, terminal, target, backend).measure()
        return 0
    except Failure as failure:
        terminal.clear_progress()
        print(f"ERROR: {failure}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    finally:
        if backend is not None:
            backend.close()
        terminal.restore()


if __name__ == "__main__":
    sys.exit(main())
