#!/usr/bin/env python3
"""WP13 (f): runs one KWin script through org.kde.kwin.Scripting and prints its result.

usage: kwin_script.py <script.js> [ARGS as JSON]

The path Kai would take: write the script to a file of its own, loadScript(path, plugin name),
run it, get the answer back on Kai's own bus name (the script calls callDBus; KWin scripts cannot
return a value to loadScript's caller), then unloadScript. Prints one JSON line: the script's
result plus the round trip in ms. Only ever run inside spikes/p3/nested.sh.
"""
import json
import os
import sys
import time

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

NAME = "dev.rootshell.KaiP3"


class Receiver(dbus.service.Object):
    def __init__(self, bus, loop):
        super().__init__(bus, "/")
        self.loop = loop
        self.result = None

    @dbus.service.method(NAME, in_signature="s", out_signature="")
    def Result(self, s):
        self.result = s
        self.loop.quit()


def main():
    if not os.environ.get("P3_DIR"):
        sys.exit("refused: not inside spikes/p3/nested.sh (P3_DIR unset)")
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    _name = dbus.service.BusName(NAME, bus)
    loop = GLib.MainLoop()
    recv = Receiver(bus, loop)
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    args["reply_to"] = NAME
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "kwin-scripts", "common.js")) as f:
        common = f.read().replace("__ARGS__", json.dumps(args))
    with open(sys.argv[1]) as f:
        body = f.read()
    path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "kai-p3-%d.js" % os.getpid())
    with open(path, "w") as f:
        f.write(common + "\n" + body)
    plugin = "kai-p3-%d" % os.getpid()
    scripting = dbus.Interface(bus.get_object("org.kde.KWin", "/Scripting"), "org.kde.kwin.Scripting")
    t0 = time.monotonic()
    sid = int(scripting.loadScript(path, plugin, signature="ss"))
    if sid < 0:
        print(json.dumps({"ok": False, "error": "loadScript returned %d" % sid}))
        return 1
    script = dbus.Interface(bus.get_object("org.kde.KWin", "/Scripting/Script%d" % sid), "org.kde.kwin.Script")
    script.run()
    GLib.timeout_add(5000, loop.quit)
    loop.run()
    ms = round((time.monotonic() - t0) * 1000, 1)
    unloaded = bool(scripting.unloadScript(plugin))
    os.unlink(path)
    if recv.result is None:
        print(json.dumps({"ok": False, "error": "no result within 5 s", "script_id": sid}))
        return 1
    out = json.loads(recv.result)
    out["_round_trip_ms"] = ms
    out["_script_id"] = sid
    out["_unloaded"] = unloaded
    print(json.dumps(out))
    return 0 if out.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
