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
//!   The browser's trusted press takes the same turn ([`take_turn`]) before its focus check and
//!   sends its key in it ([`act`]).
//! - **Kai for Firefox's input spans** (`crate::browser::spans`, kai-firefox design §6.3): in its
//!   turn, before its first event, every call opens a span in each linked Firefox, so Firefox
//!   counts what it gets meanwhile as Kai's input, never as the user's act its focus rule reads;
//!   the spans end after the last event, `browser.input_settle_ms` later in the background. A
//!   Firefox whose span did not open is logged and its tabs count as touched by Kai (best effort:
//!   the input goes on).
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
//! - **Kai's browser site rules** (`crate::desktop::sites`): a Firefox window that shows a site Kai
//!   may only read or does not reach (or one Kai cannot tell, while some site is so) is refused
//!   before the confirmation (for `ui_press` before its element is looked up) and again when the
//!   call runs after it; `ui_press` presses only inside a page whose page and frame Kai may act
//!   on (`sites::pages`); `key_press` refuses Firefox's own tab, address and history chords there,
//!   and the desktop's window shortcuts while some site is `none`; and before each chord and
//!   character the focus guard hands the window to `sites::keys::KeyWatch`, which checks it again
//!   once the focus is back after the dialog, whenever its title changes, and checks the frame
//!   with the keyboard focus. A refusal names such a window by its id, never its title. The
//!   browser's trusted press ([`act`]) is checked by the browser tool that sends it.

use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::accessibility::{Found, Query, Target};
use kai_desktop::caps::input::{Guard, keysyms, parse_keys};
use kai_desktop::caps::windows::{Window, Windows};
use kai_desktop::caps::{Failure, Fut};

use tokio_util::sync::CancellationToken;

use super::launch::ANSWER_MARGIN;
use super::{Ctx, Services};
use crate::browser::shown::Reach;
use crate::desktop::resolve::{self, quoted, shown_text, window_line};
use crate::desktop::sites;
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
                ask: None,
                spoken: None,
                confirmed: false,
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
            let w = keyed(services, args).await?;
            sites::check(services, &w, Reach::Act, name).await?;
            Ok(Confirmed::Focus(w))
        }
        "key_press" => {
            let chords = parse_keys(args.text("keys").unwrap_or_default())?;
            let w = keyed(services, args).await?;
            sites::keys::chords(services, &w, &chords, name)?;
            sites::check(services, &w, Reach::Act, name).await?;
            Ok(Confirmed::Focus(w))
        }
        "ui_press" => {
            let (Some(access), Some(windows)) = (&services.accessibility, &services.windows) else {
                return missing("accessibility");
            };
            let query = Query::parse(args.text("element").unwrap_or_default())?;
            let snap = windows.snapshot().await.map_err(say)?;
            let win = resolve::window(&snap, args.text("window"))?;
            resolve::not_kais(win)?;
            sites::check(services, win, Reach::Act, name).await?;
            let target = Target {
                pid: win.pid,
                title: win.title.clone(),
                exact: sites::exact(services, win),
            };
            let hold = match &services.a11y {
                Some(s) => Some(Arc::new(s.hold().await)),
                None => None,
            };
            let found = access.find(&target, &query).await.map_err(say)?;
            sites::pages::found(services, win, &found, name)?;
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
            sites::named(services, named),
            sites::named(services, &focused)
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
    let (confirmed, prepared_first) = match prepared {
        Some(c) => ((*c).clone(), true),
        // A call with no confirmation step (Kai's own, the tests) resolves now.
        None => (
            confirmed(services, name, args, Origin::default()).await?,
            false,
        ),
    };
    let _turn = take_turn(services, ctx.deadline, ctx.timeout_ms).await?;
    if prepared_first {
        // The site rules again: the tab in front may have changed since, while a dialog waited.
        let (Confirmed::Focus(w) | Confirmed::Element(w, ..)) = &confirmed;
        sites::check_now(services, &w.id, Reach::Act, name).await?;
    }
    let span = match &services.spans {
        Some(s) => Some(s.begin(name_of(name), &CancellationToken::new()).await),
        None => None,
    };
    let done = act(services, name, args, confirmed).await;
    if let Some(open) = span {
        open.end_later();
    }
    done
}

/// The input tool's name as the spans log it.
fn name_of(name: &str) -> &'static str {
    match name {
        "type_text" => "type_text",
        "key_press" => "key_press",
        "ui_press" => "ui_press",
        _ => "an input tool",
    }
}

/// The input of a call whose caller holds its turn ([`take_turn`]) and brackets it (the
/// browser's trusted press, which takes the turn before its focus check).
pub async fn act(
    services: &Services,
    name: &str,
    args: &Args,
    confirmed: Confirmed,
) -> Result<String, String> {
    match (name, confirmed) {
        ("type_text", Confirmed::Focus(window)) => {
            let Some(input) = &services.input else {
                return missing("keyboard input");
            };
            let keysyms = text(args)?;
            // The focused frame is checked before the first character and after each that can
            // move the focus (Tab, a line break).
            let frames = std::iter::once(true)
                .chain(keysyms.iter().map(|k| matches!(*k, 0xff09 | 0xff0d)))
                .take(keysyms.len())
                .collect();
            let watch = sites::keys::KeyWatch::new(services, &window, frames);
            let guard = FocusGuard::new(services, window, keysyms.len(), Unit::Characters, watch)?;
            guard.check(0).await.map_err(say)?;
            input.type_keysyms(&keysyms, guard).await.map_err(say)?;
            Ok(format!("typed {} characters", keysyms.len()))
        }
        ("key_press", Confirmed::Focus(window)) => {
            let Some(input) = &services.input else {
                return missing("keyboard input");
            };
            let chords = parse_keys(args.text("keys").unwrap_or_default())?;
            sites::keys::chords(services, &window, &chords, name)?;
            // Any chord can move the focus: the focused frame is checked before each.
            let watch = sites::keys::KeyWatch::new(services, &window, vec![true; chords.len()]);
            let guard = FocusGuard::new(services, window, chords.len(), Unit::Chords, watch)?;
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
            // The rules may have changed while the dialog waited; the provider presses only
            // while the element's pages are still these.
            sites::pages::found(services, &window, &found, name)?;
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

/// A call's turn among the input calls (module doc), held until dropped: at most until
/// `deadline` less the answer's margin (`timeout_ms` names it).
pub async fn take_turn(
    services: &Services,
    deadline: Option<tokio::time::Instant>,
    timeout_ms: Option<u64>,
) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
    let lock = services.input_turn.clone().lock_owned();
    match deadline {
        None => Ok(lock.await),
        Some(d) => {
            let until = d.checked_sub(ANSWER_MARGIN).unwrap_or(d);
            tokio::time::timeout_at(until, lock).await.map_err(|_| {
                format!(
                    "another input call was still running when this one's time was up \
                     (lmgw/timeout_ms = {}, less {:.1} s for the answer); nothing was typed or \
                     pressed",
                    timeout_ms.unwrap_or_default(),
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
    /// Kai's site rules while the keys go into a Firefox window (`sites::keys`).
    watch: Option<sites::keys::KeyWatch>,
}

impl FocusGuard {
    fn new(
        services: &Services,
        want: Window,
        total: usize,
        unit: Unit,
        watch: Option<sites::keys::KeyWatch>,
    ) -> Result<Arc<FocusGuard>, String> {
        Ok(Arc::new(FocusGuard {
            windows: windows(services)?.clone(),
            card: services.card_keyboard.clone(),
            want,
            total,
            unit,
            wait: services.focus_wait,
            watch,
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
                if !card && let Some(w) = now.as_ref().filter(|w| w.id == self.want.id) {
                    // Kai's site rules on the window as it is now: at the first unit once the
                    // focus is back after the dialog, and before each one after.
                    if let Some(watch) = &self.watch {
                        watch.before(w, done).await.map_err(|r| {
                            Failure::Refused(format!("{r}; {}", self.progress(done)))
                        })?;
                    }
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
