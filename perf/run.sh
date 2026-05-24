#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PERF_DIR="$ROOT/perf"
SCENARIO_DIR="$PERF_DIR/scenarios"
OUT_BASE="${OUT_BASE:-$ROOT/target/perf}"
SCROLLMUX_BIN="${SCROLLMUX_BIN:-$ROOT/target/release/scrollmux}"
BUILD=1
RUN_STRACE=0
SELECTED=()

usage() {
  cat <<'EOF'
Usage: perf/run.sh [OPTIONS]

Runs terminal-output perf scenarios under a real pseudo terminal.

Options:
  --scenario NAME   Run only one scenario. May be passed multiple times.
  --out DIR         Write results under DIR instead of target/perf/<timestamp>.
  --bin PATH        Use an existing scrollmux binary.
  --no-build        Do not run cargo build --release before measuring.
  --strace          Also run commands under strace -c when available.
  --list            Print available scenario names and exit.
  -h, --help        Show this help.

Environment:
  OUT_BASE          Base output directory. Default: target/perf
  SCROLLMUX_BIN     scrollmux binary path. Default: target/release/scrollmux
EOF
}

need() {
  if ! type -P "$1" >/dev/null; then
    echo "missing required command: $1" >&2
    echo "enter the Nix dev shell first: nix develop" >&2
    exit 2
  fi
}

time_bin() {
  local t
  t="$(type -P time || true)"
  if [[ -z "$t" ]]; then
    echo "missing required command: GNU time" >&2
    echo "enter the Nix dev shell first: nix develop" >&2
    exit 2
  fi
  printf '%s\n' "$t"
}

scenario_files() {
  find "$SCENARIO_DIR" -maxdepth 1 -type f -name '*.sh' | sort
}

scenario_name_from_file() {
  local file="$1"
  # shellcheck source=/dev/null
  source "$file"
  printf '%s\n' "$SCENARIO_NAME"
}

list_scenarios() {
  local file
  for file in $(scenario_files); do
    scenario_name_from_file "$file"
  done
}

selected_contains() {
  local name="$1"
  local selected
  if [[ "${#SELECTED[@]}" -eq 0 ]]; then
    return 0
  fi
  for selected in "${SELECTED[@]}"; do
    [[ "$selected" == "$name" ]] && return 0
  done
  return 1
}

quote() {
  printf '%q' "$1"
}

count_pcre() {
  local file="$1"
  local pattern="$2"
  { grep -aoP "$pattern" "$file" 2>/dev/null || true; } | wc -l | tr -d ' '
}

bytes_of() {
  wc -c < "$1" | tr -d ' '
}

extract_time_field() {
  local file="$1"
  local key="$2"
  awk -F: -v key="$key" '$1 == key { gsub(/^[ \t]+/, "", $2); print $2; found=1 } END { if (!found) print "" }' "$file"
}

write_metrics() {
  local scenario="$1"
  local target="$2"
  local exit_code="$3"
  local output_file="$4"
  local time_file="$5"
  local metrics_file="$6"

  local bytes clear_all clear_line cursor_moves sgr enter_alt leave_alt user_s system_s elapsed max_rss
  bytes="$(bytes_of "$output_file")"
  clear_all="$(count_pcre "$output_file" '\x1b\[2J')"
  clear_line="$(count_pcre "$output_file" '\x1b\[2K')"
  cursor_moves="$(count_pcre "$output_file" '\x1b\[[0-9;]*[Hf]')"
  sgr="$(count_pcre "$output_file" '\x1b\[[0-9;]*m')"
  enter_alt="$(count_pcre "$output_file" '\x1b\[\?1049h')"
  leave_alt="$(count_pcre "$output_file" '\x1b\[\?1049l')"
  user_s="$(extract_time_field "$time_file" "User time (seconds)")"
  system_s="$(extract_time_field "$time_file" "System time (seconds)")"
  elapsed="$(extract_time_field "$time_file" "Elapsed (wall clock) time (h:mm:ss or m:ss)")"
  max_rss="$(extract_time_field "$time_file" "Maximum resident set size (kbytes)")"

  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$scenario" "$target" "$exit_code" "$bytes" "$clear_all" "$clear_line" \
    "$cursor_moves" "$sgr" "$enter_alt" "$leave_alt" "$user_s" "$system_s" \
    "$elapsed" "$max_rss" >> "$metrics_file"
}

run_pty() {
  local mode="$1"
  local rows="$2"
  local cols="$3"
  local command_path="$4"
  local output_file="$5"
  local error_file="$6"
  local time_file="$7"
  local strace_file="$8"
  local driver="$9"

  local time_cmd script_cmd q_time q_command q_bin q_strace
  q_time="$(quote "$time_file")"
  q_command="$(quote "$command_path")"
  q_bin="$(quote "$SCROLLMUX_BIN")"

  if [[ "$mode" == "direct" ]]; then
    time_cmd="$(quote "$TIME_BIN") -v -o $q_time $q_command"
  else
    time_cmd="$(quote "$TIME_BIN") -v -o $q_time env SHELL=$q_command $q_bin"
  fi

  if [[ "$RUN_STRACE" == "1" ]]; then
    q_strace="$(quote "$strace_file")"
    if [[ "$mode" == "direct" ]]; then
      time_cmd="$(quote "$TIME_BIN") -v -o $q_time strace -qq -c -o $q_strace $q_command"
    else
      time_cmd="$(quote "$TIME_BIN") -v -o $q_time strace -qq -c -o $q_strace env SHELL=$q_command $q_bin"
    fi
  fi

  script_cmd="stty rows $rows cols $cols; $time_cmd"

  set +e
  set +o pipefail
  "$driver" | TERM=xterm-256color script -qfec "$script_cmd" /dev/null >"$output_file" 2>"$error_file"
  local exit_code=${PIPESTATUS[1]}
  set -o pipefail
  set -e
  return "$exit_code"
}

write_markdown_summary() {
  local summary_tsv="$1"
  local summary_md="$2"

  {
    echo "# ScrollMux Perf Summary"
    echo
    echo "Generated: $(date -Iseconds)"
    echo
    echo "| Scenario | Direct bytes | ScrollMux bytes | Amplification | Direct clears | ScrollMux clears | Direct SGR | ScrollMux SGR |"
    echo "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    awk -F '\t' '
      NR == 1 { next }
      {
        key=$1
        target=$2
        bytes[key,target]=$4
        clears[key,target]=$5
        sgr[key,target]=$8
        seen[key]=1
      }
      END {
        for (key in seen) {
          d=bytes[key,"direct"] + 0
          s=bytes[key,"scrollmux"] + 0
          amp=(d > 0 ? s / d : 0)
          printf("| %s | %d | %d | %.2fx | %d | %d | %d | %d |\n",
            key, d, s, amp,
            clears[key,"direct"] + 0,
            clears[key,"scrollmux"] + 0,
            sgr[key,"direct"] + 0,
            sgr[key,"scrollmux"] + 0)
        }
      }
    ' "$summary_tsv" | sort
  } > "$summary_md"
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --scenario)
      SELECTED+=("${2:?missing scenario name}")
      shift 2
      ;;
    --out)
      OUT_DIR="${2:?missing output directory}"
      shift 2
      ;;
    --bin)
      SCROLLMUX_BIN="${2:?missing binary path}"
      shift 2
      ;;
    --no-build)
      BUILD=0
      shift
      ;;
    --strace)
      RUN_STRACE=1
      shift
      ;;
    --list)
      list_scenarios
      exit 0
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

need grep
need script
need wc
TIME_BIN="$(time_bin)"
if [[ "$RUN_STRACE" == "1" ]]; then
  need strace
fi

if [[ "$BUILD" == "1" ]]; then
  need cargo
  (cd "$ROOT" && cargo build --release)
fi

if [[ ! -x "$SCROLLMUX_BIN" ]]; then
  echo "scrollmux binary not found or not executable: $SCROLLMUX_BIN" >&2
  exit 2
fi

OUT_DIR="${OUT_DIR:-$OUT_BASE/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$OUT_DIR"

SUMMARY_TSV="$OUT_DIR/summary.tsv"
SUMMARY_MD="$OUT_DIR/summary.md"
printf 'scenario\ttarget\texit\tbytes\tclear_all\tclear_line\tcursor_moves\tsgr\tenter_alt_screen\tleave_alt_screen\tuser_s\tsystem_s\telapsed\tmax_rss_kb\n' > "$SUMMARY_TSV"

ran=0
for file in $(scenario_files); do
  unset SCENARIO_NAME SCENARIO_ROWS SCENARIO_COLS
  unset -f scenario_prepare scenario_command scenario_drive_direct scenario_drive_scrollmux 2>/dev/null || true
  # shellcheck source=/dev/null
  source "$file"

  if ! selected_contains "$SCENARIO_NAME"; then
    continue
  fi

  ran=$((ran + 1))
  CASE_DIR="$OUT_DIR/$SCENARIO_NAME"
  mkdir -p "$CASE_DIR"
  scenario_prepare "$CASE_DIR"
  COMMAND_PATH="$(scenario_command "$CASE_DIR")"
  if [[ ! -x "$COMMAND_PATH" ]]; then
    echo "scenario $SCENARIO_NAME produced non-executable command: $COMMAND_PATH" >&2
    exit 2
  fi

  rows="${SCENARIO_ROWS:-40}"
  cols="${SCENARIO_COLS:-120}"

  echo "==> $SCENARIO_NAME direct"
  set +e
  run_pty direct "$rows" "$cols" "$COMMAND_PATH" \
    "$CASE_DIR/direct.out" "$CASE_DIR/direct.err" "$CASE_DIR/direct.time" \
    "$CASE_DIR/direct.strace" scenario_drive_direct
  direct_exit=$?
  set -e
  write_metrics "$SCENARIO_NAME" direct "$direct_exit" "$CASE_DIR/direct.out" "$CASE_DIR/direct.time" "$SUMMARY_TSV"

  echo "==> $SCENARIO_NAME scrollmux"
  set +e
  run_pty scrollmux "$rows" "$cols" "$COMMAND_PATH" \
    "$CASE_DIR/scrollmux.out" "$CASE_DIR/scrollmux.err" "$CASE_DIR/scrollmux.time" \
    "$CASE_DIR/scrollmux.strace" scenario_drive_scrollmux
  scrollmux_exit=$?
  set -e
  write_metrics "$SCENARIO_NAME" scrollmux "$scrollmux_exit" "$CASE_DIR/scrollmux.out" "$CASE_DIR/scrollmux.time" "$SUMMARY_TSV"
done

if [[ "$ran" -eq 0 ]]; then
  echo "no scenarios matched" >&2
  echo "available scenarios:" >&2
  list_scenarios >&2
  exit 2
fi

write_markdown_summary "$SUMMARY_TSV" "$SUMMARY_MD"

echo
echo "summary: $SUMMARY_MD"
cat "$SUMMARY_MD"
