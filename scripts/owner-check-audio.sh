#!/usr/bin/env bash
# The owner's checks of Kai's audio (WP3): what the automated live check could not do in a private
# PipeWire instance, because it is audible or touches this session's own default devices. Each step
# asks first. Run it from a terminal in the Plasma session, with no Kai running.
#
# usage: scripts/owner-check-audio.sh [clip.wav]
#        (a 24 kHz mono PCM16 clip; default: target/live-audio/clip.wav from the automated live check)
#
#   A  the heard position on the real output (audible): play a clip, interrupt it with Enter, then
#      hear the clip replayed up to what Kai counted as heard;
#   B  app AEC on the session's real devices: Kai switches the defaults to its canceller, is killed
#      with kill -9, and the next start restores the defaults;
#   C  the tray: the Echo mode, Microphone and Speaker menus with this session's devices, live.
#
# Reports and logs: target/owner-audio/.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/target/owner-audio
# shellcheck source=scripts/shortcuts-guard.sh
source "$here/scripts/shortcuts-guard.sh"
trap 'kgs_check || exit 1' EXIT
clip=${1:-$here/target/live-audio/clip.wav}
mkdir -p "$out"
cd "$here"
export CARGO_BUILD_JOBS=8

ask() { local a; read -r -p "$1 [y/N] " a; [[ $a == [yY]* ]]; }
meta() { { timeout 5 pw-metadata -n default 0 "$1" 2>/dev/null | grep -o "value:'[^']*'" | head -1 | sed "s/value:'\(.*\)'/\1/"; } || true; }
name_of() { python3 -c 'import json, sys
s = sys.stdin.read().strip()
print(json.loads(s)["name"] if s else "")'; }
configured() { meta "default.configured.audio.$1" | name_of; }
effective() { meta "default.audio.$1" | name_of; }
node_id() { timeout 5 pw-dump | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    if o.get("type") == "PipeWire:Interface:Node" and o["info"]["props"].get("node.name") == sys.argv[1]:
        print(o["id"])' "$1"; }
has_node() { timeout 5 pw-dump 2>/dev/null | grep -q "\"node.name\": \"$1\""; }
until_ok() {
    local secs=$1; shift
    local end=$((SECONDS + secs))
    until "$@" >/dev/null 2>&1; do (( SECONDS < end )) || return 1; sleep 0.1; done
}

if busctl --user status dev.rootshell.kai >/dev/null 2>&1; then
    echo "A Kai is running (it owns dev.rootshell.kai). Quit it from its tray first."
    exit 1
fi

cargo build -q
test_bin=$(cargo test --no-run --message-format=json 2>/dev/null | python3 -c '
import json, sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])' | tail -1)

# ------------------------------------------------------------------------------------------- A
echo
echo "A. The heard position on '$(effective sink)' (audible: set a comfortable volume)."
if [[ -f $clip ]] && ask "Run A?"; then
    KAI_OWNER_CHECK=1 KAI_LIVE_CLIP="$clip" KAI_LIVE_OUT="$out" \
        "$test_bin" --ignored --exact audio::live_tests::owner_heard_on_the_default_output --nocapture \
        2>&1 | grep -v '^  event:' | tee "$out/a.log"
    echo "-> Did the replay end where you stopped hearing the clip? Note the answer in the report."
fi

# ------------------------------------------------------------------------------------------- B
echo
echo "B. App AEC on this session's devices, kill -9, restore on the next start."
echo "   Kai switches the default output and input to its canceller for a few seconds."
if ask "Run B?"; then
    sink_before=$(configured sink); source_before=$(configured source)
    printf 'configured sink %s\nconfigured source %s\neffective sink %s\neffective source %s\n' \
        "${sink_before:-<absent>}" "${source_before:-<absent>}" "$(effective sink)" "$(effective source)" \
        | tee "$out/b-defaults-before.txt"
    cfg=$out/config
    mkdir -p "$cfg/kai"
    kai_pid=""
    # Whatever happens: Kai stops, and the defaults are what they were (by name, from the record).
    restore_b() {
        [[ -n $kai_pid ]] && kill -TERM "$kai_pid" 2>/dev/null && wait "$kai_pid" 2>/dev/null || true
        for kind in sink source; do
            local before; before=$([[ $kind == sink ]] && echo "$sink_before" || echo "$source_before")
            local now; now=$(configured "$kind")
            if [[ $now == kai_aec_* ]]; then
                if [[ -z $before ]]; then
                    timeout 5 pw-metadata -n default -d 0 "default.configured.audio.$kind" >/dev/null || true
                elif id=$(node_id "$before") && [[ -n $id ]]; then
                    timeout 5 wpctl set-default "$id" || true
                fi
                echo "safety restore: default.configured.audio.$kind back to '${before:-<absent>}'"
            fi
        done
    }
    trap 'restore_b; kgs_check || exit 1' EXIT
    start_kai() {
        XDG_CONFIG_HOME=$cfg KAI_LOG=info "$here/target/debug/kai" > "$out/$1.log" 2>&1 &
        kai_pid=$!
    }

    kai_config "$cfg/kai/config.toml" '[audio]\necho_mode = "app"\n'
    start_kai b-app
    until_ok 15 bash -c "timeout 5 pw-metadata -n default 0 default.configured.audio.sink | grep -q kai_aec_sink" \
        || { echo "app AEC did not come up; see $out/b-app.log"; exit 1; }
    child=$(pgrep -P "$kai_pid" -x pipewire || true)
    echo "switched: default output '$(effective sink)', input '$(effective source)'; echo-cancel child ${child:-none}"
    timeout 5 wpctl status | sed -n '/^Audio/,/^Video/p' | grep -E 'Kai|\*' || true
    read -r -p "   (Sound keeps playing through 'Kai: echo reference'. Press Enter for kill -9.) " _
    kill -9 "$kai_pid"; wait "$kai_pid" 2>/dev/null || true; kai_pid=""
    until_ok 3 bash -c "! kill -0 ${child:-0} 2>/dev/null" && echo "the echo-cancel child died with Kai" \
        || echo "PROBLEM: the echo-cancel child outlived kill -9 (pid $child)"
    until_ok 3 bash -c '! timeout 5 pw-dump | grep -q "\"node.name\": \"kai_aec_sink\""' || true
    echo "after kill -9: configured output '$(configured sink)' (WirePlumber falls back to '$(effective sink)'); aec.json $([[ -f ${XDG_STATE_HOME:-$HOME/.local/state}/kai/aec.json ]] && echo kept || echo missing)"

    kai_config "$cfg/kai/config.toml" '[audio]\necho_mode = "device"\n'
    start_kai b-restore
    until_ok 10 bash -c "[[ ! -f '${XDG_STATE_HOME:-$HOME/.local/state}/kai/aec.json' ]]" \
        || echo "PROBLEM: aec.json is still there; see $out/b-restore.log"
    sleep 0.5
    sink_after=$(configured sink); source_after=$(configured source)
    kill -TERM "$kai_pid"; wait "$kai_pid" || true; kai_pid=""
    if [[ $sink_after == "$sink_before" && $source_after == "$source_before" ]]; then
        echo "-> RESTORED: configured output '${sink_after:-<absent>}', input '${source_after:-<absent>}' as before"
    else
        echo "-> NOT RESTORED: output '${sink_after:-<absent>}' (was '${sink_before:-<absent>}'), input '${source_after:-<absent>}' (was '${source_before:-<absent>}')"
    fi
    grep -o "restored the default devices[^\"]*" "$out/b-restore.log" | head -1 || true
    trap 'kgs_check || exit 1' EXIT
    restore_b
fi

# ------------------------------------------------------------------------------------------- C
echo
echo "C. The tray, with this session's devices (a copy of your configuration in $out/c-config, with"
echo "   shortcuts.bind = false: your choices go there, and no global shortcuts are registered)."
echo "   1. Echo mode: four entries, 'Input device cancels echo' selected (or your choice)."
echo "   2. Microphone and Speaker: 'Default (<your current default>)' plus your devices; plug or"
echo "      unplug a USB device and reopen the menu: the list follows."
echo "   3. In 'Input device cancels echo', pick a Speaker that is not the default: the tooltip warns,"
echo "      and the menu offers 'Use half duplex'. Pick Default again."
echo "   4. Quit Kai from its menu."
if ask "Run C (just run)?"; then
    c_cfg=$out/c-config
    rm -rf "$c_cfg"
    mkdir -p "$c_cfg/kai"
    own=${XDG_CONFIG_HOME:-$HOME/.config}/kai/config.toml
    [[ -f $own ]] && cp "$own" "$c_cfg/kai/config.toml"
    if grep -q '^\[shortcuts\]$' "$c_cfg/kai/config.toml" 2>/dev/null; then
        sed -i 's/^\[shortcuts\]$/[shortcuts]\nbind = false/' "$c_cfg/kai/config.toml"
    else
        printf '[shortcuts]\nbind = false\n' >> "$c_cfg/kai/config.toml"
    fi
    XDG_CONFIG_HOME=$c_cfg just run || true
fi
echo "Done. Logs and reports: $out"
