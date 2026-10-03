#!/usr/bin/env bash
# Run inside an X11 session (CI uses dbus-run-session and xvfb-run).
set -euo pipefail
binary=${1:-target/debug/kompas}
output=${2:-smoke-artifacts}
mkdir -p "$output"
binary=$(realpath "$binary")
output=$(realpath "$output")
# Installed binaries must work without access to the repository's resources.
runtime_dir=$(mktemp -d)
(cd "$runtime_dir"; exec env RUST_LOG=kompas=info "$binary") > "$output/startup.log" 2>&1 &
app_pid=$!
trap 'kill "$app_pid" 2>/dev/null || true; rm -rf "$runtime_dir"' EXIT
window_id=''
for attempt in $(seq 1 25); do
    if ! kill -0 "$app_pid" 2>/dev/null; then
        cat "$output/startup.log"
        exit 1
    fi
    window_id=$(xwininfo -root -tree | awk '/Kompas/ { print $1; exit }')
    if [ -n "$window_id" ]; then break; fi
    sleep 1
done
if [ -z "$window_id" ]; then
    cat "$output/startup.log"
    echo 'Kompas did not create an X11 window.' >&2
    exit 1
fi
# Give background storefront discovery time to populate the first rows.
for attempt in $(seq 1 75); do
    if grep -Eq 'local catalog ready: [1-9][0-9]* results' "$output/startup.log"; then break; fi
    kill -0 "$app_pid"
    sleep 1
done
grep -Eq 'local catalog ready: [1-9][0-9]* results' "$output/startup.log"
sleep 2
kill -0 "$app_pid"
import -window "$window_id" "$output/store-x11.png"
test -s "$output/store-x11.png"

# Browse the unified catalog, using the stable second navigation entry.
xdotool mousemove --window "$window_id" 90 112 click 1
for attempt in $(seq 1 60); do
    if grep -Eq 'searched for categories \[\].*found [1-9][0-9]* results' "$output/startup.log"; then break; fi
    kill -0 "$app_pid"
    sleep 1
done
grep -Eq 'searched for categories \[\].*found [1-9][0-9]* results' "$output/startup.log"
# Trigger a fresh draw after background results replace the previous page.
xdotool mousemove --window "$window_id" 500 220
sleep 2
kill -0 "$app_pid"
import -window "$window_id" "$output/all-apps-native.png"
test -s "$output/all-apps-native.png"

# Exercise actual keyboard search and its recoverable empty state.
xdotool windowfocus "$window_id"
xdotool key --window "$window_id" ctrl+f
xdotool type --window "$window_id" --delay 100 'gimp'
for attempt in $(seq 1 20); do
    if grep -Eq 'search "gimp" ready: [1-9][0-9]* results' "$output/startup.log"; then break; fi
    kill -0 "$app_pid"
    sleep 1
done
grep -Eq 'search "gimp" ready: [1-9][0-9]* results' "$output/startup.log"
sleep 2
import -window "$window_id" "$output/search-gimp.png"
# Open a real app and use the visible back button to restore the search.
xdotool mousemove --window "$window_id" 500 300 click 1
sleep 2
import -window "$window_id" "$output/app-details.png"
xdotool mousemove --window "$window_id" 76 24 click 1
sleep 2
grep -q 'back to catalog from details' "$output/startup.log"
import -window "$window_id" "$output/back-to-search.png"
xdotool key --window "$window_id" ctrl+f
xdotool key --window "$window_id" ctrl+a
xdotool type --window "$window_id" --delay 40 'kompas-no-such-app-987654321'
sleep 3
kill -0 "$app_pid"
import -window "$window_id" "$output/search-empty.png"
# Narrow window exercises the stacked filter layout.
xdotool windowsize "$window_id" 620 768
sleep 2
import -window "$window_id" "$output/narrow.png"

# Category browsing exposes metadata-driven subcategories without clearing filters.
xdotool windowsize "$window_id" 1024 768
sleep 2
xdotool windowfocus "$window_id"
xdotool key --window "$window_id" ctrl+f ctrl+a BackSpace
xdotool mousemove --window "$window_id" 90 312 click 1
for attempt in $(seq 1 15); do
    if grep -q 'searched for categories \[Game\]' "$output/startup.log"; then break; fi
    sleep 1
done
grep -q 'searched for categories \[Game\]' "$output/startup.log"
sleep 2
import -window "$window_id" "$output/game-subcategories.png"
kill -0 "$app_pid"

# Select the first subcategory and verify a real genre search occurs.
xdotool mousemove --window "$window_id" 430 290 click 1
sleep 1
import -window "$window_id" "$output/subcategory-menu.png"
xdotool mousemove --window "$window_id" 430 374 click 1
sleep 2
grep -q 'subcategory selected: 1' "$output/startup.log"
grep -q 'searched for categories \[ActionGame, Shooter\]' "$output/startup.log"
import -window "$window_id" "$output/action-games.png"
xdotool key --window "$window_id" alt+Left
sleep 2
grep -q 'subcategory selected: 0' "$output/startup.log"
import -window "$window_id" "$output/back-to-games.png"
kill -0 "$app_pid"

# Send the same WM_DELETE_WINDOW request as a desktop window manager.
WINDOW_ID="$window_id" python3 - <<'PY_CLOSE'
import os
from Xlib import X, display, protocol
connection = display.Display()
window = connection.create_resource_object('window', int(os.environ['WINDOW_ID'], 0))
window.send_event(protocol.event.ClientMessage(
    window=window,
    client_type=connection.intern_atom('WM_PROTOCOLS'),
    data=(32, [connection.intern_atom('WM_DELETE_WINDOW'), X.CurrentTime, 0, 0, 0]),
), event_mask=0)
connection.flush()
connection.close()
PY_CLOSE
for attempt in $(seq 1 10); do
    if ! kill -0 "$app_pid" 2>/dev/null; then break; fi
    sleep 1
done
if kill -0 "$app_pid" 2>/dev/null; then
    echo 'Kompas remained running after its main window was closed.' >&2
    exit 1
fi
wait "$app_pid"
echo 'Window-manager close request terminated Kompas.'
