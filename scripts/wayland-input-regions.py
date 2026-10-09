#!/usr/bin/env python3
# The widgets' input regions from a WAYLAND_DEBUG=client log of the live check's input run
# (src/widgets/live_tests/hardening.rs, `input`): the n-th "kai-widget" layer surface is the n-th
# `INPUT <id> <w>x<h> <interactive>` line of the run's output. The region in effect is followed
# commit by commit (wl_surface's double-buffered state: the last set_input_region before a commit).
# A click-through widget's surface, at every commit once it has a buffer, has an empty region (no
# rectangle; never nil or no region at all, which is "the whole surface", nor the ring's 1x1 px
# stand-in); an interactive one's region at its last commit is its whole rectangle; the app view
# ("guarded") has its rectangle, then none while Kai's K15 dialog was open, then its rectangle.
#
# usage: scripts/wayland-input-regions.py <wayland.log> <input.log>   (exit 1 on a failed check)
import re
import sys

REQ = re.compile(r"-> (\w+)#(\d+)\.(\w+)\((.*)\)\s*$")

wayland, run = sys.argv[1], sys.argv[2]
widgets = []
for line in open(run, encoding="utf-8", errors="replace"):
    m = re.match(r"INPUT (\d+) (\d+)x(\d+) (true|false|guarded)", line.strip())
    if m:
        widgets.append((int(m[1]), int(m[2]), int(m[3]), m[4]))

regions = {}  # wl_region id -> rectangles since its create_region
tracked = {}  # wl_surface id -> index of the kai-widget surface it is
# Per kai-widget surface: the region in effect at each commit once it had a buffer ("whole" for
# nil or never set), and the pending state.
sets = []
NONE_SET = "whole"
for line in open(wayland, encoding="utf-8", errors="replace"):
    m = REQ.search(line)
    if not m:
        continue
    iface, oid, req, args = m[1], m[2], m[3], m[4]
    if iface == "wl_compositor" and req == "create_region":
        r = re.search(r"wl_region#(\d+)", args)
        if r:
            regions[r[1]] = []
    elif iface == "wl_region" and req == "add":
        regions.setdefault(oid, []).append(tuple(int(x) for x in args.split(",")))
    elif iface.endswith("layer_shell_v1") and req == "get_layer_surface" and '"kai-widget"' in args:
        s = re.search(r"wl_surface#(\d+)", args)
        if s:
            tracked[s[1]] = len(sets)
            sets.append({"commits": [], "pending": None, "current": NONE_SET, "buffer": False,
                         "attached": False})
    elif iface == "wl_surface" and oid in tracked:
        st = sets[tracked[oid]]
        if req == "set_input_region":
            r = re.search(r"wl_region#(\d+)", args)
            st["pending"] = list(regions.get(r[1], [])) if r else NONE_SET
        elif req == "attach":
            st["attached"] = not args.startswith("nil")
        elif req == "commit":
            if st["pending"] is not None:
                st["current"], st["pending"] = st["pending"], None
            st["buffer"] = st["buffer"] or st["attached"]
            if st["buffer"]:
                st["commits"].append(st["current"])
        elif req == "destroy":
            del tracked[oid]

failed = 0


def runs_of(given, whole):
    """The regions in effect commit by commit, as runs: [("rect", 3), ("none", 5), ...]."""
    out = []
    for g in given:
        label = "rect" if g == whole else "none" if g == [] else str(g)
        if out and out[-1][0] == label:
            out[-1] = (label, out[-1][1] + 1)
        else:
            out.append((label, 1))
    return out


def check(ok, what):
    global failed
    failed += not ok
    print(("ok  " if ok else "FAIL") + " " + what)


check(len(widgets) == 3 and len(sets) == 3,
      f"input: three widgets in the run ({len(widgets)}) and three kai-widget surfaces in the log "
      f"({len(sets)})")
for (wid, w, h, how), st in zip(widgets, sets):
    given = st["commits"]
    whole = [(0, 0, w, h)]
    if how == "guarded":
        # Its rectangle, then none while Kai's dialog was open, then its rectangle again.
        first = given.index(whole) if whole in given else None
        held = given.index([], first) if first is not None and [] in given[first:] else None
        check(first is not None and held is not None and given[-1] == whole
              and whole in given[held:],
              f"input: app view {wid} takes clicks ({w}x{h}), none while a K15 dialog is open, "
              f"and clicks again after ({len(given)} commits with a buffer, in runs: "
              f"{runs_of(given, whole)})")
    elif how == "true":
        check(bool(given) and given[-1] == [(0, 0, w, h)],
              f"input: interactive widget {wid}'s region is its {w}x{h} rectangle "
              f"({len(given)} commits with a buffer; at the last: {given[-1] if given else None})")
    else:
        check(bool(given) and all(g == [] for g in given),
              f"input: click-through widget {wid} takes no click: an empty region at each of its "
              f"{len(given)} commits with a buffer (no nil, no pixel; in effect: "
              f"{sorted(set(map(str, given)))})")
sys.exit(1 if failed else 0)
