#!/usr/bin/env bash
# Runs a command inside a private PipeWire instance: its own runtime, config and state directories
# under <dir>, its own D-Bus session, WirePlumber with every hardware monitor off, a Dummy driver,
# three null sinks (kai-test-sink, the default; kai-test-sink-2; kai-test-sink-follower, which does
# not drive the graph) and a test microphone (kai-test-mic, a loopback source fed by the sink
# kai-test-mic-in). Nothing is audible; the
# session's PipeWire and its defaults are never touched. Everything started here is stopped here.
#
# usage: scripts/pw-private.sh <dir> -- <command> [args…]
#        The command runs with a clean environment (pass variables with `env K=V command`), sees
#        PIPEWIRE_RUNTIME_DIR and XDG_* under <dir>, and may append the pids of anything it starts
#        in the background to $KAI_PW_PRIVATE_PIDS (it must stop them; leftovers are reported).
#        Its exit status is returned.
set -euo pipefail

if [[ "${1:-}" == "--inside" ]]; then
    shift
    dir=$1
    shift
    : > "$dir/pids.txt"
    pids=()
    track() { pids+=("$1"); echo "$1" >> "$dir/pids.txt"; }
    cleanup() {
        for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
        for p in "${pids[@]}"; do wait "$p" 2>/dev/null || true; done
    }
    trap cleanup EXIT
    trap 'exit 130' INT TERM
    export KAI_PW_PRIVATE_PIDS="$dir/pids.txt"

    pipewire -c "$dir/pipewire.conf" > "$dir/pipewire.log" 2>&1 &
    track $!
    end=$((SECONDS + 5))
    until [[ -S "$XDG_RUNTIME_DIR/pipewire-0" ]]; do
        (( SECONDS < end )) || { echo "the private pipewire did not start" >&2; exit 1; }
        sleep 0.1
    done
    wireplumber > "$dir/wireplumber.log" 2>&1 &
    track $!
    end=$((SECONDS + 10))
    until timeout 5 pw-metadata -n default 0 default.audio.sink 2>/dev/null | grep -q kai-test-sink; do
        (( SECONDS < end )) || { echo "the private WirePlumber set no default sink" >&2; exit 1; }
        sleep 0.1
    done
    status=0
    "$@" || status=$?
    exit $status
fi

dir=${1:?usage: $0 <dir> -- <command> [args…]}
[[ "${2:-}" == "--" ]] || { echo "usage: $0 <dir> -- <command> [args…]" >&2; exit 2; }
shift 2
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)
for d in rt config state data cache; do rm -rf "${dir:?}/$d"; mkdir -p "$dir/$d"; done
chmod 700 "$dir/rt"

cat > "$dir/pipewire.conf" <<'EOF'
context.properties = {
    core.daemon = true
    core.name = pipewire-0
    default.clock.rate = 48000
    link.max-buffers = 16
    log.level = 2
}
context.spa-libs = {
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}
context.modules = [
    { name = libpipewire-module-rt flags = [ ifexists nofail ] }
    { name = libpipewire-module-protocol-native }
    { name = libpipewire-module-metadata }
    { name = libpipewire-module-spa-node-factory }
    { name = libpipewire-module-client-node }
    { name = libpipewire-module-access }
    { name = libpipewire-module-adapter }
    { name = libpipewire-module-link-factory }
    { name = libpipewire-module-session-manager }
    # The test microphone: what is played into kai-test-mic-in comes out of kai-test-mic.
    { name = libpipewire-module-loopback
        args = {
            node.description = "Kai test microphone"
            audio.position = [ MONO ]
            capture.props = { node.name = "kai-test-mic-in" media.class = Audio/Sink
                              node.description = "Kai test microphone input" priority.session = 100 }
            playback.props = { node.name = "kai-test-mic" media.class = Audio/Source
                               node.description = "Kai test microphone" priority.session = 2000 }
        }
    }
]
context.objects = [
    { factory = spa-node-factory
        args = { factory.name = support.node.driver node.name = Dummy-Driver
                 node.group = pipewire.dummy priority.driver = 200000 } }
    { factory = adapter
        args = { factory.name = support.null-audio-sink node.name = "kai-test-sink"
                 node.description = "Kai test sink" media.class = Audio/Sink
                 audio.rate = 48000 audio.channels = 1 audio.position = [ MONO ]
                 priority.session = 2000 object.linger = true } }
    # A sink that does not drive the graph: the Dummy driver does, so the playback stream, the sink
    # and a monitor of it run in the same cycle (a driving sink's monitor lags by one cycle).
    { factory = adapter
        args = { factory.name = support.null-audio-sink node.name = "kai-test-sink-follower"
                 node.description = "Kai test sink (follower)" media.class = Audio/Sink
                 audio.rate = 48000 audio.channels = 1 audio.position = [ MONO ]
                 node.driver = false priority.session = 500 object.linger = true } }
    { factory = adapter
        args = { factory.name = support.null-audio-sink node.name = "kai-test-sink-2"
                 node.description = "Kai test sink 2" media.class = Audio/Sink
                 audio.rate = 48000 audio.channels = 1 audio.position = [ MONO ]
                 priority.session = 1000 object.linger = true } }
]
EOF
mkdir -p "$dir/config/wireplumber/wireplumber.conf.d"
cat > "$dir/config/wireplumber/wireplumber.conf.d/90-kai-private.conf" <<'EOF'
# A private WirePlumber: no hardware, no portals, no logind; default nodes and linking only.
wireplumber.profiles = {
  main = {
    hardware.audio = disabled
    hardware.bluetooth = disabled
    hardware.video-capture = disabled
    support.reserve-device = disabled
    monitor.alsa.reserve-device = disabled
    support.portal-permissionstore = disabled
    script.client.access-portal = disabled
    support.logind = disabled
    monitor.bluez.seat-monitoring = disabled
  }
}
EOF

# The private bus activates nothing: no service directories, so no helper (a notification server,
# gvfsd, the accessibility bus) is started on it, and none can outlive it.
cat > "$dir/session-bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$dir/rt</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF

status=0
# env -i: nothing of the session (Wayland, X11, its D-Bus, its PipeWire) reaches the command.
# GIO_USE_VFS/NO_AT_BRIDGE: GIO and GTK do not even ask for gvfsd or the accessibility bus.
env -i HOME="$HOME" PATH="$PATH" USER="${USER:-}" LANG="${LANG:-C.UTF-8}" \
    XDG_RUNTIME_DIR="$dir/rt" PIPEWIRE_RUNTIME_DIR="$dir/rt" \
    XDG_CONFIG_HOME="$dir/config" XDG_STATE_HOME="$dir/state" \
    XDG_DATA_HOME="$dir/data" XDG_CACHE_HOME="$dir/cache" \
    GIO_USE_VFS=local NO_AT_BRIDGE=1 GTK_A11Y=none \
    dbus-run-session --config-file="$dir/session-bus.conf" -- bash "$0" --inside "$dir" "$@" \
    || status=$?

left=0
while read -r pid; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then echo "LEFT RUNNING: $pid" >&2; left=1; fi
done < "$dir/pids.txt"
rm -rf "${dir:?}/rt"
if [[ $left == 0 ]]; then echo "pw-private: everything started in $dir is stopped"; fi
exit $status
