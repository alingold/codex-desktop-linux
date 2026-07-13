#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
backend="${COMPUTER_USE_BIN:-$repo_dir/target/debug/codex-computer-use-linux}"
# Each readiness poll sleeps for 100 ms. Loaded CI runners have taken slightly
# over five seconds to start Metacity, so keep one explicit 15-second budget.
readiness_attempts=150

for command in metacity xdotool xmessage xdpyinfo node timeout zenity; do
    command -v "$command" >/dev/null || {
        echo "[x11-smoke] missing required command: $command" >&2
        exit 1
    }
done
test -x "$backend" || {
    echo "[x11-smoke] backend is not executable: $backend" >&2
    exit 1
}

display_number=""
for candidate in $(seq 97 119); do
    if [ ! -e "/tmp/.X11-unix/X$candidate" ]; then
        display_number="$candidate"
        break
    fi
done
test -n "$display_number" || {
    echo "[x11-smoke] no free nested X11 display number" >&2
    exit 1
}

nested_display=":$display_number"
work_dir="$(mktemp -d)"
backend_path="$work_dir/backend-bin"
x_pid=""
wm_pid=""
first_pid=""
second_pid=""
text_pid=""

cleanup() {
    kill "$text_pid" "$second_pid" "$first_pid" "$wm_pid" "$x_pid" 2>/dev/null || true
    rm -rf "$work_dir"
}
trap cleanup EXIT

if command -v Xvfb >/dev/null; then
    Xvfb "$nested_display" -screen 0 1024x768x24 -ac -nolisten tcp \
        >"$work_dir/xserver.log" 2>&1 &
elif command -v Xephyr >/dev/null && [ -n "${DISPLAY:-}" ]; then
    host_display="$DISPLAY"
    DISPLAY="$host_display" Xephyr "$nested_display" -screen 1024x768 -ac -nolisten tcp \
        >"$work_dir/xserver.log" 2>&1 &
else
    echo "[x11-smoke] Xvfb is unavailable and Xephyr has no host display" >&2
    exit 1
fi
x_pid=$!
export DISPLAY="$nested_display"

# Give the backend xdotool but deliberately no ydotool. When this user can
# open /dev/uinput, mask it read-only so this test cannot pass through the
# privileged absolute-pointer backend instead of the X11 fallback under test.
mkdir -p "$backend_path"
for command in busctl gdbus gsettings id pgrep ps sh systemctl xdotool xprop; do
    command_path="$(command -v "$command" 2>/dev/null || true)"
    [ -n "$command_path" ] && ln -s "$command_path" "$backend_path/$command"
done
backend_command=(
    env -u WAYLAND_DISPLAY
    "PATH=$backend_path"
    XDG_SESSION_TYPE=x11
    "YDOTOOL_SOCKET=$work_dir/no-ydotool.sock"
)
if { exec 9<>/dev/uinput; } 2>/dev/null; then
    exec 9>&-
    bwrap_path="$(command -v bwrap 2>/dev/null || true)"
    [ -n "$bwrap_path" ] || {
        echo "[x11-smoke] bwrap is required to mask accessible /dev/uinput" >&2
        exit 1
    }
    backend_command+=(
        "$bwrap_path" --dev-bind / / --ro-bind /dev/null /dev/uinput --
    )
fi

for _ in $(seq 1 "$readiness_attempts"); do
    xdpyinfo >/dev/null 2>&1 && break
    sleep 0.1
done
xdpyinfo >/dev/null 2>&1 || {
    sed -n '1,120p' "$work_dir/xserver.log" >&2
    echo "[x11-smoke] X server did not become ready" >&2
    exit 1
}

metacity --replace >"$work_dir/metacity.log" 2>&1 &
wm_pid=$!
for _ in $(seq 1 "$readiness_attempts"); do
    xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q '# 0x' && break
    sleep 0.1
done
xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q '# 0x' || {
    sed -n '1,120p' "$work_dir/metacity.log" >&2
    echo "[x11-smoke] Metacity did not publish EWMH readiness" >&2
    exit 1
}

xmessage -name CodexX11Primary -geometry 320x180+40+50 'primary fixture' \
    >"$work_dir/primary.log" 2>&1 &
first_pid=$!
xmessage -name CodexX11Secondary -geometry 240x140+500+300 'secondary fixture' \
    >"$work_dir/secondary.log" 2>&1 &
second_pid=$!

doctor_file="$work_dir/doctor.json"
"${backend_command[@]}" "$backend" doctor >"$doctor_file"
node -e '
  const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  if (report.input.uinput.ok || report.input.ydotool.ok || !report.input.xdotool.ok) {
    throw new Error(`input isolation failed: ${JSON.stringify(report.input)}`);
  }
  for (const name of ["click", "scroll", "drag", "draw_path"]) {
    const action = report.readiness.input_actions?.[name];
    if (!action?.ready || action.backend !== "xdotool_x11") {
      throw new Error(`${name} readiness mismatch: ${JSON.stringify(action)}`);
    }
  }
' "$doctor_file"

windows_file="$work_dir/windows.json"
for _ in $(seq 1 "$readiness_attempts"); do
    "${backend_command[@]}" "$backend" windows >"$windows_file" 2>/dev/null || true
    node -e '
      const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
      process.exit(["CodexX11Primary", "CodexX11Secondary"].every(
        id => report.windows.some(window => window.app_id === id)
      ) ? 0 : 1);
    ' "$windows_file" 2>/dev/null && break
    sleep 0.1
done

primary_id="$(node -e '
  const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const window = report.windows.find(window => window.app_id === "CodexX11Primary");
  if (!window) process.exit(1);
  process.stdout.write(String(window.window_id));
' "$windows_file")"

mcp_call() {
    local id="$1"
    local name="$2"
    local arguments="$3"
    printf '%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"x11-runtime-smoke","version":"1"}}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        "{\"jsonrpc\":\"2.0\",\"id\":$id,\"method\":\"tools/call\",\"params\":{\"name\":\"$name\",\"arguments\":$arguments}}" \
        | timeout 5 "${backend_command[@]}" "$backend" mcp >"$work_dir/mcp-$id.jsonl" || true
    node -e '
      const lines = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n");
      const id = Number(process.argv[2]);
      const response = lines.map(line => JSON.parse(line)).find(item => item.id === id);
      if (!response || response.error || response.result?.isError || response.result?.structuredContent?.ok !== true) {
        console.error(JSON.stringify(response));
        process.exit(1);
      }
    ' "$work_dir/mcp-$id.jsonl" "$id"
}

mcp_call_rejected() {
    local id="$1"
    local name="$2"
    local arguments="$3"
    local expected_message="$4"
    printf '%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"x11-runtime-smoke","version":"1"}}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        "{\"jsonrpc\":\"2.0\",\"id\":$id,\"method\":\"tools/call\",\"params\":{\"name\":\"$name\",\"arguments\":$arguments}}" \
        | timeout 5 "${backend_command[@]}" "$backend" mcp >"$work_dir/mcp-$id.jsonl" || true
    node -e '
      const lines = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n");
      const id = Number(process.argv[2]);
      const expected = process.argv[3];
      const response = lines.map(line => JSON.parse(line)).find(item => item.id === id);
      const output = response?.result?.structuredContent;
      if (!output || output.ok !== false || !String(output.message).includes(expected)) {
        console.error(JSON.stringify(response));
        process.exit(1);
      }
    ' "$work_dir/mcp-$id.jsonl" "$id" "$expected_message"
}

require_tool_property() {
    local tool_name="$1"
    local property_name="$2"
    printf '%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"x11-runtime-smoke","version":"1"}}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
        | timeout 5 "${backend_command[@]}" "$backend" mcp >"$work_dir/tools.jsonl" || true
    node -e '
      const lines = require("fs").readFileSync(process.argv[1], "utf8").trim().split("\n");
      const response = lines.map(line => JSON.parse(line)).find(item => item.id === 2);
      const tool = response?.result?.tools?.find(item => item.name === process.argv[2]);
      if (!tool?.inputSchema?.properties?.[process.argv[3]]) {
        console.error(`${process.argv[2]} is missing required schema property ${process.argv[3]}`);
        process.exit(1);
      }
    ' "$work_dir/tools.jsonl" "$tool_name" "$property_name"
}

mcp_call 2 activate_window "{\"window_id\":$primary_id}"
mcp_call 3 move_window "{\"window_id\":$primary_id,\"x\":120,\"y\":130}"
mcp_call 4 resize_window "{\"window_id\":$primary_id,\"width\":480,\"height\":240}"
require_tool_property draw_path relative
mcp_call_rejected 5 draw_path \
    "{\"window_id\":$primary_id,\"relative\":true,\"points\":[{\"x\":10,\"y\":10},{\"x\":9999,\"y\":10}]}" \
    "Every relative draw_path point"

"${backend_command[@]}" "$backend" windows >"$windows_file"
node -e '
  const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const id = Number(process.argv[2]);
  const window = report.windows.find(item => item.window_id === id);
  if (!window) throw new Error("primary fixture disappeared");
  if (!window.focused) throw new Error("primary fixture was not focused");
  if (window.bounds?.x !== 120) throw new Error(`unexpected x: ${window.bounds?.x}`);
  if (window.bounds?.width !== 480 || window.bounds?.height !== 240) {
    throw new Error(`unexpected size: ${JSON.stringify(window.bounds)}`);
  }
  if (report.windows.some(item => item.title === "Desktop")) {
    throw new Error("desktop surface leaked into application window list");
  }
' "$windows_file" "$primary_id"

path_end_x=180
path_end_y=80
read -r expected_cursor_x expected_cursor_y < <(node -e '
  const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const window = report.windows.find(item => item.window_id === Number(process.argv[2]));
  console.log(`${window.bounds.x + Number(process.argv[3])} ${window.bounds.y + Number(process.argv[4])}`);
' "$windows_file" "$primary_id" "$path_end_x" "$path_end_y")
mcp_call 6 draw_path \
    "{\"window_id\":$primary_id,\"relative\":true,\"point_delay_ms\":2,\"points\":[{\"x\":40,\"y\":40},{\"x\":100,\"y\":60},{\"x\":$path_end_x,\"y\":$path_end_y}]}"
cursor_x="$(xdotool getmouselocation --shell | sed -n 's/^X=//p')"
cursor_y="$(xdotool getmouselocation --shell | sed -n 's/^Y=//p')"
[ "$cursor_x" = "$expected_cursor_x" ] && [ "$cursor_y" = "$expected_cursor_y" ] || {
    echo "[x11-smoke] draw_path ended at $cursor_x,$cursor_y; expected $expected_cursor_x,$expected_cursor_y" >&2
    exit 1
}

unicode_text='Grüße 🌍 — こんにちは'
zenity --entry --title='Codex X11 Unicode Fixture' --text='Unicode runtime fixture' \
    >"$work_dir/unicode.txt" 2>"$work_dir/unicode.err" &
text_pid=$!
unicode_id=""
for _ in $(seq 1 "$readiness_attempts"); do
    "${backend_command[@]}" "$backend" windows >"$windows_file" 2>/dev/null || true
    unicode_id="$(node -e '
      const report = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
      const window = report.windows.find(item => item.title === "Codex X11 Unicode Fixture");
      if (window) process.stdout.write(String(window.window_id));
    ' "$windows_file")"
    [ -n "$unicode_id" ] && break
    sleep 0.1
done
[ -n "$unicode_id" ] || {
    cat "$work_dir/unicode.err" >&2
    echo "[x11-smoke] Unicode fixture was not listed" >&2
    exit 1
}

mcp_call 7 type_text "{\"window_id\":$unicode_id,\"text\":\"$unicode_text\"}"
mcp_call 8 press_key "{\"window_id\":$unicode_id,\"key\":\"Enter\"}"
for _ in $(seq 1 "$readiness_attempts"); do
    ! kill -0 "$text_pid" 2>/dev/null && break
    sleep 0.1
done
actual_unicode="$(tr -d '\r\n' <"$work_dir/unicode.txt")"
[ "$actual_unicode" = "$unicode_text" ] || {
    printf '[x11-smoke] Unicode mismatch: expected %q, got %q\n' "$unicode_text" "$actual_unicode" >&2
    exit 1
}
text_pid=""

echo "[x11-smoke] EWMH targeting, native draw_path, readiness, and Unicode input passed on $DISPLAY"
