//! The live check of WP12's MCP Apps bridge (section `apps`), in the nested KWin:
//!
//! 1. **The postMessage path** (WP10's open point): a view's `postMessage` to its parent reaches
//!    Kai's host script in its isolated world, `MessageEvent.source` there is the frame Kai placed,
//!    and Kai's answer reaches the view: a view does `ping` and a `tools/call` against a scripted
//!    `/mcp` and reports both answers; a message over `overlay.app_max_mb` is answered `-32600`
//!    by Kai's app script and never reaches Kai; hidden, the view gets Kai's
//!    `ui/resource-teardown` first (its answer is seen). A message posted by the host page's own
//!    main world, and an `overlay_html` document's message to its parent, reach no bridge.
//! 2. **Against a dev lmgw** (scripts/live-check-widgets.sh starts it on 127.0.0.1:8897 with
//!    `scripts/mcp-apps-fixture.py` registered under the prefix `fx`, and hands the URL and a
//!    device key over in the file `KAI_LIVE_APPS`): `overlay_app`'s open reads the fixture's view
//!    through `/mcp` under the key, the view loads its data through the bridge (an app-only tool),
//!    its state-changing call asks the owner (scripted: no), and every refusal of K18 holds; its
//!    own resources are listed in its own terms; the refused CSP origin and permission are named
//!    in the answer and `overlay_list`; a crop; hidden, it gets `ui/resource-teardown`.
//! 3. **A call in Kai's conversation** (`thread_apps`, the owner's decisions of 2026-10-09): the
//!    frames Kai's bound session relays for a call of `fx__card_live` (`ready`, then `result`)
//!    show the fixture's SDK-style view by themselves, which gets `tool-input` and then
//!    `tool-result`, names its tool from `hostContext.toolInfo`, and, its app trusted
//!    (`app:fx`), calls `card_bump` without the owner's dialog; a second call replaces the first
//!    view and, its turn stopped, gets `tool-cancelled`; a tool with no view shows nothing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use super::{OUTPUTS, Run, crops, look, report_json, sleep};
use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::lmgw_link::Credentials;
use crate::lmgw_link::mcp::{Bound, McpError};
use crate::overlay::placement::Anchor;
use crate::pairing::DeviceKey;
use crate::voice::link::Pending;
use crate::widgets::app_bridge::{AppBody, Apps, Bridge, Calls, Rules, TEARDOWN_ID, Upstream};
use crate::widgets::gtk_host::Probe;
use crate::widgets::kinds::{Body, Kind};
use crate::widgets::sandbox::PageMsg;

/// A scripted `/mcp`: a call answers with the name it ran.
pub(super) struct Scripted;

impl Upstream for Scripted {
    fn request(&self, method: &'static str, params: Value) -> Pending<Result<Value, McpError>> {
        Box::pin(async move {
            match method {
                "tools/call" => {
                    Ok(json!({"content": [], "structuredContent": {"called": params["name"]}}))
                }
                _ => Err(McpError::Link("not scripted".into())),
            }
        })
    }

    fn close(&self) -> Pending<()> {
        Box::pin(async {})
    }
}

/// The owner, scripted: says no to everything, and keeps what was asked.
#[derive(Default)]
pub(super) struct No(Mutex<Vec<String>>);

impl Confirmer for No {
    fn ask(&self, req: ConfirmRequest) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} by {}", req.tool, req.caller));
        let _ = req.reply.send(false);
    }
}

pub(super) fn bound() -> Bound {
    Bound {
        bytes: 64 << 20,
        says: "overlay.app_max_mb = 64".into(),
    }
}

/// The proof's bound: small, so its view can go over it.
fn small() -> Bound {
    Bound {
        bytes: 4096,
        says: "overlay.app_max_mb (4096 bytes for the proof)".into(),
    }
}

pub(super) fn calls() -> Calls {
    Calls {
        max: 16,
        says: "overlay.app_max_calls = 16".into(),
    }
}

/// Widget `id`'s answer to Kai's `ui/resource-teardown`, relayed after it was hidden.
async fn torn_down(r: &mut Run, id: u64) -> bool {
    r.until(5, |p| match p {
        Probe::Page(i, PageMsg::Rpc(t)) if *i == id => serde_json::from_str::<Value>(t)
            .is_ok_and(|v| v["id"] == TEARDOWN_ID && v.get("result").is_some()),
        _ => false,
    })
    .await
    .is_some()
}

const PROOF: &str = r#"<!doctype html><meta charset="utf-8">
<body style="margin:0;background:#232629;color:#eff0f1;font:20px sans-serif">proof
<script>
const seen = { posted: true };
let left = 3;
const done = () => {
  location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(seen));
};
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || typeof m !== "object") return;
  if (m.method === "ui/resource-teardown") {
    parent.postMessage({ jsonrpc: "2.0", id: m.id, result: {} }, "*");
    return;
  }
  if (m.id === 1) seen.ping = m.result ? "ok" : "error";
  if (m.id === 2) {
    seen.call = m.result ? m.result.structuredContent.called : "error " + m.error.code;
    seen.sourceIsParent = e.source === window.parent;
  }
  if (m.id === 3) seen.big = m.error ? m.error.code + " " + m.error.message : "answered";
  if (m.id >= 1 && m.id <= 3 && --left === 0) done();
});
parent.postMessage({ jsonrpc: "2.0", id: 1, method: "ping" }, "*");
parent.postMessage({ jsonrpc: "2.0", id: 2, method: "tools/call",
                     params: { name: "read", arguments: {} } }, "*");
parent.postMessage({ jsonrpc: "2.0", id: 3, method: "ping",
                     params: { pad: "x".repeat(5000) } }, "*");
setTimeout(() => { seen.timeout = true; done(); }, 5000);
</script>"#;

const HTML_POSTS: &str = r#"<!doctype html><script>
parent.postMessage({ jsonrpc: "2.0", id: 1, method: "ping" }, "*");
setTimeout(() => { location.href = "kai-sandbox://widget/report?html"; }, 800);
</script>"#;

fn rpcs(r: &Run, id: u64) -> usize {
    r.seen
        .iter()
        .filter(|p| matches!(p, Probe::Page(i, PageMsg::Rpc(_)) if *i == id))
        .count()
}

async fn report(r: &mut Run, id: u64, secs: u64) -> Option<Value> {
    let p = r
        .until(secs, |p| {
            matches!(p, Probe::Refused { id: i, uri, .. } if *i == id && uri.starts_with("kai-sandbox://widget/report?"))
        })
        .await?;
    let Probe::Refused { uri, .. } = p else {
        return None;
    };
    Some(report_json(&uri))
}

async fn proof(r: &mut Run, rt: &tokio::runtime::Runtime) {
    r.line("== 5d. the MCP Apps bridge: the postMessage path into Kai's isolated world".into());
    let rules = Rules::of_list(
        "fx",
        &json!({"tools": [{"name": "fx__read", "annotations": {"readOnlyHint": true}}]}),
    );
    let ends = {
        let _in = rt.enter();
        Bridge::new(
            Arc::new(Scripted),
            rules,
            Arc::new(No::default()),
            small(),
            calls(),
            (480, 320),
        )
        .spawn()
    };
    let body = Body::App(AppBody {
        server: "fx".into(),
        html: PROOF.into(),
        ends,
        notes: vec![],
        bound: small(),
    });
    let shown = r
        .show(
            Kind::App,
            body,
            look(Anchor::TopLeft, Some(OUTPUTS[0]), 480, 320),
        )
        .await;
    let Ok(s) = shown else {
        r.check(
            false,
            &format!("the proof view shows ({})", shown.unwrap_err()),
        );
        return;
    };
    let got = report(r, s.id, 10).await.unwrap_or(Value::Null);
    r.line(format!("apps: the view saw {got}"));
    r.check(
        got["ping"] == "ok" && got["call"] == "fx__read" && got.get("timeout").is_none(),
        "a view's postMessage reaches Kai's isolated world, its source is the placed frame, and \
         Kai's answers reach the view (ping, tools/call)",
    );
    r.check(
        got["sourceIsParent"] == true,
        "the answers come from the host page (the view's parent)",
    );
    r.check(
        got["big"]
            == "-32600 the message is larger than overlay.app_max_mb (4096 bytes for the proof)",
        &format!(
            "a message over overlay.app_max_mb is answered -32600 by Kai's app script, naming it \
             ({})",
            got["big"]
        ),
    );
    let before = rpcs(r, s.id);
    r.check(
        before == 2,
        &format!("two messages relayed, the one over the bound not ({before})"),
    );
    // The host page's own window posts to itself: not the view's frame, not relayed.
    let posted = r
        .eval_main(
            s.id,
            r#"window.postMessage({jsonrpc: "2.0", id: 9, method: "ping"}, "*"); "posted""#,
        )
        .await;
    sleep(800).await;
    let after = rpcs(r, s.id);
    r.check(
        posted.is_ok() && after == before,
        &format!(
            "a message from the host page's main world is not relayed ({posted:?}; {after} \
             relayed)"
        ),
    );
    // An overlay_html document has no bridge: its message to its parent goes nowhere.
    let html = r
        .show(
            Kind::Html,
            Body::Document(HTML_POSTS.into()),
            look(Anchor::TopRight, Some(OUTPUTS[0]), 300, 200),
        )
        .await;
    if let Ok(h) = html {
        let _ = r
            .until(5, |p| {
                matches!(p, Probe::Refused { id, uri, .. } if *id == h.id && uri.ends_with("report?html"))
            })
            .await;
        let n = rpcs(r, h.id);
        r.check(
            n == 0,
            &format!("an overlay_html document's message reaches no bridge ({n})"),
        );
    } else {
        r.check(false, "the overlay_html widget shows");
    }
    r.clear("after the proof").await;
    let answered = torn_down(r, s.id).await;
    r.check(
        answered,
        "hidden, the view got Kai's ui/resource-teardown first (its answer seen)",
    );
}

/// The dev lmgw's URL and the device key, from the file the script wrote (two lines).
fn lmgw() -> Option<Credentials> {
    let path = std::env::var_os("KAI_LIVE_APPS")?;
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let url = lines.next()?.trim().to_owned();
    let key = lines.next()?.trim().to_owned();
    Some(Credentials {
        url,
        key: DeviceKey::from_store(key),
    })
}

async fn fixture(r: &mut Run, rt: &tokio::runtime::Runtime) {
    r.line("== 5e. overlay_app against a dev lmgw with the MCP Apps fixture".into());
    let Some(creds) = lmgw() else {
        r.check(
            false,
            "KAI_LIVE_APPS names the dev lmgw (scripts/live-check-widgets.sh)",
        );
        return;
    };
    let owner = Arc::new(No::default());
    let apps = Apps {
        creds: watch::channel(Some(creds)).1,
        client: reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
        connect: Duration::from_secs(10),
        bound: bound(),
        calls: calls(),
        confirm: owner.clone(),
        policy: Default::default(),
        lists: Default::default(),
        screen: Default::default(),
    };
    // A URI outside the server's namespace never reaches lmgw.
    let wrong = rt.block_on(apps.open("fx", "ui://fixture/card", (480, 360)));
    r.check(
        wrong.as_ref().is_err_and(|e| e.contains("namespace")),
        &format!(
            "a URI outside the server's namespace is refused ({:?})",
            wrong.err()
        ),
    );
    let (tx, rx) = oneshot::channel();
    let a = apps.clone();
    rt.spawn(async move {
        let _ = tx.send(a.open("fx", "ui://fx__fixture/card", (480, 360)).await);
    });
    let opened = glib_wait(rx, 30).await;
    let body = match opened {
        Some(Ok(b)) => b,
        other => {
            r.check(
                false,
                &format!("the fixture's view is read through /mcp ({other:?})"),
            );
            return;
        }
    };
    r.check(
        true,
        "the fixture's view is read through lmgw's /mcp under the device key",
    );
    let notes = body.notes.clone();
    r.check(
        notes
            .iter()
            .any(|n| n.contains("connectDomains https://api.example.com"))
            && notes.iter().any(|n| n.contains("permissions camera")),
        &format!("its requested origin and permission are refused and named ({notes:?})"),
    );
    let mut l = look(Anchor::Center, Some(OUTPUTS[0]), 480, 360);
    l.interactive = true;
    let shown = r.show(Kind::App, Body::App(body), l).await;
    let Ok(s) = shown else {
        r.check(false, &format!("the view shows ({})", shown.unwrap_err()));
        return;
    };
    r.check(
        s.notes.iter().any(|n| n.contains("connectDomains")),
        &format!("the show's answer names the refusals ({:?})", s.notes),
    );
    let got = report(r, s.id, 20).await.unwrap_or(Value::Null);
    r.line(format!("apps: the fixture's view saw {got}"));
    let text = |k: &str| got[k].as_str().unwrap_or_default().to_owned();
    for (ok, what) in [
        (
            text("init") == "2026-01-26" && got["serverTools"] == true,
            "ui/initialize answered with revision 2026-01-26 and server tools",
        ),
        (
            text("data").starts_with("fixture-"),
            "the view's data came through the bridge (card_data, app-only, read-only)",
        ),
        (
            text("bump") == "-32000 the owner denied fx__card_bump",
            "a state-changing call asked the owner, who said no",
        ),
        (
            text("modelOnly").starts_with("-32602"),
            "a model-only tool is refused to the view",
        ),
        (
            text("otherServer").starts_with("-32602"),
            "another server's tool is refused to the view",
        ),
        (
            text("read") == "extra-text" && text("readUri") == "ui://fixture/extra",
            "resources/read of its own server's URI, answered in its own terms",
        ),
        (
            text("readUrn").starts_with("-32602"),
            "a URI with no authority is refused",
        ),
        (
            text("list") == "ui://fixture/card,ui://fixture/extra,ui://fixture/live",
            "resources/list: its own server's resources in its own terms",
        ),
        (
            text("openLink").starts_with("-32601"),
            "ui/open-link is not offered",
        ),
        (text("ping") == "ok", "ping"),
    ] {
        r.check(ok, &format!("apps: {what}"));
    }
    let asked = owner.0.lock().unwrap().clone();
    r.check(
        asked == ["fx__card_bump by the view of app 'fx'"],
        &format!("the owner was asked once, for card_bump ({asked:?})"),
    );
    let lines = r.list().await;
    r.line(format!("overlay_list: {lines:?}"));
    r.check(
        lines
            .iter()
            .any(|l| l.contains(" app ") && l.contains("connectDomains")),
        "overlay_list names the view's refusals",
    );
    sleep(500).await;
    crops(r, "app").await;
    r.clear("after the fixture").await;
    let answered = torn_down(r, s.id).await;
    r.check(
        answered,
        "hidden, the fixture's view got Kai's ui/resource-teardown first",
    );
}

fn ready(index: u64, city: &str) -> Value {
    json!({"event": "ready", "index": index, "name": "fx__card_live", "arguments": {"city": city}})
}

/// lmgw's ready frame since 2026-10-09 (client-apps §7.6): the call's id, its label, whether it
/// waits for an approval and its view.
fn ready_now(index: u64, city: &str, call_id: &str) -> Value {
    json!({"event": "ready", "index": index, "name": "fx__card_live", "arguments": {"city": city},
           "call_id": call_id, "server_label": "fx", "needs_approval": false,
           "ui_resource": "ui://fx__fixture/live"})
}

fn live_result(index: u64, city: &str) -> Value {
    json!({"event": "result", "index": index, "name": "fx__card_live", "output": format!("{city}: 21"),
           "is_error": false, "ms": 5, "server_label": "fx", "ui_resource": "ui://fx__fixture/live",
           "structured_content": {"city": city, "temp": 21}})
}

/// lmgw's frames for a gated call: its `approval` frame, then `done` naming it as pending.
fn gated(city: &str, id: &str) -> (Value, Value) {
    let request = json!({"approval_request_id": id, "server_label": "fx", "name": "card_live",
                         "arguments": json!({"city": city}).to_string()});
    let mut frame = request.clone();
    frame["event"] = json!("approval");
    (frame, json!({"pending_approvals": [request]}))
}

/// The widget the next `Created` probe names.
async fn created(r: &mut Run, secs: u64) -> Option<u64> {
    match r.until(secs, |p| matches!(p, Probe::Created(_))).await {
        Some(Probe::Created(id)) => Some(id),
        _ => None,
    }
}

/// The first report of widget `id` (the live view reports what it saw at each step) that `want`
/// holds of, within `secs`.
async fn report_where(
    r: &mut Run,
    id: u64,
    secs: u64,
    want: impl Fn(&Value) -> bool,
) -> Option<Value> {
    let p = r
        .until(secs, |p| {
            matches!(p, Probe::Refused { id: i, uri, .. }
                if *i == id && uri.starts_with("kai-sandbox://widget/report?")
                    && want(&report_json(uri)))
        })
        .await?;
    let Probe::Refused { uri, .. } = p else {
        return None;
    };
    Some(report_json(&uri))
}

fn saw(v: &Value, step: &str) -> bool {
    v["order"]
        .as_array()
        .is_some_and(|o| o.iter().any(|s| s == step))
}

/// The views' K15 dialogs, as Kai's GTK thread shows them (`crate::ui::build_tool_dialog`): the
/// tools asked for, the windows open, and whether a new one is closed (denied) at once.
#[derive(Default)]
struct Dialogs {
    asked: Vec<String>,
    open: Vec<gtk::Window>,
    deny_at_once: bool,
}

async fn thread_call(r: &mut Run, rt: &tokio::runtime::Runtime) {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;

    use crate::widgets::thread_apps;
    r.line("== 5f. a call in Kai's conversation shows its view, with its input and result".into());
    let Some(creds) = lmgw() else {
        r.check(false, "KAI_LIVE_APPS names the dev lmgw");
        return;
    };
    let mut trust = crate::host::settings::PolicyConfig::default();
    trust.trusted_callers.push("app:fx".into());
    // The UI thread's way in, as Kai's: `UiCmd::Widgets`, and K15's dialog (`UiCmd::ConfirmTool`)
    // built as `crate::ui` builds it, so it holds the app views as a real one does.
    let (ui, from_ui) = async_channel::unbounded::<crate::ui::UiCmd>();
    let apps = Apps {
        creds: watch::channel(Some(creds)).1,
        client: reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
        connect: Duration::from_secs(10),
        bound: bound(),
        calls: calls(),
        confirm: Arc::new(crate::host::confirm::Dialog(ui.clone())),
        policy: Arc::new(std::sync::RwLock::new(crate::host::policy::Policy::new(
            &trust,
        ))),
        lists: Default::default(),
        screen: Default::default(),
    };
    let overlay = crate::config::OverlayConfig {
        output: Some(OUTPUTS[0].into()),
        ..Default::default()
    };
    let dialogs: Rc<RefCell<Dialogs>> = Rc::default();
    let d = dialogs.clone();
    gtk::glib::MainContext::default().spawn_local(async move {
        while let Ok(cmd) = from_ui.recv().await {
            match cmd {
                crate::ui::UiCmd::Widgets(c) => crate::widgets::remote::send(c),
                crate::ui::UiCmd::ConfirmTool(req) => {
                    let asked = format!("{} by {}", req.tool, req.caller);
                    let window = crate::ui::build_tool_dialog(req);
                    window.show_all();
                    let mut d = d.borrow_mut();
                    d.asked.push(asked);
                    if d.deny_at_once {
                        let w = window.clone();
                        gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
                            w.close()
                        });
                    } else {
                        d.open.push(window);
                    }
                }
                _ => {}
            }
        }
    });
    thread_apps::set(thread_apps::Ctx {
        apps: Arc::new(apps),
        ui,
        overlay: Arc::new(overlay),
    });
    let frame = |response: &str, event: &str, data: Value| {
        let _in = rt.enter();
        thread_apps::frame(response, event, &data);
    };
    // A tool with no view shows nothing.
    frame(
        "resp_0",
        "tool",
        json!({"event": "ready", "index": 0, "name": "fx__model_only", "arguments": {}}),
    );
    let none = created(r, 3).await;
    r.check(
        none.is_none(),
        &format!("a call of a tool with no view shows nothing ({none:?})"),
    );
    frame("resp_0", "done", json!({}));
    // An lmgw since 2026-10-09 names no view for it at its ready: nothing shown, no list asked.
    frame(
        "resp_0b",
        "tool",
        json!({"event": "ready", "index": 0, "name": "fx__model_only", "arguments": {},
               "call_id": "call_m", "server_label": "fx", "needs_approval": false,
               "ui_resource": null}),
    );
    let none = created(r, 3).await;
    r.check(
        none.is_none(),
        &format!("a ready frame that names no view shows nothing ({none:?})"),
    );
    frame("resp_0b", "done", json!({}));
    // The first call, as an lmgw before 2026-10-09 frames it: shown while it runs (the tool list
    // names its view); its input comes with its result (the ready frame does not say whether the
    // call waits for an approval).
    frame("resp_1", "tool", ready(0, "Berlin"));
    let Some(first) = created(r, 20).await else {
        r.check(false, "the call's view is shown at its ready frame");
        return;
    };
    r.check(
        true,
        "the call's view is shown at its ready frame, while the tool runs",
    );
    let mapped = r
        .until(
            10,
            |p| matches!(p, Probe::Mapped { id, .. } if *id == first),
        )
        .await;
    let first_size = match &mapped {
        Some(Probe::Mapped { width, height, .. }) => Some((*width as i32, *height as i32)),
        _ => None,
    };
    r.check(mapped.is_some(), "the call's view is mapped");
    // Its own destructive call (card_reset) asks the owner even though app:fx is trusted: while
    // the dialog is open the view is click-through and only Kai's strip, away from the dialog.
    let guarded = r
        .until(
            20,
            |p| matches!(p, Probe::Guarded { id, on: true } if *id == first),
        )
        .await
        .is_some();
    r.check(
        guarded,
        "a K15 dialog open: the app view is click-through and held (its input region in the \
         WAYLAND_DEBUG run)",
    );
    let asked = dialogs.borrow().asked.clone();
    r.check(
        asked == ["fx__card_reset by the view of app 'fx'"],
        &format!(
            "a trusted app's destructive call asks the owner, its destructiveHint: false one does \
             not ({asked:?})"
        ),
    );
    let (clear, ws) = super::nothing_over_the_dialog(&[]).await;
    r.check(
        clear,
        &format!(
            "while the dialog is open the view is its strip alone and no part of its surface \
             covers the dialog ({ws:?})"
        ),
    );
    let held = ws
        .iter()
        .filter(|w| !w.caption.starts_with(super::DIALOG_CAPTION))
        .any(|w| {
            Some(w.w) == first_size.map(|s| s.0) && w.h == crate::widgets::view::STRIP_PX as i32
        });
    r.check(
        held,
        &format!(
            "the held view is {} px tall, its width kept ({first_size:?})",
            crate::widgets::view::STRIP_PX
        ),
    );
    sleep(300).await;
    crops(r, "app-call-dialog").await;
    let open: Vec<gtk::Window> = std::mem::take(&mut dialogs.borrow_mut().open);
    for w in open {
        w.close();
    }
    dialogs.borrow_mut().deny_at_once = true;
    let back = r
        .until(
            10,
            |p| matches!(p, Probe::Guarded { id, on: false } if *id == first),
        )
        .await
        .is_some();
    r.check(back, "the dialog closed: the view takes clicks again");
    let restored = {
        let end = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let ws = super::kwin_windows().await.unwrap_or_default();
            let ok = first_size.is_some_and(|(w, h)| {
                ws.iter()
                    .any(|k| !k.caption.starts_with(super::DIALOG_CAPTION) && (k.w, k.h) == (w, h))
            });
            if ok || std::time::Instant::now() > end {
                break (ok, ws);
            }
            sleep(150).await;
        }
    };
    r.check(
        restored.0,
        &format!(
            "the dialog closed: the view is back at its size {first_size:?} ({:?})",
            restored.1
        ),
    );
    let before = report_where(r, first, 20, |v| v.get("reset").is_some()).await;
    r.line(format!(
        "apps: before its result, the call's view saw {before:?}"
    ));
    r.check(
        before.as_ref().is_some_and(|v| {
            v["order"] == json!([])
                && v["bump"] == "ran"
                && v["reset"] == "-32000 the owner denied fx__card_reset"
        }),
        "no input before Kai knows the call runs; card_bump (destructiveHint false) ran under \
         app:fx, card_reset was denied by the owner",
    );
    frame("resp_1", "tool", live_result(0, "Berlin"));
    frame("resp_1", "done", json!({}));
    let got = report_where(r, first, 20, |v| saw(v, "result"))
        .await
        .unwrap_or(Value::Null);
    r.line(format!("apps: the call's view saw {got}"));
    for (ok, what) in [
        (
            got["order"] == json!(["input", "result"]),
            "tool-input, then tool-result",
        ),
        (
            got["input"] == json!({"city": "Berlin"}),
            "the call's arguments as its input",
        ),
        (
            got["result"] == json!({"city": "Berlin", "temp": 21}) && got["text"] == "Berlin: 21",
            "the result's structuredContent and text",
        ),
        (
            got["toolName"] == "card_live" && got["toolUri"] == "ui://fixture/live",
            "hostContext.toolInfo names the tool in its server's own terms",
        ),
    ] {
        r.check(ok, &format!("apps: {what}"));
    }
    let lines = r.list().await;
    r.check(
        lines
            .iter()
            .any(|l| l.contains("shown for fx__card_live in Kai's conversation")),
        &format!("overlay_list says which call the view is for ({lines:?})"),
    );
    sleep(300).await;
    crops(r, "app-call").await;
    // The second call, as lmgw frames it now: its ready names the view and says it runs, so the
    // view is read at once and gets its input at once. It replaces the first view; its turn is
    // stopped.
    frame("resp_2", "tool", ready_now(0, "Hamburg", "call_hh"));
    let second = created(r, 20).await;
    r.check(second.is_some(), "a second call shows its view");
    let replaced = r
        .until(10, |p| matches!(p, Probe::Destroyed(id) if *id == first))
        .await
        .is_some();
    r.check(replaced, "the second call's view replaced the first");
    let answered = torn_down(r, first).await;
    r.check(answered, "the replaced view got ui/resource-teardown first");
    if let Some(id) = second {
        let input = report_where(r, id, 20, |v| saw(v, "input")).await;
        r.check(
            input.is_some(),
            "needs_approval: false: the input goes at once",
        );
        frame("resp_2", "stop", json!({}));
        frame("resp_2", "done", json!({"aborted": true}));
        let got = report_where(r, id, 20, |v| saw(v, "cancelled"))
            .await
            .unwrap_or(Value::Null);
        r.line(format!("apps: the stopped call's view saw {got}"));
        r.check(
            got["order"] == json!(["input", "cancelled"])
                && got["cancelled"] == "the turn was stopped",
            "a stopped turn's call: tool-input, then tool-cancelled with why",
        );
    }
    // A gated call (lmgw: `approval` frame, `done` with pending_approvals): its view gets no
    // input while it waits; declined by another client, it gets tool-cancelled alone.
    frame("resp_3", "tool", ready(0, "Berlin"));
    let (approval, done) = gated("Berlin", "mcpr_berlin");
    frame("resp_3", "tool", approval);
    frame("resp_3", "done", done);
    let third = created(r, 20).await;
    if let Some(id) = third {
        let early = report_where(r, id, 20, |v| v.get("reset").is_some()).await;
        let input = report_where(r, id, 2, |v| saw(v, "input")).await;
        r.check(
            early.is_some() && input.is_none(),
            &format!(
                "a gated call's view gets no input while the call waits ({early:?}, {input:?})"
            ),
        );
        {
            let _in = rt.enter();
            thread_apps::approval_decided("mcpr_berlin", None, false, Some("the dashboard"));
        }
        let got = report_where(r, id, 20, |v| saw(v, "cancelled"))
            .await
            .unwrap_or(Value::Null);
        r.line(format!("apps: the declined call's view saw {got}"));
        r.check(
            got["order"] == json!(["cancelled"])
                && got["cancelled"] == "the call was declined by the dashboard"
                && got.get("input").is_none(),
            "a declined call's view: tool-cancelled saying who declined, and no input",
        );
    } else {
        r.check(false, "the gated call's view shows (without its input)");
    }
    // Approved and resumed in Kai's own session (a later Kai): the resumed turn's frames find
    // the held call, no second view, and the input comes with the result.
    frame("resp_4", "tool", ready(0, "Ulm"));
    let (approval, done) = gated("Ulm", "mcpr_ulm");
    frame("resp_4", "tool", approval);
    frame("resp_4", "done", done);
    let fourth = created(r, 20).await;
    if let Some(id) = fourth {
        let _ = report_where(r, id, 20, |v| v.get("reset").is_some()).await;
        let mut again = ready(0, "Ulm");
        again["needs_approval"] = json!(false);
        frame("resp_5", "tool", again);
        let extra = created(r, 3).await;
        r.check(
            extra.is_none(),
            &format!("the resumed turn's ready opens no second view ({extra:?})"),
        );
        frame("resp_5", "tool", live_result(0, "Ulm"));
        frame("resp_5", "done", json!({}));
        let got = report_where(r, id, 20, |v| saw(v, "result"))
            .await
            .unwrap_or(Value::Null);
        r.check(
            got["order"] == json!(["input", "result"]) && got["input"] == json!({"city": "Ulm"}),
            &format!("the resumed call's view: its input with its result ({got})"),
        );
    } else {
        r.check(false, "the second gated call's view shows");
    }
    // A ready that says it waits (needs_approval: true) shows nothing yet.
    let mut waits = ready_now(0, "Bonn", "call_bonn");
    waits["needs_approval"] = json!(true);
    frame("resp_6", "tool", waits);
    let none = created(r, 3).await;
    r.check(
        none.is_none(),
        &format!("needs_approval: true shows nothing at the ready frame ({none:?})"),
    );
    frame("resp_6", "done", json!({"aborted": true}));
    // The frames as lmgw sends them now, whole: a Gemini call id (hundreds of characters, kept
    // whole), the result's MCP content blocks as the server sent them (an image among them), its
    // structuredContent and isError.
    let long_id = format!("call_{}", "Q2lTaWduYXR1cmU".repeat(40));
    frame("resp_7", "tool", ready_now(0, "Lübeck", &long_id));
    let seventh = created(r, 20).await;
    if let Some(id) = seventh {
        let input = report_where(r, id, 20, |v| saw(v, "input")).await;
        r.check(
            input
                .as_ref()
                .is_some_and(|v| v["input"] == json!({"city": "Lübeck"})),
            &format!(
                "the frame-named view gets the frame's arguments at once ({:?})",
                input.as_ref().map(|v| &v["input"])
            ),
        );
        let mut done = live_result(0, "Lübeck");
        done["call_id"] = json!(long_id);
        done["content"] = json!([
            {"type": "text", "text": "Lübeck: 21"},
            {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}
        ]);
        done["output"] = json!("Lübeck: 21 [image]");
        frame("resp_7", "tool", done);
        frame("resp_7", "done", json!({}));
        let got = report_where(r, id, 20, |v| saw(v, "result"))
            .await
            .unwrap_or(Value::Null);
        r.line(format!(
            "apps: the frame-named call's view saw types {}, isError {}, toolId of {} chars",
            got["types"],
            got["isError"],
            got["toolId"].as_str().map_or(0, str::len)
        ));
        for (ok, what) in [
            (
                got["order"] == json!(["input", "result"]),
                "tool-input, then tool-result",
            ),
            (
                got["types"] == json!(["text", "image"]) && got["text"] == "Lübeck: 21",
                "tool-result carries the MCP content blocks as sent, not the flattened text",
            ),
            (
                got["result"] == json!({"city": "Lübeck", "temp": 21}) && got["isError"] == false,
                "its structuredContent and isError",
            ),
            (
                got["toolId"] == json!(long_id),
                "hostContext.toolInfo.id is the call's id, whole",
            ),
        ] {
            r.check(ok, &format!("apps: {what}"));
        }
    } else {
        r.check(false, "a ready frame naming its view shows it");
    }
    {
        let _in = rt.enter();
        thread_apps::session_ended();
    }
    let open: Vec<gtk::Window> = std::mem::take(&mut dialogs.borrow_mut().open);
    for w in open {
        w.close();
    }
    r.clear("after the calls").await;
}

/// A tokio answer awaited on the GTK loop, at most `secs`.
async fn glib_wait<T>(rx: oneshot::Receiver<T>, secs: u64) -> Option<T> {
    gtk::glib::future_with_timeout(Duration::from_secs(secs), rx)
        .await
        .ok()
        .and_then(Result::ok)
}

pub async fn run(r: &mut Run) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    proof(r, &rt).await;
    fixture(r, &rt).await;
    thread_call(r, &rt).await;
    rt.shutdown_timeout(Duration::from_secs(2));
}

/// An upstream that counts the `tools/call`s that reach it.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl Upstream for Counting {
    fn request(&self, method: &'static str, params: Value) -> Pending<Result<Value, McpError>> {
        if method == "tools/call" {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Scripted.request(method, params)
    }

    fn close(&self) -> Pending<()> {
        Box::pin(async {})
    }
}

/// A view that calls a tool every 200 ms and reports each answer.
const POLLER: &str = r#"<!doctype html><meta charset="utf-8">
<body style="margin:0;background:#232629;color:#eff0f1;font:20px sans-serif">poller
<script>
let n = 0;
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || typeof m !== "object" || m.id === undefined || m.id === "kai-teardown") return;
  const r = m.error ? { err: m.error.message } : { called: m.result.structuredContent.called };
  r.n = m.id;
  location.href = "kai-sandbox://widget/report?" + encodeURIComponent(JSON.stringify(r));
});
setInterval(() => {
  parent.postMessage({ jsonrpc: "2.0", id: ++n, method: "tools/call",
                       params: { name: "read", arguments: {} } }, "*");
}, 200);
</script>"#;

/// K32's gap: a view's own script runs on while its widget is unmapped, and its `tools/call`
/// must not reach lmgw while the screen is locked; it works again after the unlock.
pub async fn lock_view(r: &mut Run, screen: &crate::screen_lock::ScreenLock) {
    use std::sync::atomic::Ordering;

    use crate::widgets::gtk_host::WidgetsCmd;
    r.line("== the screen lock: a view's tools/call".into());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let reached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    screen.set(false);
    let ends = {
        let _in = rt.enter();
        Bridge::new(
            Arc::new(Counting(reached.clone())),
            Rules::of_list(
                "fx",
                &json!({"tools": [{"name": "fx__read", "annotations": {"readOnlyHint": true}}]}),
            ),
            Arc::new(No::default()),
            bound(),
            calls(),
            (320, 120),
        )
        .with_screen(screen.clone())
        .spawn()
    };
    let body = Body::App(AppBody {
        server: "fx".into(),
        html: POLLER.into(),
        ends,
        notes: vec![],
        bound: bound(),
    });
    let mut l = look(Anchor::Center, Some(OUTPUTS[0]), 320, 120);
    l.interactive = true;
    let Ok(s) = r.show(Kind::App, body, l).await else {
        r.check(false, "the polling view shows");
        return;
    };
    let works = |v: &Value| v["called"] == "fx__read";
    let first = report_where(r, s.id, 10, works).await;
    r.check(
        first.is_some(),
        "unlocked: the view's tools/call is answered",
    );
    screen.set(true);
    r.send(WidgetsCmd::Locked(true));
    sleep(600).await;
    let at_lock = reached.load(Ordering::SeqCst);
    let refused = report_where(r, s.id, 10, |v| v["err"] == crate::screen_lock::REFUSAL).await;
    r.check(
        refused.is_some(),
        &format!("locked: the view's tools/call is refused with the lock's words ({refused:?})"),
    );
    sleep(800).await;
    r.check(
        reached.load(Ordering::SeqCst) == at_lock,
        "locked: no tools/call reached the upstream",
    );
    screen.set(false);
    r.send(WidgetsCmd::Locked(false));
    // Drop what was buffered while locked, then wait for a fresh answer.
    r.seen.clear();
    let n = reached.load(Ordering::SeqCst);
    let again = report_where(r, s.id, 10, works).await;
    r.check(
        again.is_some() && reached.load(Ordering::SeqCst) > n,
        "unlocked: the view's tools/call reaches the upstream and is answered again",
    );
    r.clear("after the lock view").await;
    rt.shutdown_timeout(Duration::from_secs(2));
}
