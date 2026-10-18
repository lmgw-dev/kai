#!/usr/bin/env bash
# Live check of an unpaired Kai, click-free: the tray item shows "not paired" with the unpaired menu
# (Pair with lmgw…, Settings…, Check for Updates…, Quit); a second kai hands a test pairing link (the
# link v2 of lmgw's device link: an https URL, fp, a one-time token) to the running one, which
# answers at once and asks the owner in its confirmation dialog (never answered); nothing is sent
# to lmgw or stored (no key in the wallet under the link's fp, no device-link.json); neither the
# token nor the link reaches a log; then Kai quits cleanly. No lmgw: nothing is confirmed, so
# nothing is enrolled.
# Usage: scripts/live-check-pairing.sh
#
# Isolation: its own session bus and Secret Service (scripts/private-bus.sh, under
# target/live-pairing-bus): a Kai the session runs, the session's wallet and its tray are never
# touched, and an item {application: dev.rootshell.kai, lmgw: <the link's fp>} could exist only in
# that keyring. Kai gets no WAYLAND_DISPLAY or DISPLAY, so the confirmation dialog cannot open on
# the owner's desktop. Kai's config lives under target/live (the owner's ~/.config/kai is
# untouched). No screen captures, nothing installed.
set -euo pipefail
self=$(realpath "$0")
cd "$(dirname "$0")/.."
# shellcheck source=scripts/private-bus.sh
source scripts/private-bus.sh "$PWD/target/live-pairing-bus" "$self" "$@"
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh

URL=https://127.0.0.1:1
FP="sha256:$(printf 'deadbeef%.0s' 1 2 3 4 5 6 7 8)"
TOKENHEX=$(printf 'c0ffee42%.0s' 1 2 3 4)
LINK="lmgw-pair:?v=2&name=kai-live-check&url=https%3A%2F%2F127.0.0.1%3A1&fp=sha256%3A${FP#sha256:}&token=lmgw-enrol-$TOKENHEX"
LIVE=target/live
EXE=$PWD/target/debug/kai
KAI_PID=""

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

cleanup() {
  set +e
  if [ -n "$KAI_PID" ] && kill -0 "$KAI_PID" 2>/dev/null; then kill -TERM "$KAI_PID"; wait "$KAI_PID"; fi
  secret-tool clear application dev.rootshell.kai lmgw "$FP" 2>/dev/null
  rm -rf "$LIVE/config"
  private_bus_stop
}
trap 'cleanup; kgs_check || exit 1' EXIT

if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "a Kai already runs; quit it first"; fi
[ -z "$(secret-tool lookup application dev.rootshell.kai lmgw "$FP" 2>/dev/null)" ] \
  || fail "an item for $FP exists already; not touching it"

rm -rf "$LIVE"
mkdir -p "$LIVE"
export XDG_CONFIG_HOME=$PWD/$LIVE/config
export KAI_LOG=debug
kai_config "$XDG_CONFIG_HOME/kai/config.toml"

# 1. Kai on the private bus, without a display.
env -u WAYLAND_DISPLAY -u DISPLAY "$EXE" >"$LIVE/kai.log" 2>&1 &
KAI_PID=$!
gdbus wait --session --timeout 15 dev.rootshell.kai || fail "Kai did not claim dev.rootshell.kai"
sleep 2 # the tray item registers and the pairing is read
SNI=$(busctl --user list --no-legend | awk -v p="org.kde.StatusNotifierItem-$KAI_PID-" 'index($1, p) == 1 {print $1}')
[ -n "$SNI" ] || fail "no StatusNotifierItem for pid $KAI_PID"
prop() { busctl --user get-property "$SNI" /StatusNotifierItem org.kde.StatusNotifierItem "$1"; }
# The menu's labels joined by |; busctl prints non-ASCII as octal escapes, which %b decodes.
menu() {
  printf '%b' "$(busctl --user call "$SNI" /MenuBar com.canonical.dbusmenu GetLayout iias 0 10 0 \
    | grep -o '"label" s "[^"]*"' | cut -d'"' -f4 | paste -sd'|')"
}
# The tooltip's text (its last string; the icon pixmap before it is skipped).
tooltip() { printf '%b' "$(prop ToolTip | sed -E 's/.* "([^"]*)"$/\1/')"; }
echo "  item $SNI: Id $(prop Id); IconName $(prop IconName)"
echo "  tooltip: $(tooltip)"
echo "  menu: $(menu)"
prop IconName | grep -q "\"kai-tray-offline\"" || fail "not the grey icon"
prop ToolTip | grep -q '"not paired<br/>' || fail "the tooltip does not say not paired"
[ "$(menu)" = "Pair with lmgw…|Settings…|Check for Updates…|Quit" ] || fail "the unpaired menu is $(menu)"
pass "tray item: grey, 'not paired', menu Pair with lmgw…, Settings…, Check for Updates… and Quit"

# 2. A second kai with a test link hands it over (K13) and exits at once: the running Kai answers
#    without waiting for the owner, and asks the owner in its own dialog (it has no display to open on and is closed, unanswered, by the quit below).
env -u WAYLAND_DISPLAY -u DISPLAY "$EXE" "$LINK" >"$LIVE/handler.log" 2>&1 || fail "the second kai failed (see $LIVE/handler.log)"
grep -q "the running Kai took the pairing link: awaiting the owner's confirmation in Kai" "$LIVE/handler.log" \
  || fail "no hand-off answer"
grep -q 'a pairing link was handed over on D-Bus' "$LIVE/kai.log" || fail "the running Kai saw no hand-off"
grep -q 'a pairing link arrived; Kai asks the owner to confirm it' "$LIVE/kai.log" || fail "no confirmation asked"
pass "the running Kai received the link from the second kai and asks the owner"

# 3. The link never reaches a log.
for f in "$LIVE"/*.log; do
  if grep -q "$TOKENHEX" "$f"; then fail "the token is in $f"; fi
  if grep -qi 'lmgw-pair:' "$f"; then fail "the link is in $f"; fi
done
pass "neither the token nor the link is in kai.log or handler.log"

# 4. Nothing is sent or stored before the owner's answer; the tray says the link waits.
[ -z "$(secret-tool lookup application dev.rootshell.kai lmgw "$FP" 2>/dev/null)" ] || fail "a key stored without a confirmation"
[ ! -e "$XDG_CONFIG_HOME/kai/device-link.json" ] || fail "device-link.json written without a confirmation"
! grep -q 'pairing: enrolling' "$LIVE/kai.log" || fail "Kai enrolled without a confirmation"
[ ! -f "$XDG_CONFIG_HOME/autostart/dev.rootshell.kai.desktop" ] || fail "Start with Plasma switched on without a confirmation"
echo "  tooltip: $(tooltip)"
prop ToolTip | grep -qE "pairing link for $URL waits for (your answer|the wallet)" || fail "the tooltip does not say the link waits"
pass "nothing enrolled or stored (no key under the link's fp, no device-link.json), Start with Plasma off; the tooltip says the link waits"

# 5. Quit: the dialog goes unanswered, and still nothing is stored.
kill -TERM "$KAI_PID"
code=0
wait "$KAI_PID" || code=$?
KAI_PID=""
[ "$code" -eq 0 ] || fail "Kai exited with $code"
grep -q 'Kai has quit' "$LIVE/kai.log" || fail "no clean quit in the log"
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "dev.rootshell.kai is still owned"; fi
[ -z "$(secret-tool lookup application dev.rootshell.kai lmgw "$FP" 2>/dev/null)" ] || fail "stored at quit"
[ ! -e "$XDG_CONFIG_HOME/kai/device-link.json" ] || fail "device-link.json written at quit"
pass "clean quit: exit 0, name released, nothing stored"
