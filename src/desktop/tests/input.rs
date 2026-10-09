//! WP16c's input tools: over fake capabilities (what they list, say, ask the provider for and
//! refuse), K15's defaults for the real specs, the portal provider against a fake RemoteDesktop
//! portal on a private bus (the session it opens, the restore token it stores and offers, the
//! refusal, the cancel that closes the session) and the token file's permissions.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kai_desktop::caps::accessibility::Found as Element;
use kai_desktop::caps::input::{Chord, Guard, Input};
use kai_desktop::caps::windows::{Action, Done, Snapshot, Window, Windows};
use kai_desktop::caps::{Capability, Failure, Fut};
use serde_json::{Value, json};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as Z};

use super::bus::{PrivateBus, temp_dir};
use super::read::FakeAccess;
use super::{FakeWindows, all_services, snapshot};
use crate::desktop::fdo::input::{Portal, TOKEN_NAME, TokenFile};
use crate::desktop::tools::{self, Ctx, Services};
use crate::desktop::{Env, entries};
use crate::host::registry::{Args, Block, Origin, SessionBus};

const T: Duration = Duration::from_secs(2);

// ---- the tools over recording fakes ----

/// What the tools had an Input service send: one entry per call, holding the units the provider
/// sent after the guard let each through.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Text(Vec<u32>),
    Keys(Vec<Vec<u32>>),
}

/// An Input that asks the guard before each unit, as the providers do. `gate`, when set, holds
/// every call at its start until a permit is added; `entered` counts the calls that reached it.
#[derive(Clone, Default)]
pub struct FakeInput {
    sent: Arc<Mutex<Vec<Sent>>>,
    fail: Option<Failure>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    entered: Arc<std::sync::atomic::AtomicUsize>,
}

impl FakeInput {
    async fn enter(&self) {
        self.entered
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(g) = &self.gate {
            g.acquire().await.unwrap().forget();
        }
    }
}

impl Input for FakeInput {
    fn type_keysyms<'a>(&'a self, keysyms: &'a [u32], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(async move {
            self.enter().await;
            let at = {
                let mut sent = self.sent.lock().unwrap();
                sent.push(Sent::Text(Vec::new()));
                sent.len() - 1
            };
            for (done, &k) in keysyms.iter().enumerate() {
                guard.check(done).await?;
                if let Sent::Text(t) = &mut self.sent.lock().unwrap()[at] {
                    t.push(k);
                }
            }
            self.fail.clone().map_or(Ok(()), Err)
        })
    }

    fn press_chords<'a>(&'a self, chords: &'a [Chord], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(async move {
            self.enter().await;
            let at = {
                let mut sent = self.sent.lock().unwrap();
                sent.push(Sent::Keys(Vec::new()));
                sent.len() - 1
            };
            for (done, c) in chords.iter().enumerate() {
                guard.check(done).await?;
                if let Sent::Keys(k) = &mut self.sent.lock().unwrap()[at] {
                    k.push(c.0.clone());
                }
            }
            self.fail.clone().map_or(Ok(()), Err)
        })
    }
}

impl FakeInput {
    /// Nothing was typed or pressed through it.
    pub fn sent_nothing(&self) -> bool {
        self.sent.lock().unwrap().is_empty()
    }
}

pub fn fake_input() -> Arc<dyn Input> {
    Arc::new(FakeInput::default())
}

/// Windows whose focus follows a script: each `active` call takes the next id of `focus` (the last
/// one stays); `None` is no window. The snapshot is the fixture's.
#[derive(Clone)]
struct FocusWindows {
    focus: Arc<Mutex<std::collections::VecDeque<Option<&'static str>>>>,
    asked: Arc<std::sync::atomic::AtomicUsize>,
}

impl FocusWindows {
    fn new(focus: &[Option<&'static str>]) -> Self {
        FocusWindows {
            focus: Arc::new(Mutex::new(focus.iter().copied().collect())),
            asked: Arc::default(),
        }
    }
}

impl Windows for FocusWindows {
    fn snapshot(&self) -> Fut<'_, Snapshot> {
        Box::pin(async { Ok(snapshot()) })
    }

    fn active(&self) -> Fut<'_, Option<Window>> {
        Box::pin(async move {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut q = self.focus.lock().unwrap();
            let id = if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q[0]
            };
            Ok(id.map(|id| snapshot().windows.into_iter().find(|w| w.id == id).unwrap()))
        })
    }

    fn act<'a>(&'a self, _: &'a str, _: &'a Action) -> Fut<'a, Done> {
        Box::pin(async { Ok("done".to_owned()) })
    }
}

async fn call(services: &Services, name: &str, args: Value) -> (String, bool) {
    call_with(services, name, args, &Ctx::default()).await
}

async fn call_with(services: &Services, name: &str, args: Value, ctx: &Ctx) -> (String, bool) {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    let r = tools::run_with(services, name, &args, ctx).await;
    match &r.blocks[..] {
        [Block::Text(t)] => (t.clone(), r.error),
        other => panic!("{other:?}"),
    }
}

/// What the host runs before the confirmation: the lines the dialog shows, and the context the
/// call then carries.
async fn prepared(services: &Services, name: &'static str, args: Value) -> Result<Ctx, String> {
    let spec = kai_desktop::contract::find(name).unwrap().1;
    let args = Args::read(spec, args.as_object().cloned()).unwrap();
    let p = tools::input::prepare(services, name)(&args, Origin::default()).await?;
    Ok(Ctx {
        prepared: Some(p),
        ..Ctx::default()
    })
}

fn lines(ctx: &Ctx) -> Vec<String> {
    ctx.prepared.as_ref().unwrap().lines.clone()
}

fn with_input(input: &FakeInput) -> Services {
    Services {
        input: Some(Arc::new(input.clone())),
        ..all_services(FakeWindows {
            snap: snapshot(),
            ..Default::default()
        })
    }
}

fn focused(input: &FakeInput, focus: &FocusWindows, wait_ms: u64) -> Services {
    Services {
        windows: Some(Arc::new(focus.clone())),
        focus_wait: Duration::from_millis(wait_ms),
        ..with_input(input)
    }
}

fn sent(input: &FakeInput) -> Vec<Sent> {
    input.sent.lock().unwrap().clone()
}

#[tokio::test]
async fn type_text_sends_keysyms_and_says_how_many_without_the_text() {
    let input = FakeInput::default();
    let s = with_input(&input);
    let (text, err) = call(&s, "type_text", json!({"text": "Hä€\nok"})).await;
    assert!(!err, "{text}");
    assert_eq!(text, "typed 6 characters");
    assert_eq!(
        sent(&input),
        [Sent::Text(vec![
            0x48,
            0xe4,
            0x0100_20ac,
            0xff0d,
            0x6f,
            0x6b
        ])]
    );
    // Refusals say what, never the text; nothing is sent.
    input.sent.lock().unwrap().clear();
    let (text, err) = call(&s, "type_text", json!({"text": ""})).await;
    assert_eq!((text.as_str(), err), ("text is empty", true));
    let (text, err) = call(&s, "type_text", json!({"text": "hunter2\u{1b}"})).await;
    assert!(
        err && text.contains("U+001B") && !text.contains("hunter2"),
        "{text}"
    );
    assert!(sent(&input).is_empty());
}

#[tokio::test]
async fn key_press_sends_chords_of_evdev_codes() {
    let input = FakeInput::default();
    let s = with_input(&input);
    let (text, err) = call(&s, "key_press", json!({"keys": "ctrl+a  Delete"})).await;
    assert!(!err, "{text}");
    assert_eq!(text, "pressed 2 chords");
    assert_eq!(sent(&input), [Sent::Keys(vec![vec![29, 30], vec![111]])]);
    let (text, _) = call(&s, "key_press", json!({"keys": "enter"})).await;
    assert_eq!(text, "pressed 1 chord");
    input.sent.lock().unwrap().clear();
    let (text, err) = call(&s, "key_press", json!({"keys": "ctrl+hunter2"})).await;
    assert!(
        err && text.starts_with("chord 1 has a key name Kai does not know"),
        "{text}"
    );
    assert!(!text.contains("hunter2"));
    assert!(sent(&input).is_empty());
}

#[tokio::test]
async fn a_providers_failure_is_the_tools_error_and_a_missing_grant_reads_as_11_1() {
    for (failure, said) in [
        (
            Failure::NotGranted {
                what: "KWin grants org_kde_kwin_fake_input per .desktop file".into(),
            },
            "needs the installed Kai: KWin grants org_kde_kwin_fake_input per .desktop file",
        ),
        (
            Failure::Refused("the owner did not allow remote control".into()),
            "the owner did not allow remote control",
        ),
    ] {
        let input = FakeInput {
            fail: Some(failure),
            ..Default::default()
        };
        let s = with_input(&input);
        for (tool, args) in [
            ("type_text", json!({"text": "x"})),
            ("key_press", json!({"keys": "a"})),
        ] {
            assert_eq!(call(&s, tool, args).await, (said.to_owned(), true));
        }
    }
}

/// The confirmation names the window that has the keyboard focus; bad arguments and no focused
/// window are refused before anyone is asked.
#[tokio::test]
async fn the_confirmation_names_the_window_that_receives_the_input() {
    let input = FakeInput::default();
    let s = with_input(&input);
    for (tool, args) in [
        ("type_text", json!({"text": "secret"})),
        ("key_press", json!({"keys": "ctrl+a"})),
    ] {
        let ctx = prepared(&s, tool, args).await.unwrap();
        assert_eq!(
            lines(&ctx),
            ["receives it: bbbb org.kde.konsole \"build — Konsole\""]
        );
    }
    assert!(
        prepared(&s, "type_text", json!({"text": "a\u{7}"}))
            .await
            .err()
            .unwrap()
            .contains("U+0007")
    );
    assert!(
        prepared(&s, "key_press", json!({"keys": "nope"}))
            .await
            .err()
            .unwrap()
            .starts_with("chord 1")
    );
    let nobody = focused(&input, &FocusWindows::new(&[None]), 0);
    assert_eq!(
        prepared(&nobody, "type_text", json!({"text": "x"}))
            .await
            .err()
            .unwrap(),
        "no window has the keyboard focus; activate one first (window_activate)"
    );
    assert!(sent(&input).is_empty());
}

/// KWin's fake input needs no permission, so its confirmation has no note; through the
/// RemoteDesktop portal the confirmation of `type_text` and `key_press` says the permission lasts
/// until forgotten in Kai's settings and that System Settings does not end it (the owner's decision
/// of 2026-10-09), in words for a stored token and for none; `ui_press` (no typing) has none.
#[tokio::test]
async fn the_portal_s_confirmation_says_how_long_the_permission_lasts() {
    let input = FakeInput::default();
    let kwin = with_input(&input);
    let ctx = prepared(&kwin, "type_text", json!({"text": "x"}))
        .await
        .unwrap();
    assert!(ctx.prepared.as_ref().unwrap().notes.is_empty());
    let dir = temp_dir();
    let token = crate::desktop::fdo::input::token_file(dir.path());
    let portal = Services {
        input_token: Some(token.clone()),
        ..with_input(&input)
    };
    for (tool, args) in [
        ("type_text", json!({"text": "x"})),
        ("key_press", json!({"keys": "enter"})),
    ] {
        let ctx = prepared(&portal, tool, args.clone()).await.unwrap();
        let notes = &ctx.prepared.as_ref().unwrap().notes;
        assert_eq!(notes.len(), 1, "{tool}");
        assert!(
            notes[0].starts_with("The desktop asks for remote control first"),
            "{notes:?}"
        );
        token.store("t").unwrap();
        let ctx = prepared(&portal, tool, args).await.unwrap();
        let notes = &ctx.prepared.as_ref().unwrap().notes;
        assert_eq!(notes, &[tools::input::remote_control_note(true)], "{tool}");
        assert!(
            notes[0].contains("until you forget it in Kai's settings")
                && notes[0].contains("revoking it in System Settings does not end it"),
            "{notes:?}"
        );
        token.forget().unwrap();
    }
    assert!(sent(&input).is_empty());
}

/// The focus moves to another window between the confirmation and the start: Kai waits for it to
/// come back (dbus.timeout_s), and types nothing when it does not.
#[tokio::test]
async fn focus_that_moved_after_the_confirmation_types_nothing() {
    let input = FakeInput::default();
    let focus = FocusWindows::new(&[Some("bbbb"), Some("aaaa")]);
    let s = focused(&input, &focus, 120);
    let ctx = prepared(&s, "type_text", json!({"text": "hunter2"}))
        .await
        .unwrap();
    assert_eq!(
        lines(&ctx),
        ["receives it: bbbb org.kde.konsole \"build — Konsole\""]
    );
    let started = std::time::Instant::now();
    let (text, err) = call_with(&s, "type_text", json!({"text": "hunter2"}), &ctx).await;
    assert!(err);
    assert_eq!(
        text,
        "focus moved to org.kde.dolphin and did not come back within 0.12 s (dbus.timeout_s); 0 \
         of 7 characters typed"
    );
    assert!(started.elapsed() >= Duration::from_millis(120));
    assert!(sent(&input).is_empty(), "the provider was never asked");

    // It comes back within the wait (the dialog handing the focus back): typed.
    let focus = FocusWindows::new(&[Some("bbbb"), None, Some("aaaa"), Some("bbbb")]);
    let s = focused(&input, &focus, 2000);
    let ctx = prepared(&s, "key_press", json!({"keys": "ctrl+a enter"}))
        .await
        .unwrap();
    let (text, err) = call_with(&s, "key_press", json!({"keys": "ctrl+a enter"}), &ctx).await;
    assert_eq!((text.as_str(), err), ("pressed 2 chords", false));
    assert_eq!(sent(&input), [Sent::Keys(vec![vec![29, 30], vec![28]])]);
}

/// WP19: while Kai's approval card holds the keyboard (KWin's list does not name it, so the
/// confirmed window still reads as active) nothing is typed: at the start Kai waits for the card
/// to let go, mid-typing it stops at once.
#[tokio::test]
async fn nothing_is_typed_while_the_approval_card_holds_the_keyboard() {
    let input = FakeInput::default();
    let focus = FocusWindows::new(&[Some("bbbb")]);
    let s = focused(&input, &focus, 120);
    let ctx = prepared(&s, "type_text", json!({"text": "hunter2"}))
        .await
        .unwrap();
    s.card_keyboard.set(true);
    let (text, err) = call_with(&s, "type_text", json!({"text": "hunter2"}), &ctx).await;
    assert!(err);
    assert_eq!(
        text,
        "Kai's approval card holds the keyboard and did not let go of it within 0.12 s \
         (dbus.timeout_s); 0 of 7 characters typed"
    );
    assert!(sent(&input).is_empty());
    let ctx = prepared(&s, "key_press", json!({"keys": "enter"}))
        .await
        .unwrap();
    let (text, err) = call_with(&s, "key_press", json!({"keys": "enter"}), &ctx).await;
    assert!(
        err && text.starts_with("Kai's approval card holds the keyboard"),
        "{text}"
    );
    assert!(sent(&input).is_empty(), "no Enter reached the card");

    // The card lets go within the wait (it was answered): typed.
    let s = focused(&input, &focus, 2000);
    s.card_keyboard.set(true);
    let holder = s.card_keyboard.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        holder.set(false);
    });
    let ctx = prepared(&s, "key_press", json!({"keys": "enter"}))
        .await
        .unwrap();
    let (text, err) = call_with(&s, "key_press", json!({"keys": "enter"}), &ctx).await;
    assert_eq!((text.as_str(), err), ("pressed 1 chord", false));
}

/// The focus moves while Kai types: it stops before the next character and says how far it got,
/// not what it typed; the same between chords.
#[tokio::test]
async fn focus_that_moves_mid_typing_stops_the_call() {
    let input = FakeInput::default();
    // prepare, the tool's first check, the provider's checks of units 0, 1, 2; then dolphin.
    let focus = FocusWindows::new(&[
        Some("bbbb"),
        Some("bbbb"),
        Some("bbbb"),
        Some("bbbb"),
        Some("bbbb"),
        Some("aaaa"),
    ]);
    let s = focused(&input, &focus, 2000);
    let ctx = prepared(&s, "type_text", json!({"text": "hunter2"}))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let (text, err) = call_with(&s, "type_text", json!({"text": "hunter2"}), &ctx).await;
    assert!(err);
    assert_eq!(
        text,
        "focus moved to org.kde.dolphin; 3 of 7 characters typed"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no wait after the first event"
    );
    assert_eq!(sent(&input), [Sent::Text(vec![0x68, 0x75, 0x6e])]);

    input.sent.lock().unwrap().clear();
    let focus = FocusWindows::new(&[Some("bbbb"), Some("bbbb"), Some("bbbb"), None]);
    let s = focused(&input, &focus, 2000);
    let args = json!({"keys": "ctrl+a delete enter"});
    let ctx = prepared(&s, "key_press", args.clone()).await.unwrap();
    let (text, err) = call_with(&s, "key_press", args, &ctx).await;
    assert!(err);
    assert_eq!(
        text,
        "focus left org.kde.konsole (no window has it); 1 of 3 chords pressed"
    );
    assert_eq!(sent(&input), [Sent::Keys(vec![vec![29, 30]])]);
}

/// Input calls take turns: a second call waits until the first ended; a waiting call stops at its
/// deadline less the answer's margin, and one that is dropped (lmgw's cancel) never types.
#[tokio::test]
async fn input_calls_take_turns() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let input = FakeInput {
        gate: Some(gate.clone()),
        ..Default::default()
    };
    let s = with_input(&input);
    let entered = || input.entered.load(std::sync::atomic::Ordering::SeqCst);
    let first = tokio::spawn({
        let s = s.clone();
        async move { call(&s, "type_text", json!({"text": "ab"})).await }
    });
    while entered() < 1 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let second = tokio::spawn({
        let s = s.clone();
        async move { call(&s, "key_press", json!({"keys": "enter"})).await }
    });
    // Dropped while it waits for its turn.
    let dropped = tokio::time::timeout(
        Duration::from_millis(50),
        call(&s, "type_text", json!({"text": "zz"})),
    )
    .await;
    assert!(dropped.is_err());
    // A deadline 0.7 s ahead: it waits 0.2 s (less the 0.5 s margin) and gives up.
    let ctx = Ctx {
        deadline: Some(tokio::time::Instant::now() + Duration::from_millis(700)),
        timeout_ms: Some(700),
        ..Ctx::default()
    };
    let t0 = std::time::Instant::now();
    let (text, err) = call_with(&s, "type_text", json!({"text": "late"}), &ctx).await;
    assert!(err);
    assert_eq!(
        text,
        "another input call was still running when this one's time was up (lmgw/timeout_ms = \
         700, less 0.5 s for the answer); nothing was typed or pressed"
    );
    assert!(t0.elapsed() >= Duration::from_millis(150) && t0.elapsed() < Duration::from_secs(1));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(entered(), 1, "only the first call reached the provider");
    gate.add_permits(1);
    assert_eq!(first.await.unwrap().0, "typed 2 characters");
    while entered() < 2 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    gate.add_permits(1);
    assert_eq!(second.await.unwrap().0, "pressed 1 chord");
    assert_eq!(
        sent(&input),
        [Sent::Text(vec![0x61, 0x62]), Sent::Keys(vec![vec![28]])],
        "in turn, and nothing of the dropped or the late call"
    );
    assert_eq!(entered(), 2);
}

/// ui_press finds the element before the confirmation (the dialog names it; nothing is pressed
/// yet), and the call presses exactly that one.
#[tokio::test]
async fn ui_press_finds_the_element_before_the_confirmation_and_presses_that_one() {
    let save = Element {
        role: "button".into(),
        name: "Save".into(),
        at: (":1.7".into(), "/org/a11y/atspi/accessible/12".into()),
    };
    let access = FakeAccess {
        found: Some(save.clone()),
        ..Default::default()
    };
    let s = Services {
        accessibility: Some(Arc::new(access.clone())),
        ..with_input(&FakeInput::default())
    };
    let ctx = prepared(&s, "ui_press", json!({"element": "button \"Save\""}))
        .await
        .unwrap();
    assert_eq!(
        lines(&ctx),
        ["presses: button \"Save\" in bbbb org.kde.konsole \"build — Konsole\""]
    );
    assert_eq!(access.finds.lock().unwrap().len(), 1);
    assert!(
        access.presses.lock().unwrap().is_empty(),
        "nothing pressed yet"
    );
    let (text, err) = call_with(&s, "ui_press", json!({"element": "button \"Save\""}), &ctx).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "pressed button \"Save\" in bbbb org.kde.konsole \"build — Konsole\""
    );
    assert_eq!(access.finds.lock().unwrap().len(), 1, "not looked up again");
    assert_eq!(
        access.presses.lock().unwrap().clone(),
        std::slice::from_ref(&save)
    );

    // Another window by name; the target is the window's.
    let ctx = prepared(
        &s,
        "ui_press",
        json!({"window": "dolphin", "element": "Save"}),
    )
    .await
    .unwrap();
    assert!(
        lines(&ctx)[0].ends_with("in aaaa org.kde.dolphin \"Home — Dolphin\""),
        "{:?}",
        lines(&ctx)
    );
    let finds = access.finds.lock().unwrap().clone();
    assert_eq!(finds[0].0.title, "build — Konsole");
    assert_eq!(finds[0].1.role.as_deref(), Some("button"));
    assert_eq!(finds[1].0.title, "Home — Dolphin");
    assert_eq!(finds[1].1.name, "Save");

    // A query with no name is refused before the desktop is asked; the provider's refusal (none,
    // several) is the preparer's, so nobody is asked about it.
    assert!(
        prepared(&s, "ui_press", json!({"element": "  "}))
            .await
            .err()
            .unwrap()
            .starts_with("element needs the name")
    );
    assert_eq!(access.finds.lock().unwrap().len(), 2);
    let none = Services {
        accessibility: Some(Arc::new(FakeAccess::default())),
        ..s
    };
    assert_eq!(
        prepared(&none, "ui_press", json!({"element": "Nothing"}))
            .await
            .err()
            .unwrap(),
        "no pressable element matches"
    );
    assert_eq!(access.presses.lock().unwrap().len(), 1);
}

/// K23: type_text and key_press need the Input and window services, ui_press accessibility and
/// windows.
#[test]
fn the_input_tools_are_listed_only_where_they_can_be_served() {
    let listed = |s: &Services| -> Vec<&'static str> {
        entries(s)
            .iter()
            .filter(|e| e.area == "input")
            .map(|e| e.spec.name)
            .collect()
    };
    let all = all_services(FakeWindows::default());
    assert_eq!(listed(&all), ["ui_press", "type_text", "key_press"]);
    assert_eq!(
        listed(&Services {
            input: None,
            ..all.clone()
        }),
        ["ui_press"]
    );
    assert_eq!(
        listed(&Services {
            accessibility: None,
            ..all.clone()
        }),
        ["type_text", "key_press"]
    );
    // The window with the focus is what the owner confirms and what the typing checks.
    assert!(
        listed(&Services {
            windows: None,
            ..all
        })
        .is_empty()
    );
    assert_eq!(
        kai_desktop::contract::input::needs("type_text"),
        [Capability::Input, Capability::Windows]
    );
}

/// K24, ruling 28: confirmed for every caller by default, and the owner's per-tool state can allow
/// each.
#[test]
fn the_input_tools_are_confirmed_by_default_and_can_be_allowed() {
    use crate::host::policy::{Caller, Decision, Policy};
    use crate::host::settings::{PolicyConfig, ToolState};
    let confirm = Policy::new(&PolicyConfig::default());
    let mut config = PolicyConfig::default();
    for t in ["ui_press", "type_text", "key_press"] {
        config.tools.insert(t.into(), ToolState::Allow);
    }
    let allow = Policy::new(&config);
    for tool in ["ui_press", "type_text", "key_press"] {
        let (_, spec) = kai_desktop::contract::find(tool).unwrap();
        for caller in [Caller::Owner, Caller::Gateway, Caller::Own] {
            assert!(
                matches!(
                    confirm.decide(spec, &caller, None),
                    Decision::Confirm { .. }
                ),
                "{tool}"
            );
            assert!(
                matches!(allow.decide(spec, &caller, None), Decision::Allow),
                "{tool}"
            );
        }
        assert!(!spec.hints.read_only && !spec.hints.destructive);
    }
}

// ---- the portal provider against a fake RemoteDesktop portal ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Answers with devices and a new restore token.
    Allow,
    /// The owner closed the dialog.
    Cancel,
    /// Never answers (the dialog stays open).
    Hang,
}

#[derive(Default)]
struct Log {
    /// `SelectDevices`' options as `key=value`.
    select: Vec<Vec<String>>,
    /// `Notify*` calls: ("keysym" | "keycode", code, state).
    keys: Vec<(&'static str, i32, u32)>,
    closed: usize,
    tokens_issued: usize,
}

struct FakePortal {
    log: Arc<Mutex<Log>>,
    start: Arc<Mutex<Start>>,
    device_types: u32,
    /// A keycode event (code, state) that is logged and then never answered.
    hang: Arc<Mutex<Option<(i32, u32)>>>,
}

/// `/org/freedesktop/portal/desktop/request/<sender>/<token>` for the caller.
fn request_path(sender: &str, options: &HashMap<String, OwnedValue>) -> String {
    let token = options
        .get("handle_token")
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap();
    format!(
        "/org/freedesktop/portal/desktop/request/{}/{token}",
        sender.trim_start_matches(':').replace('.', "_")
    )
}

/// Answers a request after a moment (the portal answers after the call returned).
fn respond(
    conn: &zbus::Connection,
    path: String,
    code: u32,
    results: HashMap<&'static str, Z<'static>>,
) {
    let conn = conn.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        conn.emit_signal(
            None::<&str>,
            path.as_str(),
            "org.freedesktop.portal.Request",
            "Response",
            &(code, results),
        )
        .await
        .unwrap();
    });
}

struct FakeSessionObject(Arc<Mutex<Log>>);

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl FakeSessionObject {
    #[zbus(name = "Close")]
    fn close(&self) {
        self.0.lock().unwrap().closed += 1;
    }
}

#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl FakePortal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
    #[zbus(property, name = "AvailableDeviceTypes")]
    fn available_device_types(&self) -> u32 {
        self.device_types
    }

    #[zbus(name = "CreateSession")]
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> OwnedObjectPath {
        let sender = header.sender().unwrap().to_string();
        let session_token = options
            .get("session_handle_token")
            .and_then(|v| <&str>::try_from(v).ok())
            .unwrap()
            .to_owned();
        let session = format!(
            "/org/freedesktop/portal/desktop/session/{}/{session_token}",
            sender.trim_start_matches(':').replace('.', "_")
        );
        conn.object_server()
            .at(session.as_str(), FakeSessionObject(self.log.clone()))
            .await
            .unwrap();
        let request = request_path(&sender, &options);
        respond(
            conn,
            request.clone(),
            0,
            HashMap::from([("session_handle", Z::new(session))]),
        );
        OwnedObjectPath::try_from(request).unwrap()
    }

    #[zbus(name = "SelectDevices")]
    async fn select_devices(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> OwnedObjectPath {
        let sender = header.sender().unwrap().to_string();
        let mut shown: Vec<String> = options
            .iter()
            .filter(|(k, _)| k.as_str() != "handle_token")
            .map(|(k, v)| format!("{k}={v:?}"))
            .collect();
        shown.sort();
        self.log.lock().unwrap().select.push(shown);
        let request = request_path(&sender, &options);
        respond(conn, request.clone(), 0, HashMap::new());
        OwnedObjectPath::try_from(request).unwrap()
    }

    #[zbus(name = "Start")]
    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> OwnedObjectPath {
        let sender = header.sender().unwrap().to_string();
        let request = request_path(&sender, &options);
        match *self.start.lock().unwrap() {
            Start::Allow => {
                let n = {
                    let mut log = self.log.lock().unwrap();
                    log.tokens_issued += 1;
                    log.tokens_issued
                };
                respond(
                    conn,
                    request.clone(),
                    0,
                    HashMap::from([
                        ("devices", Z::U32(1)),
                        ("restore_token", Z::new(format!("token-{n}"))),
                    ]),
                );
            }
            Start::Cancel => respond(conn, request.clone(), 1, HashMap::new()),
            Start::Hang => {}
        }
        OwnedObjectPath::try_from(request).unwrap()
    }

    #[zbus(name = "NotifyKeyboardKeycode")]
    async fn notify_keycode(
        &self,
        _s: OwnedObjectPath,
        _o: HashMap<String, OwnedValue>,
        code: i32,
        state: u32,
    ) {
        self.log.lock().unwrap().keys.push(("keycode", code, state));
        if *self.hang.lock().unwrap() == Some((code, state)) {
            std::future::pending::<()>().await;
        }
    }

    #[zbus(name = "NotifyKeyboardKeysym")]
    fn notify_keysym(
        &self,
        _s: OwnedObjectPath,
        _o: HashMap<String, OwnedValue>,
        sym: i32,
        state: u32,
    ) {
        self.log.lock().unwrap().keys.push(("keysym", sym, state));
    }
}

struct Fixture {
    bus: PrivateBus,
    log: Arc<Mutex<Log>>,
    start: Arc<Mutex<Start>>,
    hang: Arc<Mutex<Option<(i32, u32)>>>,
    portal: Portal,
    dir: tempfile::TempDir,
    _held: zbus::Connection,
}

async fn fixture(device_types: u32) -> Option<Fixture> {
    let bus = PrivateBus::start()?;
    let log = Arc::new(Mutex::new(Log::default()));
    let start = Arc::new(Mutex::new(Start::Allow));
    let hang = Arc::new(Mutex::new(None));
    let server = bus.connect().await;
    server
        .object_server()
        .at(
            "/org/freedesktop/portal/desktop",
            FakePortal {
                log: log.clone(),
                start: start.clone(),
                device_types,
                hang: hang.clone(),
            },
        )
        .await
        .unwrap();
    server
        .request_name("org.freedesktop.portal.Desktop")
        .await
        .unwrap();
    let dir = temp_dir();
    let portal = Portal {
        bus: SessionBus::connected(bus.connect().await, T),
        timeout: T,
        token: Some(TokenFile(dir.path().join("state/kai").join(TOKEN_NAME))),
    };
    Some(Fixture {
        bus,
        log,
        start,
        hang,
        portal,
        dir,
        _held: server,
    })
}

#[tokio::test]
async fn the_portal_types_in_a_session_it_closes_and_keeps_the_token_private() {
    let Some(f) = fixture(7).await else { return };
    let file = f.portal.token.clone().unwrap();
    assert_eq!(file.load(), None);

    f.portal
        .type_keysyms(&[0x68, 0x0100_20ac], open())
        .await
        .unwrap();
    {
        let log = f.log.lock().unwrap();
        assert_eq!(
            log.keys,
            [
                ("keysym", 0x68, 1),
                ("keysym", 0x68, 0),
                ("keysym", 0x0100_20ac, 1),
                ("keysym", 0x0100_20ac, 0)
            ]
        );
        assert_eq!(log.closed, 1, "the session is closed");
        let select = log.select[0].join(" ");
        assert!(
            select.contains("persist_mode=OwnedValue(U32(2))"),
            "{select}"
        );
        assert!(select.contains("types=OwnedValue(U32(1))"), "{select}");
        assert!(!select.contains("restore_token"), "no token yet: {select}");
    }
    // The token the portal issued is stored, readable by the owner only.
    assert_eq!(file.load().as_deref(), Some("token-1"));
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file.0), 0o600);
    assert_eq!(mode(file.0.parent().unwrap()), 0o700);

    // The next session offers it and stores the new one.
    f.portal
        .press_chords(&[Chord(vec![29, 30]), Chord(vec![28])], open())
        .await
        .unwrap();
    let log = f.log.lock().unwrap();
    assert!(
        log.select[1].join(" ").contains("restore_token"),
        "{:?}",
        log.select[1]
    );
    assert!(log.select[1].join(" ").contains("token-1"));
    assert_eq!(
        log.keys[4..],
        [
            ("keycode", 29, 1),
            ("keycode", 30, 1),
            ("keycode", 30, 0),
            ("keycode", 29, 0),
            ("keycode", 28, 1),
            ("keycode", 28, 0)
        ]
    );
    assert_eq!(log.closed, 2);
    drop(log);
    assert_eq!(file.load().as_deref(), Some("token-2"));
    assert!(
        std::fs::read_dir(file.0.parent().unwrap())
            .unwrap()
            .all(|e| e.unwrap().file_name() == TOKEN_NAME),
        "no temporary file is left"
    );
    drop(f.dir);
}

#[tokio::test]
async fn an_owner_who_closes_the_dialog_is_a_refusal_and_stores_nothing() {
    let Some(f) = fixture(7).await else { return };
    *f.start.lock().unwrap() = Start::Cancel;
    let e = f.portal.type_keysyms(&[0x61], open()).await.unwrap_err();
    assert!(
        matches!(&e, Failure::Refused(m) if m.contains("did not allow remote control")),
        "{e:?}"
    );
    assert_eq!(f.portal.token.as_ref().unwrap().load(), None);
    assert!(f.log.lock().unwrap().keys.is_empty());
    assert_eq!(f.log.lock().unwrap().closed, 1, "the session is closed");
}

/// lmgw's cancel drops the call while the owner has not answered the dialog: the session closes.
#[tokio::test]
async fn a_call_dropped_at_the_dialog_closes_its_session() {
    let Some(f) = fixture(7).await else { return };
    *f.start.lock().unwrap() = Start::Hang;
    let waited = tokio::time::timeout(
        Duration::from_millis(500),
        f.portal.type_keysyms(&[0x61], open()),
    )
    .await;
    assert!(
        waited.is_err(),
        "Start waits for the owner without a bound of Kai's"
    );
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while f.log.lock().unwrap().closed == 0 {
        assert!(std::time::Instant::now() < end, "never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(f.log.lock().unwrap().keys.is_empty());
}

/// The guard refuses from unit `at` on.
struct StopAt(usize);

impl Guard for StopAt {
    fn check(&self, done: usize) -> Fut<'_, ()> {
        Box::pin(async move {
            if done < self.0 {
                Ok(())
            } else {
                Err(Failure::Refused(format!("stopped at {done}")))
            }
        })
    }
}

fn open() -> Arc<dyn Guard> {
    Arc::new(StopAt(usize::MAX))
}

/// The guard is asked before each unit: a refusal stops the portal's call there, with every key
/// up and the session closed.
#[tokio::test]
async fn the_portal_stops_where_the_guard_refuses() {
    let Some(f) = fixture(7).await else { return };
    let e = f
        .portal
        .type_keysyms(&[0x61, 0x62, 0x63], Arc::new(StopAt(2)))
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "stopped at 2");
    let e = f
        .portal
        .press_chords(&[Chord(vec![29, 30]), Chord(vec![28])], Arc::new(StopAt(1)))
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "stopped at 1");
    let log = f.log.lock().unwrap();
    assert_eq!(
        log.keys,
        [
            ("keysym", 0x61, 1),
            ("keysym", 0x61, 0),
            ("keysym", 0x62, 1),
            ("keysym", 0x62, 0),
            ("keycode", 29, 1),
            ("keycode", 30, 1),
            ("keycode", 30, 0),
            ("keycode", 29, 0),
        ]
    );
    assert_eq!(log.closed, 2);
}

/// lmgw's cancel drops the call in the middle of a chord: the keys already down are released, in
/// reverse, before the session closes.
#[tokio::test]
async fn a_call_dropped_mid_chord_releases_its_keys_before_closing() {
    let Some(f) = fixture(7).await else { return };
    *f.hang.lock().unwrap() = Some((30, 1));
    let waited = tokio::time::timeout(
        Duration::from_millis(300),
        f.portal
            .press_chords(&[Chord(vec![29, 30, 31]), Chord(vec![28])], open()),
    )
    .await;
    assert!(waited.is_err(), "the press of 30 never returns");
    let end = std::time::Instant::now() + Duration::from_secs(3);
    while f.log.lock().unwrap().closed == 0 {
        assert!(std::time::Instant::now() < end, "never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        f.log.lock().unwrap().keys,
        [
            ("keycode", 29, 1),
            ("keycode", 30, 1),
            ("keycode", 30, 0),
            ("keycode", 29, 0),
        ],
        "released in reverse, then closed"
    );
}

/// The portal's pacing: every event waits for its reply, every release is followed by
/// RELEASE_PAUSE (so 50 characters take at least 50 pauses).
#[tokio::test]
async fn the_portal_paces_its_releases() {
    let Some(f) = fixture(7).await else { return };
    let text: Vec<u32> = (0..50).map(|i| 0x61 + i % 26).collect();
    let t0 = std::time::Instant::now();
    f.portal.type_keysyms(&text, open()).await.unwrap();
    assert!(t0.elapsed() >= kai_desktop::caps::input::RELEASE_PAUSE * 50);
    assert_eq!(f.log.lock().unwrap().keys.len(), 100);
}

fn env_with(bus: &PrivateBus, conn: zbus::Connection, state: &std::path::Path) -> Env {
    let _ = bus;
    Env {
        session: Some(SessionBus::connected(conn, T)),
        timeout: T,
        state_dir: state.to_owned(),
        ..Env::inert()
    }
}

#[tokio::test]
async fn the_portal_is_offered_where_it_can_control_a_keyboard() {
    for (types, offered) in [(7, true), (1, true), (6, false)] {
        let Some(f) = fixture(types).await else {
            return;
        };
        let env = env_with(&f.bus, f.bus.connect().await, f.dir.path());
        let offer = crate::desktop::fdo::input::offer(&env).await;
        assert_eq!(offer.capability, Capability::Input);
        assert_eq!(offer.found.is_ok(), offered, "types {types}");
    }
    // No portal on the bus, and none activatable.
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let dir = temp_dir();
    let env = env_with(&bus, bus.connect().await, dir.path());
    let offer = crate::desktop::fdo::input::offer(&env).await;
    assert_eq!(
        offer.found.err().as_deref(),
        Some("xdg-desktop-portal is neither running nor activatable")
    );
}

/// Without a Wayland display or a session bus a provider offers nothing and says why.
#[tokio::test]
async fn the_providers_offer_nothing_without_their_transport() {
    let offer = crate::desktop::plasma::input::offer(&Env::inert()).await;
    assert_eq!(offer.capability, Capability::Input);
    assert_eq!(offer.found.err().as_deref(), Some("no Wayland display"));
    let offer = crate::desktop::fdo::input::offer(&Env::inert()).await;
    assert_eq!(offer.found.err().as_deref(), Some("no session bus"));
}

#[test]
fn the_token_is_only_rewritten_when_it_changes_and_a_missing_directory_is_made() {
    let dir = temp_dir();
    let file = TokenFile(dir.path().join("a/b").join(TOKEN_NAME));
    assert_eq!(file.load(), None);
    file.store("one").unwrap();
    let before = std::fs::metadata(&file.0).unwrap().modified().unwrap();
    file.store("one").unwrap();
    assert_eq!(
        std::fs::metadata(&file.0).unwrap().modified().unwrap(),
        before
    );
    file.store("two").unwrap();
    assert_eq!(file.load().as_deref(), Some("two"));
    std::fs::write(&file.0, "  \n").unwrap();
    assert_eq!(file.load(), None, "an empty file is no token");
    // Forgetting it (kai --forget-remote-control).
    assert!(file.forget().unwrap());
    assert!(!file.forget().unwrap(), "nothing left to forget");
    assert_eq!(file.load(), None);
}

/// A store cut short before its rename leaves the token in the temporary file: forgetting removes
/// that too, with or without the file itself.
#[test]
fn forgetting_also_removes_a_store_cut_short() {
    let dir = temp_dir();
    let file = TokenFile(dir.path().join(TOKEN_NAME));
    let tmp = dir.path().join(format!("{TOKEN_NAME}.tmp"));
    file.store("stored").unwrap();
    assert!(!tmp.exists(), "a completed store leaves no temporary file");
    std::fs::write(&tmp, "cut-short").unwrap();
    assert!(file.forget().unwrap());
    assert!(!file.0.exists() && !tmp.exists(), "both forgotten");
    std::fs::write(&tmp, "cut-short").unwrap();
    assert!(
        file.forget().unwrap(),
        "a token only in the temporary file was stored too"
    );
    assert!(!tmp.exists());
    assert!(!file.forget().unwrap(), "nothing left to forget");
}

/// A state directory that existed with looser modes is made 0700; one that is a symlink is never
/// followed: the token is not stored there.
#[test]
fn the_tokens_directory_is_made_private_and_a_symlink_is_refused() {
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let dir = temp_dir();
    let state = dir.path().join("kai");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = crate::desktop::fdo::input::token_file(&state);
    file.store("one").unwrap();
    assert_eq!(mode(&state), 0o700);
    assert_eq!(mode(&file.0), 0o600);
    // Unchanged token, loosened directory: tightened again.
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o775)).unwrap();
    file.store("one").unwrap();
    assert_eq!(mode(&state), 0o700);

    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = dir.path().join("linked");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let e = crate::desktop::fdo::input::token_file(&link)
        .store("two")
        .unwrap_err();
    assert!(
        e.to_string().contains("not a directory of this user's"),
        "{e}"
    );
    assert_eq!(mode(&elsewhere), 0o755, "the symlink's target is untouched");
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
}

mod live;
