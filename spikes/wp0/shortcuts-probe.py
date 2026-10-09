#!/usr/bin/env python3
"""WP0 (f): XDG GlobalShortcuts portal on KWin 6. Binds ONE test shortcut (the portal shows its bind dialog), then listens
and reports whether Activated/Deactivated arrive and whether a held key repeats Activated.
usage: shortcuts-probe.py [--listen SECS] [--kglobalaccel]    (run it inside a scope named app-<appid>-<n>.scope; owner-checks.sh does)
--kglobalaccel skips the portal and uses org.kde.kglobalaccel directly (Pressed/Released on the component object); written
from the KGlobalAccel D-Bus API, only meaningful as a fallback if the portal run found no Deactivated."""
import sys, time, os
from gi.repository import Gio, GLib

listen = float(sys.argv[sys.argv.index("--listen") + 1]) if "--listen" in sys.argv else 25.0
bus = Gio.bus_get_sync(Gio.BusType.SESSION)
loop = GLib.MainLoop()
events = []  # (monotonic, kind)
T0 = time.monotonic()
def log(m): print(f"[{time.monotonic()-T0:6.2f}s] {m}", flush=True)

def wait(cond, timeout):
    end = time.monotonic() + timeout
    ctx = GLib.MainContext.default()
    while time.monotonic() < end and not cond():
        ctx.iteration(False); time.sleep(0.01)
    return cond()

def listen_phase(seconds, what):
    log(f"listening {seconds:.0f} s: {what}")
    wait(lambda: False, seconds)

def verdict(kind, text):
    print(f"VERDICT {kind}: {text}", flush=True)

def analyse(label):
    acts = [t for t, k in events if k == "A"]; deacts = [t for t, k in events if k == "D"]
    run = best = 0
    for _, k in events:
        run = run + 1 if k == "A" else 0
        best = max(best, run)
    seq = "".join(k for _, k in events)
    repeat = best > 1
    s = (f"{label}: Activated x{len(acts)}, Deactivated x{len(deacts)}, sequence {seq or '-'}; "
         f"{'Deactivated arrives' if deacts else 'NO Deactivated'}; "
         f"{'held key REPEATS Activated (max ' + str(best) + ' in a row)' if repeat else 'no Activated repeat while held'}")
    if deacts and acts:
        s += f"; first press->release {1000*(deacts[0]-acts[0]):.0f} ms"
    verdict("f", s)

if "--kglobalaccel" in sys.argv:
    comp, act = "dev.rootshell.kaiwp0test-kga", "wp0-test"
    aid = GLib.Variant("as", [comp, act, "Kai WP0 test (KGA)", "Kai WP0 test"])
    KGA = ("org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel")
    bus.call_sync(*KGA, "doRegister", GLib.Variant("(as)", (aid.unpack(),)), None, 0, -1, None)
    key = 0x04000000 | 0x02000000 | 0x01000038  # Ctrl+Shift+F9 (Qt::CTRL | Qt::SHIFT | Qt::Key_F9)
    for fl in (6 | 8, 6):
        bus.call_sync(*KGA, "setShortcut", GLib.Variant("(asaiu)", (aid.unpack(), [key], fl)), None, 0, -1, None)
    path = bus.call_sync(*KGA, "getComponent", GLib.Variant("(s)", (comp,)), None, 0, -1, None).unpack()[0]
    log(f"component {path}; shortcut Ctrl+Shift+F9 registered")
    def sig(_c, _s, _p, _i, name, params):
        events.append((time.monotonic(), "A" if name == "globalShortcutPressed" else "D")); log(name)
    bus.signal_subscribe("org.kde.kglobalaccel", "org.kde.kglobalaccel.Component", None, path, None, 0, sig)
    print("PRESS Ctrl+Shift+F9: once briefly, then hold it for 3 s, then release.", flush=True)
    listen_phase(listen, "press once, then hold 3 s")
    analyse("KGlobalAccel")
    try: bus.call_sync(*KGA, "unregister", GLib.Variant("(ss)", (comp, act)), None, 0, -1, None)
    except Exception as e: log(f"unregister: {e}")
    sys.exit(0)

P = ("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop")
GS = "org.freedesktop.portal.GlobalShortcuts"
me = bus.get_unique_name().lstrip(":").replace(".", "_")
responses = {}
def on_resp(_c, _s, path, _i, _n, params):
    responses[path] = params.unpack()
bus.signal_subscribe(P[0], "org.freedesktop.portal.Request", "Response", None, None, 0, on_resp)
def on_act(_c, _s, _p, _i, name, params):
    events.append((time.monotonic(), "A" if name == "Activated" else "D")); log(f"{name} {params.unpack()[1]}")
bus.signal_subscribe(P[0], GS, "Activated", None, None, 0, on_act)
bus.signal_subscribe(P[0], GS, "Deactivated", None, None, 0, on_act)

def req(method, params, token, timeout):
    path = f"/org/freedesktop/portal/desktop/request/{me}/{token}"
    bus.call_sync(P[0], P[1], GS, method, params, None, 0, -1, None)
    if not wait(lambda: path in responses, timeout):
        return None
    return responses[path]

log(f"app cgroup: {open('/proc/self/cgroup').read().strip().splitlines()[-1]}")
try:
    r = req("CreateSession", GLib.Variant("(a{sv})", ({"handle_token": GLib.Variant("s", "h1"), "session_handle_token": GLib.Variant("s", "s1")},)), "h1", 10)
except GLib.Error as e:
    verdict("f", f"CreateSession failed: {e.message}"); sys.exit(1)
if not r or r[0] != 0:
    verdict("f", f"CreateSession response {r}"); sys.exit(1)
session = r[1]["session_handle"]
log(f"session {session}")
log("BIND DIALOG: the portal asks you to confirm the test shortcut. Pick a key with a normal key part (e.g. Ctrl+Shift+F9), not a bare modifier. You have 60 s.")
r = req("BindShortcuts", GLib.Variant("(oa(sa{sv})sa{sv})", (session, [("wp0-test", {"description": GLib.Variant("s", "Kai WP0 test"), "preferred_trigger": GLib.Variant("s", "CTRL+SHIFT+F9")})], "", {"handle_token": GLib.Variant("s", "h2")})), "h2", 60)
if not r or r[0] != 0:
    verdict("f", f"BindShortcuts did not complete (response {r}); portal path FAILED or dialog cancelled -> run with --kglobalaccel")
    sys.exit(1)
log(f"bound: {r[1]}")
try:
    out = bus.call_sync(P[0], P[1], GS, "ListShortcuts", GLib.Variant("(oa{sv})", (session, {"handle_token": GLib.Variant("s", "h3")})), None, 0, -1, None)
    wait(lambda: any(k.endswith("/h3") for k in responses), 3)
    lst = [v for k, v in responses.items() if k.endswith("/h3")]
    log(f"ListShortcuts: {lst[0][1] if lst else out.unpack()}")
except GLib.Error as e:
    log(f"ListShortcuts: {e.message}")
print("NOW: press your shortcut once briefly, wait 2 s, then HOLD it for 3 s, then release.", flush=True)
listen_phase(listen, "press briefly, then hold 3 s")
analyse("portal")
try: bus.call_sync(P[0], session, "org.freedesktop.portal.Session", "Close", None, None, 0, -1, None)
except GLib.Error: pass
