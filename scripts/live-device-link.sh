# shellcheck shell=bash
# Sourced by the live checks that pair Kai with a dev lmgw over lmgw's device link (its design §3:
# the link v2, Kai's enrolment, the owner's Confirm): the dev lmgw's device listener turned on, a
# device minted for this computer, a new token for its row, the owner's Confirm of the code the
# device shows, each as the owner through the dev lmgw's /api.
#
# The caller defines, before it calls any of these:
#   op <name> <json>   the owner's POST /api/op/<name> (its bearer from a pipe, never in argv)
#   owner_get <path>   the owner's GET <path>
#   fail <words>       ends the check
#
#   devices_on                       the device listener on (loopback, the HTTP port plus one), as
#                                    lmgw's pairing form turns it on before a first pairing
#   mint <name> [<json fields>]      key_create for this computer (local_only), any more of the
#                                    device's policy as `"hosts_label": "kai"`: sets DEV_ID, DEV_FP,
#                                    DEV_URL (the link's first URL) and DEV_LINK (a credential while
#                                    its token lives: never printed or logged)
#   new_token <id> [<json fields>]   device_token for the row (`"end_current": true` ends its key
#                                    now; `"urls": [...]` replaces its URLs): DEV_URL and DEV_LINK
#   accept_entry <link>              what KAI_LIVE_LINK_ACCEPT lists for a link this check minted
#                                    (src/live_test.rs): its first URL and its token's SHA-256
#   token_hex <link>                 the token's 32 hex digits, which no log may hold
#   code_in <text>                   the confirmation code in Kai's tooltip ("Confirm 7F3A-91C2 on
#                                    lmgw's Devices card"), or nothing
#   confirm <id> <code>              device_confirm with the code the device shows
#   enrol_files <test bin> <dir>     Kai's enrolment of DEV_LINK outside a running Kai (the test
#                                    binary's pairing::tests::live_enrol), then the owner's Confirm:
#                                    <dir>/device-link.json and <dir>/key.pem, for a live test
#   card <id> <jq path>              a field of the row's pairing as lmgw's Devices card shows it
#                                    (the Keys list, GET /api/usage/keys): `.confirm_code`,
#                                    `.kept_code`, `.client`, `.state`, …; empty when it has none

devices_on() {
  local state said
  state=$(owner_get /api/status | jq -r '.devices.listener.state // empty' 2>/dev/null || true)
  [[ $state == listening ]] && return 0
  said=$(op settings_set '{"devices": {"listener": true}}')
  jq -e '.ok == true' >/dev/null 2>&1 <<<"$said" \
    || fail "settings_set did not turn the device listener on: $(jq -c . <<<"$said" 2>/dev/null || echo unreadable)"
  local _
  for _ in $(seq 1 50); do
    state=$(owner_get /api/status | jq -r '.devices.listener.state // empty' 2>/dev/null || true)
    [[ $state == listening ]] && return 0
    sleep 0.1
  done
  fail "the device listener does not listen: $(owner_get /api/status | jq -c '.devices.listener' 2>/dev/null || echo unreadable)"
}

# The fields of a DevicePaired answer (key_create, device_token), its link kept out of every message.
dev_paired() { # <op> <answer>
  jq -e '.ok == true and (.link | startswith("lmgw-pair:?v=2&"))' >/dev/null 2>&1 <<<"$2" \
    || fail "$1 answered: $(jq -c 'del(.link)' <<<"$2" 2>/dev/null || echo unreadable)"
  DEV_LINK=$(jq -r .link <<<"$2")
  DEV_URL=$(jq -r '.urls[0]' <<<"$2")
}

mint() { # <name> [<json fields>]
  local made
  made=$(op key_create "{\"kind\": \"device\", \"name\": \"$1\", \"local_only\": true${2:+, $2}}")
  dev_paired key_create "$made"
  DEV_ID=$(jq -r .id <<<"$made")
  DEV_FP=$(jq -r .fp <<<"$made")
}

new_token() { # <id> [<json fields>]
  dev_paired device_token "$(op device_token "{\"id\": $1${2:+, $2}}")"
}

token_hex() { # <link>
  sed -E 's/.*[?&]token=lmgw-enrol-([0-9a-f]{32}).*/\1/' <<<"$1"
}

accept_entry() { # <link>: the link's first URL is the answer's urls[0], as lmgw normalized it
  local url token
  url=$(python3 -I -c 'import sys, urllib.parse as u
q = u.parse_qs(sys.stdin.read().strip().split("?", 1)[1])
print(q["url"][0])' <<<"$1")
  token="lmgw-enrol-$(token_hex "$1")"
  printf '%s#%s' "$url" "$(printf %s "$token" | sha256sum | cut -d' ' -f1)"
}

code_in() { # <text>
  grep -oE 'Confirm [0-9A-F]{4}-[0-9A-F]{4}' <<<"$1" | head -1 | cut -d' ' -f2 || true
}

confirm() { # <id> <code>
  local said
  said=$(op device_confirm "{\"id\": $1, \"confirm_code\": \"$2\"}")
  jq -e '.ok == true' >/dev/null 2>&1 <<<"$said" \
    || fail "device_confirm answered: $(jq -c . <<<"$said" 2>/dev/null || echo unreadable)"
}

card() { # <id> <jq path>
  owner_get /api/usage/keys \
    | jq -r --argjson id "$1" ".keys[] | select(.id == \$id) | .pairing | $2 // empty" 2>/dev/null || true
}

# enrol_files <kai test binary> <dir>: Kai's enrolment of DEV_LINK by the test binary's
# pairing::tests::live_enrol (a new key; the link handed over in a file of mode 600, never in
# argv), then the owner's Confirm of its code. <dir>/device-link.json and <dir>/key.pem (mode 600)
# are the pairing a live test reads; <dir> is under target/, and the caller deletes it.
enrol_files() {
  local bin=$1 dir=$2
  mkdir -p "$dir"
  (umask 077 && printf '%s\n' "$DEV_LINK" >"$dir/link")
  KAI_LIVE_ENROL=$dir "$bin" --ignored --exact pairing::tests::live_enrol --test-threads=1 \
    >"$dir/enrol.log" 2>&1 || fail "Kai's enrolment failed: $(grep -m1 'panicked\|the enrolment' "$dir/enrol.log" || tail -3 "$dir/enrol.log")"
  rm -f "$dir/link"
  [[ -s $dir/device-link.json && -s $dir/key.pem && -s $dir/code ]] || fail "the enrolment wrote no pairing in $dir"
  confirm "$DEV_ID" "$(tr -d '\n' <"$dir/code")"
}
