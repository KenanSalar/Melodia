#!/usr/bin/env bash
# Measures Melodia's memory, CPU and GPU footprint on Linux the same way every run.
#
# Launches Melodia (or attaches to a running one), lets it settle, then samples one window per
# scenario while you switch between scenarios by hand. Every sample goes to samples.csv and the
# per-scenario figures to summary.md. Run with --help for the options.
#
# Anonymous is the headline figure: anonymous memory in RAM plus what of it sits in swap, zram
# included. Linux overcommits and keeps no commit charge per process, so this is the nearest it
# has. It doesn't fall when memory pressure pushes Melodia into swap, and nothing another process
# does moves it, so it tracks what Melodia allocated. Chromium reports the same sum as its private
# footprint on Linux.
#
# PSS is KDE System Monitor's Memory column. It is exact on a kernel that keeps per-page mapcounts,
# and the report says whether this one does, but it splits every shared page between the
# processes mapping it: start another OpenGL application and Melodia's PSS drops while Melodia
# does nothing. It is recorded because it is the number people check a README against, not
# because two runs can be compared on it.
#
# USS is the pages no other process maps, what exiting would give back, and System Monitor's
# Private column. RSS counts shared pages in full. All four come from smaps_rollup, which walks the
# page tables, rather than status, whose counters the kernel updates asynchronously.
#
# GPU memory is what the driver charges the process: framebuffer memory from nvidia-smi pmon on
# NVIDIA, whose DRM file descriptors publish no usage, and the drm-resident-* keys in fdinfo on
# drivers that implement DRM usage stats. Those are the sources System Monitor's GPU columns read.
# smaps sees none of it, so report it beside the memory columns, never summed into them.
#
# CPU is process CPU time over wall time, one full core being 100%. System Monitor's CPU column
# divides by the logical processor count, which is the "all cores" column. GPU is the busiest
# engine's running time over wall time. NVIDIA publishes only a per-process utilization in whole
# percents each second, so there it is the average of those.
#
# Threads, file descriptors and memory maps should hold flat from one scenario to the next. A climb
# is a leak.

set -euo pipefail
shopt -s nullglob

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
readonly REPO_ROOT="${SCRIPT_DIR%/*}"

readonly DEFAULT_SCENARIOS=('Idle' 'Playing, list view' 'Playing, visualizer live')
readonly DEFAULT_SETTLE_SECONDS=30
readonly DEFAULT_SWITCH_SETTLE_SECONDS=10
readonly DEFAULT_DURATION_SECONDS=60
readonly DEFAULT_INTERVAL_SECONDS=1
readonly MAX_WAIT_SECONDS=3600
readonly MIN_DURATION_SECONDS=1
readonly MIN_INTERVAL_SECONDS=0.1
readonly MAX_INTERVAL_SECONDS=60

readonly MICROSECONDS_PER_SECOND=1000000
readonly BYTES_PER_MIB=1048576
readonly PROGRESS_STEP_MICROSECONDS=250000
readonly PROGRESS_BAR_WIDTH=24
readonly FALLBACK_TERMINAL_COLUMNS=80
readonly VERSION_TIMEOUT_SECONDS=5
# console.warn from a KWin script reaches the journal a moment after the script runs.
readonly JOURNAL_POLL_ATTEMPTS=20
readonly JOURNAL_POLL_SECONDS=0.1

# In the order read_sample writes them.
readonly SAMPLE_COLUMNS=(
  seconds anonymous_bytes swap_bytes pss_bytes uss_bytes rss_bytes
  pss_anon_bytes pss_file_bytes pss_shmem_bytes peak_rss_bytes cpu_seconds threads memory_maps
  nvidia_framebuffer_bytes drm_resident_bytes file_descriptors
)
# shellcheck disable=SC2016 # awk source, whose fields awk expands
readonly SAMPLE_HEADER='FNR == 1 { for (i = 1; i <= NF; i++) column[$i] = i; next }'

# pmon names its columns in a header, and their order follows --select.
# shellcheck disable=SC2016 # awk source, whose fields awk expands
readonly PMON_HEADER='
  /^#/ {
    if ($0 ~ /[[:space:]]pid[[:space:]]/) {
      sub(/^#/, "")
      for (i = 1; i <= NF; i++) column[$i] = i
    }
    next
  }
  !("pid" in column) { next }
'

# awk and printf write decimals the locale's way, and a comma decimal splits a CSV column in two.
# Melodia is launched with the caller's own setting.
readonly INHERITED_LC_ALL="${LC_ALL-}"
readonly INHERITED_LC_ALL_IS_SET="${LC_ALL+set}"
export LC_ALL=C

CLOCK_TICKS="$(getconf CLK_TCK)"
LOGICAL_PROCESSORS="$(getconf _NPROCESSORS_ONLN)"
readonly CLOCK_TICKS LOGICAL_PROCESSORS

if [[ -t 1 ]]; then
  readonly COLOR_TITLE=$'\033[36m' COLOR_RESULT=$'\033[32m' COLOR_RESET=$'\033[0m'
else
  readonly COLOR_TITLE='' COLOR_RESULT='' COLOR_RESET=''
fi

exe=''
data_dir=''
attach_pid=''
scenarios=("${DEFAULT_SCENARIOS[@]}")
settle_seconds=$DEFAULT_SETTLE_SECONDS
switch_settle_seconds=$DEFAULT_SWITCH_SETTLE_SECONDS
duration_seconds=$DEFAULT_DURATION_SECONDS
interval_seconds=$DEFAULT_INTERVAL_SECONDS
interval_microseconds=0
out_dir=''

melodia_pid=''
launched_melodia=''
origin=''
work_dir=''
nvidia_log=''
nvidia_monitor_pid=''
countdown_activity=''
countdown_started=0
countdown_total=0
progress_enabled=''
progress_visible=''
terminal_columns=$FALLBACK_TERMINAL_COLUMNS
declare -A summary=()

main() {
  parse_arguments "$@"
  validate_arguments
  if [[ -t 2 ]]; then
    progress_enabled=yes
    read_terminal_columns
  fi

  work_dir="$(mktemp -d)"
  trap cleanup EXIT
  trap 'exit 130' INT TERM

  if [[ -n $attach_pid ]]; then
    attach_melodia
  else
    start_melodia
  fi
  measure_footprint
}

usage() {
  cat <<EOF
Usage: scripts/measure-linux-footprint.sh [options]

Measures Melodia's memory, CPU and GPU footprint on Linux. The comment at the top of the script
explains every column.

Options:
  --exe PATH                 Binary to launch. Defaults to the release build:
                             cargo build --release -p melodia
  --data-dir DIR             Data root to launch against, passed as MELODIA_DATA_DIR, so every run
                             sees the same library.
  --pid PID                  Measure an already running Melodia instead of launching one.
  --scenario NAME            One measurement window per name, in order; repeat it for more. You are
                             prompted before each. Defaults to "${DEFAULT_SCENARIOS[0]}", "${DEFAULT_SCENARIOS[1]}"
                             and "${DEFAULT_SCENARIOS[2]}".
  --settle-seconds N         Wait before the first scenario, so startup work stays out of every
                             window. Default $DEFAULT_SETTLE_SECONDS.
  --switch-settle-seconds N  Wait after each prompt, so the switch itself (a view being built,
                             covers decoding) stays out of the window. Default $DEFAULT_SWITCH_SETTLE_SECONDS.
  --duration-seconds N       Length of each scenario's window. Default $DEFAULT_DURATION_SECONDS.
  --interval-seconds N       Time between memory samples. Default $DEFAULT_INTERVAL_SECONDS.
  --out-dir DIR              Where samples.csv and summary.md go. Defaults to a timestamped folder
                             under target/linux-footprint.
  -h, --help                 Show this help.

Examples:
  scripts/measure-linux-footprint.sh
  scripts/measure-linux-footprint.sh --data-dir ~/melodia-bench --scenario Idle --scenario 'Playing, list view'
EOF
}

parse_arguments() {
  local custom_scenarios=()
  while (( $# > 0 )); do
    case $1 in
      -h|--help)
        usage
        exit 0
        ;;
      --exe) require_value "$@"; exe=$2 ;;
      --data-dir) require_value "$@"; data_dir=$2 ;;
      --pid) require_value "$@"; attach_pid=$2 ;;
      --scenario) require_value "$@"; custom_scenarios+=("$2") ;;
      --settle-seconds) require_value "$@"; settle_seconds=$2 ;;
      --switch-settle-seconds) require_value "$@"; switch_settle_seconds=$2 ;;
      --duration-seconds) require_value "$@"; duration_seconds=$2 ;;
      --interval-seconds) require_value "$@"; interval_seconds=$2 ;;
      --out-dir) require_value "$@"; out_dir=$2 ;;
      *) die "Unknown option $1. See --help." ;;
    esac
    shift 2
  done

  if (( ${#custom_scenarios[@]} > 0 )); then
    scenarios=("${custom_scenarios[@]}")
  fi
}

require_value() {
  if (( $# < 2 )); then
    die "$1 needs a value."
  fi
}

validate_arguments() {
  if [[ -n $attach_pid ]]; then
    if [[ -n $exe || -n $data_dir ]]; then
      die '--pid measures a running Melodia, so it takes neither --exe nor --data-dir.'
    fi
    if [[ ! $attach_pid =~ ^[1-9][0-9]*$ ]]; then
      die "--pid must be a process id, not $attach_pid."
    fi
  fi

  require_seconds --settle-seconds "$settle_seconds" 0 "$MAX_WAIT_SECONDS"
  require_seconds --switch-settle-seconds "$switch_settle_seconds" 0 "$MAX_WAIT_SECONDS"
  require_seconds --duration-seconds "$duration_seconds" "$MIN_DURATION_SECONDS" "$MAX_WAIT_SECONDS"
  require_seconds --interval-seconds "$interval_seconds" "$MIN_INTERVAL_SECONDS" "$MAX_INTERVAL_SECONDS"
  interval_microseconds="$(to_microseconds "$interval_seconds")"
  if (( interval_microseconds > $(to_microseconds "$duration_seconds") )); then
    die '--interval-seconds must not exceed --duration-seconds.'
  fi

  exe=${exe:-$REPO_ROOT/target/release/Melodia}
  out_dir=${out_dir:-$REPO_ROOT/target/linux-footprint/$(date +%Y%m%d-%H%M%S)}
}

require_seconds() {
  local option=$1 value=$2 min=$3 max=$4
  if [[ ! $value =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    die "$option must be a number of seconds, not $value."
  fi
  local value_microseconds min_microseconds max_microseconds
  value_microseconds="$(to_microseconds "$value")"
  min_microseconds="$(to_microseconds "$min")"
  max_microseconds="$(to_microseconds "$max")"
  if (( value_microseconds < min_microseconds || value_microseconds > max_microseconds )); then
    die "$option must be between $min and $max."
  fi
}

read_terminal_columns() {
  local columns=''
  if read -r _ columns < <(stty size <&2 2>/dev/null) && (( columns > 0 )); then
    terminal_columns=$columns
  fi
}

measure_footprint() {
  echo "Measuring Melodia (pid $melodia_pid), $origin."
  start_nvidia_monitor
  wait_countdown 'Waiting for Melodia to settle' "$settle_seconds"

  local index title
  for index in "${!scenarios[@]}"; do
    title="Scenario $(( index + 1 ))/${#scenarios[@]}: ${scenarios[index]}"
    read_confirmation "$title"
    wait_countdown "$title, settling after the switch" "$switch_settle_seconds"
    measure_scenario "$index" "$title"
    write_scenario_result "$index"
  done

  save_report
  echo "Melodia (pid $melodia_pid) is still running. Close it before the next run."
}

start_melodia() {
  if [[ ! -f $exe || ! -x $exe ]]; then
    die "No Melodia binary at $exe. Build one with: cargo build --release -p melodia"
  fi

  local environment=(-u LC_ALL)
  if [[ -n $INHERITED_LC_ALL_IS_SET ]]; then
    environment+=("LC_ALL=$INHERITED_LC_ALL")
  fi
  if [[ -n $data_dir ]]; then
    environment+=("MELODIA_DATA_DIR=$(realpath -m -- "$data_dir")")
  fi

  # A session of its own, so Ctrl-C here or closing the terminal leaves Melodia running. Its log
  # is mirrored to stderr, which would scroll the progress bar away.
  env "${environment[@]}" setsid "$exe" </dev/null >/dev/null 2>&1 &
  melodia_pid=$!
  launched_melodia=yes

  if [[ -n $data_dir ]]; then
    origin="launched with MELODIA_DATA_DIR=$data_dir"
  else
    origin='launched against its default data root'
  fi
}

attach_melodia() {
  if [[ ! -r /proc/$attach_pid/stat ]]; then
    die "No process with pid $attach_pid."
  fi
  melodia_pid=$attach_pid
  origin="attached to pid $attach_pid"
}

# A launched Melodia that exits stays a zombie until it is reaped, and a zombie still has a /proc
# entry.
assert_running() {
  local stat=''
  { read -r stat < "/proc/$melodia_pid/stat"; } 2>/dev/null || true
  local fields=${stat##*) }
  local state=${fields%% *}
  if [[ -n $stat && $state != Z && $state != X ]]; then
    return 0
  fi

  if [[ -z $launched_melodia ]]; then
    die "Melodia (pid $melodia_pid) exited."
  fi
  local code=0
  wait "$melodia_pid" || code=$?
  die "Melodia exited with code $code. Straight after a launch that means another instance holds this data root and the launch was forwarded to it: close that one, or measure it with --pid. Otherwise, '$exe --logs' prints where its log is."
}

wait_countdown() {
  start_countdown "$1" "$2"
  wait_until "$countdown_total"
  complete_progress
}

start_countdown() {
  countdown_activity=$1
  countdown_total="$(to_microseconds "$2")"
  read_clock countdown_started
}

# Sleeps in short slices so the bar keeps moving and an exited Melodia is noticed promptly.
wait_until() {
  local target=$1 now remaining step step_seconds
  while true; do
    assert_running
    read_clock now
    remaining=$(( target - (now - countdown_started) ))
    if (( remaining <= 0 )); then
      return 0
    fi

    write_progress $(( now - countdown_started ))
    step=$(( remaining < PROGRESS_STEP_MICROSECONDS ? remaining : PROGRESS_STEP_MICROSECONDS ))
    microseconds_to_seconds step_seconds "$step"
    sleep "$step_seconds"
  done
}

write_progress() {
  if [[ -z $progress_enabled ]]; then
    return 0
  fi
  local elapsed=$(( $1 < countdown_total ? $1 : countdown_total ))
  local filled=$(( elapsed * PROGRESS_BAR_WIDTH / countdown_total ))
  local seconds_left=$(( (countdown_total - elapsed + MICROSECONDS_PER_SECOND - 1) / MICROSECONDS_PER_SECOND ))
  local done_part remaining_part
  printf -v done_part '%*s' "$filled" ''
  printf -v remaining_part '%*s' $(( PROGRESS_BAR_WIDTH - filled )) ''
  local line="$countdown_activity [${done_part// /#}${remaining_part// /-}] $seconds_left s left"
  printf '\r\033[K%s' "${line:0:terminal_columns - 1}" >&2
  progress_visible=yes
}

complete_progress() {
  if [[ -n $progress_visible ]]; then
    printf '\r\033[K' >&2
    progress_visible=''
  fi
}

read_confirmation() {
  printf '\n%s%s%s\n' "$COLOR_TITLE" "$1" "$COLOR_RESET"
  echo '  Set Melodia up for it, keep its window visible and unminimized, then press Enter.'
  # A closed stdin reads end-of-input at once, which would sample a scenario nobody set up.
  if ! read -r; then
    die 'No input to confirm the scenario with. Run the script from an interactive terminal.'
  fi
}

measure_scenario() {
  local index=$1 title=$2
  local samples="$work_dir/samples-$index.tsv"
  local engines_at_start="$work_dir/engines-$index-start"
  local engines_at_end="$work_dir/engines-$index-end"
  join_by $'\t' "${SAMPLE_COLUMNS[@]}" > "$samples"

  read_drm_usage > "$engines_at_start"
  local nvidia_lines_at_start
  nvidia_lines_at_start="$(count_nvidia_log_lines)"
  start_countdown "$title, measuring" "$duration_seconds"

  local tick=0 now
  while (( tick * interval_microseconds <= countdown_total )); do
    wait_until $(( tick * interval_microseconds ))
    read_clock now
    read_sample $(( now - countdown_started )) >> "$samples"
    tick=$(( tick + 1 ))
  done

  read_clock now
  local window=$(( now - countdown_started ))
  read_drm_usage > "$engines_at_end"
  local nvidia_lines_at_end
  nvidia_lines_at_end="$(count_nvidia_log_lines)"
  complete_progress

  local drm_percent nvidia_percent
  drm_percent="$(drm_busiest_engine_percent "$engines_at_start" "$engines_at_end" "$window")"
  nvidia_percent="$(nvidia_utilization_percent "$nvidia_lines_at_start" "$nvidia_lines_at_end")"
  summarize_scenario "$samples" "$(larger_of "$drm_percent" "$nvidia_percent")" > "$work_dir/summary-$index.txt"
}

write_scenario_result() {
  load_summary "$1"
  printf '%s  Anonymous %s, PSS %s, CPU %s of one core (%s of all cores), GPU %s%s\n' "$COLOR_RESULT" \
    "$(format_mib "${summary[anonymous]}")" "$(format_mib "${summary[pss]}")" \
    "$(format_percent "${summary[cpu_one_core]}")" "$(format_percent "${summary[cpu_all_cores]}")" \
    "$(format_percent "${summary[gpu]}")" "$COLOR_RESET"
}

read_sample() {
  local seconds counters nvidia_framebuffer drm_resident
  microseconds_to_seconds seconds "$1"
  if ! counters="$(read_process_counters)"; then
    assert_running
    die "Could not read the memory counters under /proc/$melodia_pid."
  fi
  nvidia_framebuffer="$(read_nvidia_framebuffer)"
  drm_resident="$(read_drm_usage | awk '$1 == "memory" { print $2 }')"
  local descriptors=("/proc/$melodia_pid/fd"/*)
  printf '%s\t%s\t%s\t%s\t%s\n' "$seconds" "$counters" "$nvidia_framebuffer" "$drm_resident" "${#descriptors[@]}"
}

# Prints anonymous through memory_maps, in SAMPLE_COLUMNS order.
read_process_counters() {
  local proc="/proc/$melodia_pid"
  awk -v clock_ticks="$CLOCK_TICKS" -v rollup="$proc/smaps_rollup" -v status="$proc/status" \
      -v stat="$proc/stat" -v maps="$proc/maps" '
    function kib(amount) { return amount * 1024 }
    FILENAME == rollup && $3 == "kB" { memory[$1] = kib($2) }
    FILENAME == status && $1 == "VmHWM:" { peak_rss = kib($2) }
    FILENAME == status && $1 == "Threads:" { threads = $2 }
    # comm sits in parentheses and may itself hold spaces or parentheses.
    FILENAME == stat { sub(/^.*\) /, ""); cpu_ticks = $12 + $13 }
    FILENAME == maps { memory_maps++ }
    END {
      printf "%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.3f\t%d\t%d\n",
        memory["Anonymous:"] + memory["Swap:"], memory["Swap:"], memory["Pss:"],
        memory["Private_Clean:"] + memory["Private_Dirty:"] + memory["Private_Hugetlb:"], memory["Rss:"],
        memory["Pss_Anon:"], memory["Pss_File:"], memory["Pss_Shmem:"], peak_rss,
        cpu_ticks / clock_ticks, threads, memory_maps
    }
  ' "$proc/smaps_rollup" "$proc/status" "$proc/stat" "$proc/maps"
}

# Prints "memory <bytes>" and one "engine <driver/pdev/engine> <nanoseconds>" line per engine,
# nothing when the process holds no DRM client.
read_drm_usage() {
  local fdinfo=("/proc/$melodia_pid/fdinfo"/*)
  if (( ${#fdinfo[@]} == 0 )); then
    return 0
  fi
  # getline rather than awk's own file operands: a descriptor closed after the glob leaves no
  # fdinfo to open, which awk treats as fatal for an operand and as a -1 for getline.
  awk '
    function bytes(amount, unit) {
      if (unit == "KiB") return amount * 1024
      if (unit == "MiB") return amount * 1048576
      if (unit == "GiB") return amount * 1073741824
      return amount
    }
    # A dup-ed descriptor repeats the counters of the client behind it, so each client counts
    # once. Newer amdgpu prints drm-resident-* beside the legacy drm-memory-* it replaces.
    function read_client(path,    line, field, key, driver, pdev, id, resident, has_resident, legacy, engine, name) {
      while ((getline line < path) > 0) {
        split(line, field, /[ \t]+/)
        key = field[1]
        if (key == "drm-driver:") driver = field[2]
        else if (key == "drm-pdev:") pdev = field[2]
        else if (key == "drm-client-id:") id = field[2]
        else if (key ~ /^drm-resident-/) { resident += bytes(field[2], field[3]); has_resident = 1 }
        else if (key ~ /^drm-memory-/) legacy += bytes(field[2], field[3])
        else if (key ~ /^drm-engine-/ && key !~ /^drm-engine-capacity-/) engine[substr(key, 12, length(key) - 12)] = field[2]
      }
      close(path)
      if (driver == "" || (driver, pdev, id) in counted) return
      counted[driver, pdev, id] = 1
      clients++
      memory += has_resident ? resident : legacy
      for (name in engine) engine_time[driver "/" pdev "/" name] += engine[name]
    }
    BEGIN {
      for (operand = 1; operand < ARGC; operand++) read_client(ARGV[operand])
      if (clients == 0) exit
      printf "memory %.0f\n", memory
      for (name in engine_time) printf "engine %s %.0f\n", name, engine_time[name]
    }
  ' "${fdinfo[@]}"
}

# Engines are separate queues, and adding render, compute and video can pass 100%. An engine
# absent at the start was created inside the window, so its whole running time belongs to it.
drm_busiest_engine_percent() {
  awk -v window_microseconds="$3" '
    FILENAME == ARGV[1] && $1 == "engine" { at_start[$2] = $3; next }
    FILENAME == ARGV[2] && $1 == "engine" {
      measured = 1
      percent = ($3 - at_start[$2]) / (window_microseconds * 1000) * 100
      if (percent > busiest) busiest = percent
    }
    END { if (measured) printf "%.4f\n", busiest }
  ' "$1" "$2"
}

# Runs from the start, so a sample taken in a scenario's first second already has a row to read.
start_nvidia_monitor() {
  if ! command -v nvidia-smi >/dev/null; then
    return 0
  fi
  nvidia_log="$work_dir/nvidia-pmon.log"
  nvidia-smi pmon --select um --delay 1 --options T > "$nvidia_log" 2>&1 &
  nvidia_monitor_pid=$!
}

count_nvidia_log_lines() {
  if [[ -z $nvidia_log ]]; then
    echo 0
    return 0
  fi
  awk 'END { print NR }' "$nvidia_log"
}

# The latest second's rows, summed across GPUs.
read_nvidia_framebuffer() {
  if [[ -z $nvidia_log ]]; then
    return 0
  fi
  awk -v pid="$melodia_pid" "$PMON_HEADER"'
    $column["pid"] != pid { next }
    {
      if ($column["Time"] != latest_second) {
        latest_second = $column["Time"]
        framebuffer = 0
      }
      if ($column["fb"] ~ /^[0-9]+$/) framebuffer += $column["fb"] * 1048576
    }
    END { if (latest_second != "") printf "%.0f\n", framebuffer }
  ' "$nvidia_log"
}

# pmon prints "-" for a second without a utilization sample, which is an idle one. Seconds come
# from every process's rows, so a second with no Melodia row still counts, as an idle one.
nvidia_utilization_percent() {
  if [[ -z $nvidia_log ]]; then
    return 0
  fi
  awk -v pid="$melodia_pid" -v first_line="$1" -v last_line="$2" "$PMON_HEADER"'
    FNR <= first_line || FNR > last_line { next }
    { busiest_in_second[$column["Time"]] += 0 }
    $column["pid"] == pid {
      listed = 1
      if ($column["sm"] ~ /^[0-9]+$/ && $column["sm"] + 0 > busiest_in_second[$column["Time"]]) {
        busiest_in_second[$column["Time"]] = $column["sm"] + 0
      }
    }
    END {
      if (!listed) exit
      for (second in busiest_in_second) {
        seconds++
        total += busiest_in_second[second]
      }
      printf "%.4f\n", total / seconds
    }
  ' "$nvidia_log"
}

# Memory is the median sample, so one spike can't set a scenario's figure. CPU is the total over
# the window, where a median of per-interval rates would drop a periodic burst.
summarize_scenario() {
  local samples=$1 gpu_percent=$2
  local cpu_one_core
  cpu_one_core="$(cpu_percent_of "$samples")"

  echo "anonymous $(median_of "$samples" anonymous_bytes)"
  echo "pss $(median_of "$samples" pss_bytes)"
  echo "uss $(median_of "$samples" uss_bytes)"
  echo "rss $(median_of "$samples" rss_bytes)"
  echo "gpu_memory $(sum_of_present "$(median_of "$samples" nvidia_framebuffer_bytes)" "$(median_of "$samples" drm_resident_bytes)")"
  echo "cpu_one_core $cpu_one_core"
  echo "cpu_all_cores $(awk -v percent="$cpu_one_core" -v processors="$LOGICAL_PROCESSORS" 'BEGIN { printf "%.4f\n", percent / processors }')"
  echo "gpu $gpu_percent"
  echo "peak_rss $(last_of "$samples" peak_rss_bytes)"
  echo "swap $(median_of "$samples" swap_bytes)"
  echo "threads $(last_of "$samples" threads)"
  echo "file_descriptors $(last_of "$samples" file_descriptors)"
  echo "memory_maps $(last_of "$samples" memory_maps)"
}

load_summary() {
  summary=()
  local key value
  while read -r key value; do
    summary[$key]=$value
  done < "$work_dir/summary-$1.txt"
}

# Empty samples are a source that never reported, which leaves the median empty rather than zero.
median_of() {
  awk -F'\t' -v name="$2" "$SAMPLE_HEADER"' $column[name] != "" { print $column[name] }' "$1" \
    | sort -n \
    | awk '
        { values[NR] = $1 }
        END {
          if (NR == 0) exit
          middle = int((NR + 1) / 2)
          if (NR % 2 == 1) print values[middle]
          else printf "%.0f\n", (values[middle] + values[middle + 1]) / 2
        }
      '
}

last_of() {
  awk -F'\t' -v name="$2" "$SAMPLE_HEADER"' { last = $column[name] } END { print last }' "$1"
}

cpu_percent_of() {
  awk -F'\t' "$SAMPLE_HEADER"'
    FNR == 2 { first_seconds = $column["seconds"]; first_cpu = $column["cpu_seconds"] }
    { last_seconds = $column["seconds"]; last_cpu = $column["cpu_seconds"] }
    END { printf "%.4f\n", (last_cpu - first_cpu) / (last_seconds - first_seconds) * 100 }
  ' "$1"
}

sum_of_present() {
  awk -v first="$1" -v second="$2" 'BEGIN { if (first != "" || second != "") printf "%.0f\n", first + second }'
}

larger_of() {
  awk -v first="$1" -v second="$2" '
    BEGIN {
      if (first == "") print second
      else if (second == "" || first + 0 >= second + 0) print first
      else print second
    }'
}

save_report() {
  mkdir -p "$out_dir"
  write_samples_csv > "$out_dir/samples.csv"
  format_report > "$out_dir/summary.md"
  echo
  cat "$out_dir/summary.md"
  echo
  echo "Written to $(realpath "$out_dir")"
}

write_samples_csv() {
  join_by , scenario "${SAMPLE_COLUMNS[@]}"
  local index
  for index in "${!scenarios[@]}"; do
    scenario_name=${scenarios[index]} awk -F'\t' -v OFS=, '
      FNR == 1 { next }
      {
        name = ENVIRON["scenario_name"]
        gsub(/"/, "\"\"", name)
        $1 = $1
        print "\"" name "\"", $0
      }
    ' "$work_dir/samples-$index.tsv"
  done
}

format_report() {
  echo "# Melodia footprint, $(date '+%Y-%m-%d %H:%M')"
  echo
  echo "- Binary: $(describe_binary)"
  echo "- Process: $origin"
  echo "- OS: $(os_name)"
  echo "- Kernel: $(describe_kernel)"
  echo "- Session: $(describe_session)"
  echo "- CPU: $(cpu_model), $LOGICAL_PROCESSORS logical processors"
  echo "- GPU: $(describe_gpus)"
  echo "- Display: $(describe_display)"
  echo "- Method: $settle_seconds s settle, $switch_settle_seconds s after each switch, $duration_seconds s per scenario sampled every $interval_seconds s. Memory is the median sample, CPU and GPU the total over the window."
  echo
  echo '| Scenario | Anonymous | PSS | USS | RSS | GPU memory | CPU (1 core) | CPU (all cores) | GPU |'
  echo '| --- | --- | --- | --- | --- | --- | --- | --- | --- |'
  local index
  for index in "${!scenarios[@]}"; do
    load_summary "$index"
    printf '| %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' "${scenarios[index]}" \
      "$(format_mib "${summary[anonymous]}")" "$(format_mib "${summary[pss]}")" \
      "$(format_mib "${summary[uss]}")" "$(format_mib "${summary[rss]}")" "$(format_mib "${summary[gpu_memory]}")" \
      "$(format_percent "${summary[cpu_one_core]}")" "$(format_percent "${summary[cpu_all_cores]}")" \
      "$(format_percent "${summary[gpu]}")"
  done
  echo
  echo '| Scenario | Peak RSS | Swap | Threads | File descriptors | Memory maps |'
  echo '| --- | --- | --- | --- | --- | --- |'
  for index in "${!scenarios[@]}"; do
    load_summary "$index"
    printf '| %s | %s | %s | %s | %s | %s |\n' "${scenarios[index]}" \
      "$(format_mib "${summary[peak_rss]}")" "$(format_mib "${summary[swap]}")" \
      "${summary[threads]}" "${summary[file_descriptors]}" "${summary[memory_maps]}"
  done
}

# Read through /proc/<pid>/exe, which still opens the binary that is running after a rebuild has
# replaced it on disk, and then reads "(deleted)".
describe_binary() {
  local exe_path="/proc/$melodia_pid/exe"
  local version
  version="$(timeout "$VERSION_TIMEOUT_SECONDS" "$exe_path" --version 2>/dev/null)" || version='unknown'
  version=${version%%$'\n'*}
  echo "$(readlink "$exe_path"), version ${version#Melodia }, built $(date -r "$exe_path" '+%Y-%m-%d %H:%M')"
}

os_name() {
  awk '/^PRETTY_NAME=/ { sub(/^PRETTY_NAME=/, ""); gsub(/^"|"$/, ""); print }' /etc/os-release
}

# A kernel built with CONFIG_NO_PAGE_MAPCOUNT keeps no per-page mapcount for large folios, and
# smaps then estimates their PSS from an average and can file private pages under shared.
describe_kernel() {
  local release config
  release="$(uname -r)"
  config="$(kernel_config "$release")"
  case $config in
    '') echo "$release" ;;
    *$'\n'CONFIG_NO_PAGE_MAPCOUNT=y*) echo "$release, built without per-page mapcounts, so PSS and USS are estimates" ;;
    *) echo "$release, keeping per-page mapcounts, so PSS and USS are exact" ;;
  esac
}

kernel_config() {
  if [[ -r /boot/config-$1 ]]; then
    cat "/boot/config-$1"
  elif [[ -r /proc/config.gz ]]; then
    gzip -dc /proc/config.gz
  fi
}

describe_session() {
  local desktop=${XDG_CURRENT_DESKTOP:-unknown desktop}
  local session_type=${XDG_SESSION_TYPE:-unknown session}
  if [[ $desktop == KDE ]] && command -v plasmashell >/dev/null; then
    desktop="KDE Plasma $(plasmashell --version 2>/dev/null | awk 'NR == 1 { print $NF }')"
  fi
  case $session_type in
    wayland) session_type=Wayland ;;
    x11) session_type=X11 ;;
  esac
  echo "$desktop on $session_type"
}

cpu_model() {
  awk -F'\t*: ' '$1 == "model name" && model == "" { model = $2 } END { print (model == "" ? "unknown" : model) }' /proc/cpuinfo
}

describe_gpus() {
  local card device driver version
  local gpus=()
  for card in /sys/class/drm/card*; do
    # cardN-<connector> entries are outputs, and a firmware framebuffer has no PCI vendor.
    if [[ ! ${card##*/} =~ ^card[0-9]+$ || ! -r $card/device/vendor ]]; then
      continue
    fi
    device="$(readlink -f "$card/device")"
    driver=''
    version=''
    if [[ -L $device/driver ]]; then
      driver="$(basename "$(readlink "$device/driver")")"
    fi
    if [[ -r /sys/module/$driver/version ]]; then
      version=" $(< "/sys/module/$driver/version")"
    fi
    gpus+=("$(pci_device_name "${device##*/}") ($driver$version)")
  done

  if (( ${#gpus[@]} == 0 )); then
    echo 'unknown'
    return 0
  fi
  local joined
  printf -v joined '%s; ' "${gpus[@]}"
  echo "${joined%; }"
}

pci_device_name() {
  if ! command -v lspci >/dev/null; then
    echo "PCI device $1"
    return 0
  fi
  local description
  description="$(lspci -s "$1")"
  description=${description#*: }
  echo "${description% (rev *)}"
}

# Only KWin will say which output holds another client's window, and only to a script loaded into
# it, whose console.warn lands in the user journal.
describe_display() {
  if ! kwin_is_reachable; then
    echo 'unknown, the screen under the window is only found under KWin'
    return 0
  fi
  local output
  output="$(find_window_output)"
  if [[ -z $output ]]; then
    echo 'unknown, the window was hidden'
    return 0
  fi
  describe_output "$output"
}

kwin_is_reachable() {
  command -v gdbus >/dev/null && command -v journalctl >/dev/null || return 1
  local reply
  reply="$(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
    --method org.freedesktop.DBus.NameHasOwner org.kde.KWin 2>/dev/null)" || return 1
  [[ $reply == '(true,)' ]]
}

find_window_output() {
  local token="melodia-footprint-$$-$RANDOM"
  local script="$work_dir/$token.js"
  cat > "$script" <<EOF
let output = "";
for (const candidate of workspace.windowList()) {
    if (candidate.pid === $melodia_pid && candidate.output) {
        output = candidate.output.name;
        break;
    }
}
console.warn("$token", output);
EOF

  local since reply script_id
  since="$(date +%s)"
  reply="$(gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.loadScript "$script" "$token")" || return 0
  script_id=${reply#(}
  script_id=${script_id%,)}
  if [[ ! $script_id =~ ^[0-9]+$ ]]; then
    return 0
  fi

  if gdbus call --session --dest org.kde.KWin --object-path "/Scripting/Script$script_id" \
    --method org.kde.kwin.Script.run >/dev/null; then
    read_journal_report "$token" "$since"
  fi
  gdbus call --session --dest org.kde.KWin --object-path /Scripting \
    --method org.kde.kwin.Scripting.unloadScript "$token" >/dev/null || true
}

# Prints the word logged after the token, which is empty when the script found no window.
read_journal_report() {
  local token=$1 since=$2 attempt report
  for (( attempt = 0; attempt < JOURNAL_POLL_ATTEMPTS; attempt++ )); do
    report="$(journalctl --user --since "@$(( since - 1 ))" --output cat --no-pager 2>/dev/null \
      | awk -v token="$token" '
          !found { for (i = 1; i <= NF; i++) if ($i == token) { found = 1; print "reported", $(i + 1) } }
        ')"
    if [[ -n $report ]]; then
      echo "${report#reported }"
      return 0
    fi
    sleep "$JOURNAL_POLL_SECONDS"
  done
}

describe_output() {
  local output=$1
  if ! command -v kscreen-doctor >/dev/null || ! command -v jq >/dev/null; then
    echo "$output, its mode unknown without kscreen-doctor and jq"
    return 0
  fi
  local mode
  mode="$(kscreen-doctor --json 2>/dev/null | jq -r --arg name "$output" '
    .outputs[] | select(.name == $name) | . as $output
    | ($output.modes[] | select(.id == $output.currentModeId)) as $mode
    | "\($mode.size.width)x\($mode.size.height) at \($mode.refreshRate | round) Hz, scale \($output.scale)"
  ')"
  echo "${mode:-$output, its mode unknown}, the screen the window was on"
}

cleanup() {
  complete_progress
  if [[ -n $nvidia_monitor_pid ]]; then
    kill "$nvidia_monitor_pid" 2>/dev/null || true
    wait "$nvidia_monitor_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}

die() {
  complete_progress
  echo "ERROR: $1" >&2
  exit 1
}

join_by() {
  local IFS=$1
  shift
  echo "$*"
}

to_microseconds() {
  awk -v seconds="$1" -v scale="$MICROSECONDS_PER_SECOND" 'BEGIN { printf "%.0f\n", seconds * scale }'
}

# Sets the named variable, so the hot loops don't fork a subshell for it.
microseconds_to_seconds() {
  printf -v "$1" '%d.%06d' $(( $2 / MICROSECONDS_PER_SECOND )) $(( $2 % MICROSECONDS_PER_SECOND ))
}

read_clock() {
  printf -v "$1" '%s' "${EPOCHREALTIME/./}"
}

format_mib() {
  if [[ -z $1 ]]; then
    echo 'n/a'
    return 0
  fi
  local tenths=$(( ($1 * 10 + BYTES_PER_MIB / 2) / BYTES_PER_MIB ))
  printf '%d.%d MiB\n' $(( tenths / 10 )) $(( tenths % 10 ))
}

format_percent() {
  if [[ -z $1 ]]; then
    echo 'n/a'
    return 0
  fi
  printf '%.2f%%\n' "$1"
}

main "$@"
