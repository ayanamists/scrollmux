#!/usr/bin/env bash

SCENARIO_NAME="ansi-burst"
SCENARIO_ROWS=40
SCENARIO_COLS=120

scenario_prepare() {
  local dir="$1"
  local cmd="$dir/run-ansi-burst"

  cat > "$cmd" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

for frame in $(seq 1 80); do
  printf '\033[2J\033[H'
  for row in $(seq 1 35); do
    color=$((31 + (row % 7)))
    col=$((1 + ((frame + row) % 20)))
    printf '\033[%d;%dH\033[%dmframe %03d row %02d  abcdefghijklmnopqrstuvwxyz  0123456789\033[0m' \
      "$row" "$col" "$color" "$frame" "$row"
  done
  sleep 0.005
done
sleep 0.1
EOF
  chmod +x "$cmd"
}

scenario_command() {
  printf '%s/run-ansi-burst\n' "$1"
}

scenario_drive_direct() {
  sleep 1
}

scenario_drive_scrollmux() {
  sleep 1.5
  printf '\033q'
}
