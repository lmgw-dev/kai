//! The tools of `contract::input` (WP16c) over the chosen services. Every one acts as the owner, so
//! K15 has confirmed it before it gets here. An answer is one short line that says what happened,
//! never the text typed or the keys pressed; **nothing typed or pressed is logged**, and a refusal
//! about the text names a position or a code point, not the text.
//!
//! - **The remote-control permission** (the owner's decision of 2026-10-09): when the RemoteDesktop
//!   portal types (`Services::input_token`), the confirmation of `type_text` and `key_press` says
//!   that its permission lasts until forgotten in Kai's settings, and that revoking it in System
//!   Settings does not end it ([`remote_control_note`]).
//! - **Before the confirmation** ([`prepare`], the host's `Preparer`): the arguments are checked,
//!   and what the call will act on is resolved, so the dialog names it: for `type_text` and
//!   `key_press` the window that has the keyboard focus now (`receives it: <id> <app> "<title>"`;
//!   none is a refusal), for `ui_press` the window and the one element the query names
//!   (`presses: <role> "<name>" in <id> <app> "<title>"`; none, several or a disabled one is a
//!   refusal, so the owner is asked about nothing ambiguous).
//! - **A call approved in lmgw** (`Origin::approved_in_lmgw`) must name its window for `type_text`,
//!   `key_press` and `ui_press`: not omitted, not `active` (`resolve::explicit_if_approved`).
//!   Without it, the window resolves when the call arrives, after the owner's click, to whatever
//!   has the focus then, such as lmgw's dashboard, which is not one of Kai's own windows.
//! - **One call at a time** (`Services::input_turn`): a call waits for the one before it to end,
//!   at most until its own deadline less `launch::ANSWER_MARGIN`; a cancelled call stops waiting.
//! - **The focus guard:** before the first event and before every further character and chord the
//!   window with the keyboard focus must be the confirmed one, and Kai's approval card must not hold
//!   the keyboard (`crate::approvals::keyboard`: KWin's window list does not name layer-shell
//!   surfaces, so the window it calls active can be another while the keys would land on the
//!   card). At the start Kai waits for it up to
//!   `dbus.timeout_s` (the confirmation's dialog, or the portal's, hands the focus back as it
//!   closes); after the first event a change stops the call at once, its answer
//!   `focus moved to <app>; N of M characters typed` (or `chords pressed`), never the text.
//! - `ui_press` presses exactly the element that was confirmed; one that is gone or changed is a
//!   refusal and nothing is pressed.
//! - **Never Kai's own windows** (`resolve::not_kais`): the focused window for `type_text` and
//!   `key_press`, `ui_press`'s window; refused before the confirmation, so no input call can
//!   answer Kai's own dialog or drive its configuration window.

use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::accessibility::{Found, Query, Target};
use kai_desktop::caps::input::{Guard, keysyms, parse_keys};
use kai_desktop::caps::windows::{Window, Windows};
use kai_desktop::caps::{Failure, Fut};

use super::launch::ANSWER_MARGIN;
use super::{Ctx, Services};
use crate::desktop::resolve::{self, quoted, shown_text, window_line};
use crate::host::registry::{Args, Origin, Prepared, Preparer};

/// How often the guard looks again while it waits for the focus to come back at the start.
const FOCUS_POLL: Duration = Duration::from_millis(25);

/// Whether `name` is a tool of this file.
pub fn handles(name: &str) -> bool {
    !kai_desktop::contract::input::needs(name).is_empty()
}

fn say(f: Failure) -> String {
    f.to_string()
}

fn missing<T>(what: &str) -> Result<T, String> {
    // Not listed without the capability (K23): only a race with a relist gets here.
    Err(format!("this desktop offers Kai no {what}"))
}

/// What an input call was confirmed for (module doc).
#[derive(Debug, Clone)]
pub enum Confirmed {
    /// `type_text`, `key_press`: the window that had the keyboard focus.
    Focus(Window),
    /// `ui_press`: the window and the element in it, and the accessibility switch held from the
    /// find through the confirmation to the press (`fdo::atspi::switch`: the app stays on the bus
    /// for the press; released when the call ends, run or not).
    Element(
        Window,
        Found,
        Option<Arc<crate::desktop::fdo::atspi::switch::Hold>>,
    ),
}

impl Confirmed {
    /// The confirmation's lines.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Confirmed::Focus(w) => vec![format!("receives it: {}", window_line(w))],
            Confirmed::Element(w, f, _) => vec![format!(
                "presses: {} {} in {}",
                shown_text(&f.role),
                quoted(&f.name),
                window_line(w)
            )],
        }
    }
}

/// The input tools' resolution before the confirmation (module doc); the host runs it after the
/// policy, and its refusal asks nobody.
pub fn prepare(services: &Services, name: &'static str) -> Preparer {
    let services = services.clone();
    Arc::new(move |args: &Args, origin: Origin| {
        let services = services.clone();
        let args = args.clone();
        Box::pin(async move {
            let c = confirmed(&services, name, &args, origin).await?;
            let notes = match (&c, &services.input_token) {
                (Confirmed::Focus(_), Some(token)) => {
                    vec![remote_control_note(token.load().is_some())]
                }
                _ => Vec::new(),
            };
            Ok(Prepared {
                lines: c.lines(),
                state: Arc::new(c),
                options: Vec::new(),
                notes,
            })
        })
    })
}

/// The confirmation's line for `type_text` and `key_press` when the RemoteDesktop portal types
/// (the owner's decision of 2026-10-09, WP16c): the portal's permission is Kai's stored restore
/// token, which lasts until the owner forgets it in Kai's settings; revoking Kai in System
/// Settings does not end it. `stored`: Kai holds one now (else the desktop asks first).
pub fn remote_control_note(stored: bool) -> String {
    if stored {
        "Kai types through the desktop's remote-control permission, which lasts until you forget \
         it in Kai's settings (Desktop page); revoking it in System Settings does not end it."
            .into()
    } else {
        "The desktop asks for remote control first. If you allow it to be remembered, the \
         permission lasts until you forget it in Kai's settings (Desktop page); revoking it in \
         System Settings does not end it."
            .into()
    }
}

/// Checks the arguments and resolves what the call acts on.
async fn confirmed(
    services: &Services,
    name: &str,
    args: &Args,
    origin: Origin,
) -> Result<Confirmed, String> {
    if matches!(name, "type_text" | "key_press" | "ui_press") {
        resolve::explicit_if_approved(args.text("window"), origin)?;
    }
    match name {
        "type_text" => {
            text(args)?;
            Ok(Confirmed::Focus(keyed(services, args).await?))
        }
        "key_press" => {
            parse_keys(args.text("keys").unwrap_or_default())?;
            Ok(Confirmed::Focus(keyed(services, args).await?))
        }
        "ui_press" => {
            let (Some(access), Some(windows)) = (&services.accessibility, &services.windows) else {
                return missing("accessibility");
            };
            let query = Query::parse(args.text("element").unwrap_or_default())?;
            let snap = windows.snapshot().await.map_err(say)?;
            let win = resolve::window(&snap, args.text("window"))?;
            resolve::not_kais(win)?;
            let target = Target {
                pid: win.pid,
                title: win.title.clone(),
            };
            let hold = match &services.a11y {
                Some(s) => Some(Arc::new(s.hold().await)),
                None => None,
            };
            let found = access.find(&target, &query).await.map_err(say)?;
            Ok(Confirmed::Element(win.clone(), found, hold))
        }
        other => Err(format!("Kai's desktop module has no tool {other}")),
    }
}

/// The window that receives the keys: the one with the keyboard focus, and when the call names a
/// `window`, that one must be it (a named window without the focus is a refusal: activate it first).
async fn keyed(services: &Services, args: &Args) -> Result<Window, String> {
    let focused = not_kais(focus(services).await?)?;
    if resolve::implicit(args.text("window")) {
        return Ok(focused);
    }
    let snap = windows(services)?.snapshot().await.map_err(say)?;
    let named = resolve::window(&snap, args.text("window"))?;
    if named.id != focused.id {
        return Err(format!(
            "{} does not have the keyboard focus (the keys would go to {}); activate it first \
             (window_activate)",
            window_line(named),
            window_line(&focused)
        ));
    }
    Ok(focused)
}

/// `w` unless it is one of Kai's own windows (`resolve::not_kais`): the keys would go to Kai.
fn not_kais(w: Window) -> Result<Window, String> {
    resolve::not_kais(&w)?;
    Ok(w)
}

/// `type_text`'s text as keysyms; empty or a control character is a refusal.
fn text(args: &Args) -> Result<Vec<u32>, String> {
    let text = args.text("text").unwrap_or_default();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    keysyms(text)
}

fn windows(services: &Services) -> Result<&Arc<dyn Windows>, String> {
    services
        .windows
        .as_ref()
        .map_or_else(|| missing("window control"), Ok)
}

/// The window that has the keyboard focus now.
async fn focus(services: &Services) -> Result<Window, String> {
    windows(services)?
        .active()
        .await
        .map_err(say)?
        .ok_or_else(|| {
            "no window has the keyboard focus; activate one first (window_activate)".to_owned()
        })
}

pub async fn run(
    services: &Services,
    name: &str,
    args: &Args,
    ctx: &Ctx,
) -> Result<String, String> {
    let prepared = ctx
        .prepared
        .as_ref()
        .and_then(|p| p.state.clone().downcast::<Confirmed>().ok());
    let confirmed = match prepared {
        Some(c) => (*c).clone(),
        // A call with no confirmation step (Kai's own, the tests) resolves now.
        None => confirmed(services, name, args, Origin::default()).await?,
    };
    let _turn = turn(services, ctx).await?;
    match (name, confirmed) {
        ("type_text", Confirmed::Focus(window)) => {
            let Some(input) = &services.input else {
                return missing("keyboard input");
            };
            let keysyms = text(args)?;
            let guard = FocusGuard::new(services, window, keysyms.len(), Unit::Characters)?;
            guard.check(0).await.map_err(say)?;
            input.type_keysyms(&keysyms, guard).await.map_err(say)?;
            Ok(format!("typed {} characters", keysyms.len()))
        }
        ("key_press", Confirmed::Focus(window)) => {
            let Some(input) = &services.input else {
                return missing("keyboard input");
            };
            let chords = parse_keys(args.text("keys").unwrap_or_default())?;
            let guard = FocusGuard::new(services, window, chords.len(), Unit::Chords)?;
            guard.check(0).await.map_err(say)?;
            input.press_chords(&chords, guard).await.map_err(say)?;
            Ok(match chords.len() {
                1 => "pressed 1 chord".to_owned(),
                n => format!("pressed {n} chords"),
            })
        }
        ("ui_press", Confirmed::Element(window, found, _hold)) => {
            let Some(access) = &services.accessibility else {
                return missing("accessibility");
            };
            let pressed = access.press(&found).await.map_err(say)?;
            let name = if pressed.name.is_empty() {
                String::new()
            } else {
                format!(" {}", quoted(&pressed.name))
            };
            Ok(format!(
                "pressed {}{name} in {}",
                shown_text(&pressed.role),
                window_line(&window)
            ))
        }
        (other, _) => Err(format!(
            "{other} was prepared as another tool's call; it did not run"
        )),
    }
}

/// This call's turn among the input calls (module doc).
async fn turn<'a>(
    services: &'a Services,
    ctx: &Ctx,
) -> Result<tokio::sync::MutexGuard<'a, ()>, String> {
    let lock = services.input_turn.lock();
    match ctx.deadline {
        None => Ok(lock.await),
        Some(d) => {
            let until = d.checked_sub(ANSWER_MARGIN).unwrap_or(d);
            tokio::time::timeout_at(until, lock).await.map_err(|_| {
                format!(
                    "another input call was still running when this one's time was up \
                     (lmgw/timeout_ms = {}, less {:.1} s for the answer); nothing was typed or \
                     pressed",
                    ctx.timeout_ms.unwrap_or_default(),
                    ANSWER_MARGIN.as_secs_f64()
                )
            })
        }
    }
}

#[derive(Clone, Copy)]
enum Unit {
    Characters,
    Chords,
}

/// The focus guard (module doc).
struct FocusGuard {
    windows: Arc<dyn Windows>,
    card: crate::approvals::keyboard::Holder,
    want: Window,
    total: usize,
    unit: Unit,
    /// How long the first check waits for the focus to come back (`dbus.timeout_s`).
    wait: Duration,
}

impl FocusGuard {
    fn new(
        services: &Services,
        want: Window,
        total: usize,
        unit: Unit,
    ) -> Result<Arc<FocusGuard>, String> {
        Ok(Arc::new(FocusGuard {
            windows: windows(services)?.clone(),
            card: services.card_keyboard.clone(),
            want,
            total,
            unit,
            wait: services.focus_wait,
        }))
    }

    fn progress(&self, done: usize) -> String {
        match self.unit {
            Unit::Characters => format!("{done} of {} characters typed", self.total),
            Unit::Chords => format!("{done} of {} chords pressed", self.total),
        }
    }

    fn moved(&self, now: Option<&Window>, card: bool, done: usize) -> Failure {
        let to = match now {
            _ if card => "Kai's approval card holds the keyboard".to_owned(),
            Some(w) => format!("focus moved to {}", app(w)),
            None => format!("focus left {} (no window has it)", app(&self.want)),
        };
        let waited = if done == 0 && !self.wait.is_zero() {
            format!(
                " and {} within {} s (dbus.timeout_s)",
                if card {
                    "did not let go of it"
                } else {
                    "did not come back"
                },
                self.wait.as_secs_f64()
            )
        } else {
            String::new()
        };
        Failure::Refused(format!("{to}{waited}; {}", self.progress(done)))
    }
}

/// A window as a stop names it: its app id, else its title.
fn app(w: &Window) -> String {
    if w.app.is_empty() {
        quoted(&w.title)
    } else {
        shown_text(&w.app).into_owned()
    }
}

impl Guard for FocusGuard {
    fn check(&self, done: usize) -> Fut<'_, ()> {
        Box::pin(async move {
            let end = tokio::time::Instant::now() + self.wait;
            loop {
                let now = self.windows.active().await.map_err(|e| {
                    Failure::Failed(format!(
                        "the window with the focus could not be read: {e}; {}",
                        self.progress(done)
                    ))
                })?;
                let card = self.card.holds();
                if !card && now.as_ref().is_some_and(|w| w.id == self.want.id) {
                    return Ok(());
                }
                if done > 0 || tokio::time::Instant::now() >= end {
                    return Err(self.moved(now.as_ref(), card, done));
                }
                tokio::time::sleep(FOCUS_POLL).await;
            }
        })
    }
}
