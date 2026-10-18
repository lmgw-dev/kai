//! WP19's live check (`scripts/live-check-approvals.sh`, `just live-check-approvals`): Kai's core
//! (the real feed, realtime adapter and hosted server) against a dev lmgw on 127.0.0.1:8897 with
//! mocked models (`mock-openai.py` with `TOOL_SCRIPT` calling the live-test build's `live_note`, a
//! state-changing tool; the speech-to-text mock answering "Ja."), and the approval card on the GTK
//! side, all in a nested KWin with a private bus and a private PipeWire instance: nothing on the
//! session's screen, nothing audible. Keys go to the nested KWin through its fake input (this
//! binary gets the grant from a `.desktop` file in the nested data directory). Ignored by default;
//! it refuses to run outside that environment or against another gateway.
//!
//! 1. **The card and the route** (an owner's thread): the gated turn's `approval.requested` shows
//!    the card (KWin lists the surface; a crop of it in `shots/`); an Enter right at the map counts
//!    nothing (not armed); after the arming delay a plain Enter still approves nothing (a key
//!    reached the card before it armed for this call), Ctrl+Enter approves; Kai sends the verdict
//!    through `…/approvals` with its device key; the resumed turn calls `kailive__live_note` with
//!    Kai's device as approver, which K15 trusts: no dialog, the tool runs; `approval.decided`
//!    closes the card.
//! 2. **Esc declines** another call: the tool does not run, the thread says it was declined.
//! 3. **A self-approval is no approval**: a second device key (in `policy.trusted_approvers`)
//!    starts a turn and approves its own call: Kai's dialog asks (the check denies), the tool does
//!    not run.
//! 4. **The bound session (K17)**: a push-to-talk turn in Kai's conversation stops on a call; the
//!    session's `mcp_approval_request` shows the card; a press of the talk key before the card
//!    armed is talk (nothing transcribed, nothing sent); no append and no commit go to the session
//!    while it waits; after the arming the talk key records the answer, `…/transcribe` hears
//!    "Ja.", and Kai sends `mcp_approval_response` then `response.create`; the resumed reply runs
//!    the tool, no dialog.
//! 5. **A call of another thread** (an owner's): its card holds the keyboard, and Kai's own
//!    `key_press` refuses while it does (KWin's list does not name the card); a push-to-talk turn
//!    in Kai's conversation meanwhile streams and commits (the call holds back none of Kai's
//!    audio, and the talk key is not an answer to it); Esc declines it.
//!
//!    *5b.* **The screen lock** (the owner's decision, 2026-10-09): a stand-in screen saver service
//!    on the private bus (`ActiveChanged`, `GetActive`) and Kai's own lock watch: a waiting call's
//!    card goes at the lock and is back after the unlock; approved by the owner in lmgw while
//!    locked, the resumed turn's call of Kai's tool is refused; after the unlock Kai is idle.
//! 6. No device key and no tool argument in Kai's log; the card's surface and its web process go.

// The owner's requests to the dev lmgw's plain listener (its dashboard and admin API), never the
// device link: device link D15's rule (`clippy.toml`) is a paired Kai's connections'.
#![allow(clippy::disallowed_methods)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kai_desktop::caps::input::{Guard, Input, parse_keys};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use super::link::ClientEvent;
use super::lmgw_live_tests::{
    Lmgw, Mic, Report, Tap, Tapped, URL, device, env, log_is_clean, set_up_mocks,
};
use crate::audio::live_tests::{MIC, MIC_IN, SINK, read_wav, settings};
use crate::audio::{Audio, RATE};
use crate::config::Config;
use crate::core::{Command, Core};
use crate::lmgw_link::{Credentials, FeedLink, FeedSettings, RealtimeLink, RealtimeSettings};
use crate::paths::Paths;
use crate::secret::MemoryStore;
use crate::tray::model::{Link, Pairing, TrayModel, VoiceShown};
use crate::ui::UiCmd;
use crate::widgets::live_tests::{guard, kwin_windows};

const LABEL: &str = "kailive";

/// Every key the check presses goes through: nothing stops it.
struct Go;

impl Guard for Go {
    fn check(&self, _done: usize) -> kai_desktop::caps::Fut<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// Presses `key` (`enter`, `esc`) in the nested KWin.
async fn press(key: &str) {
    let chords = parse_keys(key).unwrap();
    let input = crate::desktop::plasma::input::FakeInput {
        timeout: Duration::from_secs(5),
    };
    input
        .press_chords(&chords, Arc::new(Go))
        .await
        .unwrap_or_else(|e| panic!("the fake input's {key}: {e}"));
}

/// Kai's own `key_press` of Enter while the approval card shows, as the desktop module runs it,
/// with the nested KWin's real fake input and a window list that names a konsole active (KWin's
/// list never names the card): its answer.
async fn key_press_beside_the_card() -> String {
    use kai_desktop::caps::windows::{Action, Done, Snapshot, Window, Windows};
    struct Listed;
    impl Windows for Listed {
        fn snapshot(&self) -> kai_desktop::caps::Fut<'_, Snapshot> {
            Box::pin(async {
                Ok(Snapshot {
                    windows: vec![Window {
                        id: "konsole-1".into(),
                        app: "org.kde.konsole".into(),
                        title: "build".into(),
                        pid: 1,
                        active: true,
                        ..Window::default()
                    }],
                    ..Snapshot::default()
                })
            })
        }
        fn act<'a>(&'a self, _id: &'a str, _a: &'a Action) -> kai_desktop::caps::Fut<'a, Done> {
            Box::pin(async { Ok(String::new()) })
        }
    }
    let services = crate::desktop::tools::Services {
        windows: Some(Arc::new(Listed)),
        input: Some(Arc::new(crate::desktop::plasma::input::FakeInput {
            timeout: Duration::from_secs(5),
        })),
        focus_wait: Duration::from_millis(500),
        card_keyboard: crate::approvals::keyboard::Holder::global(),
        ..Default::default()
    };
    let spec = kai_desktop::contract::find("key_press").unwrap().1;
    let args =
        crate::host::registry::Args::read(spec, json!({"keys": "enter"}).as_object().cloned())
            .unwrap();
    let reply = crate::desktop::tools::run_with(
        &services,
        "key_press",
        &args,
        &crate::desktop::tools::Ctx::default(),
    )
    .await;
    assert!(reply.error, "key_press ran beside the card");
    match &reply.blocks[..] {
        [crate::host::registry::Block::Text(t)] => t.clone(),
        other => panic!("{other:?}"),
    }
}

/// The grant for this binary: a `.desktop` naming it in the nested data directory (WP13 (d)).
async fn grant() -> Duration {
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let apps = PathBuf::from(std::env::var("XDG_DATA_HOME").unwrap()).join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("kai-live-approvals.desktop"),
        format!(
            "[Desktop Entry]\nType=Application\nName=Kai live approvals\nNoDisplay=true\nExec={}\n\
             X-KDE-Wayland-Interfaces={}\n",
            exe.display(),
            crate::desktop::plasma::input::FAKE_INPUT
        ),
    )
    .unwrap();
    let t0 = Instant::now();
    loop {
        let input = crate::desktop::plasma::input::FakeInput {
            timeout: Duration::from_secs(5),
        };
        // A press of a key nothing listens to yet: it binds the interface, or says why not.
        if input
            .press_chords(&parse_keys("shift").unwrap(), Arc::new(Go))
            .await
            .is_ok()
        {
            return t0.elapsed();
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "no fake input grant"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// The card's surface as KWin shows it (this process's only window), once it is there.
async fn card_window(secs: u64) -> crate::widgets::live_tests::KWinWindow {
    let t0 = Instant::now();
    loop {
        if let Ok(w) = kwin_windows().await
            && let Some(w) = w.into_iter().next()
        {
            return w;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "no card within {secs} s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn no_card(secs: u64) {
    let t0 = Instant::now();
    while !kwin_windows().await.unwrap_or_default().is_empty() {
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "the card is still there after {secs} s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A crop of the card from a full capture of the nested KWin (spectacle), for the owner's look.
async fn crop(out: &std::path::Path, name: &str) -> String {
    let shots = out.join("shots");
    let _ = std::fs::create_dir_all(&shots);
    let full = out.join(format!("full-{name}.png"));
    let ok = tokio::process::Command::new("spectacle")
        .args(["-b", "-n", "-f", "-o"])
        .arg(&full)
        .env("QT_QPA_PLATFORM", "wayland")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    if !ok || !full.exists() {
        return format!("crop {name}: skipped (spectacle wrote no capture)");
    }
    let w = card_window(5).await;
    let shot = shots.join(format!("{name}.png"));
    let cropped = tokio::process::Command::new("magick")
        .arg(&full)
        .args([
            "-crop",
            &format!("{}x{}+{}+{}", w.w, w.h, w.x, w.y),
            "+repage",
        ])
        .arg(&shot)
        .status()
        .await
        .is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&full);
    if cropped {
        format!("crop {name}: {}", shot.display())
    } else {
        format!("crop {name}: magick failed")
    }
}

/// The thread as the owner reads it, as text (its messages and their tool records).
async fn thread_text(lmgw: &Lmgw, id: i64) -> String {
    lmgw.owner("GET", &format!("/chat/api/threads/{id}"), None)
        .await
        .to_string()
}

async fn until_thread(lmgw: &Lmgw, id: i64, what: &str, f: impl Fn(&str) -> bool) -> String {
    let t0 = Instant::now();
    loop {
        let t = thread_text(lmgw, id).await;
        if f(&t) {
            return t;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "thread {id} never got to: {what}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Who a request goes as: the owner (its bearer, on the plain listener) or a device (its
/// certificate, on the device listener).
#[derive(Clone)]
enum Who {
    Owner(String),
    Device(Credentials),
}

impl Who {
    fn request(&self, method: &str, path: &str) -> reqwest::RequestBuilder {
        let (http, url) = match self {
            Who::Owner(_) => (reqwest::Client::new(), format!("{URL}{path}")),
            Who::Device(c) => (c.http(), format!("{}{path}", c.url())),
        };
        let rb = match method {
            "GET" => http.get(url),
            _ => http.post(url),
        };
        match self {
            Who::Owner(token) => rb.bearer_auth(token),
            Who::Device(_) => rb,
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String) {
        let mut rb = self.request(method, path).timeout(Duration::from_secs(60));
        if let Some(b) = body {
            rb = rb
                .header("content-type", "application/json")
                .body(b.to_string());
        }
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.text().await.unwrap())
    }
}

/// A thread with Kai's tools, every call waiting for an approval; `who` creates it.
async fn gated_thread(lmgw: &Lmgw, who: &Who) -> i64 {
    let (st, body) = who
        .call(
            "POST",
            "/chat/api/threads",
            Some(json!({"model_alias": "kai-chat"})),
        )
        .await;
    assert!((200..300).contains(&st), "a new thread: {st} {body}");
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_i64()
        .unwrap();
    gate(lmgw, id, who).await;
    id
}

async fn gate(lmgw: &Lmgw, id: i64, who: &Who) {
    let _ = lmgw;
    let (st, body) = who
        .call(
            "POST",
            &format!("/chat/api/threads/{id}/settings"),
            Some(json!({"mcp_tools": [{"server_label": LABEL, "require_approval": "always"}]})),
        )
        .await;
    assert!((200..300).contains(&st), "thread {id}'s tools: {st} {body}");
}

/// A message sent as `who`, its stream read to its end (the gated stop) in a task.
fn send(lmgw: &Lmgw, id: i64, who: &Who) -> tokio::task::JoinHandle<(u16, String)> {
    let rb = who.request("POST", &format!("/chat/api/threads/{id}/send"));
    let _ = lmgw;
    tokio::spawn(async move {
        let r = rb
            .header("content-type", "application/json")
            .body(json!({"content": "keep a note"}).to_string())
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .unwrap();
        let st = r.status().as_u16();
        (st, r.text().await.unwrap_or_default())
    })
}

/// The dialogs Kai asked for (PD4's), answered with `allow`.
fn dialogs(ui: &async_channel::Receiver<UiCmd>, allow: bool) -> Vec<String> {
    let mut asked = Vec::new();
    while let Ok(cmd) = ui.try_recv() {
        if let UiCmd::ConfirmTool(req) = cmd {
            asked.push(format!("{} asked by {}", req.tool, req.caller));
            let _ = req.reply.send(allow);
        }
    }
    asked
}

async fn scenario(out: PathBuf, notes: Arc<Mutex<Report>>) {
    let line = |s: String| notes.lock().unwrap().line(s);
    let clip = read_wav(&env("KAI_LIVE_CLIP"));
    let dir = tempfile::tempdir().unwrap();
    let mut lmgw = Lmgw::new();
    let boot = lmgw.start().await;
    set_up_mocks(&lmgw).await;
    let folder = lmgw
        .owner(
            "POST",
            "/chat/api/folders",
            Some(
                json!({"name": "Kai", "defaults": {"model_alias": "kai-chat"},
                "ongoing": {"idle_minutes": 0}}),
            ),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    let kai = device(&lmgw, "kai-live", json!({"hosts_label": LABEL})).await;
    // A device reaches another's hosted label only through a list naming it (L16).
    let peer = device(
        &lmgw,
        "kai-peer",
        json!({"tool_scope_mode": "allow", "tool_scope_patterns": format!("{LABEL}__*")}),
    )
    .await;
    let owner = Who::Owner(lmgw.token.clone());
    let as_peer = Who::Device(peer.creds.clone());
    line(format!(
        "dev lmgw up in {} ms; devices kai-live (hosts {LABEL}) and kai-peer; folder {folder}",
        boot.as_millis()
    ));
    let granted = grant().await;
    line(format!(
        "the fake input's grant {} ms after the .desktop was written",
        granted.as_millis()
    ));

    // Kai: the core with the hosted server, its audio in the private PipeWire instance.
    let mut s = settings(dir.path());
    s.input = Some(MIC.into());
    s.output = Some(SINK.into());
    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let audio = Audio::start(s, audio_tx).unwrap();
    let mut sb = settings(&dir.path().join("feeder"));
    sb.output = Some(MIC_IN.into());
    let (btx, _brx) = mpsc::unbounded_channel();
    let mic_audio = Audio::start(sb, btx).unwrap();
    let mut mic = Mic::new(&mic_audio);
    let paths = Paths::under_config_home(dir.path());
    std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &paths.config_file,
        format!(
            "[lmgw]\nretry_max_s = 2\n[audio]\ninput = \"{MIC}\"\noutput = \"{SINK}\"\n\
             [conversation]\nfolder_id = {folder}\n[voice]\ngrace_s = 2\nmode = \"push_to_talk\"\n\
             [shortcuts]\nbind = false\n[policy]\ntrusted_approvers = [\"device:kai-peer\"]\n\
             [approvals]\narm_ms = 1500\n"
        ),
    )
    .unwrap();
    let config = Config::load(&paths.config_file).unwrap();
    let store = Arc::new(MemoryStore::default());
    kai.store(&paths, &store);
    let feed = FeedLink::new(FeedSettings::from_config(&config));
    let tap = Tap::default();
    let voice_link = Tapped {
        link: RealtimeLink::new(RealtimeSettings::from_config(&config)),
        tap: tap.clone(),
    };
    let (model_tx, mut model) = watch::channel(TrayModel::starting(false));
    let (ui_tx, ui) = async_channel::unbounded();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let screen = crate::screen_lock::ScreenLock::default();
    let (host, policy_notes) = crate::hosted_server(
        &config,
        cmds.clone(),
        ui_tx.clone(),
        crate::host::registry::SessionBus::lazy(config.dbus.timeout()),
        screen.clone(),
        crate::desktop::fdo::atspi::switch::A11ySwitch::inert(),
    );
    let mut core = Core::new(
        config,
        paths,
        "/usr/bin/kai".into(),
        store,
        feed,
        voice_link,
        (audio, audio_rx),
        model_tx,
        ui_tx,
        None,
    );
    core.set_host(Box::new(host), policy_notes);
    core.set_desktop_state(
        screen.clone(),
        crate::desktop::fdo::atspi::switch::A11ySwitch::inert(),
    );
    let task = tokio::spawn(core.run(cmd_rx));
    let linked = |m: &TrayModel| {
        matches!(
            &m.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        ) && m
            .tools
            .iter()
            .any(|t| t.contains(&format!("linked as {LABEL}")))
    };
    tokio::time::timeout(Duration::from_secs(30), model.wait_for(linked))
        .await
        .expect("the feed up and the tools linked")
        .unwrap();
    line(format!(
        "the feed is up and the tools are linked as {LABEL}"
    ));

    // ---- 1. the card and the route: an owner's thread, Enter after the arming delay
    let t1 = gated_thread(&lmgw, &owner).await;
    let sent = send(&lmgw, t1, &owner);
    let w = card_window(20).await;
    let shown = Instant::now();
    let (st, stopped) = sent.await.unwrap();
    assert!(
        st == 200 && stopped.contains("pending_approvals"),
        "the turn stopped on the call: {st}"
    );
    line(format!(
        "1 the card: {}x{} at {},{} on {} (anchor top, width 560)",
        w.w, w.h, w.x, w.y, w.output
    ));
    // At once: the card is not armed, Enter answers nothing.
    press("enter").await;
    let early = shown.elapsed();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let t = thread_text(&lmgw, t1).await;
    assert!(
        !t.contains("live_note: s1"),
        "an Enter {} ms after the map approved nothing",
        early.as_millis()
    );
    assert!(!kwin_windows().await.unwrap().is_empty(), "the card stays");
    line(format!(
        "1 an Enter {} ms after KWin listed the card: nothing answered, the card stays (not armed: \
         approvals.arm_ms 1500 here, counted again from that key)",
        early.as_millis()
    ));
    line(crop(&out, "card").await);
    tokio::time::sleep(Duration::from_millis(1800)).await;
    // Armed now, but a key reached the card before it armed: Enter alone approves nothing.
    press("enter").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let t = thread_text(&lmgw, t1).await;
    assert!(
        !t.contains("live_note: s1"),
        "an Enter after keys before the arming approved"
    );
    assert!(!kwin_windows().await.unwrap().is_empty(), "the card stays");
    line(
        "1 an Enter after the arming delay, with a key before it: nothing answered, the card \
         stays (Enter alone no longer approves this call)"
            .into(),
    );
    let enter = Instant::now();
    press("ctrl+enter").await;
    let t = until_thread(&lmgw, t1, "the tool's result", |t| {
        t.contains("live_note: s1")
    })
    .await;
    let ran = enter.elapsed();
    assert!(
        t.contains("Tool result: live_note: s1"),
        "the reply echoes it"
    );
    no_card(10).await;
    let asked = dialogs(&ui, false);
    assert!(
        asked.is_empty(),
        "no dialog for a trusted approval: {asked:?}"
    );
    line(format!(
        "1 Ctrl+Enter approved: the route took the verdict, the resumed turn ran \
         kailive__live_note {} ms after the chord with no dialog (Kai's device approved: K15 \
         trusts it), the reply carries its result, and the card went",
        ran.as_millis()
    ));

    // ---- 2. Esc declines
    let t2 = gated_thread(&lmgw, &owner).await;
    let sent = send(&lmgw, t2, &owner);
    card_window(20).await;
    let _ = sent.await;
    tokio::time::sleep(Duration::from_millis(1800)).await;
    press("esc").await;
    let t = until_thread(&lmgw, t2, "the decline", |t| t.contains("declined")).await;
    assert!(!t.contains("live_note: s2"), "a declined call does not run");
    no_card(10).await;
    assert!(dialogs(&ui, false).is_empty());
    line(
        "2 Esc declined: the tool did not run, the thread says the user declined, the card went"
            .into(),
    );

    // ---- 3. a self-approval is no approval
    let t3 = gated_thread(&lmgw, &as_peer).await;
    let sent = send(&lmgw, t3, &as_peer);
    let (st, _) = sent.await.unwrap();
    assert_eq!(st, 200, "kai-peer's turn");
    // Kai's card may show kai-peer's call too (a thread Kai's device sees); kai-peer decides it.
    let call = {
        let v = lmgw
            .owner("GET", &format!("/chat/api/threads/{t3}"), None)
            .await;
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["pending_approvals"].as_array())
            .flatten()
            .find_map(|p| p["approval_request_id"].as_str().map(str::to_owned))
            .expect("kai-peer's call waits")
    };
    // Its answer streams the resumed turn, which waits on Kai's dialog: read in a task.
    let decided = {
        let rb = as_peer.request("POST", &format!("/chat/api/threads/{t3}/approvals"));
        let body = json!({"decisions": [{"approval_request_id": call, "approve": true}]});
        tokio::spawn(async move {
            let r = rb
                .header("content-type", "application/json")
                .body(body.to_string())
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .unwrap();
            let st = r.status().as_u16();
            (st, r.text().await.unwrap_or_default())
        })
    };
    let t0 = Instant::now();
    let asked = loop {
        let a = dialogs(&ui, false);
        if !a.is_empty() || decided.is_finished() {
            break a;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "no dialog for a self-approval"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let (st, body) = decided.await.unwrap();
    if st == 403 {
        line(format!(
            "3 skipped: lmgw refused kai-peer's own approval ({st}: {}), so the self-approval never \
             reaches Kai",
            serde_json::from_str::<Value>(&body).unwrap_or_default()["code"]
        ));
    } else {
        assert_eq!(st, 200, "kai-peer approved its own call");
        assert!(!asked.is_empty(), "no dialog for a self-approval");
        let t = until_thread(&lmgw, t3, "the resumed reply", |t| {
            t.contains("Tool result")
        })
        .await;
        assert!(!t.contains("live_note: s3"), "the tool did not run");
        line(format!(
            "3 kai-peer (in policy.trusted_approvers) approved its own call: Kai's dialog asked \
             ({}), denied there; the tool did not run",
            asked.join("; ")
        ));
    }
    no_card(10).await;

    // ---- 4. the bound session (K17)
    let conv = tap
        .with(|t| t.currents.last().and_then(|c| c.3.clone().ok()))
        .map(|(id, _)| id)
        .expect("Kai asked for its conversation's thread");
    gate(&lmgw, conv, &owner).await;
    let press_at = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let said = mic.say(&clip);
    tokio::time::sleep_until((said + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    card_window(30).await;
    let waiting = Instant::now();
    // A press before the card armed (approvals.arm_ms 1500 here) is talk, not an answer: nothing
    // is recorded for …/transcribe, and nothing reaches the session (the call still waits).
    let early = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let said = mic.say(&clip[..clip.len().min(RATE as usize / 2)]);
    tokio::time::sleep_until((said + Duration::from_millis(100)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let early_ms = early.duration_since(waiting).as_millis();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        tap.with(|t| t.transcribes.iter().all(|(at, _)| *at < early)),
        "a press before the card armed went to …/transcribe"
    );
    assert!(
        tap.sent_after(early, |s| matches!(
            s.event,
            Some(ClientEvent::ApprovalResponse { .. })
        ))
        .is_none(),
        "a press before the card armed answered"
    );
    assert!(!kwin_windows().await.unwrap().is_empty(), "the card stays");
    line(format!(
        "4 a talk press {early_ms} ms after the card showed (before it armed): talk, not an \
         answer (nothing transcribed, no answer sent, the card stays)"
    ));
    let item = tap
        .got_after(press_at, |g| {
            super::lmgw_live_tests::is_server(g, |e| {
                matches!(e, super::link::ServerEvent::ApprovalRequest(_))
            })
        })
        .is_some();
    tokio::time::sleep_until((waiting + Duration::from_millis(2200)).into()).await;
    let answer_press = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let said = mic.say(&clip[..clip.len().min(RATE as usize)]);
    tokio::time::sleep_until((said + Duration::from_millis(200)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let released = Instant::now();
    let response = tap
        .until_sent(answer_press, 30, "the approval's answer", |s| {
            matches!(s.event, Some(ClientEvent::ApprovalResponse { .. }))
        })
        .await;
    let create = tap
        .until_sent(response.at, 10, "response.create", |s| {
            s.kind == "response.create"
        })
        .await;
    let mut leaked = Vec::new();
    let mut since = waiting;
    while let Some(s) = tap.sent_after(since, |s| {
        s.at < response.at && matches!(s.kind, "append" | "commit")
    }) {
        leaked.push(s.kind);
        since = s.at + Duration::from_nanos(1);
    }
    assert!(
        leaked.is_empty(),
        "no append and no commit while the call waited: {leaked:?}"
    );
    let Some(ClientEvent::ApprovalResponse { approve, .. }) = &response.event else {
        unreachable!()
    };
    assert!(*approve, "the spoken \"Ja.\" approves");
    let t = until_thread(&lmgw, conv, "the voice turn's tool result", |t| {
        t.contains("live_note: s4")
    })
    .await;
    assert!(t.contains("live_note: s4"));
    no_card(10).await;
    assert!(
        dialogs(&ui, false).is_empty(),
        "no dialog: Kai's own session, Kai's approval"
    );
    line(format!(
        "4 the bound session: the mcp_approval_request item came ({item}), the card showed; no \
         append and no commit for {} ms of waiting; the answer's hold went to …/transcribe \
         (\"Ja.\"), mcp_approval_response {} ms after the release, response.create {} ms after it; \
         the resumed reply ran kailive__live_note with no dialog; the card went",
        (answer_press - waiting).as_millis(),
        response.at.saturating_duration_since(released).as_millis(),
        create.at.saturating_duration_since(response.at).as_millis()
    ));

    // ---- 5. a call of another thread: the keyboard flag, and no hold on Kai's audio
    // Kai's conversation without the label again, so its next turn calls no tool.
    let (st, body) = lmgw
        .call(
            "POST",
            &format!("/chat/api/threads/{conv}/settings"),
            Some(json!({"mcp_tools": []})),
            &lmgw.token,
        )
        .await;
    assert!((200..300).contains(&st), "conv's tools off: {st} {body}");
    let t5 = gated_thread(&lmgw, &owner).await;
    let sent = send(&lmgw, t5, &owner);
    card_window(20).await;
    let _ = sent.await;
    let holder = crate::approvals::keyboard::Holder::global();
    let t0 = Instant::now();
    while !holder.holds() {
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "the card never held the keyboard"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let refused = key_press_beside_the_card().await;
    assert!(
        refused.starts_with("Kai's approval card holds the keyboard"),
        "{refused}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = thread_text(&lmgw, t5).await;
    assert!(
        !t.contains("declined") && !t.contains("live_note: s5"),
        "the key reached the card"
    );
    line(format!(
        "5 the card holds the keyboard (focus-in): Kai's key_press refused (\"{refused}\"), \
         nothing reached the card"
    ));
    // Step 4's resumed reply has played and its press session closed (a press while Kai speaks
    // only stops it, K6).
    tokio::time::timeout(
        Duration::from_secs(60),
        model.wait_for(|m| super::lmgw_live_tests::shown(m) == VoiceShown::Off),
    )
    .await
    .expect("step 4's session closes after its reply")
    .unwrap();
    let ptt = Instant::now();
    cmds.send(Command::Talk(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let said = mic.say(&clip);
    tokio::time::sleep_until((said + Duration::from_millis(300)).into()).await;
    cmds.send(Command::Talk(false)).unwrap();
    let released = Instant::now();
    let appended = tap
        .until_sent(ptt, 30, "an append of the hold", |s| s.kind == "append")
        .await;
    let commit = tap
        .until_sent(released, 30, "the hold's commit", |s| s.kind == "commit")
        .await;
    assert!(
        tap.with(|t| t.transcribes.iter().all(|(at, _)| *at < ptt)),
        "the hold went to …/transcribe"
    );
    assert!(!kwin_windows().await.unwrap().is_empty(), "the card stays");
    line(format!(
        "5 a push-to-talk turn in Kai's conversation while another thread's call waits: appends \
         from {} ms after the press, the commit {} ms after the release; nothing transcribed, the \
         card stays",
        appended.at.saturating_duration_since(ptt).as_millis(),
        commit.at.saturating_duration_since(released).as_millis()
    ));
    tokio::time::sleep(Duration::from_millis(1800)).await;
    press("esc").await;
    until_thread(&lmgw, t5, "the decline", |t| t.contains("declined")).await;
    no_card(10).await;
    assert!(!holder.holds(), "the card let go of the keyboard");
    assert!(dialogs(&ui, false).is_empty());
    line("5 Esc declined it; the card went and let go of the keyboard".into());

    // ---- 5b. the screen lock (the owner's decision, 2026-10-09), from a stand-in screen saver
    // service on the private bus through Kai's own lock watch.
    lock_step(&lmgw, &cmds, &screen, &mut model, &line).await;

    // ---- 6. quit
    cmds.send(Command::Quit).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(20), task).await;
    drop(mic);
    line(format!("6 lmgw: {}", lmgw.stop().await));
    line(log_is_clean(
        &out.join("kai.log"),
        &[&kai.secret(), &peer.secret()],
    ));
    let log = std::fs::read_to_string(out.join("kai.log")).unwrap_or_default();
    for n in 1..=7 {
        assert!(
            !log.contains(&format!("s{n}-arg")),
            "a tool argument (s{n}) is in Kai's log"
        );
    }
    line("6 no tool argument in Kai's log".into());
}

/// The screen saver service as KScreenLocker serves it (`GetActive`, `ActiveChanged`), on the
/// nested session's private bus; it stands in for the lock, which this check never takes.
struct FakeSaver(Arc<std::sync::atomic::AtomicBool>);

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl FakeSaver {
    #[zbus(name = "GetActive")]
    fn get_active(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The call that waits in thread `id`.
async fn waiting_call(lmgw: &Lmgw, id: i64) -> String {
    let v = lmgw
        .owner("GET", &format!("/chat/api/threads/{id}"), None)
        .await;
    v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["pending_approvals"].as_array())
        .flatten()
        .find_map(|p| p["approval_request_id"].as_str().map(str::to_owned))
        .expect("a call waits")
}

/// 5b: a waiting call's card goes at the lock and comes back after the unlock; an approval the
/// owner gives in lmgw while the screen is locked reaches Kai's tool, which refuses.
async fn lock_step(
    lmgw: &Lmgw,
    cmds: &mpsc::UnboundedSender<Command>,
    screen: &crate::screen_lock::ScreenLock,
    model: &mut watch::Receiver<TrayModel>,
    line: &impl Fn(String),
) {
    let t = Duration::from_secs(5);
    let session = zbus::Connection::session().await.unwrap();
    assert!(
        !crate::desktop::has_owner(&session, "org.freedesktop.ScreenSaver", t)
            .await
            .unwrap(),
        "the nested session has no screen saver service of its own"
    );
    let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saver = zbus::connection::Builder::session()
        .unwrap()
        .serve_at("/ScreenSaver", FakeSaver(active.clone()))
        .unwrap()
        .name("org.freedesktop.ScreenSaver")
        .unwrap()
        .build()
        .await
        .unwrap();
    let set = |on: bool| {
        let (saver, active) = (saver.clone(), active.clone());
        async move {
            active.store(on, std::sync::atomic::Ordering::SeqCst);
            saver
                .emit_signal(
                    None::<()>,
                    "/ScreenSaver",
                    "org.freedesktop.ScreenSaver",
                    "ActiveChanged",
                    &(on,),
                )
                .await
                .unwrap();
        }
    };
    let watch = tokio::spawn(crate::screen_lock::watch(
        cmds.clone(),
        screen.clone(),
        Some(session),
        None,
        None,
        t,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A call that waits through a lock.
    let owner = Who::Owner(lmgw.token.clone());
    let t6 = gated_thread(lmgw, &owner).await;
    let sent = send(lmgw, t6, &owner);
    card_window(20).await;
    let _ = sent.await;
    let locked_at = Instant::now();
    set(true).await;
    no_card(5).await;
    let gone = locked_at.elapsed();
    tokio::time::timeout(Duration::from_secs(5), model.wait_for(|m| m.voice.locked))
        .await
        .expect("the tray says locked")
        .unwrap();
    line(format!(
        "5b locked (ActiveChanged true): the card went {} ms after the signal; the tray says \
         \"{}\"",
        gone.as_millis(),
        crate::tray::model::render(&model.borrow()).status
    ));
    set(false).await;
    let w = card_window(10).await;
    line(format!(
        "5b unlocked: the call still waits and its card is back ({}x{} on {})",
        w.w, w.h, w.output
    ));
    // Approved by the owner in lmgw while the screen is locked: Kai's tool refuses.
    set(true).await;
    no_card(5).await;
    assert!(screen.is_locked());
    let call = waiting_call(lmgw, t6).await;
    let (st, body) = lmgw
        .call(
            "POST",
            &format!("/chat/api/threads/{t6}/approvals"),
            Some(json!({"decisions": [{"approval_request_id": call, "approve": true}]})),
            &lmgw.token,
        )
        .await;
    assert_eq!(st, 200, "the owner's approval: {body}");
    let text = until_thread(lmgw, t6, "the refused call", |t| {
        t.contains("the screen is locked; Kai is not active")
    })
    .await;
    assert!(!text.contains("live_note: s6"), "the tool did not run");
    line(
        "5b approved in lmgw while locked: the resumed turn's call of kailive__live_note was \
         refused (\"the screen is locked; Kai is not active\"), the tool did not run"
            .into(),
    );
    set(false).await;
    tokio::time::timeout(Duration::from_secs(5), model.wait_for(|m| !m.voice.locked))
        .await
        .expect("unlocked")
        .unwrap();
    assert!(!model.borrow().voice.on, "idle after the unlock");
    line("5b unlocked again: Kai is idle (listening off), nothing resumed".into());
    watch.abort();
}

#[test]
#[ignore = "needs scripts/live-check-approvals.sh (a nested KWin, a private PipeWire, a dev lmgw)"]
fn live_approvals_nested() {
    if let Err(e) = guard() {
        println!("skipped: {e}");
        return;
    }
    assert_eq!(
        std::env::var("KAI_LIVE_LMGW_URL").as_deref(),
        Ok(URL),
        "only the dev lmgw on 127.0.0.1:8897"
    );
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    let log = std::fs::File::create(out.join("kai.log")).unwrap();
    let _ = tracing_subscriber::util::SubscriberInitExt::try_init(crate::logging::subscriber(
        &std::env::var("KAI_LOG").unwrap_or_else(|_| "trace".into()),
        Mutex::new(log),
        false,
    ));
    gtk::init().expect("GTK");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let report = Arc::new(Mutex::new(Report(String::new())));
    let run = rt.spawn(scenario(out.clone(), report.clone()));
    // The card's side on this thread's glib context, as Kai's GTK thread runs it.
    let result = gtk::glib::MainContext::default().block_on(async move {
        let card = crate::approvals::overlay::start();
        let result = run.await;
        card.quit().await;
        result
    });
    let text = report.lock().unwrap().0.clone();
    std::fs::write(out.join("report.txt"), &text).unwrap();
    // No web process of the card is left (K1).
    std::thread::sleep(Duration::from_millis(500));
    let me = std::process::id().to_string();
    let left: Vec<String> = std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            let ppid = stat.rsplit(") ").next()?.split(' ').nth(1)?.to_owned();
            let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
            (ppid == me && comm.starts_with("WebKit")).then(|| comm.trim().to_owned())
        })
        .collect();
    drop(rt);
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
    assert!(left.is_empty(), "WebKit processes left: {left:?}");
    println!("no WebKit process of the card is left");
}
