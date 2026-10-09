#!/usr/bin/env bash
# WP2's live check of Kai's lmgw link against a dev lmgw on 127.0.0.1:8897 (§15 WP2, §3.1–§3.3):
#   1. pairing: a device key minted on the dev lmgw, handed over as a link (K13's handler path),
#      confirmed by the live-test build (no dialog: the link is one this script minted and listed in
#      KAI_LIVE_LINK_ACCEPT; any other would get the owner's dialog), stored; the feed's hello names
#      the device;
#   2. the icon follows the dev lmgw's GPU hold (the amber badge on and off), and the tooltip what
#      the device's admin tools may do (key_set self_admin: read only, full, capped by the
#      gateway's read_only until settings_set_full raises it, then off, which says nothing);
#   3. the dev lmgw stopping (grey, the backoff in the tooltip; a tray click says "lmgw is not
#      running" at once) and starting again (idle, connected);
#   4. disable, rotate and expiry each end the feed with its revoked event, whose kind Kai reads
#      (device_disabled, key_unknown, key_expired; lmgw 7d6d18c), and a restarted Kai meets the 401
#      (device_disabled, device_key_unknown, key_expired), with no retry. Rotate says "pair again"
#      with only Pair and Quit in the menu; a disable says "enable it on Usage → Keys, or pair
#      again" and an expiry "move its expiry on Usage → Keys, or pair again", each with Try again,
#      Pair and Quit, the key kept, and Try again connects with it once the device is enabled or
#      the expiry moved (review KAI-3 K3-6);
#   5. a confirmed re-pair to another URL deletes Kai's item for the old one;
#   6. no key and no link in Kai's logs (at KAI_LOG=trace, review KAI-2 K2-1); a clean quit.
# Usage: scripts/live-check-link.sh [lmgw checkout]   (default: ../lmgw)
#
# Isolation: Kai's config and state live under target/live-link (the owner's ~/.config/kai is not
# touched); Kai is the live-test build, built into target/live-test (never target/debug, the dev
# handler's binary) (KAI_LIVE_LINK=1: the links this script minted confirmed without a dialog,
# notifications logged instead of shown, no shortcuts bound) and cannot reach PipeWire (an empty
# PIPEWIRE_RUNTIME_DIR). Its own session bus and Secret Service (scripts/private-bus.sh, under
# target/live-link-bus): a Kai the session runs, the session's wallet and its tray are never
# touched; items there only for http://127.0.0.1:8897 and, for the re-pair, http://localhost:8897
# (the same dev lmgw), deleted at the end with the keyring. The dev lmgw is the checkout's
# headless example, built beforehand (`cargo build -p lmgw-core --example headless`) and copied, run
# on its own data dir and container prefix; the hold is set on it only. Nothing here talks to
# 127.0.0.1:8001. No screen captures. Verdicts: target/live-link/verdicts.txt.
set -euo pipefail
self=$(realpath "$0")
cd "$(dirname "$0")/.."
# shellcheck source=scripts/private-bus.sh
source scripts/private-bus.sh "$PWD/target/live-link-bus" "$self" "$@"
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh

LMGW=${1:-$PWD/../lmgw}
ADDR=127.0.0.1:8897
URL=http://$ADDR
URL2=http://localhost:8897
OUT=$PWD/target/live-link
DATA=$OUT/lmgw-data
KAI=$OUT/kai-live
HEADLESS=$OUT/headless
VERDICTS=$OUT/verdicts.txt
LMGW_PID=""
KAI_PID=""
SNI=""

fail() { echo "FAIL: $*" | tee -a "$VERDICTS" >&2; exit 1; }
pass() { echo "PASS: $*" | tee -a "$VERDICTS"; }
note() { echo "  $*" | tee -a "$VERDICTS"; }

stop_kai() {
  [ -n "$KAI_PID" ] || return 0
  kill -TERM "$KAI_PID" 2>/dev/null || true
  local code=0
  wait "$KAI_PID" || code=$?
  KAI_PID=""
  SNI=""
  return "$code"
}

stop_lmgw() {
  [ -n "$LMGW_PID" ] || return 0
  # Ctrl-C is the headless runner's quit (its graceful teardown); a hang is killed after 15 s.
  kill -INT "$LMGW_PID" 2>/dev/null || true
  local i
  for i in $(seq 1 75); do
    kill -0 "$LMGW_PID" 2>/dev/null || break
    sleep 0.2
  done
  if kill -0 "$LMGW_PID" 2>/dev/null; then
    note "the dev lmgw did not quit within 15 s of SIGINT; killed"
    kill -KILL "$LMGW_PID" 2>/dev/null || true
  fi
  wait "$LMGW_PID" 2>/dev/null || true
  LMGW_PID=""
}

cleanup() {
  set +e
  stop_kai >/dev/null 2>&1
  stop_lmgw >/dev/null 2>&1
  for u in "$URL" "$URL2"; do secret-tool clear application dev.rootshell.kai lmgw "$u" 2>/dev/null; done
  # The dev lmgw's log carries its owner login link; its data dir the minted keys.
  rm -rf "$DATA" "$OUT/lmgw.log" "$OUT/config" "$OUT/state" "$OUT/no-pipewire" "$HEADLESS" "$KAI"
  private_bus_stop
}
trap 'cleanup; kgs_check || exit 1' EXIT

# ---------------------------------------------------------------- preconditions
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "a Kai already runs; quit it first"; fi
locked=$(busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/aliases/default \
  org.freedesktop.Secret.Collection Locked 2>/dev/null || echo "unknown")
[ "$locked" = "b false" ] || fail "the wallet is locked or unknown ($locked): unlock KWallet first"
for u in "$URL" "$URL2"; do
  [ -z "$(secret-tool lookup application dev.rootshell.kai lmgw "$u" 2>/dev/null)" ] \
    || fail "an item for $u exists already; not touching it"
done
if ss -ltnH "sport = :8897" | grep -q .; then fail "something listens on 8897 already"; fi
[ -x "$LMGW/target/debug/examples/headless" ] \
  || fail "no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"

rm -rf "$OUT"
mkdir -p "$OUT/config/kai" "$OUT/state" "$OUT/no-pipewire" "$DATA"
: >"$VERDICTS"
note "lmgw: $(git -C "$LMGW" rev-parse --short HEAD) ($(git -C "$LMGW" status --short | wc -l) changed files); kai: $(git rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)"
# A checkout away from the pin is said, not refused (review KAI-3 K3-5).
scripts/lmgw-pin.sh check "$LMGW" 2>&1 | tee -a "$VERDICTS"

# The binaries this run uses, fixed for its length: the dev lmgw (a rebuild in the checkout cannot
# change it between a stop and a start) and Kai's live-test build, from its own target directory
# (review KAI-2 K2-2: target/debug/kai is never a live-test build).
cp --reflink=auto "$LMGW/target/debug/examples/headless" "$HEADLESS"
CARGO_BUILD_JOBS=8 cargo build --quiet --features live-test --target-dir target/live-test
cp --reflink=auto target/live-test/debug/kai "$KAI"

# ---------------------------------------------------------------- the dev lmgw
start_lmgw() {
  LMGW_DATA_DIR=$DATA LMGW_DEV=1 LMGW_CONTAINER_PREFIX=lmgw-kailive \
    "$HEADLESS" "$ADDR" >>"$OUT/lmgw.log" 2>&1 &
  LMGW_PID=$!
  local i
  for i in $(seq 1 300); do
    if curl -s -o /dev/null --max-time 1 "$URL/v1/models"; then return 0; fi
    kill -0 "$LMGW_PID" 2>/dev/null || fail "the dev lmgw exited (see $OUT/lmgw.log)"
    sleep 0.2
  done
  fail "the dev lmgw did not answer within 60 s"
}

start_lmgw
TOKEN=$(grep -o 'dashboard login: [^ ]*token=[^ ]*' "$OUT/lmgw.log" | tail -1 | sed 's/.*token=//')
[ -n "$TOKEN" ] || fail "no owner login line in the dev lmgw's log"
op() {
  curl -sS --max-time 10 -X POST "$URL/api/op/$1" -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' -d "$2"
}
hold() { op hold_set "{\"active\": $1}" >/dev/null; }

created=$(op key_create "{\"kind\": \"device\", \"name\": \"kai-live\", \"url\": \"$URL\"}")
echo "$created" | jq -e .key >/dev/null 2>&1 || fail "key_create answered: $(echo "$created" | jq -c 'del(.. | .key?, .link?)' 2>/dev/null || echo unreadable)"
ID=$(echo "$created" | jq -r .id)
KEY=$(echo "$created" | jq -r .key)
LINK=$(echo "$created" | jq -r .link)
KEYS=("$KEY")
note "minted device key id $ID ('kai-live'); the answer's fields: $(echo "$created" | jq -c 'keys')"
# The links this script mints, which the live-test Kai confirms without a dialog: <url>#<sha256 of
# the key>. A Kai started later gets the list as it is then.
accept_entry() { printf '%s#%s' "$1" "$(printf %s "$2" | sha256sum | cut -d' ' -f1)"; }
ACCEPT=$(accept_entry "$URL" "$KEY")

# ---------------------------------------------------------------- Kai
kai_config "$OUT/config/kai/config.toml" '[lmgw]\nretry_max_s = 4\n'
start_kai() {
  XDG_CONFIG_HOME=$OUT/config XDG_STATE_HOME=$OUT/state PIPEWIRE_RUNTIME_DIR=$OUT/no-pipewire \
    KAI_LIVE_LINK=1 KAI_LIVE_LINK_ACCEPT=$ACCEPT KAI_LOG=trace "$KAI" >>"$OUT/kai.log" 2>&1 &
  KAI_PID=$!
  gdbus wait --session --timeout 15 dev.rootshell.kai || fail "Kai did not claim dev.rootshell.kai"
  local i
  for i in $(seq 1 50); do
    SNI=$(busctl --user list --no-legend | awk -v p="org.kde.StatusNotifierItem-$KAI_PID-" 'index($1, p) == 1 {print $1}')
    [ -n "$SNI" ] && return 0
    sleep 0.2
  done
  fail "no StatusNotifierItem for pid $KAI_PID"
}
prop() { busctl --user get-property "$SNI" /StatusNotifierItem org.kde.StatusNotifierItem "$1"; }
icon() { prop IconName | cut -d'"' -f2; }
badge() { prop OverlayIconName | cut -d'"' -f2; }
# The tooltip's text (its last string; busctl escapes ' and non-ASCII, which this undoes).
tooltip() { printf '%b' "$(prop ToolTip | sed -E 's/.* "([^"]*)"$/\1/')" | sed "s/\\\\'/'/g"; }
menu() {
  printf '%b' "$(busctl --user call "$SNI" /MenuBar com.canonical.dbusmenu GetLayout iias 0 10 0 \
    | grep -o '"label" s "[^"]*"' | cut -d'"' -f4 | paste -sd'|')"
}
# until <seconds> <what> <command…>
until_() {
  local t=$1 what=$2
  shift 2
  local end=$((SECONDS + t))
  until "$@"; do
    ((SECONDS < end)) || fail "$what (waited $t s); icon $(icon), badge '$(badge)', tooltip: $(tooltip)"
    sleep 0.2
  done
}
tip_has() { tooltip | grep -qF -- "$1"; }
tip_lacks() { ! tooltip | grep -qF -- "$1"; }
icon_is() { [ "$(icon)" = "$1" ]; }
badge_is() { [ "$(badge)" = "$1" ]; }
logged() { grep -qF -- "$1" "$OUT/kai.log"; }
pair() { # the scheme handler's path: a second kai hands the link to the running one
  XDG_CONFIG_HOME=$OUT/config KAI_LIVE_LINK=1 "$KAI" "$1" >>"$OUT/handler.log" 2>&1 \
    || fail "the handler failed (see $OUT/handler.log)"
}
connected() { icon_is kai-tray-idle && tip_has "device 'kai-live' on $1"; }
stored_key() { secret-tool lookup application dev.rootshell.kai lmgw "$1" 2>/dev/null || true; }
feed_connects() { grep -c 'lmgw link: connecting the feed' "$OUT/kai.log" || true; }
# A 401 on the key: the feed's, or the host link's (both open at Try again; the first 401 stops
# the other, so either one says the key was tried).
feed_401s() { grep -cE 'lmgw link: the feed answered 401|host: the device key opens nothing \(401\)' "$OUT/kai.log" || true; }
more_401s() { [ "$(feed_401s)" -gt "$1" ]; }
# The host link's own 401s: counted apart, so a host link that keeps trying a refused key shows.
host_401s() { grep -cF 'host: the device key opens nothing (401)' "$OUT/kai.log" || true; }
# click <label>: a top-level item of the tray's menu, through dbusmenu.
click() {
  local id
  id=$(busctl --user --json=short -- call "$SNI" /MenuBar com.canonical.dbusmenu GetLayout iias 0 1 0 \
    | python3 -c 'import json, sys
for k in json.load(sys.stdin)["data"][1][2]:
    nid, props, _ = k["data"]
    if props.get("label", {}).get("data") == sys.argv[1]:
        print(nid)
        sys.exit(0)
sys.exit(1)' "$1") || fail "no menu item '$1': $(menu)"
  busctl --user -- call "$SNI" /MenuBar com.canonical.dbusmenu Event isvu "$id" clicked s "" 0 >/dev/null
}
# A key that waits for the owner (review KAI-3 K3-6; lmgw 7d6d18c for the expiry): its state line,
# lmgw's words, grey, Try again, Pair, Settings…, Check for Updates… and Quit, no retry, and the key still in
# the wallet.
# Neither the feed nor the host link tries again during the 6 s wait.
waiting() { # <what> <state line> <words in lmgw's message> <the stored key>
  until_ 20 "$1: $2" tip_has "$2"
  tip_has "$3" || fail "$1: lmgw's message lacks '$3': $(tooltip)"
  icon_is kai-tray-offline || fail "$1: icon $(icon)"
  [ "$(menu)" = "Try again|Pair with lmgw…|Settings…|Check for Updates…|Quit" ] || fail "$1: menu $(menu)"
  local before host_before
  before=$(feed_connects)
  host_before=$(host_401s)
  sleep 6
  [ "$(feed_connects)" = "$before" ] || fail "$1: the feed retried while waiting"
  [ "$(host_401s)" = "$host_before" ] || fail "$1: the host link retried while waiting ($host_before → $(host_401s) 401s)"
  [ "$(stored_key "$URL")" = "$4" ] || fail "$1: the key left the wallet"
  note "$1: $(tooltip | sed 's/<br\/>/ | /g')"
}
DISABLED="this device was disabled on lmgw: enable it on Usage → Keys, or pair again"
EXPIRED="this device's key expired on lmgw: move its expiry on Usage → Keys, or pair again"
disabled() { waiting "$1" "$DISABLED" "$2" "$KEY"; }
expired() { waiting "$1" "$EXPIRED" "$2" "$KEY2"; }
# The feed's revoked event was read by its kind (api §1.6), not its words.
kind_read() { # <kind> <what Kai makes of it>
  until_ 5 "the revoked event's kind $1" logged "lmgw link: revoked kind=$1"
  grep -F "lmgw link: revoked kind=$1" "$OUT/kai.log" | tail -1 | grep -qF "loss=Some($2)" \
    || fail "the revoked kind $1 is not read as $2: $(grep -F 'lmgw link: revoked' "$OUT/kai.log" | tail -1)"
}
# A revocation: "pair again" with the server's words, grey, only Pair, Settings…, Check for Updates… and Quit,
# and no retry.
revoked() { # <what> <words in the reason>
  until_ 20 "$1: pair again" tip_has "this device's key was revoked"
  tip_has "$2" || fail "$1: the reason lacks '$2': $(tooltip)"
  tip_has "pair again" || fail "$1: no 'pair again'"
  icon_is kai-tray-offline || fail "$1: icon $(icon)"
  [ "$(menu)" = "Pair with lmgw…|Settings…|Check for Updates…|Quit" ] || fail "$1: menu $(menu)"
  local before host_before
  before=$(feed_connects)
  host_before=$(host_401s)
  sleep 6
  [ "$(feed_connects)" = "$before" ] || fail "$1: the feed retried after the revocation"
  [ "$(host_401s)" = "$host_before" ] || fail "$1: the host link retried after the revocation ($host_before → $(host_401s) 401s)"
  note "$1: $(tooltip | sed 's/<br\/>.*//')"
}

start_kai
until_ 10 "unpaired at start" tip_has "not paired"
icon_is kai-tray-offline || fail "unpaired icon $(icon)"
pass "unpaired start: grey, 'not paired'"

# 1. Pairing.
pair "$LINK"
until_ 20 "paired and connected" connected "$URL"
[ "$(stored_key "$URL")" = "$KEY" ] || fail "the wallet holds no (or another) key for $URL"
grep -q "^url = \"$URL\"" "$OUT/config/kai/config.toml" || fail "lmgw.url not written"
[ -f "$OUT/config/autostart/dev.rootshell.kai.desktop" ] || fail "a first pairing did not switch Start with Plasma on"
logged 'live-test: the pairing link is confirmed without a dialog (the live script minted it)' || fail "not confirmed by the live-test path"
logged 'lmgw link: the feed is up device=kai-live' || fail "no hello in the log"
pass "pairing: confirmed, key stored for $URL, lmgw.url written, Start with Plasma on (in target/), the feed's hello names device 'kai-live'"
note "tooltip: $(tooltip)"

# 2. The hold.
hold true
until_ 10 "hold on: the amber badge" badge_is kai-tray-held
tip_has "GPU hold" || fail "the tooltip does not say the hold"
icon_is kai-tray-idle || fail "the hold changed the icon to $(icon)"
hold false
until_ 10 "hold off: no badge" badge_is ""
pass "the icon follows the dev hold: badge on, then off"

# 2b. What the device's admin tools may do: a level by name (off, read_only, full), said in the
# tooltip unless it is off; a change reaches Kai live as the feed's state. Since lmgw 3ac1155 the
# feed says the device's level capped by the gateway's own (a fresh dev lmgw's is read_only), so a
# device at full reads "read only" until the gateway's level is raised too.
tip_lacks "admin tools" || fail "an admin-tools line at off: $(tooltip)"
op key_set "{\"id\": $ID, \"self_admin\": \"read_only\"}" >/dev/null
until_ 10 "admin tools: read only" tip_has "admin tools: read only"
op key_set "{\"id\": $ID, \"self_admin\": \"full\"}" >/dev/null
sleep 2
tip_has "admin tools: read only" || fail "the device at full under the gateway's read_only: $(tooltip)"
op settings_set_full '{"self_admin": "full"}' >/dev/null
until_ 10 "admin tools: full" tip_has "admin tools: full"
grep -q 'lmgw link: the live state again.*admin_tools=full' "$OUT/kai.log" || fail "no state with the level in the log"
pass "what the admin tools may do follows the key and the gateway live: 'read only'; at full under the gateway's read_only still 'read only'; the gateway raised to full, 'full' in the tooltip"

# 3. The dev lmgw stops and starts.
stop_lmgw
until_ 15 "lmgw stopped: grey" icon_is kai-tray-offline
until_ 15 "the backoff in the tooltip" tip_has "next try in"
sleep 8
waits=$(grep -o 'next try in [0-9]* s' "$OUT/kai.log" | awk '{print $4}' | paste -sd' ')
note "the tries' waits so far (lmgw.retry_max_s = 4): $waits"
case "$waits" in *"1 2 4 4"*) ;; *) fail "the backoff is not 1, 2, 4, 4: $waits" ;; esac
note "tooltip: $(tooltip)"
t0=$(date +%s%N)
busctl --user call "$SNI" /StatusNotifierItem org.kde.StatusNotifierItem Activate ii 0 0
until_ 2 "a click says lmgw is not running" logged 'summary="Kai: lmgw is not running"'
note "said $(( ($(date +%s%N) - t0) / 1000000 )) ms after the click (within the 0.2 s poll)"
start_lmgw
until_ 20 "lmgw back: connected" connected "$URL"
pass "the icon follows the dev lmgw: grey with the backoff (1, 2, 4, 4 s) while it is stopped, a click says 'lmgw is not running', idle again once it is back"
# The new feed's hello says the level the key has (full, from 2b); off takes the line away.
tip_has "admin tools: full" || fail "the hello after the restart did not say the level: $(tooltip)"
op key_set "{\"id\": $ID, \"self_admin\": \"off\"}" >/dev/null
until_ 10 "admin tools off: no line" tip_lacks "admin tools"
pass "the admin-tools level from the hello after the restart ('full'), and no line once it is off"

# 4a. Disable: the revoked event, Try again while still disabled, then a 401 at a restart; enabled,
# Try again connects with the same key, no re-pair.
op key_set "{\"id\": $ID, \"enabled\": false}" >/dev/null
disabled "disable (revoked event)" "was disabled"
kind_read device_disabled Disabled
n401=$(feed_401s)
click "Try again"
until_ 20 "Try again while disabled: the key is tried" more_401s "$n401"
disabled "disable (Try again while still disabled: 401 device_disabled)" "is disabled"
stop_kai || fail "Kai's quit failed"
start_kai
disabled "disable (401 device_disabled at a restart)" "is disabled"
op key_set "{\"id\": $ID, \"enabled\": true}" >/dev/null
click "Try again"
until_ 20 "Try again after enabling: connected" connected "$URL"
[ "$(stored_key "$URL")" = "$KEY" ] || fail "the wallet's key changed"
logged 'the owner asked to try the stored device key again' || fail "no Try again in the log"
pass "disable: 'enable it on Usage → Keys, or pair again' from the revoked event (kind device_disabled) and from the 401, with Try again, Pair and Quit, no retry; Try again while disabled met the 401 again; enabled, Try again connected with the stored key"

# 4b. Rotate: the revoked event, then a 401 for the old key at a restart.
rotated=$(op key_rotate "{\"id\": $ID, \"url\": \"$URL\"}")
KEY2=$(echo "$rotated" | jq -r .key)
LINK2=$(echo "$rotated" | jq -r .link)
[ -n "$KEY2" ] && [ "$KEY2" != null ] || fail "key_rotate answered no key"
KEYS+=("$KEY2")
ACCEPT="$(accept_entry "$URL" "$KEY2") $(accept_entry "$URL2" "$KEY2")"
revoked "rotate (revoked event)" "rotated"
kind_read key_unknown PairAgain
stop_kai || fail "Kai's quit failed"
start_kai
revoked "rotate (401 device_key_unknown at a restart)" "rotated or deleted"
pair "$LINK2"
until_ 20 "paired with the rotated key" connected "$URL"
[ "$(stored_key "$URL")" = "$KEY2" ] || fail "the wallet does not hold the rotated key"
pass "rotate: 'pair again' from the revoked event (kind key_unknown) and from the 401; paired with the new link"

# 4c. Expiry (lmgw 7d6d18c: the expiry may be moved, so the key waits): the revoked event when it
# passes, Try again while still expired, then a 401 at a restart; the expiry moved, Try again
# connects with the same key, no re-pair.
expiry=$(date -u -d '+8 seconds' +%Y-%m-%dT%H:%M:%SZ)
op key_set "{\"id\": $ID, \"expires_at\": \"$expiry\"}" >/dev/null
expired "expiry (revoked event)" "expired"
kind_read key_expired Expired
n401=$(feed_401s)
click "Try again"
until_ 20 "Try again while expired: the key is tried" more_401s "$n401"
expired "expiry (Try again while still expired: 401 key_expired)" "expired on"
stop_kai || fail "Kai's quit failed"
start_kai
expired "expiry (401 key_expired at a restart)" "expired on"
op key_set "{\"id\": $ID, \"expires_at\": \"\"}" >/dev/null
click "Try again"
until_ 20 "Try again after moving the expiry: connected" connected "$URL"
[ "$(stored_key "$URL")" = "$KEY2" ] || fail "the wallet's key changed"
pass "expiry: 'move its expiry on Usage → Keys, or pair again' from the revoked event (kind key_expired) and from the 401, with Try again, Pair and Quit, no retry; Try again while expired met the 401 again; the expiry moved, Try again connected with the stored key"

# 5. A re-pair to another URL deletes Kai's item for the old one (the owner's decision).
pair "lmgw-pair:?v=1&url=http%3A%2F%2Flocalhost%3A8897&name=kai-live&key=$KEY2"
until_ 20 "re-paired at $URL2" connected "$URL2"
[ -z "$(stored_key "$URL")" ] || fail "the item for $URL stayed"
[ "$(stored_key "$URL2")" = "$KEY2" ] || fail "no item for $URL2"
logged 'the old gateway'"'"'s device key is deleted from the wallet' || fail "no deletion in the log"
pass "re-pair to $URL2: its key stored, Kai's item for $URL deleted"

# 6. No key or link in Kai's logs; a clean quit.
for k in "${KEYS[@]}"; do
  hex=${k#lmgw-device-}
  for f in "$OUT/kai.log" "$OUT/handler.log"; do
    if grep -q "$hex" "$f"; then fail "a key is in $f"; fi
  done
done
for f in "$OUT/kai.log" "$OUT/handler.log"; do
  if grep -qi 'lmgw-pair:' "$f"; then fail "a link is in $f"; fi
done
stop_kai || fail "Kai exited with an error"
grep -q 'Kai has quit' "$OUT/kai.log" || fail "no clean quit in the log"
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "dev.rootshell.kai is still owned"; fi
pass "no key and no link in Kai's logs; a clean quit"
echo "all passed; verdicts in $VERDICTS"
