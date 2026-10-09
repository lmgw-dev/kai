//! The tools of `contract::windows` (WP14) over the chosen services: `desktop_context`,
//! `window_list` and the window actions. Results are compact lines (K22): no empty part, no echo
//! of the arguments, nothing cut. A failure of the desktop is the tool's error, and a missing grant
//! reads as §11.1 says (`Failure::NotGranted`). The session and media tools (WP16b) are
//! [`session`].

use std::sync::Arc;

use kai_desktop::caps::Failure;
use kai_desktop::caps::audio::Audio;
use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::launch::Launch;
use kai_desktop::caps::media::{Media, Playback, Player};
use kai_desktop::caps::notify::Notify;
use kai_desktop::caps::session::{Battery, Charge, Lock, Session};
use kai_desktop::caps::windows::{Action, Side, Snapshot, Window, Windows};

use super::resolve::{self, quoted, shown_text, window_line};
use crate::host::registry::{Args, Call, Origin, Prepared, Preparer, Reply};

/// The services the tools use, each the chosen provider's (or none).
#[derive(Clone, Default)]
pub struct Services {
    pub windows: Option<Arc<dyn Windows>>,
    pub session: Option<Arc<dyn Session>>,
    pub media: Option<Arc<dyn Media>>,
    pub notify: Option<Arc<dyn Notify>>,
    pub audio: Option<Arc<dyn Audio>>,
    pub clipboard: Option<Arc<dyn Clipboard>>,
    pub launch: Option<Arc<dyn Launch>>,
    // WP16c's.
    pub input: Option<Arc<dyn kai_desktop::caps::input::Input>>,
    /// The RemoteDesktop portal's token file when the portal serves `input` (`fdo::input`): the
    /// confirmation of `type_text` and `key_press` then says how long its permission lasts.
    pub input_token: Option<crate::desktop::fdo::input::TokenFile>,
    /// One input call at a time, whichever provider serves it (`input`'s module doc).
    pub input_turn: Arc<tokio::sync::Mutex<()>>,
    /// How long an input call waits at its start for the confirmed window to have the focus again
    /// (`dbus.timeout_s`).
    pub focus_wait: std::time::Duration,
    /// "The approval card holds the keyboard" (WP19): no input call types while it does.
    pub card_keyboard: crate::approvals::keyboard::Holder,
    // WP15's (`super::read`).
    pub capture: Option<Arc<dyn kai_desktop::caps::capture::Capture>>,
    pub accessibility: Option<Arc<dyn kai_desktop::caps::accessibility::Accessibility>>,
    /// Kai's accessibility switch, held across `ui_press`'s confirmation (`input`).
    pub a11y: Option<crate::desktop::fdo::atspi::switch::A11ySwitch>,
    pub search: Option<Arc<dyn kai_desktop::caps::search::Search>>,
    /// `search.limit` (`find_files`).
    pub find_limit: usize,
}

impl Services {
    /// `desktop_context` has something to say.
    pub fn any(&self) -> bool {
        self.windows.is_some()
            || self.session.is_some()
            || self.media.is_some()
            || self.notify.is_some()
    }
}

/// What of a call besides its arguments the tools use: its deadline, the room its result has
/// within the link's message limit, and what was prepared before its confirmation
/// (`host::registry::Call`).
#[derive(Clone, Default)]
pub struct Ctx {
    pub deadline: Option<tokio::time::Instant>,
    pub timeout_ms: Option<u64>,
    pub room: Option<u64>,
    pub room_says: String,
    pub prepared: Option<Prepared>,
}

impl Ctx {
    pub fn of(call: &Call) -> Self {
        Self {
            deadline: call.deadline,
            timeout_ms: call.timeout_ms,
            room: call.room,
            room_says: call.room_says.clone(),
            prepared: call.prepared.clone(),
        }
    }
}

/// Runs the tool `name` (one of `contract::windows`) with no call context.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn run(services: &Services, name: &str, args: &Args) -> Reply {
    run_with(services, name, args, &Ctx::default()).await
}

/// Runs the tool `name` of the desktop's contracts for a call with `ctx`.
pub async fn run_with(services: &Services, name: &str, args: &Args, ctx: &Ctx) -> Reply {
    let result = match name {
        n if session::handles(n) => session::run(services, n, args).await,
        n if launch::handles(n) => launch::run(services, n, args, ctx).await,
        n if input::handles(n) => input::run(services, n, args, ctx).await,
        "desktop_context" => Ok(desktop_context(services).await),
        "window_list" => window_list(services).await,
        _ => window_action(services, name, args, ctx).await,
    };
    match result {
        Ok(text) => Reply::text(text),
        Err(e) => Reply::error(e),
    }
}

fn windows(services: &Services) -> Result<&Arc<dyn Windows>, String> {
    // Not listed without the capability (K23): only a race with a relist gets here.
    services
        .windows
        .as_ref()
        .ok_or_else(|| "this desktop offers Kai no window control".to_owned())
}

async fn window_list(services: &Services) -> Result<String, String> {
    let snap = windows(services)?.snapshot().await.map_err(say)?;
    Ok(list_lines(&snap))
}

/// `window_list`'s lines: `<id> <app> "<title>" (<state>)`, topmost first. The output and the
/// desktop are named only where there is more than one.
pub fn list_lines(snap: &Snapshot) -> String {
    if snap.windows.is_empty() {
        return "no windows".into();
    }
    let lines: Vec<String> = snap
        .windows
        .iter()
        .map(|w| {
            let mut state = Vec::new();
            if snap.outputs.len() > 1 && !w.output.is_empty() {
                state.push(shown_text(&w.output).into_owned());
            }
            if snap.desktops.len() > 1 {
                state.push(if w.desktops.is_empty() {
                    "all desktops".to_owned()
                } else {
                    let names: Vec<String> = w
                        .desktops
                        .iter()
                        .map(|d| shown_text(d).into_owned())
                        .collect();
                    names.join(" + ")
                });
            }
            for (on, word) in [
                (w.active, "active"),
                (w.minimized, "minimized"),
                (w.maximized, "maximized"),
                (w.fullscreen, "fullscreen"),
            ] {
                if on {
                    state.push(word.to_owned());
                }
            }
            if state.is_empty() {
                window_line(w)
            } else {
                format!("{} ({})", window_line(w), state.join(", "))
            }
        })
        .collect();
    lines.join("\n")
}

fn say(f: Failure) -> String {
    f.to_string()
}

/// The state-changing window tools: every one of `contract::windows` that is not read-only.
pub fn is_window_action(name: &str) -> bool {
    kai_desktop::contract::windows::AREA
        .tools
        .iter()
        .any(|t| t.name == name && !t.hints.read_only)
}

/// The window a window action was confirmed for: its id (the dialog showed the window).
struct Resolved(String);

/// A window action's resolution before the confirmation (the host runs it after the policy): the
/// window `window` names now, shown in the dialog as `acts on: <id> <app> "<title>"` and the only
/// one the action then runs on. A call approved in lmgw must name its window
/// (`resolve::explicit_if_approved`); one of Kai's own windows is a refusal.
pub fn prepare_window(services: &Services) -> Preparer {
    let services = services.clone();
    Arc::new(move |args: &Args, origin: Origin| {
        let services = services.clone();
        let window = args.text("window").map(str::to_owned);
        Box::pin(async move {
            resolve::explicit_if_approved(window.as_deref(), origin)?;
            let snap = windows(&services)?.snapshot().await.map_err(say)?;
            let target = resolve::window(&snap, window.as_deref())?;
            resolve::not_kais(target)?;
            Ok(Prepared {
                lines: vec![format!("acts on: {}", window_line(target))],
                state: Arc::new(Resolved(target.id.clone())),
                notes: Vec::new(),
                options: Vec::new(),
            })
        })
    })
}

/// A window action: the window the confirmation showed (by its id; gone is a refusal), else
/// `window` resolved in a fresh snapshot (K22); then the action on its id. The answer names the
/// window it acted on.
async fn window_action(
    services: &Services,
    name: &str,
    args: &Args,
    ctx: &Ctx,
) -> Result<String, String> {
    let w = windows(services)?;
    let snap = w.snapshot().await.map_err(say)?;
    let confirmed = ctx
        .prepared
        .as_ref()
        .and_then(|p| p.state.clone().downcast::<Resolved>().ok());
    let target: &Window = match confirmed {
        Some(r) => snap.windows.iter().find(|w| w.id == r.0).ok_or_else(|| {
            "the window this call was confirmed for is gone; nothing was done".to_owned()
        })?,
        None => resolve::window(&snap, args.text("window"))?,
    };
    resolve::not_kais(target)?;
    let action = match name {
        "window_activate" => Action::Activate,
        "window_close" => Action::Close,
        "window_minimize" => Action::Minimize,
        "window_maximize" => Action::ToggleMaximize,
        "window_fullscreen" => Action::ToggleFullscreen,
        "window_to_desktop" => Action::ToDesktop(resolve::desktop(
            &snap,
            args.text("desktop").unwrap_or_default(),
        )?),
        "window_to_output" => Action::ToOutput(resolve::output(
            &snap,
            args.text("output").unwrap_or_default(),
        )?),
        "window_tile" => Action::Tile(
            Side::parse(args.text("side").unwrap_or_default()).ok_or_else(|| {
                let sides: Vec<&str> = Side::ALL.iter().map(|s| s.key()).collect();
                format!("side is one of {}", sides.join(", "))
            })?,
        ),
        other => return Err(format!("Kai's desktop module has no tool {other}")),
    };
    let done = w.act(&target.id, &action).await.map_err(say)?;
    Ok(format!("{done}: {}", window_line(target)))
}

/// `desktop_context`'s lines (module doc of `contract::windows`): every part the desktop serves,
/// read side by side; a part that fails says so on its line, the others still answer.
pub async fn desktop_context(services: &Services) -> String {
    let snap = async {
        match &services.windows {
            Some(w) => Some(w.snapshot().await),
            None => None,
        }
    };
    let lock = async {
        match &services.session {
            Some(s) => Some((s.lock().await, s.battery().await)),
            None => None,
        }
    };
    let dnd = async {
        match &services.notify {
            Some(n) => Some(n.dnd().await),
            None => None,
        }
    };
    let players = async {
        match &services.media {
            Some(m) => Some(m.players().await),
            None => None,
        }
    };
    let (snap, session, dnd, players) = tokio::join!(snap, lock, dnd, players);
    let mut lines = Vec::new();
    match snap {
        Some(Ok(s)) => lines.extend(snapshot_lines(&s)),
        Some(Err(e)) => lines.push(format!("windows: unavailable ({e})")),
        None => {}
    }
    if let Some((lock, battery)) = session {
        lines.push(match lock {
            Ok(l) => lock_line(&l),
            Err(e) => format!("screen: unavailable ({e})"),
        });
        lines.push(match battery {
            Ok(b) => battery_line(b.as_ref()),
            Err(e) => format!("battery: unavailable ({e})"),
        });
    }
    match dnd {
        Some(Ok(on)) => lines.push(format!("do not disturb: {}", if on { "on" } else { "off" })),
        // A server with no such state has nothing to say.
        Some(Err(Failure::Refused(_))) => {}
        Some(Err(e)) => lines.push(format!("do not disturb: unavailable ({e})")),
        None => {}
    }
    match players {
        Some(Ok(p)) => lines.push(media_line(&p)),
        Some(Err(e)) => lines.push(format!("media: unavailable ({e})")),
        None => {}
    }
    lines.join("\n")
}

fn snapshot_lines(s: &Snapshot) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match s.windows.iter().find(|w| w.active) {
        Some(w) => format!(
            "active window: {} {} (id {})",
            shown_text(&w.app),
            quoted(&w.title),
            shown_text(&w.id)
        ),
        None => "active window: none".into(),
    });
    if !s.outputs.is_empty() {
        let outputs: Vec<String> = s
            .outputs
            .iter()
            .map(|o| {
                let mut t = format!(
                    "{} {}x{} at {},{}",
                    shown_text(&o.name),
                    o.width,
                    o.height,
                    o.x,
                    o.y
                );
                if (o.scale - 1.0).abs() > f64::EPSILON {
                    t.push_str(&format!(" scale {}", o.scale));
                }
                if o.active && s.outputs.len() > 1 {
                    t.push_str(" (active)");
                }
                t
            })
            .collect();
        lines.push(format!("outputs: {}", outputs.join("; ")));
    }
    if !s.desktop.is_empty() {
        let at = s.desktops.iter().position(|d| *d == s.desktop);
        lines.push(match at {
            Some(i) => format!(
                "virtual desktop: {} ({} of {})",
                shown_text(&s.desktop),
                i + 1,
                s.desktops.len()
            ),
            None => format!("virtual desktop: {}", shown_text(&s.desktop)),
        });
    }
    if !s.activity.is_empty() {
        lines.push(format!("activity: {}", shown_text(&s.activity)));
    }
    lines
}

fn lock_line(l: &Lock) -> String {
    let mut parts = Vec::new();
    match l.locked {
        Some(true) => parts.push("locked".to_owned()),
        Some(false) => parts.push("unlocked".to_owned()),
        None => {}
    }
    if let Some(s) = l.idle_s {
        parts.push(format!("idle {s} s"));
    }
    if parts.is_empty() {
        "screen: the desktop does not say whether it is locked".into()
    } else {
        format!("screen: {}", parts.join(", "))
    }
}

fn battery_line(b: Option<&Battery>) -> String {
    match b {
        None => "battery: none".into(),
        Some(b) => {
            let state = match &b.charge {
                Charge::Charging => "charging".to_owned(),
                Charge::Discharging => "discharging".to_owned(),
                Charge::Full => "full".to_owned(),
                Charge::Other(s) => s.clone(),
            };
            format!("battery: {:.0} %, {state}", b.percent)
        }
    }
}

fn media_line(players: &[Player]) -> String {
    let shown: Vec<String> = players
        .iter()
        .filter(|p| p.playback != Playback::Stopped)
        .map(|p| {
            let status = match p.playback {
                Playback::Playing => "playing",
                Playback::Paused => "paused",
                Playback::Stopped => "stopped",
                Playback::Unknown => "did not say what it plays",
            };
            let mut t = format!("{} {status}", shown_text(&p.name));
            if !p.title.is_empty() {
                t.push(' ');
                t.push_str(&quoted(&p.title));
            }
            if !p.artists.is_empty() {
                t.push_str(" by ");
                let artists: Vec<String> = p
                    .artists
                    .iter()
                    .map(|a| shown_text(a).into_owned())
                    .collect();
                t.push_str(&artists.join(", "));
            }
            t
        })
        .collect();
    if shown.is_empty() {
        "media: nothing playing".into()
    } else {
        format!("media: {}", shown.join("; "))
    }
}

pub mod input;
pub mod launch;
mod session;

#[cfg(test)]
mod tests;
