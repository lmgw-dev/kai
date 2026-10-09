#!/usr/bin/env python3
"""WP13 (b), the owner's probe: Klipper's D-Bus history calls in Plasma 6 (`clipboard_history`).

usage: klipper_probe.py [--show]

Read-only. Reports who owns org.kde.klipper, the methods on /klipper, how many history items
getClipboardHistoryMenu and getClipboardHistoryItem return, their lengths, how many are empty
(non-text entries such as images come back as empty strings, if they do), whether the two calls
agree, and how long they take. Prints no clipboard text unless --show is given (then the first
40 characters of each item, on the terminal only).
"""
import hashlib
import sys
import time

import dbus


def main():
    show = "--show" in sys.argv
    bus = dbus.SessionBus()
    dbo = dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus"), "org.freedesktop.DBus")
    if not dbo.NameHasOwner("org.kde.klipper"):
        print("org.kde.klipper: no owner on this bus (clipboard applet not running?)")
        return 1
    owner = dbo.GetNameOwner("org.kde.klipper")
    pid = int(dbo.GetConnectionUnixProcessID(owner))
    try:
        with open("/proc/%d/comm" % pid) as f:
            comm = f.read().strip()
    except OSError:
        comm = "?"
    print("org.kde.klipper owned by %s (pid %d, %s)" % (owner, pid, comm))
    obj = bus.get_object("org.kde.klipper", "/klipper")
    xml = dbus.Interface(obj, "org.freedesktop.DBus.Introspectable").Introspect()
    import xml.etree.ElementTree as ET

    root = ET.fromstring(xml)
    for iface in root.findall("interface"):
        if iface.get("name").startswith("org.freedesktop"):
            continue
        sigs = []
        for m in iface.findall("method"):
            ins = "".join(a.get("type") for a in m.findall("arg") if a.get("direction", "in") == "in")
            outs = "".join(a.get("type") for a in m.findall("arg") if a.get("direction") == "out")
            sigs.append("%s(%s)%s" % (m.get("name"), ins, "->" + outs if outs else ""))
        print("interface %s: %s" % (iface.get("name"), ", ".join(sigs)))
    k = dbus.Interface(obj, "org.kde.klipper.klipper")
    t0 = time.monotonic()
    menu = [str(s) for s in k.getClipboardHistoryMenu()]
    t_menu = (time.monotonic() - t0) * 1000
    print("getClipboardHistoryMenu: %d items in %.1f ms" % (len(menu), t_menu))
    items = []
    t0 = time.monotonic()
    i = 0
    while True:
        s = str(k.getClipboardHistoryItem(i))
        if s == "" and i >= len(menu):
            break
        items.append(s)
        i += 1
        if i > len(menu) + 5:  # past the menu's count and still answering: report, do not loop on
            break
    t_items = (time.monotonic() - t0) * 1000
    print("getClipboardHistoryItem(0..%d): %d items in %.1f ms" % (i - 1, len(items), t_items))
    lengths = [len(s) for s in items]
    print("lengths: %s" % lengths)
    print("empty items: %d; multi-line items: %d; longest: %d chars" % (
        sum(1 for s in items if not s), sum(1 for s in items if "\n" in s), max(lengths or [0])))
    same = sum(1 for a, b in zip(menu, items) if a == b)
    print("menu and item calls agree on %d of %d" % (same, min(len(menu), len(items))))
    cur = str(k.getClipboardContents())
    print("getClipboardContents: %d chars, equals item 0: %s" % (len(cur), bool(items) and cur == items[0]))
    print("item fingerprints (sha1, 8 hex): %s" % [hashlib.sha1(s.encode()).hexdigest()[:8] for s in items])
    if show:
        for n, s in enumerate(items):
            print("  %2d %r" % (n, s[:40]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
