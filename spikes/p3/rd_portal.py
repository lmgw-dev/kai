#!/usr/bin/env python3
"""WP13 (h): types a text through the RemoteDesktop portal (NotifyKeyboardKeysym), the way Kai's
`type_text` would without a fake-input grant of its own.

usage: rd_portal.py <app id> <text> [--start-timeout S] [--keycodes] [--persist] [--restore-token T]

Registers <app id> with the host portal registry (needs a .desktop file of that name), creates a
session, selects the keyboard, starts it (portal-kde asks unless the app is pre-authorised in the
permission store's kde-authorized/remote-desktop entry), then sends one keysym press and release
per character (--keycodes: evdev codes for ASCII letters instead). Prints timings and results.
Only ever run inside spikes/p3/nested.sh.
"""
import os
import sys
import time

import dbus
import dbus.mainloop.glib
from gi.repository import GLib


def keysym_of(ch):
    c = ord(ch)
    if ch == "\n":
        return 0xFF0D  # Return
    if 0x20 <= c <= 0x7E or 0xA0 <= c <= 0xFF:
        return c
    return 0x01000000 + c


def main():
    if not os.environ.get("P3_DIR"):
        sys.exit("refused: not inside spikes/p3/nested.sh (P3_DIR unset)")
    app_id, text = sys.argv[1], sys.argv[2]
    start_timeout = 8.0
    if "--start-timeout" in sys.argv:
        start_timeout = float(sys.argv[sys.argv.index("--start-timeout") + 1])
    use_keycodes = "--keycodes" in sys.argv
    select = {"types": dbus.UInt32(1)}
    if "--persist" in sys.argv:
        select["persist_mode"] = dbus.UInt32(2)
    if "--restore-token" in sys.argv:
        select["restore_token"] = sys.argv[sys.argv.index("--restore-token") + 1]
        select.setdefault("persist_mode", dbus.UInt32(2))
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    loop = GLib.MainLoop()
    sender = bus.get_unique_name()[1:].replace(".", "_")
    portal = bus.get_object("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop")
    t0 = time.monotonic()

    def log(msg):
        print("rd t=%6.0f %s" % ((time.monotonic() - t0) * 1000, msg), flush=True)

    try:
        dbus.Interface(portal, "org.freedesktop.host.portal.Registry").Register(app_id, dbus.Dictionary({}, signature="sv"))
        log("registered as %s" % app_id)
    except dbus.DBusException as e:
        log("Register failed: %s" % e)
    rd = dbus.Interface(portal, "org.freedesktop.portal.RemoteDesktop")
    props = dbus.Interface(portal, "org.freedesktop.DBus.Properties")
    log("RemoteDesktop version %s, AvailableDeviceTypes %s" % (
        props.Get("org.freedesktop.portal.RemoteDesktop", "version"),
        props.Get("org.freedesktop.portal.RemoteDesktop", "AvailableDeviceTypes")))
    counter = [0]

    def request(call, args, options, timeout):
        counter[0] += 1
        token = "p3_%d_%d" % (os.getpid(), counter[0])
        path = "/org/freedesktop/portal/desktop/request/%s/%s" % (sender, token)
        res = {}

        def on_response(code, results):
            res["code"] = int(code)
            res["results"] = results
            loop.quit()

        match = bus.add_signal_receiver(on_response, "Response", "org.freedesktop.portal.Request", path=path)
        options = dict(options)
        options["handle_token"] = token
        call(*args, dbus.Dictionary(options, signature="sv"))
        tid = GLib.timeout_add(int(timeout * 1000), loop.quit)
        loop.run()
        GLib.source_remove(tid) if "code" in res else None
        match.remove()
        return res

    r = request(rd.CreateSession, [], {"session_handle_token": "p3s%d" % os.getpid()}, 5)
    if r.get("code") != 0:
        log("CreateSession: %s" % r)
        return 1
    session = r["results"]["session_handle"]
    log("session %s" % session)
    r = request(rd.SelectDevices, [dbus.ObjectPath(session)], select, 5)
    log("SelectDevices(keyboard): code %s" % r.get("code"))
    log("Start: waiting up to %.0f s (a dialog shows unless pre-authorised)" % start_timeout)
    r = request(rd.Start, [dbus.ObjectPath(session), ""], {}, start_timeout)
    if "code" not in r:
        log("Start: NO ANSWER within %.0f s" % start_timeout)
        return 2
    log("Start: code %d results %s" % (r["code"], dict(r.get("results", {}))))
    if "restore_token" in r.get("results", {}):
        print("RESTORE_TOKEN=%s" % r["results"]["restore_token"], flush=True)
    if r["code"] != 0:
        return 3
    opts = dbus.Dictionary({}, signature="sv")
    sent = 0
    for ch in text:
        try:
            if use_keycodes:
                code = {"a": 30, "b": 48, "c": 46, "d": 32, "e": 18, "f": 33, "g": 34, "h": 35, "i": 23,
                        "j": 36, "k": 37, "l": 38, "m": 50, "n": 49, "o": 24, "p": 25, "q": 16, "r": 19,
                        "s": 31, "t": 20, "u": 22, "v": 47, "w": 17, "x": 45, "y": 21, "z": 44, " ": 57}[ch]
                rd.NotifyKeyboardKeycode(dbus.ObjectPath(session), opts, dbus.Int32(code), dbus.UInt32(1))
                rd.NotifyKeyboardKeycode(dbus.ObjectPath(session), opts, dbus.Int32(code), dbus.UInt32(0))
            else:
                ks = keysym_of(ch)
                rd.NotifyKeyboardKeysym(dbus.ObjectPath(session), opts, dbus.Int32(ks), dbus.UInt32(1))
                rd.NotifyKeyboardKeysym(dbus.ObjectPath(session), opts, dbus.Int32(ks), dbus.UInt32(0))
            sent += 1
        except dbus.DBusException as e:
            log("Notify failed at %r: %s" % (ch, e))
            break
        time.sleep(0.015)
    log("sent %d of %d characters" % (sent, len(text)))
    try:
        fd = rd.ConnectToEIS(dbus.ObjectPath(session), dbus.Dictionary({}, signature="sv"))
        log("ConnectToEIS on a started session: got fd %s (a libei client would use it; not driven here)" % fd.take())
    except dbus.DBusException as e:
        log("ConnectToEIS: %s" % e.get_dbus_name())
    dbus.Interface(bus.get_object("org.freedesktop.portal.Desktop", session), "org.freedesktop.portal.Session").Close()
    log("session closed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
