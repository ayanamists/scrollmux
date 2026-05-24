#!/usr/bin/env bash

SCENARIO_NAME="nvim-scroll"
SCENARIO_ROWS=40
SCENARIO_COLS=120

scenario_prepare() {
  local dir="$1"
  local fixture="$dir/nvim-scroll.txt"
  local cmd="$dir/run-nvim-scroll"

  if ! type -P nvim >/dev/null; then
    echo "nvim not found; enter the Nix dev shell first: nix develop" >&2
    exit 2
  fi

  seq -f 'line %06g  abcdefghijklmnopqrstuvwxyz  0123456789  scrollmux nvim scroll sample' 1 5000 > "$fixture"

  {
    echo '#!/usr/bin/env bash'
    echo 'set -euo pipefail'
    printf 'exec nvim --clean %q\n' "$fixture"
  } > "$cmd"
  chmod +x "$cmd"
}

scenario_command() {
  printf '%s/run-nvim-scroll\n' "$1"
}

scenario_drive_direct() {
  sleep 1
  for _ in $(seq 1 120); do
    printf 'j'
    sleep 0.01
  done
  sleep 0.5
  printf ':qa!\r'
}

scenario_drive_scrollmux() {
  sleep 1
  for _ in $(seq 1 120); do
    printf 'j'
    sleep 0.01
  done
  sleep 0.5
  printf '\033q'
}
