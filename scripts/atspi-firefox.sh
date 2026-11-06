#!/usr/bin/env bash
# Runs a command (default: Kai's AT-SPI live test against a headless Firefox,
# src/desktop/fdo/atspi/live_tests.rs) on a PRIVATE D-Bus session with its own AT-SPI bus, so
# nothing of the session's accessibility is read or changed: no window opens, org.a11y.Status of
# the session is never touched, and the session's at-spi/bus_0 keeps its inode and mtime (checked
# before and after, and said). A port of kai-firefox's spikes/ext/atspi-selftest.sh, with every one
# of its safety properties:
#
# - EVERYTHING it starts (the bus, the AT-SPI launcher and registry, the command and the headless
#   Firefox the command starts) runs with a private XDG_RUNTIME_DIR (mktemp -d
#   /tmp/kai-atspi-XXXXXX, mode 0700: short, since a unix socket path holds 108 bytes; only sockets
#   live there, removed on every exit path) and with DISPLAY, WAYLAND_DISPLAY and XAUTHORITY unset.
#   at-spi-bus-launcher binds $XDG_RUNTIME_DIR/at-spi/bus_<display> after unlinking whatever is
#   there: run with the session's runtime folder it replaced the session's accessibility socket with
#   a dead one (seen in kai-firefox's earlier self-test). Before anything else runs, the private launcher's address
#   must be a socket inside the private runtime folder, else the script stops.
# - The private bus is a dbus-daemon this script starts with a config that lists no service
#   directory: nothing is D-Bus activated on it (no portal, no secret service). GSettings is in
#   memory, never the user's dconf.
# - On every exit path (trap: exit, Ctrl+C, TERM, HUP) it ends what it started by pid, then every
#   process of the user whose /proc/<pid>/environ carries this bus's exact address or the private
#   XDG_RUNTIME_DIR (a leftover Firefox included), then the bus, then removes the runtime folder.
#   Processes on the real session bus are never matched. Everything it spawns but the command has
#   stdin, stdout and stderr on /dev/null, so a survivor cannot hold the caller's pipe. A SIGKILL
#   on the script itself cannot run the trap.
# - The command's Firefox keeps its profile and HOME under the worktree's target/ (the test does
#   this), never /tmp (RAM) and never ~/.mozilla.
#
#   usage: scripts/atspi-firefox.sh [<test binary>]
#          scripts/atspi-firefox.sh -- <command> [args…]
# `just atspi-firefox` builds the test binary and runs it here.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

unset NO_AT_BRIDGE AT_SPI_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE GTK_A11Y
real_bus="${DBUS_SESSION_BUS_ADDRESS:-}"
real_runtime="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
session_socket="$real_runtime/at-spi/bus_0"
cmd=()
if [[ "${1:-}" == "--" ]]; then
  shift
  cmd=("$@")
elif [[ -n "${1:-}" ]]; then
  cmd=("$1" --ignored --exact --nocapture --test-threads=1 desktop::fdo::atspi::live_tests::firefox_on_a_private_bus)
else
  echo "usage: scripts/atspi-firefox.sh <test binary> | -- <command> [args…]" >&2
  exit 2
fi
for c in dbus-daemon busctl /usr/libexec/at-spi-bus-launcher /usr/libexec/at-spi2-registryd python3; do
  command -v "$c" >/dev/null || { echo "atspi-firefox: needs $c" >&2; exit 2; }
done

# The session's accessibility socket, as it is before: compared at the end.
socket_stat() { stat -c '%i %Y' "$session_socket" 2>/dev/null || echo "absent"; }
before_socket="$(socket_stat)"

rt=""
addr=""
addr_file=""
bus_pid=""
cmd_pid=""
launcher=""
registry=""

# pids of the user's processes (not this script, not the helper) whose environment has the private
# bus address as DBUS_SESSION_BUS_ADDRESS or DBUS_STARTER_ADDRESS, or the private XDG_RUNTIME_DIR;
# one pid per line
IFS= read -r -d '' sweep_py <<'PY' || true
import os, sys
addr = sys.argv[1].encode()
want = {b"DBUS_SESSION_BUS_ADDRESS=" + addr, b"DBUS_STARTER_ADDRESS=" + addr, b"XDG_RUNTIME_DIR=" + sys.argv[3].encode()}
skip = {int(sys.argv[2]), os.getpid(), os.getppid()}
me = os.getuid()
for d in os.listdir("/proc"):
    if not d.isdigit() or int(d) in skip:
        continue
    try:
        if os.stat("/proc/" + d).st_uid != me:
            continue
        with open("/proc/%s/environ" % d, "rb") as f:
            env = f.read().split(b"\0")
    except OSError:
        continue
    if want.intersection(env):
        print(d)
PY

sweep() { python3 -I -c "$sweep_py" "$addr" "$$" "$rt" </dev/null; }
alive() { kill -0 "$1" 2>/dev/null; }
# wait up to $2 tenths of a second for pid $1 to be gone
gone_within() {
  local _
  for _ in $(seq "$2"); do alive "$1" || return 0; sleep 0.1; done
  ! alive "$1"
}

cleanup() {
  local rc=$? pids sig round p _ after_socket
  trap - EXIT INT TERM HUP
  set +e
  # our own children first, by pid: the command gets TERM so it stops its Firefox itself
  if [[ -n "$cmd_pid" ]] && alive "$cmd_pid"; then
    kill -TERM "$cmd_pid" 2>/dev/null
    gone_within "$cmd_pid" 150 || kill -KILL "$cmd_pid" 2>/dev/null
  fi
  for p in $registry $launcher; do kill -TERM "$p" 2>/dev/null; done
  # then everything that carries the private bus address or runtime folder
  if [[ -n "$addr" && -n "$rt" ]]; then
    for round in 1 2 3; do
      for sig in TERM KILL; do
        pids="$(sweep)"
        [[ -z "$pids" ]] && continue
        # shellcheck disable=SC2086
        kill -"$sig" $pids 2>/dev/null
        for _ in $(seq 30); do [[ -z "$(sweep)" ]] && break; sleep 0.1; done
      done
      [[ -z "$(sweep)" ]] && break
    done
    pids="$(sweep)"
    # shellcheck disable=SC2086
    [[ -n "$pids" ]] && echo "atspi-firefox: still on the private bus after cleanup: $(echo $pids)" >&2
  fi
  if [[ -n "$bus_pid" ]]; then
    kill -TERM "$bus_pid" 2>/dev/null
    gone_within "$bus_pid" 30 || kill -KILL "$bus_pid" 2>/dev/null
  fi
  wait 2>/dev/null
  [[ -n "$addr_file" ]] && rm -f "$addr_file"
  if [[ -n "$rt" && "$rt" == /tmp/kai-atspi-* && -d "$rt" && ! -L "$rt" ]]; then rm -rf -- "$rt"; fi
  after_socket="$(socket_stat)"
  if [[ "$after_socket" == "$before_socket" ]]; then
    echo "atspi-firefox: the session's $session_socket is as before (inode and mtime: $after_socket)"
  else
    echo "atspi-firefox: THE SESSION'S $session_socket CHANGED: was '$before_socket', is '$after_socket'" >&2
    [[ $rc -eq 0 ]] && rc=3
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

rt="$(mktemp -d /tmp/kai-atspi-XXXXXX)"
chmod 700 "$rt"
if [[ "$rt" != /tmp/kai-atspi-* || "$rt" == "$real_runtime"* ]]; then
  echo "atspi-firefox: refusing the runtime dir '$rt'" >&2
  exit 1
fi
export XDG_RUNTIME_DIR="$rt"
unset DISPLAY WAYLAND_DISPLAY XAUTHORITY DBUS_SESSION_BUS_ADDRESS
export GSETTINGS_BACKEND=memory

addr_file="$rt/addr"
cat >"$rt/bus.conf" <<CONF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:path=$rt/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONF
dbus-daemon --config-file="$rt/bus.conf" --nofork --print-address=3 3>"$addr_file" </dev/null >/dev/null 2>&1 &
bus_pid=$!
for _ in $(seq 50); do
  [[ -s "$addr_file" ]] && break
  alive "$bus_pid" || { echo "atspi-firefox: dbus-daemon did not start" >&2; exit 1; }
  sleep 0.1
done
addr="$(head -n 1 "$addr_file")"
if [[ -z "$addr" || "$addr" == "$real_bus" || "$addr" != "unix:path=$rt/"* ]]; then
  echo "atspi-firefox: refusing the bus address '$addr' (empty, the session's, or not a socket inside $rt)" >&2
  addr=""
  exit 1
fi
export DBUS_SESSION_BUS_ADDRESS="$addr"
echo "atspi-firefox: private bus $addr (dbus-daemon $bus_pid), runtime dir $rt"

/usr/libexec/at-spi-bus-launcher </dev/null >/dev/null 2>&1 &
launcher=$!
for _ in $(seq 50); do busctl --user status org.a11y.Bus </dev/null >/dev/null 2>&1 && break; sleep 0.1; done
a11y_address_full="$(busctl --user call org.a11y.Bus /org/a11y/bus org.a11y.Bus GetAddress </dev/null 2>/dev/null | sed -n 's/^s "\(.*\)"$/\1/p')"
a11y_socket="${a11y_address_full#unix:path=}"
a11y_socket="${a11y_socket%%,*}"
if [[ "$a11y_socket" != "$rt"/* ]]; then
  echo "atspi-firefox: refusing: the private launcher's AT-SPI address is '$a11y_address_full', not a socket inside $rt" >&2
  exit 1
fi
echo "atspi-firefox: private AT-SPI bus $a11y_socket"
/usr/libexec/at-spi2-registryd </dev/null >/dev/null 2>&1 &
registry=$!
for _ in $(seq 50); do busctl --address="$a11y_address_full" status org.a11y.atspi.Registry </dev/null >/dev/null 2>&1 && break; sleep 0.1; done
# IsEnabled on, on the private bus only (a headless Firefox needs accessibility.force_disabled = -1
# besides, which the test's profile sets).
busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true </dev/null >/dev/null 2>&1 || true

rc=0
KAI_ATSPI_PRIVATE=1 KAI_ATSPI_RUNTIME="$rt" "${cmd[@]}" </dev/null &
cmd_pid=$!
wait "$cmd_pid" || rc=$?
cmd_pid=""
exit "$rc"
