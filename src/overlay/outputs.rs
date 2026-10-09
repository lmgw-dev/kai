//! The facts the output choice needs (§9.1): the outputs' connector names and KWin's active
//! output.
//!
//! *Connector names.* GDK3 gives a monitor's geometry, make and model, never its connector. A
//! short-lived Wayland connection of Kai's own binds every wl_output at version 4 and reads its
//! `name` and position; [`super::placement::monitor_for`] then finds the GDK monitor.
//!
//! *The active output.* The focused window's output would need `org_kde_plasma_window_management`,
//! a restricted interface §13 keeps out of the `.desktop` file until a phase-3 tool uses it. KWin
//! says the same over D-Bus without a grant: `org.kde.KWin.activeOutputName` names the output KWin
//! itself treats as active (the focused window's, or the pointer's under "active screen follows
//! mouse").

use std::time::Duration;

use gtk::gdk::prelude::*;
use gtk::gio;
use gtk::glib;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};

use super::placement::WlOutput;

/// How long Kai waits for KWin's answer (named in the note when it passes).
pub const KWIN_TIMEOUT: Duration = Duration::from_millis(1000);

/// How long the ring waits for the compositor to list its outputs (two Wayland roundtrips, which
/// take a few ms); past it no output is resolved, with a logged note naming this: a ring that is up
/// stays where it is, a new map leaves the output to the compositor.
pub const OUTPUTS_TIMEOUT: Duration = Duration::from_millis(1000);

/// A query for the outputs is running (its worker may be parked on a stalled compositor).
static QUERY_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The right to run [`wayland_outputs`]: one query at a time, released when it ends.
pub struct Query(());

impl Query {
    /// `None` while an earlier query still runs.
    pub fn start() -> Option<Query> {
        (!QUERY_RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel)).then_some(Query(()))
    }

    /// Runs the query (blocking: off the GTK thread).
    pub fn run(self) -> Result<Vec<WlOutput>, String> {
        wayland_outputs()
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        QUERY_RUNNING.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Default)]
struct Outputs {
    found: Vec<Found>,
}

#[derive(Default)]
struct Found {
    name: Option<String>,
    x: i32,
    y: i32,
    make: String,
    model: String,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Outputs {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_output"
        {
            let index = state.found.len();
            state.found.push(Found::default());
            registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, index);
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(f) = state.found.get_mut(*index) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                x, y, make, model, ..
            } => {
                (f.x, f.y, f.make, f.model) = (x, y, make, model);
            }
            wl_output::Event::Name { name } => f.name = Some(name),
            _ => {}
        }
    }
}

/// The compositor's outputs with their connector names. Blocking (two roundtrips): run it off the
/// GTK thread.
pub fn wayland_outputs() -> Result<Vec<WlOutput>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland connection: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut state = Outputs::default();
    for _ in 0..2 {
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland: {e}"))?;
    }
    let total = state.found.len();
    let named: Vec<WlOutput> = state
        .found
        .into_iter()
        .filter_map(|f| {
            Some(WlOutput {
                name: f.name?,
                x: f.x,
                y: f.y,
                make: f.make,
                model: f.model,
            })
        })
        .collect();
    if named.len() < total {
        return Err(format!(
            "{} of {total} outputs have no name (the compositor's wl_output is older than version 4)",
            total - named.len()
        ));
    }
    Ok(named)
}

/// GDK's monitors, in GDK's order.
pub fn gdk_monitors() -> Vec<(gtk::gdk::Monitor, super::placement::Monitor)> {
    let Some(display) = gtk::gdk::Display::default() else {
        return Vec::new();
    };
    (0..display.n_monitors())
        .filter_map(|i| display.monitor(i))
        .map(|m| {
            let g = m.geometry();
            let info = super::placement::Monitor {
                x: g.x(),
                y: g.y(),
                make: m.manufacturer().map(|s| s.to_string()).unwrap_or_default(),
                model: m.model().map(|s| s.to_string()).unwrap_or_default(),
            };
            (m, info)
        })
        .collect()
}

/// KWin's active output, asked on the session bus.
pub async fn kwin_active_output() -> Result<String, String> {
    let bus = gio::bus_get_future(gio::BusType::Session)
        .await
        .map_err(|e| format!("no session bus: {e}"))?;
    let reply = bus
        .call_future(
            Some("org.kde.KWin"),
            "/KWin",
            "org.kde.KWin",
            "activeOutputName",
            None,
            Some(glib::VariantTy::new("(s)").expect("a valid type")),
            gio::DBusCallFlags::NO_AUTO_START,
            KWIN_TIMEOUT.as_millis() as i32,
        )
        .await
        .map_err(|e| {
            format!(
                "org.kde.KWin.activeOutputName (waited at most {} ms): {e}",
                KWIN_TIMEOUT.as_millis()
            )
        })?;
    reply
        .get::<(String,)>()
        .map(|(name,)| name)
        .ok_or_else(|| "org.kde.KWin.activeOutputName: an unexpected reply".to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_output_query_at_a_time() {
        let q = super::Query::start().expect("none runs");
        assert!(
            super::Query::start().is_none(),
            "a second one waits for the first"
        );
        drop(q);
        assert!(super::Query::start().is_some());
    }
}

#[cfg(test)]
mod live {
    /// Read-only: lists the outputs and asks KWin once (`just live-check-ring` runs it in the
    /// session).
    #[test]
    #[ignore = "needs the Plasma session (scripts/live-check-ring.sh)"]
    fn live_outputs_and_kwin() {
        let outputs = super::wayland_outputs().expect("outputs");
        eprintln!("wl_output: {outputs:?}");
        assert!(!outputs.is_empty());
        let ctx = gtk::glib::MainContext::new();
        let active = ctx.block_on(super::kwin_active_output());
        eprintln!("KWin's active output: {active:?}");
        let active = active.expect("KWin answers");
        assert!(
            outputs.iter().any(|o| o.name == active),
            "{active} among {outputs:?}"
        );
    }
}
