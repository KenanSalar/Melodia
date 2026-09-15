#Requires -Version 7.4
<#
.SYNOPSIS
Measures Melodia's memory, CPU and GPU footprint on Windows the same way every run.

.DESCRIPTION
Launches Melodia (or attaches to a running one), lets it settle, then samples one window per
scenario while you switch between scenarios by hand. Every sample goes to samples.csv and the
per-scenario figures to summary.md.

Commit is the headline figure: the private memory Windows has promised the process, whether it
sits in RAM or in the page file. It doesn't fall when the memory manager trims the working set
under pressure, so it tracks what Melodia allocated rather than what Windows left resident.
Task Manager calls it "Commit size", and it is the field Chromium reports as its private
footprint on Windows.

Private working set is Task Manager's default "Memory" column. It is recorded because it is the
number people check a README against, but it only says what was in RAM at that moment.

There is no PSS on Windows. The kernel keeps no proportional count, and one synthesized from
QueryWorkingSetEx divides each shared page by ShareCount, a 3-bit field that saturates at 7. A
system DLL mapped by two hundred processes is charged as if seven shared it, so the figure moves
with whatever else happens to be running.

GPU memory comes from the "GPU Process Memory" counters Task Manager reads, because FemtoVG's
textures and framebuffers are driver allocations commit and working set don't describe.
Dedicated is VRAM, shared is system RAM mapped for the GPU. Report them beside commit, never
summed into it.

CPU is process CPU time over wall time, one full core being 100%. Task Manager's Details tab
divides by the logical processor count, which is the "all cores" column. GPU is the busiest
engine's running time over wall time, the same per-process definition Task Manager uses.

Handles, threads, GDI and USER objects should hold flat from one scenario to the next. A climb
is a leak.

.PARAMETER Exe
Binary to launch. Defaults to the release build: cargo build --release -p melodia

.PARAMETER DataDir
Data root to launch against, passed as MELODIA_DATA_DIR, so every run sees the same library.

.PARAMETER ProcessId
Measure an already running Melodia instead of launching one.

.PARAMETER Scenarios
One measurement window per name, in order. You are prompted before each.

.PARAMETER SettleSeconds
Wait before the first scenario, so startup work stays out of every window.

.PARAMETER SwitchSettleSeconds
Wait after each prompt, so the switch itself (a view being built, covers decoding) stays out of
the window.

.PARAMETER DurationSeconds
Length of each scenario's window.

.PARAMETER IntervalSeconds
Time between memory samples.

.PARAMETER OutDir
Where samples.csv and summary.md go. Defaults to a timestamped folder under target\windows-footprint.

.EXAMPLE
./scripts/measure-windows-footprint.ps1

.EXAMPLE
./scripts/measure-windows-footprint.ps1 -DataDir D:\melodia-bench -Scenarios 'Idle', 'Playing, list view'
#>
[CmdletBinding(DefaultParameterSetName = 'Launch')]
param(
    [Parameter(ParameterSetName = 'Launch')]
    [string] $Exe = (Join-Path $PSScriptRoot '..' 'target' 'release' 'Melodia.exe'),

    [Parameter(ParameterSetName = 'Launch')]
    [string] $DataDir,

    [Parameter(ParameterSetName = 'Attach', Mandatory)]
    [int] $ProcessId,

    [string[]] $Scenarios = @('Idle', 'Playing, list view', 'Playing, visualizer live'),

    [ValidateRange(0, 3600)]
    [int] $SettleSeconds = 30,

    [ValidateRange(0, 3600)]
    [int] $SwitchSettleSeconds = 10,

    [ValidateRange(1, 3600)]
    [int] $DurationSeconds = 60,

    [ValidateRange(0.1, 60)]
    [double] $IntervalSeconds = 1,

    [string] $OutDir = (Join-Path $PSScriptRoot '..' 'target' 'windows-footprint' (Get-Date -Format 'yyyyMMdd-HHmmss'))
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# "Running Time" is the raw value behind "Utilization Percentage", a Timer100Ns counter, so it
# counts in TimeSpan ticks.
$GpuTicksPerSecond = [TimeSpan]::TicksPerSecond
$ProgressStepSeconds = 0.25

function Measure-Footprint($Process, [string] $Origin) {
    # Holding the handle keeps ExitCode readable once the process is gone.
    $null = $Process.Handle
    Write-Host "Measuring Melodia (pid $($Process.Id)), $Origin."
    Wait-Countdown $Process 'Waiting for Melodia to settle' $SettleSeconds

    $measurements = for ($index = 0; $index -lt $Scenarios.Count; $index++) {
        $title = "Scenario $($index + 1)/$($Scenarios.Count): $($Scenarios[$index])"
        Read-Confirmation $title
        Wait-Countdown $Process "$title, settling after the switch" $SwitchSettleSeconds

        $measurement = Measure-Scenario $Process $Scenarios[$index] $title
        Write-ScenarioResult $measurement.Summary
        $measurement
    }

    Save-Report $Process $Origin $measurements
    Write-Host "Melodia (pid $($Process.Id)) is still running. Close it before the next run."
}

function Start-Melodia([string] $Path, [string] $DataRoot) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "No Melodia binary at $Path. Build one with: cargo build --release -p melodia"
    }

    $inheritedDataDir = $env:MELODIA_DATA_DIR
    if ($DataRoot) {
        $env:MELODIA_DATA_DIR = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($DataRoot)
    }
    try {
        Start-Process -FilePath $Path -PassThru
    }
    finally {
        $env:MELODIA_DATA_DIR = $inheritedDataDir
    }
}

function Assert-Running($Process) {
    if (-not $Process.HasExited) { return }

    throw ("Melodia exited with code $($Process.ExitCode). Straight after a launch that means another " +
        'instance holds this data root and the launch was forwarded to it: close that one, or measure it with -ProcessId.')
}

function Wait-Countdown($Process, [string] $Activity, [double] $Seconds) {
    $countdown = New-Countdown $Activity $Seconds
    Wait-Until $Process $countdown $Seconds
    Complete-Countdown $countdown
}

function Read-Confirmation([string] $Title) {
    Write-Host ''
    Write-Host $Title -ForegroundColor Cyan
    Write-Host '  Set Melodia up for it, keep its window visible and unminimized, then press Enter.'
    # A host with no stdin reads end-of-input at once, which would sample a scenario nobody set up.
    if ($null -eq [Console]::ReadLine()) {
        throw 'No console input to confirm the scenario with. Run the script from an interactive terminal.'
    }
}

function Measure-Scenario($Process, [string] $Scenario, [string] $Title) {
    $engineTicksAtStart = Read-GpuEngineTicks $Process.Id
    $countdown = New-Countdown "$Title, measuring" $DurationSeconds

    $samples = [Collections.Generic.List[object]]::new()
    for ($tick = 0; $tick * $IntervalSeconds -le $DurationSeconds; $tick++) {
        Wait-Until $Process $countdown ($tick * $IntervalSeconds)
        $samples.Add((Read-Sample $Process $Scenario $countdown.Clock.Elapsed.TotalSeconds))
    }

    $engineSeconds = $countdown.Clock.Elapsed.TotalSeconds
    $engineTicksAtEnd = Read-GpuEngineTicks $Process.Id
    Complete-Countdown $countdown
    $gpuPercent = Get-BusiestEnginePercent $engineTicksAtStart $engineTicksAtEnd $engineSeconds

    [pscustomobject]@{
        Samples = $samples
        Summary = Get-ScenarioSummary $Scenario $samples $gpuPercent
    }
}

function New-Countdown([string] $Activity, [double] $Seconds) {
    [pscustomobject]@{
        Activity = $Activity
        Seconds  = $Seconds
        Clock    = [Diagnostics.Stopwatch]::StartNew()
    }
}

# Sleeps in short slices so the bar keeps moving and an exited Melodia is noticed promptly.
function Wait-Until($Process, $Countdown, [double] $Seconds) {
    while ($true) {
        Assert-Running $Process
        $remaining = $Seconds - $Countdown.Clock.Elapsed.TotalSeconds
        if ($remaining -le 0) { return }

        Write-Countdown $Countdown
        Start-Sleep -Duration ([TimeSpan]::FromSeconds([math]::Min($remaining, $ProgressStepSeconds)))
    }
}

function Write-Countdown($Countdown) {
    $elapsed = [math]::Min($Countdown.Clock.Elapsed.TotalSeconds, $Countdown.Seconds)
    $secondsLeft = [math]::Ceiling($Countdown.Seconds - $elapsed)
    Write-Progress -Activity $Countdown.Activity -Status "$secondsLeft s left" -PercentComplete ($elapsed / $Countdown.Seconds * 100)
}

function Complete-Countdown($Countdown) {
    Write-Progress -Activity $Countdown.Activity -Completed
}

function Write-ScenarioResult($Summary) {
    Write-Host ('  Commit {0}, private WS {1}, CPU {2} of one core ({3} of all cores), GPU {4}' -f (Format-MiB $Summary.Commit),
        (Format-MiB $Summary.PrivateWorkingSet), (Format-Percent $Summary.CpuOneCore), (Format-Percent $Summary.CpuAllCores),
        (Format-Percent $Summary.Gpu)) -ForegroundColor Green
}

function Read-Sample($Process, [string] $Scenario, [double] $Seconds) {
    $Process.Refresh()
    $memory = [MelodiaFootprint.Native]::ReadMemory($Process.Handle)
    $gpuMemory = [Diagnostics.PerformanceCounterCategory]::new('GPU Process Memory').ReadCategory()

    [pscustomobject]@{
        scenario            = $Scenario
        seconds             = [math]::Round($Seconds, 3)
        commit_bytes        = $memory.Commit
        peak_commit_bytes   = $memory.PeakCommit
        private_ws_bytes    = $memory.PrivateWorkingSet
        working_set_bytes   = $memory.WorkingSet
        gpu_dedicated_bytes = Get-CounterTotal $gpuMemory['Dedicated Usage'] $Process.Id
        gpu_shared_bytes    = Get-CounterTotal $gpuMemory['Shared Usage'] $Process.Id
        cpu_seconds         = $Process.TotalProcessorTime.TotalSeconds
        handles             = $Process.HandleCount
        threads             = $Process.Threads.Count
        gdi_objects         = [MelodiaFootprint.Native]::GdiObjects($Process.Handle)
        user_objects        = [MelodiaFootprint.Native]::UserObjects($Process.Handle)
    }
}

# GPU counter instances are named pid_<pid>_luid_<adapter>_..., one per adapter or engine.
function Select-ProcessInstances($Counter, [int] $Id) {
    $Counter.Values | Where-Object InstanceName -Like "pid_$($Id)_*"
}

function Get-CounterTotal($Counter, [int] $Id) {
    [long](Select-ProcessInstances $Counter $Id | Measure-Object -Property RawValue -Sum).Sum
}

function Read-GpuEngineTicks([int] $Id) {
    $runningTime = [Diagnostics.PerformanceCounterCategory]::new('GPU Engine').ReadCategory()['Running Time']
    $ticksByEngine = @{}
    foreach ($engine in Select-ProcessInstances $runningTime $Id) {
        $ticksByEngine[$engine.InstanceName] = $engine.RawValue
    }
    $ticksByEngine
}

# Task Manager charges a process its busiest engine rather than the sum, since 3D, copy and video
# engines are separate queues and adding them can pass 100%. An engine absent at the start was
# created inside the window, so its whole running time belongs to it.
function Get-BusiestEnginePercent([hashtable] $StartTicks, [hashtable] $EndTicks, [double] $Seconds) {
    $busiest = 0.0
    foreach ($engine in $EndTicks.Keys) {
        $ticks = $EndTicks[$engine] - ($StartTicks[$engine] ?? 0)
        $busiest = [math]::Max($busiest, $ticks / ($Seconds * $GpuTicksPerSecond) * 100)
    }
    $busiest
}

# Memory is the median sample, so one spike can't set a scenario's figure. CPU is the total over
# the window, where a median of per-interval rates would drop a periodic burst.
function Get-ScenarioSummary([string] $Scenario, $Samples, [double] $GpuPercent) {
    $first = $Samples[0]
    $last = $Samples[-1]
    $cpuPercent = ($last.cpu_seconds - $first.cpu_seconds) / ($last.seconds - $first.seconds) * 100

    [pscustomobject]@{
        Scenario          = $Scenario
        Commit            = Get-Median $Samples.commit_bytes
        PrivateWorkingSet = Get-Median $Samples.private_ws_bytes
        WorkingSet        = Get-Median $Samples.working_set_bytes
        GpuDedicated      = Get-Median $Samples.gpu_dedicated_bytes
        GpuShared         = Get-Median $Samples.gpu_shared_bytes
        CpuOneCore        = $cpuPercent
        CpuAllCores       = $cpuPercent / [Environment]::ProcessorCount
        Gpu               = $GpuPercent
        PeakCommit        = $last.peak_commit_bytes
        Handles           = $last.handles
        Threads           = $last.threads
        GdiObjects        = $last.gdi_objects
        UserObjects       = $last.user_objects
    }
}

function Get-Median([double[]] $Values) {
    $sorted = $Values.Clone()
    [Array]::Sort($sorted)
    $middle = [int][math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return $sorted[$middle] }
    ($sorted[$middle - 1] + $sorted[$middle]) / 2
}

function Save-Report($Process, [string] $Origin, $Measurements) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $Measurements | ForEach-Object { $_.Samples } | Export-Csv -Path (Join-Path $OutDir 'samples.csv')

    $report = Format-Report $Process $Origin $Measurements.Summary
    Set-Content -Path (Join-Path $OutDir 'summary.md') -Value $report
    Write-Host ''
    Write-Host $report
    Write-Host ''
    Write-Host "Written to $((Resolve-Path $OutDir).Path)"
}

function Format-Report($Process, [string] $Origin, $Summaries) {
    $binary = Get-Item -LiteralPath $Process.Path
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add("# Melodia footprint, $(Get-Date -Format 'yyyy-MM-dd HH:mm')")
    $lines.Add('')
    $lines.Add("- Binary: $($binary.FullName), version $($binary.VersionInfo.ProductVersion), built $($binary.LastWriteTime.ToString('yyyy-MM-dd HH:mm'))")
    $lines.Add("- Process: $Origin")
    foreach ($line in Get-SystemDescription $Process) { $lines.Add("- $line") }
    $lines.Add("- Method: $SettleSeconds s settle, $SwitchSettleSeconds s after each switch, $DurationSeconds s per scenario sampled every $IntervalSeconds s. Memory is the median sample, CPU and GPU the total over the window.")
    $lines.Add('')
    $lines.Add('| Scenario | Commit | Private WS | Working set | GPU dedicated | GPU shared | CPU (1 core) | CPU (all cores) | GPU |')
    $lines.Add('| --- | --- | --- | --- | --- | --- | --- | --- | --- |')
    foreach ($summary in $Summaries) {
        $lines.Add(('| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} |' -f $summary.Scenario,
                (Format-MiB $summary.Commit), (Format-MiB $summary.PrivateWorkingSet), (Format-MiB $summary.WorkingSet),
                (Format-MiB $summary.GpuDedicated), (Format-MiB $summary.GpuShared),
                (Format-Percent $summary.CpuOneCore), (Format-Percent $summary.CpuAllCores), (Format-Percent $summary.Gpu)))
    }
    $lines.Add('')
    $lines.Add('| Scenario | Peak commit | Handles | Threads | GDI objects | USER objects |')
    $lines.Add('| --- | --- | --- | --- | --- | --- |')
    foreach ($summary in $Summaries) {
        $lines.Add(('| {0} | {1} | {2} | {3} | {4} | {5} |' -f $summary.Scenario, (Format-MiB $summary.PeakCommit),
                $summary.Handles, $summary.Threads, $summary.GdiObjects, $summary.UserObjects))
    }
    $lines -join [Environment]::NewLine
}

function Get-SystemDescription($Process) {
    $os = Get-CimInstance Win32_OperatingSystem
    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $gpus = (Get-CimInstance Win32_VideoController).Name -join '; '

    "OS: $($os.Caption), build $($os.BuildNumber)"
    "CPU: $($cpu.Name.Trim()), $([Environment]::ProcessorCount) logical processors"
    "GPU: $gpus"
    "Display: $(Get-WindowDisplay $Process)"
}

# WMI reports one refresh rate per adapter, so on a multi-monitor desktop it can name a screen the
# window isn't on. Asking for the monitor under the window can't.
function Get-WindowDisplay($Process) {
    $Process.Refresh()
    if ($Process.MainWindowHandle -eq [IntPtr]::Zero) {
        return 'unknown, the window was hidden'
    }

    $mode = [MelodiaFootprint.Native]::DisplayOf($Process.MainWindowHandle)
    "$($mode.Width)x$($mode.Height) at $($mode.RefreshRate) Hz, the screen the window was on"
}

function Format-MiB([double] $Bytes) { '{0:0.0} MiB' -f ($Bytes / 1MB) }

function Format-Percent([double] $Value) { '{0:0.00}%' -f $Value }

if ($IntervalSeconds -gt $DurationSeconds) {
    throw '-IntervalSeconds must not exceed -DurationSeconds.'
}

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

namespace MelodiaFootprint
{
    public sealed class MemoryCounters
    {
        public ulong Commit;
        public ulong PeakCommit;
        public ulong WorkingSet;
        public ulong PrivateWorkingSet;
    }

    public sealed class DisplayMode
    {
        public int Width;
        public int Height;
        public int RefreshRate;
    }

    public static class Native
    {
        // PROCESS_MEMORY_COUNTERS_EX2 is the first revision carrying PrivateWorkingSetSize, so it
        // needs Windows 10 or 11 22H2 with the September 2023 update or later.
        [StructLayout(LayoutKind.Sequential)]
        private struct ProcessMemoryCountersEx2
        {
            public uint cb;
            public uint PageFaultCount;
            public UIntPtr PeakWorkingSetSize;
            public UIntPtr WorkingSetSize;
            public UIntPtr QuotaPeakPagedPoolUsage;
            public UIntPtr QuotaPagedPoolUsage;
            public UIntPtr QuotaPeakNonPagedPoolUsage;
            public UIntPtr QuotaNonPagedPoolUsage;
            public UIntPtr PagefileUsage;
            public UIntPtr PeakPagefileUsage;
            public UIntPtr PrivateUsage;
            public UIntPtr PrivateWorkingSetSize;
            public ulong SharedCommitUsage;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct Rect
        {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct MonitorInfoEx
        {
            public int cbSize;
            public Rect rcMonitor;
            public Rect rcWork;
            public uint dwFlags;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
            public string szDevice;
        }

        // DEVMODEW with the display arm of its union, cut off after the last field read here;
        // dmSize tells the API how much of it there is.
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct DevMode
        {
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
            public string dmDeviceName;
            public short dmSpecVersion;
            public short dmDriverVersion;
            public short dmSize;
            public short dmDriverExtra;
            public int dmFields;
            public int dmPositionX;
            public int dmPositionY;
            public int dmDisplayOrientation;
            public int dmDisplayFixedOutput;
            public short dmColor;
            public short dmDuplex;
            public short dmYResolution;
            public short dmTTOption;
            public short dmCollate;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
            public string dmFormName;
            public short dmLogPixels;
            public int dmBitsPerPel;
            public int dmPelsWidth;
            public int dmPelsHeight;
            public int dmDisplayFlags;
            public int dmDisplayFrequency;
        }

        private const uint GR_GDIOBJECTS = 0;
        private const uint GR_USEROBJECTS = 1;
        private const uint MONITOR_DEFAULTTONEAREST = 2;
        private const int ENUM_CURRENT_SETTINGS = -1;

        [DllImport("psapi.dll", SetLastError = true)]
        private static extern bool GetProcessMemoryInfo(IntPtr process, ref ProcessMemoryCountersEx2 counters, uint size);

        [DllImport("user32.dll")]
        private static extern uint GetGuiResources(IntPtr process, uint flags);

        [DllImport("user32.dll")]
        private static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfoEx info);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern bool EnumDisplaySettings(string device, int mode, ref DevMode devMode);

        public static MemoryCounters ReadMemory(IntPtr process)
        {
            var counters = new ProcessMemoryCountersEx2 { cb = (uint)Marshal.SizeOf<ProcessMemoryCountersEx2>() };
            if (!GetProcessMemoryInfo(process, ref counters, counters.cb))
            {
                throw new Win32Exception();
            }

            return new MemoryCounters
            {
                // PagefileUsage and PrivateUsage are the same commit charge under two names.
                Commit = counters.PrivateUsage.ToUInt64(),
                PeakCommit = counters.PeakPagefileUsage.ToUInt64(),
                WorkingSet = counters.WorkingSetSize.ToUInt64(),
                PrivateWorkingSet = counters.PrivateWorkingSetSize.ToUInt64(),
            };
        }

        public static uint GdiObjects(IntPtr process) { return GetGuiResources(process, GR_GDIOBJECTS); }

        public static uint UserObjects(IntPtr process) { return GetGuiResources(process, GR_USEROBJECTS); }

        public static DisplayMode DisplayOf(IntPtr window)
        {
            var info = new MonitorInfoEx { cbSize = Marshal.SizeOf<MonitorInfoEx>() };
            if (!GetMonitorInfo(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), ref info))
            {
                throw new InvalidOperationException("GetMonitorInfo found no monitor under the window.");
            }

            var mode = new DevMode { dmSize = (short)Marshal.SizeOf<DevMode>() };
            if (!EnumDisplaySettings(info.szDevice, ENUM_CURRENT_SETTINGS, ref mode))
            {
                throw new InvalidOperationException("EnumDisplaySettings failed for " + info.szDevice + ".");
            }

            return new DisplayMode { Width = mode.dmPelsWidth, Height = mode.dmPelsHeight, RefreshRate = mode.dmDisplayFrequency };
        }
    }
}
'@

# Export-Csv and -f follow the session culture, and a comma decimal splits a CSV column in two.
$sessionCulture = [cultureinfo]::CurrentCulture
[cultureinfo]::CurrentCulture = [cultureinfo]::InvariantCulture
try {
    if ($PSCmdlet.ParameterSetName -eq 'Attach') {
        $melodia = Get-Process -Id $ProcessId
        $origin = "attached to pid $ProcessId"
    }
    else {
        $melodia = Start-Melodia $Exe $DataDir
        $origin = if ($DataDir) { "launched with MELODIA_DATA_DIR=$DataDir" } else { 'launched against its default data root' }
    }
    Measure-Footprint $melodia $origin
}
finally {
    [cultureinfo]::CurrentCulture = $sessionCulture
}
