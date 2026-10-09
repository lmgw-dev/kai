#!/usr/bin/env bash
# Runs a command against a nested KWin of its own: `kwin_wayland --virtual` with two virtual outputs
# (Virtual-0 and Virtual-1, 1280x720 side by side), its own D-Bus session and XDG directories under
# <dir>. Nothing shows on the session's screen, and the session's compositor, its outputs and its
# display settings are never touched: the command sees only the nested KWin's Wayland display and
# bus. The private bus activates nothing (it has no service directories), so whatever runs in the
# nested session was started here, and everything started here is stopped here.
#
# usage: scripts/nested-kwin.sh <dir> -- <command> [args…]
#        <dir> is new, empty, or one this script used before (it holds a .kai-nested-kwin marker);
#        any other directory is refused. Only what the script makes in it is cleared: its
#        subdirectories run, config, data, cache, state and home, and its own files.
#        The command runs with a clean environment (pass variables with `env K=V command`) and sees:
#        - WAYLAND_DISPLAY, the nested KWin's socket, <dir>/run/kai-nested;
#        - DBUS_SESSION_BUS_ADDRESS, the private bus, listening in <dir>/run (the address is in
#          <dir>/bus too, for the guards);
#        - KAI_NESTED_OUTPUT, a command `<enable|disable> <Virtual-N>` that switches one of the
#          nested outputs (kscreen-doctor through a kscreen backend on the private bus; it refuses
#          any display but the nested KWin's and any bus but the one in <dir>/bus);
#        - KAI_NESTED_KWIN_LOG, KWin's log, where a KWin script's print() lands (category `js`).
#        Its exit status is returned; anything still running in the nested environment afterwards
#        is reported (LEFT) and fails it. scripts/live-check-ring.sh runs the ring's output check
#        with it (review KAI-5 R5-14).
#        Optional, from the caller's environment (scripts/readme-shots.sh sets them; the live checks
#        keep the defaults): KAI_NESTED_OUTPUTS (2), KAI_NESTED_WIDTH and KAI_NESTED_HEIGHT (1280,
#        720: each output's size in logical pixels) and KAI_NESTED_SCALE (1).
set -euo pipefail

marker=.kai-nested-kwin

if [[ "${1:-}" == "--inside" ]]; then
    shift
    dir=$1
    shift
    # The bus dbus-run-session started from session-bus.conf: in this directory, nowhere else.
    [[ ${DBUS_SESSION_BUS_ADDRESS:-} == *"path=$dir/run/"* ]] \
        || { echo "the private bus is not in $dir/run: ${DBUS_SESSION_BUS_ADDRESS:-}" >&2; exit 1; }
    printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" > "$dir/bus"
    pids=()
    cleanup() {
        for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
        for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
    }
    trap cleanup EXIT
    trap 'exit 130' INT TERM

    # KWin's own log on stderr (not the journal); print() of a KWin script under `js`.
    QT_FORCE_STDERR_LOGGING=1 QT_LOGGING_RULES="js.debug=true" \
        kwin_wayland --virtual --output-count "$KAI_NESTED_OUTPUTS" --width "$KAI_NESTED_WIDTH" \
        --height "$KAI_NESTED_HEIGHT" --scale "$KAI_NESTED_SCALE" --socket kai-nested \
        --no-lockscreen > "$dir/kwin.log" 2>&1 &
    pids+=($!)
    end=$((SECONDS + 10))
    until [[ -S $XDG_RUNTIME_DIR/kai-nested ]]; do
        (( SECONDS < end )) || { echo "the nested KWin did not start (see $dir/kwin.log)" >&2; exit 1; }
        sleep 0.1
    done
    export WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/kai-nested

    # kscreen-doctor's in-process backend loses its output ids between reading and applying; the
    # launcher on the private bus keeps them. The bus activates nothing, so it is started here.
    launcher=$(sed -n 's/^Exec=//p' /usr/share/dbus-1/services/org.kde.kscreen.service)
    QT_QPA_PLATFORM=wayland "$launcher" > "$dir/kscreen.log" 2>&1 &
    pids+=($!)
    end=$((SECONDS + 10))
    until busctl --user status org.kde.KScreen > /dev/null 2>&1; do
        (( SECONDS < end )) || { echo "the kscreen backend did not start (see $dir/kscreen.log)" >&2; exit 1; }
        sleep 0.1
    done

    export KAI_NESTED_OUTPUT=$dir/output.sh KAI_NESTED_KWIN_LOG=$dir/kwin.log
    code=0
    "$@" || code=$?
    exit "$code"
fi

usage() { echo "usage: $0 <dir> -- <command> [args…]" >&2; exit 2; }
dir=${1:-}
[[ -n $dir && "${2:-}" == "--" ]] || usage
shift 2
(( $# )) || usage
command -v kwin_wayland > /dev/null || { echo "no kwin_wayland" >&2; exit 2; }
command -v kscreen-doctor > /dev/null || { echo "no kscreen-doctor" >&2; exit 2; }
# Never a directory of anything else's: only a new or empty one, or one with this script's marker.
if [[ -e $dir ]]; then
    [[ -d $dir ]] || { echo "refused: $dir is not a directory" >&2; exit 2; }
    if [[ ! -e $dir/$marker && -n $(ls -A "$dir") ]]; then
        echo "refused: $dir is not empty and holds no $marker, so it is not one of this script's" >&2
        exit 2
    fi
fi
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)
: > "$dir/$marker"
for d in run config data cache state home; do rm -rf "${dir:?}/$d"; mkdir -p "$dir/$d"; done
rm -f "${dir:?}"/{kwin.log,kscreen.log,output.sh,bus,session-bus.conf} "${dir:?}"/query-*.js
chmod 700 "$dir/run"
geometry=(KAI_NESTED_OUTPUTS="${KAI_NESTED_OUTPUTS:-2}" KAI_NESTED_WIDTH="${KAI_NESTED_WIDTH:-1280}"
    KAI_NESTED_HEIGHT="${KAI_NESTED_HEIGHT:-720}" KAI_NESTED_SCALE="${KAI_NESTED_SCALE:-1}")

# The private bus listens in the nested runtime directory and activates nothing: no service
# directories, so no helper a client asks for (a portal, a notifier, kded) is started on it.
cat > "$dir/session-bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$dir/run</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

# The output switch, refusing any display but the nested KWin's and any bus but the private one.
cat > "$dir/output.sh" <<EOF
#!/usr/bin/env bash
# usage: output.sh <enable|disable> <Virtual-N>   (the nested KWin of $dir only)
set -euo pipefail
[[ \${1:-} == enable || \${1:-} == disable ]] && [[ \${2:-} == Virtual-[0-9] ]] \\
    || { echo "usage: \$0 <enable|disable> <Virtual-N>" >&2; exit 2; }
[[ \${WAYLAND_DISPLAY:-} == "$dir/run/kai-nested" ]] \\
    || { echo "refused: WAYLAND_DISPLAY is not the nested KWin's" >&2; exit 3; }
[[ -s "$dir/bus" && \${DBUS_SESSION_BUS_ADDRESS:-} == "\$(< "$dir/bus")" ]] \\
    || { echo "refused: not the nested session's private bus" >&2; exit 3; }
QT_QPA_PLATFORM=wayland timeout 15 kscreen-doctor "output.\$2.\$1" > /dev/null
EOF
chmod +x "$dir/output.sh"

code=0
env -i HOME="$dir/home" PATH="$PATH" LANG=C.UTF-8 \
    XDG_RUNTIME_DIR="$dir/run" XDG_CONFIG_HOME="$dir/config" XDG_DATA_HOME="$dir/data" \
    XDG_CACHE_HOME="$dir/cache" XDG_STATE_HOME="$dir/state" \
    XDG_DATA_DIRS=/usr/local/share:/usr/share XDG_CONFIG_DIRS=/etc/xdg \
    GIO_USE_VFS=local NO_AT_BRIDGE=1 "${geometry[@]}" \
    dbus-run-session --config-file="$dir/session-bus.conf" -- "$0" --inside "$dir" "$@" || code=$?
# Anything still running in the nested session's environment (its runtime directory) is a leftover;
# a second look after a moment, for a process on its way out.
leftovers() {
    local e p
    for e in /proc/[0-9]*/environ; do
        { tr '\0' '\n' < "$e"; } 2> /dev/null | grep -qxF "XDG_RUNTIME_DIR=$dir/run" || continue
        p=${e#/proc/}
        p=${p%/environ}
        printf '%s %s; ' "$p" "$({ tr '\0' ' ' < "/proc/$p/cmdline"; } 2> /dev/null | cut -c1-100)"
    done
}
left=$(leftovers)
[[ -z $left ]] || { sleep 1; left=$(leftovers); }
if [[ -n $left ]]; then
    echo "LEFT running after the nested KWin: $left" >&2
    (( code )) || code=1
fi
exit "$code"
