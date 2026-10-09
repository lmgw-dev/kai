#!/usr/bin/env bash
# WP13 (a) pre-test of the owner's probe, against test apps in a nested KWin only: a private
# AT-SPI bus (at-spi-bus-launcher on the nested private bus, the registry started by hand), Qt 6
# apps (kdialog) started before and after accessibility is switched on, and a GTK3 window.
# It proves atspi_probe.py runs and shows the mechanism; the verdict needs the owner's session
# (owner-a.sh), where the real apps, Chromium and the real cost are.
# usage: spikes/p3/run-a-pretest.sh     (output: spikes/p3/.run/a-pretest.log)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
if [[ ${1:-} == --inside ]]; then
    set +e
    unset NO_AT_BRIDGE
    say() { echo; echo "## $*"; }
    # org.a11y.Status is backed by GSettings; the private bus has no dconf service, so keep it in memory
    GSETTINGS_BACKEND=memory /usr/libexec/at-spi-bus-launcher > "$P3_DIR/a11y.log" 2>&1 &
    for i in $(seq 50); do busctl --user status org.a11y.Bus > /dev/null 2>&1 && break; sleep 0.1; done
    # the a11y bus cannot activate the registry here (its activation goes through systemd), so start it
    /usr/libexec/at-spi2-registryd > "$P3_DIR/registryd.log" 2>&1 &
    sleep 1
    kdialog --inputbox "Qt A" "" > /dev/null 2>&1 &
    echo "Qt A (plain) pid $!"
    QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 kdialog --inputbox "Qt B" "" > /dev/null 2>&1 &
    echo "Qt B (QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1) pid $!"
    "$here/target/debug/p3" testwin org.example.gtk Gtk 30,160,60 60 > /dev/null 2>&1 &
    sleep 2
    say "IsEnabled false"
    python3 "$here/atspi_probe.py" --apps kdialog | grep -E 'apps on|pid'
    say "IsEnabled set true (3 s later)"
    busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true
    sleep 3
    python3 "$here/atspi_probe.py" --apps kdialog | grep -E 'apps on|pid'
    kdialog --inputbox "Qt C" "" > /dev/null 2>&1 &
    echo "Qt C (started while enabled) pid $!"
    sleep 2
    say "after Qt C started"
    python3 "$here/atspi_probe.py" --apps kdialog | grep -E 'apps on|pid'
    say "ScreenReaderEnabled set true (3 s later)"
    busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status ScreenReaderEnabled b true
    sleep 3
    python3 "$here/atspi_probe.py" --apps kdialog | grep -E 'apps on|pid'
    busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status ScreenReaderEnabled b false
    sleep 1
    say "ScreenReaderEnabled back to false (1 s later)"
    python3 "$here/atspi_probe.py" --apps kdialog | grep -E 'apps on|pid'
    busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b false
    say "the probe's --enable mode (cost columns)"
    python3 "$here/atspi_probe.py" --enable --secs 3 --apps kdialog,p3 | grep -vE '^\s*$'
    echo "after: IsEnabled $(busctl --user get-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled), ScreenReaderEnabled $(busctl --user get-property org.a11y.Bus /org/a11y/bus org.a11y.Status ScreenReaderEnabled)"
    kill $(jobs -p) 2>/dev/null; wait
    exit 0
fi
"$here/nested.sh" a-pretest -- "$0" --inside 2>&1 | tee "$here/.run/a-pretest.log"
