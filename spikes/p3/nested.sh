#!/usr/bin/env bash
# Runs a command against a nested, invisible KWin of its own (WP13). A trimmed copy of
# scripts/nested-kwin.sh: one virtual output 1280x720, a private D-Bus session that activates
# nothing, private XDG directories under spikes/p3/.run/<name>, a clean environment. Nothing shows
# on the session's screen and the session's compositor, bus and data are never touched.
#
# usage: spikes/p3/nested.sh <name> [--seed <dir>] [--kwin-env K=V]… -- <command> [args…]
#   <name>        a directory name under spikes/p3/.run/ (re-created on every run)
#   --seed <dir>  copied into the nested XDG_DATA_HOME before KWin starts (e.g. applications/)
#   --kwin-env    an extra variable for kwin_wayland only
# The command sees WAYLAND_DISPLAY, DBUS_SESSION_BUS_ADDRESS, P3_DIR (the run directory),
# P3_KWIN_LOG (KWin's stderr; a KWin script's print() lands there, category js). Anything still
# running in the nested environment afterwards is reported (LEFT) and fails the run.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)

if [[ "${1:-}" == "--inside" ]]; then
    shift
    dir=$1
    shift
    kenv=()
    while [[ $1 != -- ]]; do kenv+=("$1"); shift; done
    shift
    [[ ${DBUS_SESSION_BUS_ADDRESS:-} == *"path=$dir/run/"* ]] \
        || { echo "the private bus is not in $dir/run: ${DBUS_SESSION_BUS_ADDRESS:-}" >&2; exit 1; }
    pids=()
    cleanup() {
        for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
        for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
    }
    trap cleanup EXIT
    trap 'exit 130' INT TERM
    env "${kenv[@]}" QT_FORCE_STDERR_LOGGING=1 QT_LOGGING_RULES="js.debug=true;kwin_core.debug=true" \
        kwin_wayland --virtual --output-count 1 --width 1280 --height 720 --socket kai-nested \
        --no-lockscreen > "$dir/kwin.log" 2>&1 &
    pids+=($!)
    end=$((SECONDS + 10))
    until [[ -S $XDG_RUNTIME_DIR/kai-nested ]]; do
        (( SECONDS < end )) || { echo "the nested KWin did not start (see $dir/kwin.log)" >&2; exit 1; }
        sleep 0.1
    done
    end=$((SECONDS + 10))
    until busctl --user status org.kde.KWin > /dev/null 2>&1; do
        (( SECONDS < end )) || { echo "KWin did not take its bus name" >&2; exit 1; }
        sleep 0.1
    done
    export WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/kai-nested P3_DIR=$dir P3_KWIN_LOG=$dir/kwin.log
    code=0
    "$@" || code=$?
    exit "$code"
fi

usage() { echo "usage: $0 <name> [--seed <dir>] [--kwin-env K=V]… -- <command> [args…]" >&2; exit 2; }
name=${1:-}
[[ -n $name && $name != */* && $name != .* ]] || usage
shift
seed=
kenv=()
while [[ ${1:-} != -- ]]; do
    case ${1:-} in
        --seed) seed=$2; shift 2 ;;
        --kwin-env) kenv+=("$2"); shift 2 ;;
        *) usage ;;
    esac
done
shift
(( $# )) || usage
dir=$here/.run/$name
rm -rf "${dir:?}"
mkdir -p "$dir"/{run,config,data,cache,state,home}
chmod 700 "$dir/run"
[[ -z $seed ]] || cp -a "$seed"/. "$dir/data/"

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

code=0
env -i HOME="$dir/home" PATH="$PATH" LANG=C.UTF-8 \
    XDG_RUNTIME_DIR="$dir/run" XDG_CONFIG_HOME="$dir/config" XDG_DATA_HOME="$dir/data" \
    XDG_CACHE_HOME="$dir/cache" XDG_STATE_HOME="$dir/state" \
    XDG_DATA_DIRS=/usr/local/share:/usr/share XDG_CONFIG_DIRS=/etc/xdg \
    XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland KDE_FULL_SESSION=true \
    GIO_USE_VFS=local NO_AT_BRIDGE=1 GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland \
    dbus-run-session --config-file="$dir/session-bus.conf" -- "$0" --inside "$dir" "${kenv[@]}" -- "$@" || code=$?
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
