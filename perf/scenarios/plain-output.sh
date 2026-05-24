#!/usr/bin/env bash

SCENARIO_NAME="plain-output"
SCENARIO_ROWS=40
SCENARIO_COLS=120

scenario_prepare() {
  local dir="$1"
  local fixture="$dir/plain-output.txt"
  local cmd="$dir/run-plain-output"

  seq -f 'line %06g  abcdefghijklmnopqrstuvwxyz  0123456789  scrollmux plain output sample' 1 5000 > "$fixture"

  {
    echo '#!/usr/bin/env bash'
    echo 'set -euo pipefail'
    printf 'cat %q\n' "$fixture"
    echo 'sleep 0.2'
  } > "$cmd"
  chmod +x "$cmd"
}

scenario_command() {
  printf '%s/run-plain-output\n' "$1"
}

scenario_drive_direct() {
  sleep 0.5
}

scenario_drive_scrollmux() {
  sleep 1
  printf '\033q'
}
