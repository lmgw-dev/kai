#!/usr/bin/env bash
# WP2's live check of Kai's lmgw link against a dev lmgw on 127.0.0.1:8897 (§15 WP2, §3.1–§3.3), over
# lmgw's device link (its design §3, §5, §7.2; scripts/live-device-link.sh):
#   1. pairing: the dev lmgw's device listener turned on (127.0.0.1:8898), a device minted for this
#      computer (key_create, local_only), its v2 link handed over (K13's handler path) and confirmed
#      by the live-test build (no dialog: the link is one this script minted and listed in
#      KAI_LIVE_LINK_ACCEPT; any other would get the owner's dialog); Kai enrols its own key, lmgw
#      answers 403 device_unconfirmed and the tray shows the code, the one lmgw's Devices card
#      shows; the owner's Confirm (device_confirm with that code); the feed's hello names the
#      device; the key in the wallet under lmgw's CA fingerprint, device-link.json written, Start
#      with Plasma on; Open in lmgw names the dashboard address the hello sent (no browser);
#   2. the icon follows the dev lmgw's GPU hold (the amber badge on and off), and the tooltip what
#      the device's admin tools may do (key_set self_admin: read only, full, capped by the
#      gateway's read_only until settings_set_full raises it, then off, which says nothing);
#   3. the dev lmgw stopping (grey, the backoff in the tooltip; a tray click says "lmgw is not
#      running" at once) and starting again (idle, connected); the row's URLs changed meanwhile
#      (key_set urls) reach device-link.json through the next hello;
#   4. disable, a new token that ends the key at once, and expiry each end the feed with its
#      revoked event, whose kind Kai reads (device_disabled, key_unknown, key_expired), and a
#      restarted Kai meets the 401 (device_disabled, device_key_unknown, key_expired), with no
#      retry. The ended key says "pair again" with only Pair, Settings…, Check for Updates… and Quit
#      in the menu, and Kai pairs again with the new link (enrolled, confirmed); a disable says
#      "enable it on Usage → Keys, or pair again" and an expiry "move its expiry on Usage → Keys, or
#      pair again", each with Try again, Pair and Quit, the key kept, and Try again connects with it
#      once the device is enabled or the expiry moved (review KAI-3 K3-6);
#   5. a re-pair that keeps the current key (device_token with other URLs): Kai enrols a new key,
#      which waits for its Confirm beside the kept one (the card shows both codes); confirmed, Kai
#      connects at the new URL and the wallet's one item for this lmgw holds the new key;
#   6. no key, token or link in Kai's logs (at KAI_LOG=trace, review KAI-2 K2-1); a clean quit.
# Usage: scripts/live-check-link.sh [lmgw checkout]   (default: ../lmgw)
#
# Isolation: Kai's config and state live under target/live-link (the owner's ~/.config/kai is not
# touched); Kai is the live-test build, built into target/live-test (never target/debug, the dev
# handler's binary) (KAI_LIVE_LINK=1: the links this script minted confirmed without a dialog,
# notifications logged instead of shown, no browser opened, no shortcuts bound) and cannot reach
# PipeWire (an empty PIPEWIRE_RUNTIME_DIR). Its own session bus and Secret Service
# (scripts/private-bus.sh, under target/live-link-bus): a Kai the session runs, the session's
# wallet and its tray are never touched; the only item there is the device key Kai keeps for the
# dev lmgw, deleted at the end with the keyring. The dev lmgw is the checkout's headless example,
# built beforehand (`cargo build -p lmgw-core --example headless`) and copied, run on its own data
# dir and container prefix (its device CA's key plain in that data dir, as a headless start keeps
# it); the hold is set on it only. Nothing here talks to 127.0.0.1:8001. No screen captures.
# Verdicts: target/live-link/verdicts.txt.
set -euo pipefail
self=$(realpath "$0")
cd "$(dirname "$0")/.."
# shellcheck source=scripts/private-bus.sh
source scripts/private-bus.sh "$PWD/target/live-link-bus" "$self" "$@"
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh
# shellcheck source=scripts/live-device-link.sh
source scripts/live-device-link.sh

LMGW=${1:-$PWD/../lmgw}
ADDR=127.0.0.1:8897
URL=http://$ADDR
# The device listener: loopback, the HTTP port plus one (lmgw's device link §4.1).
DEVICE_PORT=8898
DEV_URL2=https://localhost:$DEVICE_PORT
OUT=$PWD/target/live-link
DATA=$OUT/lmgw-data
KAI=$OUT/kai-live
HEADLESS=$OUT/headless
VERDICTS=$OUT/verdicts.txt
LMGW_PID=""
KAI_PID=""
SNI=""
DEV_FP=""
# What no log may hold: each minted token's hex and a line of each key Kai stored.
SECRETS=()

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
  [ -z "$DEV_FP" ] || secret-tool clear application dev.rootshell.kai lmgw "$DEV_FP" 2>/dev/null
  # The dev lmgw's log carries its owner login link; its data dir the device CA and the rows.
  rm -rf "$DATA" "$OUT/lmgw.log" "$OUT/config" "$OUT/state" "$OUT/no-pipewire" "$HEADLESS" "$KAI"
  private_bus_stop
}
trap 'cleanup; kgs_check || exit 1' EXIT

# ---------------------------------------------------------------- preconditions
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "a Kai already runs; quit it first"; fi
locked=$(busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/aliases/default \
  org.freedesktop.Secret.Collection Locked 2>/dev/null || echo "unknown")
[ "$locked" = "b false" ] || fail "the private keyring is locked or unknown ($locked)"
for port in 8897 "$DEVICE_PORT"; do
  if ss -ltnH "sport = :$port" | grep -q .; then fail "something listens on $port already"; fi
done
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
# The owner's requests, the bearer from a pipe, never in curl's argv.
bearer() { printf 'Authorization: Bearer %s\n' "$TOKEN"; }
op() {
  bearer | curl -sS --max-time 10 -X POST "$URL/api/op/$1" -H @- \
    -H 'Content-Type: application/json' -d "$2"
}
owner_get() { bearer | curl -sS --max-time 10 "$URL$1" -H @-; }
hold() { op hold_set "{\"active\": $1}" >/dev/null; }

devices_on
mint kai-live
ID=$DEV_ID
DEV_URL1=$DEV_URL
[ "$DEV_URL1" = "https://127.0.0.1:$DEVICE_PORT" ] || fail "this computer's link names $DEV_URL1, not the device listener on 127.0.0.1:$DEVICE_PORT"
[ -z "$(secret-tool lookup application dev.rootshell.kai lmgw "$DEV_FP" 2>/dev/null)" ] \
  || fail "an item for $DEV_FP exists already; not touching it"
LINK1=$DEV_LINK
SECRETS+=("$(token_hex "$LINK1")")
note "minted device id $ID ('kai-live') for this computer at $DEV_URL1; lmgw's device CA $DEV_FP"
# The links this script mints, which the live-test Kai confirms without a dialog. A Kai started
# later gets the list as it is then.
ACCEPT=$(accept_entry "$LINK1")

# ---------------------------------------------------------------- Kai
kai_config "$OUT/config/kai/config.toml" '[lmgw]\nretry_max_s = 4\n'
LINKFILE=$OUT/config/kai/device-link.json
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
stored_key() { secret-tool lookup application dev.rootshell.kai lmgw "$DEV_FP" 2>/dev/null || true; }
# The second line of a PEM: a run of the key that no log may hold.
pem_line() { sed -n 2p <<<"$1"; }
feed_connects() { grep -c 'lmgw link: connecting the feed' "$OUT/kai.log" || true; }
# A 401 on the key: the feed's, or the host link's (both open at Try again; the first 401 stops
# the other, so either one says the key was tried).
feed_401s() { grep -cE 'lmgw link: lmgw refused the feed|host: the pairing opens nothing' "$OUT/kai.log" || true; }
more_401s() { [ "$(feed_401s)" -gt "$1" ]; }
# The host link's own 401s: counted apart, so a host link that keeps trying a refused key shows.
host_401s() { grep -cF 'host: the pairing opens nothing' "$OUT/kai.log" || true; }
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
# Kai enrolled a link's key and waits for the owner's Confirm (lmgw's device link §3.5): the tray
# says so with the code, the one lmgw's Devices card shows for the waiting key; then the owner's
# Confirm with it, and the feed comes up at <url>. Sets CODE.
enrol_and_confirm() { # <what> <url>
  until_ 30 "$1: waiting for the owner's confirmation" tip_has "waiting for your confirmation on lmgw"
  CODE=$(code_in "$(tooltip)")
  [ -n "$CODE" ] || fail "$1: no code in the tooltip: $(tooltip)"
  icon_is kai-tray-offline || fail "$1: icon $(icon) while unconfirmed"
  until_ 10 "$1: lmgw's 403 device_unconfirmed" logged 'lmgw link: the device is not confirmed yet'
  local on_card
  on_card=$(card "$ID" .confirm_code)
  [ "$on_card" = "$CODE" ] || fail "$1: lmgw's Devices card shows '$on_card' where Kai shows $CODE"
  card "$ID" .client | grep -q '^Kai ' || fail "$1: the card names the client '$(card "$ID" .client)'"
  note "$1: Kai and lmgw's card show $CODE (client: $(card "$ID" .client)); the owner confirms it"
  confirm "$ID" "$CODE"
  until_ 30 "$1: confirmed and connected" connected "$2"
}
# A key that waits for the owner (review KAI-3 K3-6; lmgw 7d6d18c for the expiry): its state line,
# lmgw's words, grey, Try again, Pair, Settings…, Check for Updates… and Quit, no retry, and the key
# still in the wallet. Neither the feed nor the host link tries again during the 6 s wait.
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
  [ "$(stored_key)" = "$4" ] || fail "$1: the key left the wallet"
  note "$1: $(tooltip | sed 's/<br\/>/ | /g')"
}
DISABLED="this device was disabled on lmgw: enable it on Usage → Keys, or pair again"
EXPIRED="this device's key expired on lmgw: move its expiry on Usage → Keys, or pair again"
disabled() { waiting "$1" "$DISABLED" "$2" "$PEM1"; }
expired() { waiting "$1" "$EXPIRED" "$2" "$PEM2"; }
# The feed's revoked event was read by its kind (api §1.6), not its words.
kind_read() { # <kind> <what Kai makes of it>
  until_ 5 "the revoked event's kind $1" logged "lmgw link: revoked kind=$1"
  grep -F "lmgw link: revoked kind=$1" "$OUT/kai.log" | tail -1 | grep -qF "loss=Some($2)" \
    || fail "the revoked kind $1 is not read as $2: $(grep -F 'lmgw link: revoked' "$OUT/kai.log" | tail -1)"
}
# A revocation: "pair again" with the server's words, grey, only Pair, Settings…, Check for Updates…
# and Quit, and no retry.
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
pair "$LINK1"
enrol_and_confirm "pairing" "$DEV_URL1"
PEM1=$(stored_key)
[[ $PEM1 == "-----BEGIN PRIVATE KEY-----"* ]] || fail "the wallet holds no key for $DEV_FP"
SECRETS+=("$(pem_line "$PEM1")")
jq -e --arg u "$DEV_URL1" --arg fp "$DEV_FP" '.fp == $fp and .credentials.urls == [$u] and .name == "kai-live"' \
  "$LINKFILE" >/dev/null || fail "device-link.json does not hold the pairing: $(jq -c 'del(.credentials.certificate, .credentials.ca_certificate)' "$LINKFILE" 2>/dev/null || echo unreadable)"
[ -f "$OUT/config/autostart/dev.rootshell.kai.desktop" ] || fail "a first pairing did not switch Start with Plasma on"
logged 'live-test: the pairing link is confirmed without a dialog (the live script minted it)' || fail "not confirmed by the live-test path"
logged 'lmgw link: the feed is up device=kai-live' || fail "no hello in the log"
pass "pairing: the link confirmed, Kai's key enrolled, '$CODE' shown by Kai and lmgw's card until the owner confirmed it, the key in the wallet under $DEV_FP, device-link.json written, Start with Plasma on (in target/), the feed's hello names device 'kai-live'"
note "tooltip: $(tooltip)"
# Open in lmgw: the dashboard address the hello sent (this computer: the plain listener's).
click "Open in lmgw"
until_ 5 "Open in lmgw names the dashboard" logged "live test: no browser is opened url=$URL/chat"
pass "Open in lmgw: $(grep -o "no browser is opened url=[^ ]*" "$OUT/kai.log" | tail -1 | sed 's/.*url=//'), from the hello's dashboard_url"

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

# 3. The dev lmgw stops and starts; the row's URLs changed meanwhile reach Kai in the next hello.
op key_set "{\"id\": $ID, \"urls\": [\"$DEV_URL1\", \"$DEV_URL2\"]}" | jq -e '.ok == true' >/dev/null \
  || fail "key_set refused the row's URLs"
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
until_ 20 "lmgw back: connected" connected "$DEV_URL1"
pass "the icon follows the dev lmgw: grey with the backoff (1, 2, 4, 4 s) while it is stopped, a click says 'lmgw is not running', idle again once it is back"
jq -e --arg a "$DEV_URL1" --arg b "$DEV_URL2" '.credentials.urls == [$a, $b]' "$LINKFILE" >/dev/null \
  || fail "the hello's URLs did not reach device-link.json: $(jq -c .credentials.urls "$LINKFILE")"
logged "lmgw's hello: this device's URLs changed" || fail "no URL change in the log"
pass "the row's URLs (key_set urls) came with the next hello: device-link.json lists $DEV_URL1 and $DEV_URL2"
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
until_ 20 "Try again after enabling: connected" connected "$DEV_URL1"
[ "$(stored_key)" = "$PEM1" ] || fail "the wallet's key changed"
logged 'the owner asked to try the stored pairing again' || fail "no Try again in the log"
pass "disable: 'enable it on Usage → Keys, or pair again' from the revoked event (kind device_disabled) and from the 401, with Try again, Pair and Quit, no retry; Try again while disabled met the 401 again; enabled, Try again connected with the stored key"

# 4b. A new token that ends the key at once (device_token end_current, a stolen key's answer): the
# revoked event, then a 401 for the old key at a restart; Kai pairs again with the new link.
new_token "$ID" '"end_current": true'
LINK2=$DEV_LINK
SECRETS+=("$(token_hex "$LINK2")")
ACCEPT="$ACCEPT $(accept_entry "$LINK2")"
revoked "end the key (revoked event)" "was replaced"
kind_read key_unknown PairAgain
stop_kai || fail "Kai's quit failed"
start_kai
revoked "end the key (401 device_key_unknown at a restart)" "re-paired or deleted"
pair "$LINK2"
enrol_and_confirm "pairing again" "$DEV_URL1"
PEM2=$(stored_key)
[[ $PEM2 == "-----BEGIN PRIVATE KEY-----"* && $PEM2 != "$PEM1" ]] || fail "the wallet does not hold a new key"
SECRETS+=("$(pem_line "$PEM2")")
pass "a new token that ends the key: 'pair again' from the revoked event (kind key_unknown) and from the 401; paired again with the new link, a new key, confirmed with '$CODE'"

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
until_ 20 "Try again after moving the expiry: connected" connected "$DEV_URL1"
[ "$(stored_key)" = "$PEM2" ] || fail "the wallet's key changed"
pass "expiry: 'move its expiry on Usage → Keys, or pair again' from the revoked event (kind key_expired) and from the 401, with Try again, Pair and Quit, no retry; Try again while expired met the 401 again; the expiry moved, Try again connected with the stored key"

# 5. A re-pair that keeps the current key (device_token with other URLs, lmgw's device link §5.2):
# the new key waits for its Confirm beside the kept one, which the card shows too; confirmed, Kai
# connects at the new URL, and the wallet's one item for this lmgw holds the new key.
CODE2=$CODE
new_token "$ID" "\"urls\": [\"$DEV_URL2\"]"
LINK3=$DEV_LINK
SECRETS+=("$(token_hex "$LINK3")")
ACCEPT="$ACCEPT $(accept_entry "$LINK3")"
stop_kai || fail "Kai's quit failed"
start_kai
# The kept key works on, and its hello names the row's new URL.
until_ 20 "the kept key connects at the row's new URL" connected "$DEV_URL2"
pair "$LINK3"
until_ 30 "re-pair: waiting for the owner's confirmation" tip_has "waiting for your confirmation on lmgw"
kept=$(card "$ID" .kept_code)
[ "$kept" = "$CODE2" ] || fail "re-pair: the card's kept key is '$kept', not $CODE2"
enrol_and_confirm "re-pair" "$DEV_URL2"
PEM3=$(stored_key)
[[ $PEM3 == "-----BEGIN PRIVATE KEY-----"* && $PEM3 != "$PEM2" ]] || fail "the wallet does not hold the re-pair's key"
SECRETS+=("$(pem_line "$PEM3")")
items=$(secret-tool search --all application dev.rootshell.kai 2>&1 | grep -c '^\[' || true)
[ "$items" = 1 ] || fail "the wallet holds $items items of Kai's, not one"
jq -e --arg u "$DEV_URL2" '.credentials.urls == [$u]' "$LINKFILE" >/dev/null \
  || fail "device-link.json does not name $DEV_URL2: $(jq -c .credentials.urls "$LINKFILE")"
pass "re-pair keeping the key: the new key waited for its Confirm beside the kept one ($CODE2 kept, $CODE waiting on the card); confirmed, connected at $DEV_URL2, the one wallet item holds the new key"

# 6. No key, token or link in Kai's logs; a clean quit.
for secret in "${SECRETS[@]}"; do
  [ -n "$secret" ] || fail "an empty secret to look for"
  for f in "$OUT/kai.log" "$OUT/handler.log"; do
    if grep -qF -- "$secret" "$f"; then fail "a key or token is in $f"; fi
  done
done
for f in "$OUT/kai.log" "$OUT/handler.log"; do
  if grep -qi 'lmgw-pair:' "$f"; then fail "a link is in $f"; fi
done
stop_kai || fail "Kai exited with an error"
grep -q 'Kai has quit' "$OUT/kai.log" || fail "no clean quit in the log"
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "dev.rootshell.kai is still owned"; fi
pass "no key, token or link in Kai's logs; a clean quit"
echo "all passed; verdicts in $VERDICTS"
