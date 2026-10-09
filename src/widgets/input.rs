//! A widget's input region (§10.1 "Input"): its rectangle when it takes clicks, else **none**.
//!
//! GDK3 sends an empty region as `wl_surface.set_input_region(nil)`, which means "the whole
//! surface takes input", so the ring stands in a 1×1 px region for none (WP0). A click-through
//! widget takes no click at all: Kai sets an empty `wl_region` on the widget's surface itself,
//! through GDK's own Wayland objects, and asks for a frame so GDK's next commit carries it.
//!
//! GDK sends a `nil` region of its own in the first frame after the map (the live check's Wayland
//! log: empty, empty, nil), from its end of paint, before the frame's commit (after paint). So Kai
//! sets its empty region again in the paint phase of the first frame after a map or a size change,
//! after GDK's own paint handler: the same commit carries Kai's region, and it is the last one.

use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk::{cairo, gdk};

#[repr(C)]
struct WlProxy(c_void);
#[repr(C)]
struct WlInterface(c_void);

const WL_COMPOSITOR_CREATE_REGION: u32 = 1;
const WL_SURFACE_SET_INPUT_REGION: u32 = 5;
const WL_REGION_DESTROY: u32 = 0;
const WL_MARSHAL_FLAG_DESTROY: u32 = 1;

#[link(name = "wayland-client")]
unsafe extern "C" {
    static wl_region_interface: WlInterface;
    fn wl_proxy_get_version(proxy: *mut WlProxy) -> u32;
    fn wl_proxy_marshal_flags(
        proxy: *mut WlProxy,
        opcode: u32,
        interface: *const WlInterface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut WlProxy;
}

#[link(name = "gdk-3")]
unsafe extern "C" {
    fn gdk_wayland_window_get_wl_surface(window: *mut gdk::ffi::GdkWindow) -> *mut WlProxy;
    fn gdk_wayland_display_get_wl_compositor(display: *mut gdk::ffi::GdkDisplay) -> *mut WlProxy;
}

/// A widget's input region as kept: whether it takes clicks now ([`Input::set`]: an app view goes
/// click-through while a Kai confirmation is open, §10.1).
#[derive(Clone)]
pub struct Input {
    interactive: Rc<Cell<bool>>,
    /// Set none once more in the next frame's paint phase.
    again: Rc<Cell<bool>>,
}

/// Keeps `w`'s input region: its whole rectangle when `interactive`, else none. Set after the map
/// and after every size change (GDK sends a region at map only, WP0 (d)), and for none once more
/// in the next frame's paint phase (GDK's own `nil` goes out there).
pub fn keep(w: &gtk::Window, interactive: bool) -> Input {
    let input = Input {
        interactive: Rc::new(Cell::new(interactive)),
        again: Rc::default(),
    };
    let i = input.clone();
    w.connect_map_event(move |w, _| {
        apply(w, i.interactive.get(), true);
        i.again.set(true);
        gtk::glib::Propagation::Proceed
    });
    let i = input.clone();
    w.connect_size_allocate(move |w, _| {
        apply(w, i.interactive.get(), true);
        i.again.set(true);
    });
    // The frame clock exists once the window is realized; GDK's own paint handler (whose end of
    // paint sends GDK's region) was connected then, so this one runs after it, before the commit.
    let i = input.clone();
    let on_paint = move |w: &gtk::Window| {
        let Some(clock) = w.frame_clock() else { return };
        let (weak, i) = (w.downgrade(), i.clone());
        clock.connect_paint(move |_| {
            if i.again.replace(false)
                && !i.interactive.get()
                && let Some(w) = weak.upgrade()
            {
                apply(&w, false, false);
            }
        });
    };
    if w.is_realized() {
        on_paint(w);
    } else {
        w.connect_realize(on_paint);
    }
    input
}

impl Input {
    /// Takes clicks from now on, or none; applied at once if `w` is mapped.
    pub fn set(&self, w: &gtk::Window, interactive: bool) {
        if self.interactive.replace(interactive) == interactive {
            return;
        }
        if w.is_mapped() {
            apply(w, interactive, true);
            self.again.set(true);
        }
    }
}

/// Sets the region; `redraw`: asks for a frame to carry it (not from within one).
fn apply(w: &gtk::Window, interactive: bool, redraw: bool) {
    let Some(gw) = w.window() else { return };
    if interactive {
        let a = w.allocation();
        let region =
            cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, a.width(), a.height()));
        gw.input_shape_combine_region(&region, 0, 0);
        return;
    }
    let display = gw.display();
    if !display.type_().name().starts_with("GdkWayland") {
        // Only Wayland reaches here (layer-shell); nothing else is touched.
        return;
    }
    // SAFETY: a mapped GdkWaylandWindow and its GdkWaylandDisplay; the region is created on GDK's
    // compositor proxy, handed to the surface and destroyed at once (the surface keeps a copy of
    // its contents, wl_surface.set_input_region).
    unsafe {
        let surface = gdk_wayland_window_get_wl_surface(gw.to_glib_none().0);
        let compositor = gdk_wayland_display_get_wl_compositor(display.to_glib_none().0);
        if surface.is_null() || compositor.is_null() {
            return;
        }
        let region = wl_proxy_marshal_flags(
            compositor,
            WL_COMPOSITOR_CREATE_REGION,
            &raw const wl_region_interface,
            wl_proxy_get_version(compositor),
            0,
            std::ptr::null_mut::<c_void>(),
        );
        if region.is_null() {
            return;
        }
        wl_proxy_marshal_flags(
            surface,
            WL_SURFACE_SET_INPUT_REGION,
            std::ptr::null(),
            wl_proxy_get_version(surface),
            0,
            region,
        );
        wl_proxy_marshal_flags(
            region,
            WL_REGION_DESTROY,
            std::ptr::null(),
            wl_proxy_get_version(region),
            WL_MARSHAL_FLAG_DESTROY,
        );
    }
    // The region is pending until the surface's next commit: GDK's next frame.
    if redraw {
        w.queue_draw();
    }
}
