#!/usr/bin/env python3
"""A plain backdrop for the README's screenshots: a layer-shell surface on the BACKGROUND layer of
every output, drawn with a graphite gradient and a soft blue glow (Breeze's colours), so Kai's
overlays are shown over something calmer than an empty compositor. No image, no text, no network.

usage: scripts/readme-backdrop.py   (in the nested KWin of scripts/nested-kwin.sh; runs until killed)
"""

import math
import signal

import cairo
import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GtkLayerShell", "0.1")
from gi.repository import Gdk, GLib, Gtk, GtkLayerShell  # noqa: E402


def draw(widget, cr):
    w = widget.get_allocated_width()
    h = widget.get_allocated_height()
    base = cairo.LinearGradient(0, 0, w, h)
    base.add_color_stop_rgb(0.0, 0x1B / 255, 0x1E / 255, 0x20 / 255)
    base.add_color_stop_rgb(1.0, 0x2A / 255, 0x2E / 255, 0x32 / 255)
    cr.set_source(base)
    cr.paint()
    for cx, cy, r, a in ((0.78, 0.18, 0.75, 0.22), (0.12, 0.92, 0.6, 0.12)):
        glow = cairo.RadialGradient(cx * w, cy * h, 0, cx * w, cy * h, r * math.hypot(w, h) / 2)
        glow.add_color_stop_rgba(0.0, 0x3D / 255, 0xAE / 255, 0xE9 / 255, a)
        glow.add_color_stop_rgba(1.0, 0x3D / 255, 0xAE / 255, 0xE9 / 255, 0.0)
        cr.set_source(glow)
        cr.paint()
    return False


def main():
    Gtk.init([])
    display = Gdk.Display.get_default()
    windows = []
    for i in range(display.get_n_monitors()):
        win = Gtk.Window()
        GtkLayerShell.init_for_window(win)
        GtkLayerShell.set_layer(win, GtkLayerShell.Layer.BACKGROUND)
        GtkLayerShell.set_monitor(win, display.get_monitor(i))
        GtkLayerShell.set_namespace(win, "readme-backdrop")
        GtkLayerShell.set_exclusive_zone(win, -1)
        for edge in (
            GtkLayerShell.Edge.LEFT,
            GtkLayerShell.Edge.RIGHT,
            GtkLayerShell.Edge.TOP,
            GtkLayerShell.Edge.BOTTOM,
        ):
            GtkLayerShell.set_anchor(win, edge, True)
        area = Gtk.DrawingArea()
        area.connect("draw", draw)
        win.add(area)
        win.show_all()
        windows.append(win)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, Gtk.main_quit)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, Gtk.main_quit)
    Gtk.main()


if __name__ == "__main__":
    main()
