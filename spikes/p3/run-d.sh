#!/usr/bin/env bash
# WP13 (d): does KWin grant the restricted interfaces (ruling 16) to a dev build through a .desktop
# file in the nested session's data directories? Each case runs in a fresh nested KWin; the probe
# is this spike's debug binary, the grant is checked on the Wayland side (`p3 globals`) and the
# D-Bus side (ScreenShot2.CaptureWorkspace from the same binary).
# usage: spikes/p3/run-d.sh            (output: spikes/p3/.run/d.log)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$here/target/debug/p3
[[ -x $bin ]] || { echo "build first: (cd $here && cargo build --offline -j 4)" >&2; exit 1; }
mkdir -p "$here/.run"
log=$here/.run/d.log
: > "$log"

desktop() { # <exec line> → a .desktop with ruling 16's keys
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Kai (WP13 probe)
Exec=$1
NoDisplay=true
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
X-KDE-Wayland-Interfaces=org_kde_plasma_window_management,zkde_screencast_unstable_v1,org_kde_kwin_fake_input
EOF
}
# The check runs inside the nested session (a clean environment), so it lives in a file there.
cat > "$here/.run/d-check.sh" <<EOF
check() { # <label>: the probe's view from inside the nested session
    {
        echo "== \$1"
        "$bin" globals | grep RESTRICTED
        "$bin" shot workspace | sed -n '1p'
    } 2>&1 | tee -a "$log"
}
EOF

seed=$here/.run/d-seed
run_case() { # <name> <seed exec or ""> -- <inside script>
    local name=$1 exec=$2 script=$3
    rm -rf "$seed"; mkdir -p "$seed/applications"
    [[ -z $exec ]] || desktop "$exec" > "$seed/applications/dev.rootshell.kai-p3.desktop"
    "$here/nested.sh" "d-$name" --seed "$seed" -- bash -c ". $here/.run/d-check.sh; $script"
}

# 1. no .desktop at all
run_case none "" 'check "1 no .desktop"'
# 2. present before KWin starts, absolute Exec with a field code (as packaging/ writes it)
run_case before "$bin %u" 'check "2 .desktop before KWin start, Exec=<abs> %u"'
# 3. written after KWin started (KSycoca must notice); a connection made before it keeps its answer
script3=$(cat <<EOF
check "3a before writing"
mkdir -p "\$XDG_DATA_HOME/applications"
cat > "\$XDG_DATA_HOME/applications/dev.rootshell.kai-p3.desktop" <<'D'
$(desktop "$bin %u")
D
check "3b immediately after writing"
t0=\$(date +%s%N)
for i in \$(seq 100); do "$bin" globals | grep -q 'fake_input: offered' && break; sleep 0.05; done
echo "3 granted to a new connection \$(( (\$(date +%s%N) - t0) / 1000000 )) ms after the write" | tee -a "$log"
sleep 2
check "3c 2 s after writing"
rm "\$XDG_DATA_HOME/applications/dev.rootshell.kai-p3.desktop"
sleep 2
check "3d 2 s after removing it"
EOF
)
run_case after "" "$script3"
# 4. Exec through a symlink to the binary
ln -sfn "$bin" "$here/.run/p3-link"
run_case symlink "$here/.run/p3-link" 'check "4 Exec=<symlink to the binary>"'
# 5. Exec as a bare name found on PATH
run_case bare "p3 %u" "export PATH=$here/target/debug:\$PATH; check '5 Exec=p3 (bare name on PATH)'"
# 6. a .desktop for another binary does not leak to this one
run_case other "/usr/bin/true" 'check "6 .desktop naming another binary"'
echo "done: $log"
