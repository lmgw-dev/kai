#!/usr/bin/env python3
"""The tray's crop of a full-desktop capture, from the real panel geometry.

Usage: scripts/tray-crop.py <capture.png>
Prints WxH+X+Y: the strip along the screen edge Plasma's panel sits on (top or bottom), the panel's
thickness plus a margin for a floating panel's gap, the screen's full width, in the capture's
pixels. Prints nothing, and why on stderr (exit 1), when that cannot be found reliably: more than
one screen, a rotated one, no single horizontal panel, or a capture of another size.

Read only: the capture's PNG header, `kscreen-doctor -j`, plasmashell's `dumpCurrentLayoutJS`
(D-Bus) and ~/.config/plasmashellrc (the panel's thickness).
"""

import configparser
import json
import os
import re
import struct
import subprocess
import sys

# Plasma's grid unit at the default font: a panel's height in the layout dump is in these.
GRID_UNIT = 18
# A floating panel's gap to the edge, and its shadow, in logical pixels.
MARGIN = 16


def give_up(why):
    print(f"tray-crop: {why}", file=sys.stderr)
    sys.exit(1)


def png_size(path):
    with open(path, "rb") as f:
        head = f.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        give_up(f"{path} is no PNG")
    return struct.unpack(">II", head[16:24])


def screen():
    out = subprocess.run(["kscreen-doctor", "-j"], capture_output=True, text=True, check=True).stdout
    outputs = [o for o in json.loads(out)["outputs"] if o.get("enabled")]
    if len(outputs) != 1:
        give_up(f"{len(outputs)} screens enabled; pass the crop as the argument")
    o = outputs[0]
    if o.get("rotation", 1) != 1:
        give_up(f"{o['name']} is rotated")
    scale = float(o.get("scale") or 1)
    return o["name"], o["size"]["width"] / scale, o["size"]["height"] / scale


def panel():
    out = subprocess.run(
        ["busctl", "--user", "--json=short", "call", "org.kde.plasmashell", "/PlasmaShell",
         "org.kde.PlasmaShell", "dumpCurrentLayoutJS"],
        capture_output=True, text=True, check=True).stdout
    js = bytes(json.loads(out)["data"][0]).decode()
    at = js.index("var layout = ") + len("var layout = ")
    layout, _ = json.JSONDecoder().raw_decode(js[at:])
    horizontal = [p for p in layout.get("panels", []) if p.get("location") in ("top", "bottom")]
    if len(horizontal) != 1:
        give_up(f"{len(horizontal)} panels on a top or bottom edge; pass the crop as the argument")
    return horizontal[0]


def thickness(p):
    """The panel's thickness in logical pixels: plasmashellrc's, when it names one panel only."""
    rc = configparser.ConfigParser(strict=False, interpolation=None)
    try:
        rc.read(os.path.expanduser("~/.config/plasmashellrc"))
    except configparser.Error:
        return round(float(p.get("height", 0)) * GRID_UNIT)
    found = {}
    for section in rc.sections():
        m = re.fullmatch(r"PlasmaViews\]\[Panel (\d+)\]\[Defaults", section.strip("[]"))
        if m and rc.has_option(section, "thickness"):
            found[m.group(1)] = rc.getint(section, "thickness")
    if len(found) == 1:
        return next(iter(found.values()))
    return round(float(p.get("height", 0)) * GRID_UNIT)


def main():
    if len(sys.argv) != 2:
        give_up("usage: tray-crop.py <capture.png>")
    width, height = png_size(sys.argv[1])
    name, lw, lh = screen()
    factor = width / lw
    if abs(height / lh - factor) > 0.02 * factor:
        give_up(f"the capture is {width}x{height}, {name} is {lw:.0f}x{lh:.0f} logical")
    p = panel()
    strip = min(height, round((thickness(p) + MARGIN) * factor))
    if strip < 8:
        give_up(f"no usable panel thickness ({strip} px)")
    y = 0 if p["location"] == "top" else height - strip
    print(f"{width}x{strip}+0+{y}")


if __name__ == "__main__":
    main()
