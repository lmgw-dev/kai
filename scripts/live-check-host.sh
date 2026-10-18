#!/usr/bin/env bash
# WP8's live check (§15.1.4): Kai's hosted MCP server against a dev lmgw on 127.0.0.1:8897 with
# lmgw's host link (M1, api WP7) and the tool-calling mock (M7, scripts/mock-openai.py TOOL_SCRIPT):
#   1. the grant: a device minted for this computer with hosts_label "kailive" over lmgw's device
#      link (scripts/live-device-link.sh: the device listener on, key_create's v2 link), paired by
#      the handler's path (the live-test build confirms the link this script minted, no dialog),
#      Kai's enrolment, the owner's Confirm of the code Kai shows (device_confirm); the tooltip says
#      "tools: linked as kailive";
#   2. the device's row on lmgw's MCP page: device:kai-live, transport device, prefix kailive,
#      status ready; the link's limits Kai read from initialize's _meta["lmgw/host_limits"] are
#      lmgw's settings (64 MiB messages, 16 MiB frames, 20 s pings) and the row's timeout_ms;
#   3. /mcp lists kailive__live_echo for the owner and not for a client key whose tool scope is
#      deny (api §5.6);
#   4. a Chat turn on the mock model calls kailive__live_echo (the live-test build's own tool,
#      beside the desktop tools every build offers) through lmgw and Kai, and the reply echoes its
#      result;
#   5. the dev lmgw restarts with mcp.host_max_message_mb 0: the tooltip says the link is offline,
#      then linked again, and the new link's limits say no message bound (null), frames still
#      bounded;
#   6. no key, token and no tool argument in Kai's log (KAI_LOG=trace); a clean quit.
# Usage: scripts/live-check-host.sh [lmgw checkout]   (default: ../lmgw, with M1 and M7 in it)
#
# Isolation as scripts/live-check-link.sh: Kai's config and state under target/live-host, the
# live-test build from target/live-test (never target/debug), no PipeWire (an empty
# PIPEWIRE_RUNTIME_DIR), no global shortcuts, its own session bus and Secret Service
# (scripts/private-bus.sh, under target/live-host-bus): a Kai the session runs, the session's
# wallet and its tray are never touched. The dev lmgw is the checkout's headless example, built
# beforehand (`cargo build -p lmgw-core --example headless`) and copied, on its own data dir and
# container prefix; the mock chat on a free port. Nothing talks to 127.0.0.1:8001. No window is
# shown: the only tool called is read-only and its caller the owner, so K15 asks nothing.
# Verdicts: target/live-host/verdicts.txt.
set -euo pipefail
self=$(realpath "$0")
cd "$(dirname "$0")/.."
# shellcheck source=scripts/private-bus.sh
source scripts/private-bus.sh "$PWD/target/live-host-bus" "$self" "$@"
# shellcheck source=scripts/shortcuts-guard.sh
source scripts/shortcuts-guard.sh
# shellcheck source=scripts/live-device-link.sh
source scripts/live-device-link.sh

LMGW=$(realpath "${1:-../lmgw}")
ADDR=127.0.0.1:8897
URL=http://$ADDR
LABEL=kailive
ARG=live-check-host-$RANDOM$RANDOM
OUT=$PWD/target/live-host
DATA=$OUT/lmgw-data
KAI=$OUT/kai-live
HEADLESS=$OUT/headless
VERDICTS=$OUT/verdicts.txt
LMGW_PID=""
KAI_PID=""
MOCK_PID=""
SNI=""
DEV_FP=""

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
  if [ -n "$MOCK_PID" ]; then kill -TERM "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; fi
  [ -z "$DEV_FP" ] || secret-tool clear application dev.rootshell.kai lmgw "$DEV_FP" 2>/dev/null
  private_bus_stop
  left=$(podman ps -a --format '{{.Names}}' 2>/dev/null | grep '^lmgw-kaihost' || true)
  [[ -z $left ]] || echo "WARNING: dev containers left: $left"
  # The dev lmgw's log carries its owner login link; its data dir the device CA and the keys.
  rm -rf "$DATA" "$OUT/lmgw.log" "$OUT/config" "$OUT/state" "$OUT/no-pipewire" "$HEADLESS" "$KAI"
}
trap 'cleanup; kgs_check || exit 1' EXIT

# ---------------------------------------------------------------- preconditions
if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then fail "a Kai already runs; quit it first"; fi
locked=$(busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/aliases/default \
  org.freedesktop.Secret.Collection Locked 2>/dev/null || echo "unknown")
[ "$locked" = "b false" ] || fail "the private keyring is locked or unknown ($locked)"
# 8898: the device listener (loopback, the HTTP port plus one).
for port in 8897 8898; do
  if ss -ltnH "sport = :$port" | grep -q .; then fail "something listens on $port already"; fi
done
[ -x "$LMGW/target/debug/examples/headless" ] \
  || fail "no dev lmgw build: cargo build -p lmgw-core --example headless in $LMGW"
grep -q TOOL_SCRIPT "$LMGW/scripts/mock-openai.py" || fail "$LMGW's mock-openai.py has no TOOL_SCRIPT (M7)"
git -C "$LMGW" grep -q '"/mcp/host"' -- crates/lmgw-core/src || fail "$LMGW has no /mcp/host route (M1)"

rm -rf "$OUT"
mkdir -p "$OUT/config/kai" "$OUT/state" "$OUT/no-pipewire" "$DATA"
: >"$VERDICTS"
note "lmgw: $(git -C "$LMGW" rev-parse --short HEAD) ($(git -C "$LMGW" status --short | wc -l) changed files); kai: $(git rev-parse --short HEAD); LMGW_PIN $(scripts/lmgw-pin.sh commit | cut -c1-7)"
scripts/lmgw-pin.sh check "$LMGW" 2>&1 | tee -a "$VERDICTS"

cp --reflink=auto "$LMGW/target/debug/examples/headless" "$HEADLESS"
CARGO_BUILD_JOBS=8 cargo build --quiet --features live-test --target-dir target/live-test
cp --reflink=auto target/live-test/debug/kai "$KAI"

# ---------------------------------------------------------------- the mock and the dev lmgw
CHAT=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')
# One scripted turn: the model calls Kai's tool, then echoes its result (M7).
TOOL_SCRIPT="[[{\"name\": \"${LABEL}__live_echo\", \"arguments\": {\"text\": \"$ARG\"}}]]" \
  STREAM_DELAY=0 setsid python3 "$LMGW/scripts/mock-openai.py" "$CHAT" >"$OUT/mock-chat.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$CHAT/v1/models" && break; sleep 0.1; done

start_lmgw() {
  LMGW_DATA_DIR=$DATA LMGW_DEV=1 LMGW_CONTAINER_PREFIX=lmgw-kaihost \
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
# Requests with a bearer from a pipe, never in curl's argv.
bearer() { printf 'Authorization: Bearer %s\n' "$1"; }
op() {
  bearer "$TOKEN" | curl -sS --max-time 10 -X POST "$URL/api/op/$1" -H @- \
    -H 'Content-Type: application/json' -d "$2"
}
owner_get() { bearer "$TOKEN" | curl -sS --max-time 10 "$URL$1" -H @-; }
op upstream_set "{\"action\": \"create\", \"name\": \"mock-chat\", \"protocol\": \"llama_cpp\", \"base_url\": \"http://127.0.0.1:$CHAT/v1\"}" >/dev/null
op model_set '{"action": "create", "alias": "kai-chat", "upstream": "mock-chat", "upstream_model": "triage"}' >/dev/null

devices_on
mint kai-live "\"hosts_label\": \"$LABEL\""
LINK=$DEV_LINK
TOKENHEX=$(token_hex "$LINK")
denied=$(op key_create '{"kind": "client", "name": "kailive-deny", "tool_scope_mode": "deny"}')
# A client key answers its secret once as `key` (a device's pairing is its link).
DKEY=$(echo "$denied" | jq -r .key)
[ -n "$DKEY" ] && [ "$DKEY" != null ] || fail "the deny key was not minted: $(echo "$denied" | jq -c 'del(.key)')"
ACCEPT=$(accept_entry "$LINK")

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
tooltip() { printf '%b' "$(prop ToolTip | sed -E 's/.* "([^"]*)"$/\1/')" | sed "s/\\\\'/'/g"; }
until_() {
  local t=$1 what=$2
  shift 2
  local end=$((SECONDS + t))
  until "$@"; do
    ((SECONDS < end)) || fail "$what (waited $t s); tooltip: $(tooltip)"
    sleep 0.2
  done
}
tip_has() { tooltip | grep -qF -- "$1"; }
logged() { grep -qF -- "$1" "$OUT/kai.log"; }

start_kai
XDG_CONFIG_HOME=$OUT/config KAI_LIVE_LINK=1 "$KAI" "$LINK" >>"$OUT/handler.log" 2>&1 \
  || fail "the handler failed (see $OUT/handler.log)"
# Kai enrolled its key; the owner confirms the code it shows (lmgw's device link §3.5).
until_ 30 "waiting for the owner's confirmation" tip_has "waiting for your confirmation on lmgw"
CODE=$(code_in "$(tooltip)")
[ -n "$CODE" ] || fail "no code in the tooltip: $(tooltip)"
confirm "$DEV_ID" "$CODE"
note "Kai enrolled its key; the owner confirmed $CODE"

# 1. The grant.
until_ 30 "the tools linked as $LABEL" tip_has "tools: linked as $LABEL ("
pass "the grant: paired with hosts_label '$LABEL', the tooltip says '$(tooltip | grep -o "tools: linked as $LABEL ([0-9]* tools\\?)")'"

# 2. The row on the MCP page.
# The grant's row (M1 as built): named after the device key, device:<name>, the label its prefix;
# the MCP page's list is GET /api/mcp-servers (no op).
row() {
  bearer "$TOKEN" | curl -sS --max-time 10 "$URL/api/mcp-servers" -H @- | jq -c --arg l "$LABEL" \
    '.mcp_servers[] | select(.transport == "device" and .name == "device:kai-live" and .tool_prefix == $l)'
}
row_ready() { row | jq -e 'select(.status == "ready")' >/dev/null; }
until_ 20 "the device row ready on the MCP page" row_ready
ROW_TIMEOUT=$(row | jq -r .timeout_ms)
pass "the device's row on the MCP page: device:kai-live, transport device, prefix $LABEL, ready (timeout_ms $ROW_TIMEOUT)"

# The limits Kai read from initialize's _meta["lmgw/host_limits"]: lmgw's defaults and the row's
# timeout.
announced() { grep -F "host: lmgw announced the link's limits" "$OUT/kai.log" | sed -n "${1}p"; }
lim=$(announced 1)
[ -n "$lim" ] || fail "Kai's log has no limits announced in initialize"
for want in max_message_bytes=67108864 max_frame_bytes=16777216 ping_interval_s=20 "call_timeout_ms=$ROW_TIMEOUT"; do
  grep -qF -- "$want" <<<"$lim" || fail "the announced limits lack $want: $lim"
done
pass "the link's limits read from initialize: 64 MiB messages, 16 MiB frames, 20 s pings, the row's $ROW_TIMEOUT ms"

# 3. /mcp for the owner and for a deny key.
mcp_tools() { # <bearer>: the tool names /mcp lists to it, one per line
  python3 -I - "$URL/mcp" "$1" <<'PY'
import json, sys, urllib.request
url, key = sys.argv[1], sys.argv[2]
session = None
def rpc(body):
    global session
    h = {"Authorization": "Bearer " + key, "Content-Type": "application/json",
         "Accept": "application/json, text/event-stream"}
    if session:
        h["Mcp-Session-Id"] = session
    with urllib.request.urlopen(urllib.request.Request(url, json.dumps(body).encode(), h), timeout=20) as r:
        session = r.headers.get("Mcp-Session-Id") or session
        text = r.read().decode()
    if not text.strip():
        return None
    if text.lstrip().startswith("{"):
        return json.loads(text)
    for line in text.splitlines():
        if line.startswith("data:"):
            return json.loads(line[5:])
rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "live-check-host", "version": "1"}}})
rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
for t in rpc({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})["result"]["tools"]:
    print(t["name"])
print("#listed")
PY
}
owner_tools=$(mcp_tools "$TOKEN")
echo "$owner_tools" | grep -qx "${LABEL}__live_echo" || fail "/mcp does not list ${LABEL}__live_echo for the owner: $(echo "$owner_tools" | paste -sd' ')"
deny_tools=$(mcp_tools "$DKEY") || fail "/mcp did not list tools to the deny key: $deny_tools"
echo "$deny_tools" | grep -qx '#listed' || fail "/mcp's list to the deny key did not read"
echo "$deny_tools" | grep -q "^${LABEL}__" && fail "/mcp lists Kai's tools to a deny key"
pass "/mcp lists ${LABEL}__live_echo to the owner and none of Kai's tools to a key with tool scope deny"

# 4. A mock turn calls Kai's tool.
chat() { # <path> <json>
  bearer "$TOKEN" | curl -sS --max-time 60 -X POST "$URL/chat/api/$1" -H @- \
    -H 'Content-Type: application/json' -d "$2"
}
tid=$(chat threads '{"model_alias": "kai-chat"}' | jq -r .id)
[ -n "$tid" ] && [ "$tid" != null ] || fail "no thread"
chat "threads/$tid/settings" "{\"mcp_tools\": [{\"server_label\": \"$LABEL\"}]}" >/dev/null
chat "threads/$tid/send" '{"content": "call the live tool"}' >"$OUT/turn.sse" || true
grep -qF "live_echo: $ARG" "$OUT/turn.sse" || fail "the turn's reply lacks Kai's result (see $OUT/turn.sse)"
logged 'host: a call tool="live_echo"' || logged 'host: a call tool=live_echo' || fail "Kai's log has no call"
logged 'host: answered' || fail "Kai's log has no answer"
pass "a Chat turn on the mock model called ${LABEL}__live_echo through lmgw and Kai, and the reply carries Kai's result"
note "the call in Kai's log: $(grep -F 'host: a call' "$OUT/kai.log" | tail -1 | sed -E 's/^.*host: a call/host: a call/')"

# 5. The dev lmgw restarts.
links=$(grep -c 'host: linked' "$OUT/kai.log" || true)
# The next link announces no message bound (mcp.host_max_message_mb 0: `null`).
op settings_set_full '{"mcp": {"host_max_message_mb": 0}}' | jq -e '.ok == true' >/dev/null \
  || fail "settings_set_full refused mcp.host_max_message_mb 0"
stop_lmgw
until_ 20 "the tools offline" tip_has "tools: offline"
note "tooltip while lmgw is away: $(tooltip | tr '\n' ' ' | grep -o 'tools: offline[^<]*' | head -1)"
start_lmgw
until_ 30 "the tools linked again" tip_has "tools: linked as $LABEL ("
[ "$(grep -c 'host: linked' "$OUT/kai.log")" -gt "$links" ] || fail "no second link in the log"
pass "the dev lmgw's restart: 'tools: offline', then linked again by itself"
lim=$(announced 2)
[ -n "$lim" ] || fail "the second link announced no limits"
if grep -qF max_message_bytes= <<<"$lim" || ! grep -qF max_frame_bytes=16777216 <<<"$lim"; then
  fail "the second link's limits are not 'no message bound, 16 MiB frames': $lim"
fi
pass "the second link (mcp.host_max_message_mb 0): no message bound read from the null, frames 16 MiB"

# 6. The log and the quit.
stop_kai || fail "Kai's quit failed"
PEMLINE=$(secret-tool lookup application dev.rootshell.kai lmgw "$DEV_FP" 2>/dev/null | sed -n 2p)
[ -n "$PEMLINE" ] || fail "no key of Kai's in the wallet for $DEV_FP"
grep -qF -- "$PEMLINE" "$OUT/kai.log" && fail "the device key is in Kai's log"
grep -qF -- "$TOKENHEX" "$OUT/kai.log" && fail "the pairing token is in Kai's log"
grep -qi 'lmgw-pair:' "$OUT/kai.log" && fail "the pairing link is in Kai's log"
grep -qF "$ARG" "$OUT/kai.log" && fail "the tool's argument is in Kai's log"
pass "no key, token, link or tool argument in Kai's log at KAI_LOG=trace; a clean quit"
