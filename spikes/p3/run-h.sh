#!/usr/bin/env bash
# WP13 (h): `type_text` and `key_press` through org_kde_kwin_fake_input (the probe's own grant)
# against the RemoteDesktop portal (xdg-desktop-portal + portal-kde, which holds the grant in its
# own /usr/share .desktop file). A nested KWin with a focused test window (a text entry); the
# portal stack runs on the nested private bus (it activates nothing, so it is started here).
# usage: spikes/p3/run-h.sh     (output: spikes/p3/.run/h.log)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$here/target/debug/p3
app=dev.rootshell.kai-p3
sample='Hallo Welt: äöü ß € ñ ç 日本 😀'

if [[ ${1:-} == --inside ]]; then
    set +e
    d=$P3_DIR
    say() { echo; echo "## $*"; }
    text() { sleep 0.3; grep 'TEXT' "$d/testwin.log" | tail -n 1; }
    focus_target() { "$bin" fake click 350 520 > /dev/null; sleep 0.2; }
    clear_entry() { "$bin" fake chord 29 30 > /dev/null; "$bin" fake key 14 > /dev/null; }  # ctrl+a, BackSpace
    "$bin" testwin org.example.target Target 30,160,60 180 > "$d/testwin.log" 2>&1 &
    sleep 1.5

    say "fake_input, keysyms (v6 keyboard_keysym): $sample"
    t0=$(date +%s%N)
    "$bin" fake text "$sample"
    echo "took $(( ($(date +%s%N) - t0) / 1000000 )) ms"; text
    say "fake_input, key_press ctrl+a then BackSpace (chord of evdev codes 29 30, then 14)"
    clear_entry; text
    say "fake_input, evdev codes for 'kai' (37 30 23)"
    "$bin" fake key 37 30 23 > /dev/null; text
    clear_entry

    say "fake_input into a Qt app (kdialog --inputbox, Qt 6), keysyms then Enter"
    timeout 15 kdialog --inputbox WP13 "" > "$d/kdialog-fake.out" 2> "$d/kdialog-fake.err" &
    kpid=$!
    sleep 2
    "$bin" fake text "$sample" > /dev/null
    "$bin" fake key 28 > /dev/null
    wait "$kpid"; echo "kdialog exit $?, text: $(cat "$d/kdialog-fake.out")"
    focus_target

    say "portal stack on the private bus"
    /usr/libexec/xdg-permission-store > "$d/permission-store.log" 2>&1 &
    /usr/libexec/xdg-desktop-portal-kde > "$d/portal-kde.log" 2>&1 &
    for n in org.freedesktop.impl.portal.PermissionStore org.freedesktop.impl.portal.desktop.kde; do
        for i in $(seq 100); do busctl --user status "$n" > /dev/null 2>&1 && break; sleep 0.1; done
    done
    /usr/libexec/xdg-desktop-portal > "$d/portal.log" 2>&1 &
    for i in $(seq 100); do busctl --user status org.freedesktop.portal.Desktop > /dev/null 2>&1 && break; sleep 0.1; done
    busctl --user list --no-pager | awk '{print $1}' | grep -E 'portal|Permission' | sort
    focus_target

    say "portal, NOT pre-authorised: what happens at Start (6 s)"
    python3 "$here/rd_portal.py" "$app" "nope" --start-timeout 6 > "$d/rd-noauth.log" 2>&1 &
    rpid=$!
    sleep 3
    echo "windows while Start waits:"; "$bin" windows | sed 's/ pid=.*//'
    wait "$rpid"; cat "$d/rd-noauth.log"; text

    say "pre-authorise $app in the permission store (kde-authorized / remote-desktop = yes)"
    gdbus call --session -d org.freedesktop.impl.portal.PermissionStore -o /org/freedesktop/impl/portal/PermissionStore \
        -m org.freedesktop.impl.portal.PermissionStore.SetPermission kde-authorized true remote-desktop "$app" "['yes']"
    focus_target; clear_entry
    say "portal, pre-authorised, keysyms: $sample"
    python3 "$here/rd_portal.py" "$app" "$sample"
    "$bin" windows | sed 's/ pid=.*//'
    text
    focus_target; clear_entry
    say "portal, pre-authorised, evdev keycodes: 'kai portal'"
    python3 "$here/rd_portal.py" "$app" "kai portal" --keycodes
    text
    say "portal, pre-authorised, into a Qt app (kdialog), keysyms then Enter via the portal"
    timeout 15 kdialog --inputbox WP13 "" > "$d/kdialog-portal.out" 2> "$d/kdialog-portal.err" &
    kpid=$!
    sleep 2
    python3 "$here/rd_portal.py" "$app" "$sample"$'\n' | grep -E 'Start|sent'
    wait "$kpid"; echo "kdialog exit $?, text: $(cat "$d/kdialog-portal.out")"
    focus_target
    say "pre-authorised for another app id only: $app-other"
    focus_target; clear_entry
    python3 "$here/rd_portal.py" "$app-other" "x" --start-timeout 4 2>&1 | grep -E 'Register|Start'
    say "restore token: a pre-authorised session with persist_mode 2"
    focus_target; clear_entry
    tok=$(python3 "$here/rd_portal.py" "$app" "t" --persist | tee /dev/stderr | sed -n 's/^RESTORE_TOKEN=//p')
    echo "token: ${tok:-none}"
    say "revoke the pre-authorisation (kde-authorized / remote-desktop = no), then start with the token"
    gdbus call --session -d org.freedesktop.impl.portal.PermissionStore -o /org/freedesktop/impl/portal/PermissionStore \
        -m org.freedesktop.impl.portal.PermissionStore.SetPermission kde-authorized true remote-desktop "$app" "['no']"
    focus_target; clear_entry
    if [[ -n $tok ]]; then
        python3 "$here/rd_portal.py" "$app" "r" --restore-token "$tok" --start-timeout 5 2>&1 | grep -E 'Start|sent|RESTORE'
    fi
    say "revoked, no token"
    python3 "$here/rd_portal.py" "$app" "n" --start-timeout 4 2>&1 | grep -E 'Start|sent'
    text
    say "portal-kde log (warnings)"
    grep -iE 'remote|warn|error' "$d/portal-kde.log" | grep -v '^$' | head -20
    kill $(jobs -p) 2>/dev/null; wait
    echo; echo "final:"; grep TEXT "$d/testwin.log" | tail -n 1
    exit 0
fi

[[ -x $bin ]] || { echo "build first: (cd $here && cargo build --offline -j 4)" >&2; exit 1; }
seed=$here/.run/h-seed
rm -rf "$seed"
mkdir -p "$seed/applications"
cat > "$seed/applications/$app.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kai (WP13 probe)
Exec=$bin %u
NoDisplay=true
X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input,org_kde_plasma_window_management
EOF
"$here/nested.sh" h --seed "$seed" -- "$0" --inside 2>&1 | tee "$here/.run/h.log"
