#!/usr/bin/env python3
"""WP13 (a), the owner's probe: AT-SPI trees of Qt and Chromium apps on Plasma 6 Wayland, and what
enabling the accessibility bus costs.

usage: atspi_probe.py [--enable] [--secs N] [--walk-budget S] [--apps a,b,…]

Read-only by default: reports org.a11y.Status (IsEnabled, ScreenReaderEnabled) and the apps
registered on the AT-SPI bus with the size of their trees. With --enable it switches IsEnabled on,
then ScreenReaderEnabled (Qt 6 follows only the latter, or QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1),
reports the trees after each, restores both values on exit (also on Ctrl+C), and measures CPU
and memory of the named apps' processes over --secs seconds in each state.
Prints only counts, roles, process names and timings: no window titles, no node names, no text.
"""
import argparse
import collections
import os
import signal
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
import dbus  # noqa: E402
from gi.repository import Atspi, GLib  # noqa: E402

DEFAULT_APPS = "dolphin,kate,konsole,systemsettings,plasmashell,firefox,chromium-browser,chrome,code,kwrite,okular"


def status_props(bus):
    obj = bus.get_object("org.a11y.Bus", "/org/a11y/bus")
    props = dbus.Interface(obj, "org.freedesktop.DBus.Properties")
    return props, {k: bool(props.Get("org.a11y.Status", k)) for k in ("IsEnabled", "ScreenReaderEnabled")}


def pids_of(names):
    out = {}
    for n in names:
        r = subprocess.run(["pgrep", "-u", str(os.getuid()), "-x", n], capture_output=True, text=True)
        pids = [int(p) for p in r.stdout.split()]
        if pids:
            out[n] = pids
    return out


def proc_sample(pids):
    ticks = 0
    rss_kb = 0
    for p in pids:
        try:
            with open("/proc/%d/stat" % p) as f:
                fields = f.read().rsplit(")", 1)[1].split()
            ticks += int(fields[11]) + int(fields[12])
            with open("/proc/%d/status" % p) as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        rss_kb += int(line.split()[1])
        except (FileNotFoundError, ProcessLookupError):
            pass
    return ticks, rss_kb


def measure(apps, secs):
    hz = os.sysconf("SC_CLK_TCK")
    start = {n: proc_sample(p) for n, p in apps.items()}
    time.sleep(secs)
    res = {}
    for n, p in apps.items():
        t1, rss = proc_sample(p)
        res[n] = (100.0 * (t1 - start[n][0]) / hz / secs, rss / 1024.0)
    return res


def walk(app, budget_s):
    t0 = time.monotonic()
    nodes = 0
    depth_max = 0
    roles = collections.Counter()
    with_text = 0
    stack = [(app, 0)]
    complete = True
    while stack:
        if time.monotonic() - t0 > budget_s:
            complete = False
            break
        node, depth = stack.pop()
        nodes += 1
        depth_max = max(depth_max, depth)
        try:
            roles[node.get_role_name()] += 1
            if node.get_text_iface() is not None:
                with_text += 1
            n = node.get_child_count()
            for i in range(n):
                c = node.get_child_at_index(i)
                if c is not None:
                    stack.append((c, depth + 1))
        except GLib.Error:
            pass
    return nodes, depth_max, roles, with_text, time.monotonic() - t0, complete


def tree_report(budget_s, title):
    print("\n## %s" % title)
    desktop = Atspi.get_desktop(0)
    n = desktop.get_child_count()
    print("apps on the AT-SPI bus: %d" % n)
    for i in range(n):
        app = desktop.get_child_at_index(i)
        if app is None:
            continue
        try:
            name = app.get_name()
            pid = app.get_process_id()
            toolkit = app.get_toolkit_name()
        except GLib.Error as e:
            print("  app %d: error %s" % (i, e))
            continue
        nodes, depth, roles, with_text, secs, complete = walk(app, budget_s)
        top = ", ".join("%s %d" % rc for rc in roles.most_common(6))
        print("  %-22s pid %-7d toolkit %-6s nodes %-6d%s depth %-3d text-ifaces %-5d walk %.2f s; roles: %s" % (
            name[:22], pid, toolkit, nodes, "" if complete else "+ (budget hit)", depth, with_text, secs, top))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--enable", action="store_true",
                    help="step through IsEnabled, then ScreenReaderEnabled, restore both after")
    ap.add_argument("--secs", type=float, default=10.0, help="CPU sampling window per phase (default 10)")
    ap.add_argument("--walk-budget", type=float, default=20.0,
                    help="seconds per app tree walk (default 20; a hit budget is printed)")
    ap.add_argument("--apps", default=DEFAULT_APPS, help="process names to measure (comma list)")
    a = ap.parse_args()
    bus = dbus.SessionBus()
    try:
        props, before = status_props(bus)
    except dbus.DBusException as e:
        print("org.a11y.Bus not reachable: %s" % e)
        return 1
    print("org.a11y.Status before: %s" % before)
    apps = pids_of([x for x in a.apps.split(",") if x])
    print("measured processes: %s" % {n: len(p) for n, p in apps.items()})

    def put_back(*sig):
        if a.enable:
            for k, v in before.items():
                props.Set("org.a11y.Status", k, dbus.Boolean(v))
            print("org.a11y.Status restored to %s" % status_props(bus)[1])
        if sig:
            sys.exit(130)

    signal.signal(signal.SIGINT, put_back)
    signal.signal(signal.SIGTERM, put_back)
    Atspi.init()
    tree_report(a.walk_budget, "trees as they are now (%s)" % before)
    if not a.enable:
        print("\n(read-only run; --enable steps through the two switches and measures their cost)")
        return 0
    if before["IsEnabled"] and before["ScreenReaderEnabled"]:
        print("\nboth switches are already on: the cost cannot be measured against 'off' in this session")
        return 0
    costs = {}
    try:
        print("\n## idle cost as it is now (%.0f s)" % a.secs)
        costs["now"] = measure(apps, a.secs)
        for k in ("IsEnabled", "ScreenReaderEnabled"):
            if before[k]:
                continue
            props.Set("org.a11y.Status", k, dbus.Boolean(True))
            print("\n%s set to True; waiting 5 s for apps to register" % k)
            time.sleep(5)
            tree_report(a.walk_budget, "trees with %s" % status_props(bus)[1])
            print("\n## idle cost after the walks (%.0f s)" % a.secs)
            costs["+" + k] = measure(apps, a.secs)
        cols = list(costs)
        print("\n  %-18s %s %s" % ("process", " ".join("%18s" % ("CPU% " + c) for c in cols),
                                    " ".join("%20s" % ("RSS MB " + c) for c in cols)))
        for n in apps:
            print("  %-18s %s %s" % (n, " ".join("%18.2f" % costs[c][n][0] for c in cols),
                                     " ".join("%20.1f" % costs[c][n][1] for c in cols)))
    finally:
        put_back()
    return 0


if __name__ == "__main__":
    sys.exit(main())
