# shellcheck shell=bash
# Sourced first by a live check that runs a Kai of its own on a session bus (live-check-link,
# live-check-host): the script runs itself again inside dbus-run-session, on a private bus that
# activates nothing (no service directories: no notification server, gvfsd or accessibility bus is
# started on it, as scripts/pw-private.sh's), with a Secret Service of its own (gnome-keyring, its
# keyring under <dir>, unlocked with a throwaway password). A Kai the session runs, the session's
# wallet and its tray are never touched, and the check's Kai's tray item is only on that bus.
#
#   source scripts/private-bus.sh <dir> <the script's absolute path> "$@"
#     before anything else of the script runs twice; <dir> (under target/, outside the check's own
#     output directory) is emptied first;
#   private_bus_stop
#     in the EXIT trap's cleanup, after Kai quit: stops the Secret Service and deletes <dir> with
#     the keyring (it held the check's device keys).
pb_dir=${1:?usage: source scripts/private-bus.sh <dir> <script> [args…]}
pb_self=${2:?usage: source scripts/private-bus.sh <dir> <script> [args…]}
shift 2

if [[ -z ${KAI_PRIVATE_BUS:-} ]]; then
  rm -rf "$pb_dir"
  mkdir -p "$pb_dir/rt" "$pb_dir/data"
  chmod 700 "$pb_dir/rt"
  cat >"$pb_dir/session-bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$pb_dir/rt</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
  # GIO_USE_VFS/NO_AT_BRIDGE: GIO and GTK do not even ask for gvfsd or the accessibility bus.
  KAI_PRIVATE_BUS=1 GIO_USE_VFS=local NO_AT_BRIDGE=1 GTK_A11Y=none \
    exec dbus-run-session --config-file="$pb_dir/session-bus.conf" -- "$pb_self" "$@"
fi

# Inside: the bus's Secret Service. Its own runtime and data directories, so it never meets the
# session's keyring sockets or files.
printf 'kai-live-check\n' | XDG_RUNTIME_DIR=$pb_dir/rt XDG_DATA_HOME=$pb_dir/data \
  gnome-keyring-daemon --foreground --components=secrets --unlock >"$pb_dir/keyring.log" 2>&1 &
PB_KEYRING_PID=$!
for _ in $(seq 1 50); do
  busctl --user status org.freedesktop.secrets >/dev/null 2>&1 && break
  sleep 0.1
done

private_bus_stop() {
  if [[ -n ${PB_KEYRING_PID:-} ]]; then
    kill -TERM "$PB_KEYRING_PID" 2>/dev/null || true
    wait "$PB_KEYRING_PID" 2>/dev/null || true
    PB_KEYRING_PID=""
  fi
  rm -rf "$pb_dir"
}
