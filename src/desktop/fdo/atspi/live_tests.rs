//! Kai for Firefox's WP12 against a real Firefox: a headless Firefox 159 on a PRIVATE D-Bus
//! session with its own AT-SPI bus (`scripts/atspi-firefox.sh`, `just atspi-firefox`: a private
//! runtime folder, no display, no D-Bus activation; the session's accessibility bus is never
//! read or changed), its profile and home under `target/atspi-live/`. Ignored by default; it
//! refuses to run anywhere but under that script.
//!
//! What it checks, through Kai's own AT-SPI code (`super`) and Firefox's own UI's reading
//! (`crate::browser::tools::chrome::parts`):
//! - the page in front is read and the background tab's page is not, though Firefox marks both
//!   visible; a cross-site frame's address and a `data:` frame's empty one;
//! - a link whose first `NActions` read may be 0 is found and pressed (the settle), and its
//!   `target=_blank` opens a tab with a trusted, user-activated click;
//! - the ref→node mapping: an `aria-kai-mark` the page sets is found as the object attribute
//!   `kai-mark` (one element, with its documents), also inside a cross-site frame, and not in the
//!   background tab; the press of the marked element is a trusted, user-activated click on it and
//!   on no other of the same name;
//! - a page's `confirm()` and `prompt()` as Firefox's dialog in the tab's panel, its title naming
//!   the page's origin, its text, its buttons; the prompt's field takes a text; OK answers;
//! - Firefox's own tool bars; a page tab pressed brings `about:preferences` to the front, which is
//!   then the page read, and the page before is no longer pressed in;
//! - the review round of WP12: a site's location prompt read as a prompt for the tab's site with
//!   its words ("Allow 127.0.0.1:<port> to access your location?", `chrome::parts`), a mark the
//!   page copied onto a second element said as two (`Marked::Several`, which Kai refuses as
//!   tampering), the tab's own window found by its selected tab (`chrome::window`) for an error
//!   page too, whose `DocURL` is the address that failed, and another extension's page
//!   (`moz-extension:`, an extension installed for the run through WebDriver BiDi as a temporary
//!   add-on) in front, which Kai never reads or presses in (`chrome::never`) and whose rule is the
//!   strictest (`SiteRules::page_rule`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use kai_desktop::caps::accessibility::{Accessibility, Element, How, Marked, Query, Target, Tree};
use tokio_tungstenite::tungstenite::Message;

use super::Atspi;
use crate::browser::tabs::Tab;
use crate::browser::tools::chrome::{never, parts, window};
use crate::host::registry::SessionBus;

const T: Duration = Duration::from_secs(10);

/// What the pages reported (`/beacon?ev=…&trusted=…&act=…`), in order.
type Beacons = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// A local HTTP server of the test's pages on 127.0.0.1, its port; every request's query is
/// recorded when its path is `/beacon`.
fn serve(pages: Arc<HashMap<&'static str, String>>, beacons: Beacons) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let pages = pages.clone();
            let beacons = beacons.clone();
            std::thread::spawn(move || {
                let mut line = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                if r.read_line(&mut line).is_err() {
                    return;
                }
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                }
                let target = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let u = url::Url::parse(&format!("http://x{target}")).unwrap();
                let body = if u.path() == "/beacon" {
                    beacons
                        .lock()
                        .unwrap()
                        .push(u.query_pairs().into_owned().collect());
                    String::new()
                } else {
                    pages.get(u.path()).cloned().unwrap_or_default()
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: \
                     no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

const BEACON: &str = "<script>function beacon(ev, e) { fetch('/beacon?' + new URLSearchParams({ev, \
                      trusted: e ? e.isTrusted : '', act: navigator.userActivation.isActive}), \
                      {keepalive: true}); }\naddEventListener('click', (e) => beacon('click-' + \
                      (e.target.id || e.target.localName), e), true);\nconst q = new \
                      URLSearchParams(location.search);\naddEventListener('load', () => { const m = \
                      q.get('mark'); if (m) document.getElementById(q.get('on')).setAttribute('aria-kai-mark', m); \
                      const d = q.get('dup'); if (d) for (const id of ['d1', 'd2']) \
                      document.getElementById(id).setAttribute('aria-kai-mark', d); });</script>";

fn pages(frame_port: u16) -> HashMap<&'static str, String> {
    HashMap::from([
        (
            "/top.html",
            format!(
                "<!doctype html><html lang=en><head><meta charset=utf-8><title>Kai live \
                 top</title>{BEACON}</head><body><h1>Kai live top</h1>\
                 <p><button id=a1>Add to cart</button> <button id=a2>Add to cart</button> \
                 <button id=a3>Add to cart</button></p>\
                 <p><a id=plain href=\"#plain\">Plain link</a> <a id=blank href=\"/opened.html\" \
                 target=\"_blank\">Price list</a></p>\
                 <p><button id=ask onclick=\"beacon('confirm-' + confirm('Empty the \
                 cart?'))\">Ask</button> <button id=name onclick=\"beacon('prompt-' + \
                 prompt('Your name?', 'Ada'))\">Name</button> <button id=loc \
                 onclick=\"navigator.geolocation.getCurrentPosition(() => {{}}, () => {{}})\">Locate</button></p>\
                 <p><button id=d1>Copy one</button> <button id=d2>Copy two</button></p>\
                 <iframe title=Payment src=\"http://localhost:{frame_port}/pay.html?mark=m-frame&on=pay\"></iframe>\
                 <iframe title=Inline src=\"data:text/html,%3Cbutton%3EInline%3C/button%3E\"></iframe>\
                 </body></html>"
            ),
        ),
        (
            "/pay.html",
            format!(
                "<!doctype html><html lang=en><head><meta charset=utf-8><title>Pay</title>{BEACON}</head>\
                 <body><button id=pay>Pay</button></body></html>"
            ),
        ),
        (
            "/back.html",
            format!(
                "<!doctype html><html lang=en><head><meta charset=utf-8><title>Kai live \
                 back</title>{BEACON}</head><body><p>Balance 1200</p><button \
                 id=transfer>Transfer</button></body></html>"
            ),
        ),
        (
            "/opened.html",
            "<!doctype html><title>Opened</title><p>opened".to_owned(),
        ),
    ])
}

/// The headless Firefox, its process group ended however the test ends, its folder removed.
struct Firefox {
    child: Child,
    dir: std::path::PathBuf,
    /// Its WebDriver BiDi port on 127.0.0.1 (the temporary add-on's install).
    bidi: u16,
}

/// The temporary add-on the run installs: its id, the UUID its pages get (Firefox's
/// `extensions.webextensions.uuids`), and the page it opens in a tab of its own.
const VAULT_ID: &str = "kai-live-vault@test.invalid";
const VAULT_UUID: &str = "0b1c2d3e-4f50-4172-8394-a5b6c7d8e9f0";

fn vault_url() -> String {
    format!("moz-extension://{VAULT_UUID}/vault.html")
}

/// The add-on's files in `dir`: a background script that opens its page in a tab.
fn vault(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let manifest = serde_json::json!({
        "manifest_version": 2,
        "name": "Kai live vault",
        "version": "1.0",
        "browser_specific_settings": {"gecko": {"id": VAULT_ID}},
        "background": {"scripts": ["bg.js"]},
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(
        dir.join("bg.js"),
        "browser.tabs.create({url: browser.runtime.getURL('vault.html')});\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("vault.html"),
        "<!doctype html><html lang=en><head><meta charset=utf-8><title>Vault</title></head>\
         <body><p>bank.example: hunter2</p><button>Unlock</button></body></html>",
    )
    .unwrap();
}

/// Installs the add-on at `dir` as a temporary one through WebDriver BiDi on `port`.
#[allow(
    clippy::disallowed_methods,
    reason = "the run's own headless Firefox on 127.0.0.1, not lmgw (device link D15)"
)]
async fn install(port: u16, dir: &std::path::Path) -> Result<(), String> {
    let url = format!("ws://127.0.0.1:{port}/session");
    let end = Instant::now() + Duration::from_secs(30);
    let mut ws = loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => break ws,
            Err(e) if Instant::now() > end => return Err(format!("BiDi: {e}")),
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    };
    let mut ask = async |id: u64,
                         method: &str,
                         params: serde_json::Value|
           -> Result<serde_json::Value, String> {
        let frame = serde_json::json!({"id": id, "method": method, "params": params});
        ws.send(Message::Text(frame.to_string().into()))
            .await
            .map_err(|e| e.to_string())?;
        while let Some(m) = ws.next().await {
            let Ok(Message::Text(t)) = m else { continue };
            let v: serde_json::Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
            if v["id"] == id {
                return if v["type"] == "success" {
                    Ok(v["result"].clone())
                } else {
                    Err(format!("{method}: {v}"))
                };
            }
        }
        Err(format!("{method}: the connection closed"))
    };
    ask(1, "session.new", serde_json::json!({"capabilities": {}})).await?;
    let r = ask(
        2,
        "webExtension.install",
        serde_json::json!({"extensionData": {"type": "path", "path": dir.to_string_lossy()}}),
    )
    .await?;
    if r["extension"] == VAULT_ID {
        Ok(())
    } else {
        Err(format!("webExtension.install answered {r}"))
    }
}

impl Drop for Firefox {
    fn drop(&mut self) {
        let pid = self.child.id() as libc::pid_t;
        // SAFETY: a signal to the process group of our own child, which has not been reaped.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above; whatever of its group is left.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn firefox(urls: &[String]) -> Firefox {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/atspi-live");
    let dir = base.join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sub = |n: &str| {
        let p = dir.join(n);
        std::fs::create_dir_all(&p).unwrap();
        p
    };
    let profile = sub("profile");
    let prefs: [(&str, &str); 10] = [
        ("accessibility.force_disabled", "-1"),
        ("browser.shell.checkDefaultBrowser", "false"),
        ("browser.aboutwelcome.enabled", "false"),
        ("browser.startup.homepage_override.mstone", "\"ignore\""),
        ("datareporting.policy.dataSubmissionEnabled", "false"),
        ("toolkit.telemetry.reportingpolicy.firstRun", "false"),
        ("browser.tabs.warnOnClose", "false"),
        ("app.update.disabledForTesting", "true"),
        ("browser.sessionstore.resume_from_crash", "false"),
        // The temporary add-on's pages get a known address (`vault_url`).
        (
            "extensions.webextensions.uuids",
            &format!("\"{{\\\"{VAULT_ID}\\\":\\\"{VAULT_UUID}\\\"}}\""),
        ),
    ];
    let js: String = prefs
        .iter()
        .map(|(k, v)| format!("user_pref(\"{k}\", {v});\n"))
        .collect();
    std::fs::write(profile.join("user.js"), js).unwrap();
    vault(&dir.join("vault"));
    // A free port for WebDriver BiDi (taken back from the system a moment before Firefox binds it).
    let bidi = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut c = Command::new("/usr/lib64/firefox/firefox");
    // The remote agent's flags after the pages: given before them, a headless Firefox 159 opened
    // the first page's tab without that page.
    c.args(["--headless", "--no-remote", "--profile"])
        .arg(&profile)
        .args(urls)
        .args(["--remote-debugging-port", &bidi.to_string()])
        .arg("--remote-allow-system-access")
        .env("HOME", sub("home"))
        .env("XDG_CONFIG_HOME", sub("config"))
        .env("XDG_CACHE_HOME", sub("cache"))
        .env("XDG_DATA_HOME", sub("data"))
        .env("XDG_STATE_HOME", sub("state"))
        .env("MOZ_HEADLESS", "1")
        .env("MOZ_REMOTE_ALLOW_SYSTEM_ACCESS", "1")
        .env("MOZ_CRASHREPORTER_DISABLE", "1")
        .env_remove("NO_AT_BRIDGE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    Firefox {
        child: c.spawn().expect("firefox"),
        dir,
        bidi,
    }
}

/// Every element below `e`.
fn all(e: &Element) -> Vec<&Element> {
    let mut out = vec![e];
    for c in &e.children {
        out.extend(all(c));
    }
    out
}

/// The verdicts, printed as they come; the test fails at the end if one did.
#[derive(Default)]
struct Verdicts(Vec<String>);

impl Verdicts {
    fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
        println!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.0.push(what.to_owned());
        }
    }
}

/// Waits until `f` says yes, at most `ms`.
async fn until(ms: u64, mut f: impl AsyncFnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn beacons_since(b: &Beacons, from: usize) -> Vec<HashMap<String, String>> {
    b.lock().unwrap()[from..].to_vec()
}

#[tokio::test]
#[ignore = "a headless Firefox on a private accessibility bus: scripts/atspi-firefox.sh"]
async fn firefox_on_a_private_bus() {
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    assert!(
        std::env::var("KAI_ATSPI_PRIVATE").as_deref() == Ok("1")
            && bus.starts_with("unix:path=/tmp/kai-atspi-"),
        "runs only under scripts/atspi-firefox.sh (a private bus), not on {bus:?}"
    );
    let beacons: Beacons = Arc::default();
    let frame_port = serve(Arc::new(pages(0)), beacons.clone());
    let port = serve(Arc::new(pages(frame_port)), beacons.clone());
    let top = format!("http://127.0.0.1:{port}/top.html?mark=m-top&on=a2&dup=m-dup");
    let back = format!("http://127.0.0.1:{port}/back.html?mark=m-back&on=transfer");
    let refused = "http://127.0.0.1:1/refused".to_owned();
    let ff = firefox(&[
        top.clone(),
        back.clone(),
        "about:preferences".into(),
        refused.clone(),
    ]);
    let pid = ff.child.id();
    let session = zbus::Connection::session().await.unwrap();
    let a = Atspi {
        bus: SessionBus::connected(session, T),
        timeout: T,
        concurrency: 16,
        action_settle: Duration::from_millis(100),
        switch: None,
    };
    let mut v = Verdicts::default();
    let details = How {
        details: true,
        into_documents: false,
    };
    // Firefox on the bus with its page in front.
    let mut tree = Tree::default();
    let on = until(30_000, async || {
        if let Ok(t) = a.app_tree(pid, details).await {
            let shown = all(&t.root)
                .iter()
                .any(|e| e.document.as_deref() == Some(top.as_str()));
            tree = t;
            return shown;
        }
        false
    })
    .await;
    v.check("Firefox's page on the private bus", on, &top);
    if !on {
        panic!("Firefox never showed its page on the bus");
    }
    // Give the pages' load handlers time to set their marks.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let tree = a.app_tree(pid, details).await.unwrap();
    let docs: Vec<String> = all(&tree.root)
        .iter()
        .filter_map(|e| e.document.clone())
        .collect();
    v.check(
        "only the page in front (and its dialogs) among the window's documents",
        docs.contains(&top)
            && !docs
                .iter()
                .any(|d| d.contains("back.html") || d == "about:preferences"),
        format!("{docs:?}"),
    );
    let window = all(&tree.root)
        .into_iter()
        .find(|e| e.role == "frame")
        .cloned()
        .unwrap();
    let target = Target {
        pid,
        title: window.name.clone(),
        exact: true,
    };
    let read = a.tree(&target).await.unwrap();
    let names: Vec<String> = all(&read.root)
        .iter()
        .flat_map(|e| [e.name.clone(), e.text.clone()])
        .filter(|s| !s.is_empty())
        .collect();
    v.check(
        "read_window's tree: the page in front, not the background tab's",
        names.iter().any(|n| n == "Add to cart")
            && !names
                .iter()
                .any(|n| n.contains("Balance") || n == "Transfer"),
        format!("{} names", names.len()),
    );
    let frame_docs: Vec<Option<String>> = all(&read.root)
        .iter()
        .filter(|e| e.document.is_some())
        .map(|e| e.document.clone())
        .collect();
    v.check(
        "a cross-site frame's DocURL and a data: frame's empty one",
        frame_docs.contains(&Some(format!(
            "http://localhost:{frame_port}/pay.html?mark=m-frame&on=pay"
        ))) && frame_docs.contains(&Some(String::new())),
        format!("{frame_docs:?}"),
    );
    // The mark: the element the page marked, found on the bus in the tab's page.
    let top_tab = Tab {
        id: 1,
        url: top.clone(),
        title: "Kai live top".into(),
        active: true,
        ..Default::default()
    };
    let page = window::of(&tree, &top_tab).map(|w| w.page.at.unwrap());
    v.check(
        "the top page's window by its address and its selected tab",
        page.is_ok(),
        format!("{page:?}"),
    );
    let page = page.unwrap_or_default();
    let from = beacons.lock().unwrap().len();
    match a.find_marked(&page, "kai-mark", "m-top").await {
        Ok(Marked::One(found)) => {
            v.check(
                "the marked element found by kai-mark",
                found.role == "button"
                    && found.name == "Add to cart"
                    && found.documents == [top.clone()],
                format!("{} {:?} in {:?}", found.role, found.name, found.documents),
            );
            let pressed = a.press(&found).await;
            let saw = until(5_000, async || {
                beacons_since(&beacons, from)
                    .iter()
                    .any(|b| b.get("ev").map(String::as_str) == Some("click-a2"))
            })
            .await;
            let got = beacons_since(&beacons, from);
            let click = got
                .iter()
                .find(|b| b.get("ev").map(String::as_str) == Some("click-a2"));
            v.check(
                "doAction on the marked element: a trusted, user-activated click on a2 alone",
                pressed.is_ok()
                    && saw
                    && click.is_some_and(|c| c["trusted"] == "true" && c["act"] == "true")
                    && !got.iter().any(|b| {
                        matches!(
                            b.get("ev").map(String::as_str),
                            Some("click-a1" | "click-a3")
                        )
                    }),
                format!("{pressed:?} {got:?}"),
            );
        }
        other => v.check(
            "the marked element found by kai-mark",
            false,
            format!("{other:?}"),
        ),
    }
    match a.find_marked(&page, "kai-mark", "m-frame").await {
        Ok(Marked::One(found)) => v.check(
            "a mark inside a cross-site frame",
            found.name == "Pay" && found.documents.len() == 2,
            format!("{:?}", found.documents),
        ),
        other => v.check(
            "a mark inside a cross-site frame",
            false,
            format!("{other:?}"),
        ),
    }
    let back_mark = a.find_marked(&page, "kai-mark", "m-back").await;
    v.check(
        "a mark in the background tab is not found in the page in front",
        matches!(back_mark, Ok(Marked::Nothing)),
        format!("{back_mark:?}"),
    );
    // The page copied the mark onto a second element: said as two, which Kai refuses as
    // tampering (`crate::browser::tools::trusted`).
    let dup = a.find_marked(&page, "kai-mark", "m-dup").await;
    v.check(
        "a duplicated mark is said as two elements",
        matches!(dup, Ok(Marked::Several(2))),
        format!("{dup:?}"),
    );
    // A link (a first NActions read can be 0), target=_blank: a new tab.
    let from = beacons.lock().unwrap().len();
    let link = a
        .find(&target, &Query::parse("link \"Price list\"").unwrap())
        .await;
    let pressed = match &link {
        Ok(l) => a.press(l).await.map(|p| p.action),
        Err(e) => Err(kai_desktop::caps::Failure::Failed(e.to_string())),
    };
    let opened = until(5_000, async || {
        beacons_since(&beacons, from)
            .iter()
            .any(|b| b.get("ev").map(String::as_str) == Some("click-blank"))
    })
    .await;
    let tabs = a
        .app_tree(pid, details)
        .await
        .map(|t| all(&t.root).iter().filter(|e| e.role == "page tab").count())
        .unwrap_or_default();
    v.check(
        "the target=_blank link pressed: trusted, a new tab",
        pressed.is_ok() && opened && tabs >= 5,
        format!("{pressed:?}, {tabs} tabs"),
    );
    // The tab Firefox opened came to the front: back to the top page by its page tab.
    let select = async |title: &str| -> bool {
        let Ok(t) = a.app_tree(pid, details).await else {
            return false;
        };
        let Some(at) = all(&t.root)
            .iter()
            .find(|e| e.role == "page tab" && e.name.starts_with(title))
            .and_then(|e| e.at.clone())
        else {
            return false;
        };
        match a.found_at(&at).await {
            Ok(f) => a.press(&f).await.is_ok(),
            Err(_) => false,
        }
    };
    v.check(
        "the top page's tab pressed",
        select("Kai live top").await,
        "",
    );
    let front = until(5_000, async || {
        a.app_tree(pid, details).await.is_ok_and(|t| {
            all(&t.root)
                .iter()
                .any(|e| e.document.as_deref() == Some(top.as_str()))
        })
    })
    .await;
    v.check("the top page in front again", front, "");
    // A page's confirm(): Firefox's dialog in the tab's panel.
    let from = beacons.lock().unwrap().len();
    let ask = a
        .find(&target, &Query::parse("button \"Ask\"").unwrap())
        .await;
    if let Ok(f) = &ask {
        let _ = a.press(f).await;
    }
    let mut dialog = None;
    until(5_000, async || {
        if let Ok(t) = a.app_tree(pid, details).await
            && let Some(frame) = all(&t.root).into_iter().find(|e| e.role == "frame")
            && let Some(page) = all(frame)
                .into_iter()
                .find(|e| e.document.as_deref() == Some(top.as_str()))
            && let Some(at) = page.at.clone()
        {
            dialog = parts::parts(frame, &at, None).dialogs.into_iter().next();
        }
        dialog.is_some()
    })
    .await;
    match dialog.and_then(|d| d.at) {
        Some(at) => {
            let sub = a.subtree(&at, details).await.unwrap();
            let title = sub.root.name.clone();
            let texts: Vec<String> = all(&sub.root)
                .iter()
                .filter(|e| e.role == "label")
                .map(|e| {
                    if e.text.is_empty() {
                        e.name.clone()
                    } else {
                        e.text.clone()
                    }
                })
                .collect();
            v.check(
                "confirm() as Firefox's dialog, its title naming the page's origin",
                parts::named_origin(&title).as_deref()
                    == Some(&format!("http://127.0.0.1:{port}")[..])
                    && texts.iter().any(|t| t == "Empty the cart?"),
                format!("{title:?} {texts:?}"),
            );
            let ok = all(&sub.root)
                .into_iter()
                .find(|e| e.role == "button" && e.name == "OK")
                .and_then(|e| e.at.clone());
            let done = match ok {
                Some(at) => match a.found_at(&at).await {
                    Ok(f) => a.press(&f).await.is_ok(),
                    Err(_) => false,
                },
                None => false,
            };
            let answered = until(5_000, async || {
                beacons_since(&beacons, from)
                    .iter()
                    .any(|b| b.get("ev").map(String::as_str) == Some("confirm-true"))
            })
            .await;
            v.check(
                "the dialog's OK pressed: confirm() returned true",
                done && answered,
                "",
            );
        }
        None => v.check("confirm() as Firefox's dialog", false, format!("{ask:?}")),
    }
    // A page's prompt(): its field takes a text.
    let from = beacons.lock().unwrap().len();
    if let Ok(f) = a
        .find(&target, &Query::parse("button \"Name\"").unwrap())
        .await
    {
        let _ = a.press(&f).await;
    }
    let mut entry = None;
    let mut ok = None;
    until(5_000, async || {
        if let Ok(t) = a.app_tree(pid, details).await
            && let Some(frame) = all(&t.root).into_iter().find(|e| e.role == "frame")
            && let Some(page) = all(frame)
                .into_iter()
                .find(|e| e.document.as_deref() == Some(top.as_str()))
            && let Some(at) = page.at.clone()
            && let Some(d) = parts::parts(frame, &at, None).dialogs.into_iter().next()
            && let Some(dat) = d.at
            && let Ok(sub) = a.subtree(&dat, details).await
        {
            entry = all(&sub.root)
                .into_iter()
                .find(|e| e.role == "entry")
                .and_then(|e| e.at.clone());
            ok = all(&sub.root)
                .into_iter()
                .find(|e| e.role == "button" && e.name == "OK")
                .and_then(|e| e.at.clone());
        }
        entry.is_some() && ok.is_some()
    })
    .await;
    let mut not_taken = None;
    let typed = match (&entry, &ok) {
        (Some(e), Some(o)) => match a.found_at(e).await {
            Ok(f) => match a.set_text(&f, "Grace").await {
                Ok(()) => match a.found_at(o).await {
                    Ok(f) => a.press(&f).await.is_ok(),
                    Err(_) => false,
                },
                Err(e) => {
                    not_taken = Some(e.to_string());
                    false
                }
            },
            Err(_) => false,
        },
        _ => false,
    };
    let answered_with = |what: &'static str| {
        let beacons = beacons.clone();
        async move {
            until(5_000, async || {
                beacons_since(&beacons, from)
                    .iter()
                    .any(|b| b.get("ev").is_some_and(|e| e.starts_with(what)))
            })
            .await
        }
    };
    match not_taken {
        None => {
            let answered = answered_with("prompt-Grace").await;
            v.check(
                "prompt()'s field took a text and OK answered it",
                typed && answered,
                format!("{:?}", beacons_since(&beacons, from)),
            );
        }
        Some(err) => {
            // Firefox kept its field's text: Kai says so and presses nothing; OK closes it here.
            let nothing = !beacons_since(&beacons, from)
                .iter()
                .any(|b| b.get("ev").is_some_and(|e| e.starts_with("prompt-")));
            v.check(
                "prompt()'s field: a text Firefox did not take is said, and nothing pressed",
                nothing && err.contains("did not take the text"),
                &err,
            );
            if let Some(o) = &ok
                && let Ok(f) = a.found_at(o).await
            {
                let _ = a.press(&f).await;
            }
            let closed = answered_with("prompt-").await;
            v.check("the prompt closed by its OK", closed, "");
        }
    }
    // A site's location prompt: a prompt for the tab's site, with its words (kai-firefox WP12's
    // review round, H2).
    if let Ok(f) = a
        .find(&target, &Query::parse("button \"Locate\"").unwrap())
        .await
    {
        let _ = a.press(&f).await;
    }
    let site = format!("127.0.0.1:{port}");
    let mut asked = None;
    until(5_000, async || {
        if let Ok(t) = a.app_tree(pid, details).await
            && let Ok(w) = window::of(&t, &top_tab)
            && let Some(at) = w.page.at.clone()
        {
            asked = parts::parts(&w.frame, &at, Some(&site))
                .read
                .into_iter()
                .find(|r| r.part == parts::Part::Prompt);
        }
        asked.is_some()
    })
    .await;
    match &asked {
        Some(r) => {
            let mut says = Vec::new();
            parts::says(&r.root, &mut says);
            let words = format!("\"Allow {site} to access your location?\"");
            v.check(
                "a site's location prompt, read as a prompt for the tab's site with its words",
                r.prompt == Some(parts::Prompt::Permission)
                    && r.hosts == [site.clone()]
                    && says.first() == Some(&words),
                format!("{:?} {:?} {says:?}", r.prompt, r.hosts),
            );
            // Named for another site than the tab's, it is not one of the tab's prompts.
            v.check(
                "the same prompt for a tab of another site is not read",
                parts::prompt(&r.root, Some("shop.example")) == parts::Prompt::Other,
                "",
            );
            let mut block = Vec::new();
            parts::pressables(&r.root, &mut block);
            if let Some(b) = block.into_iter().find(|e| e.name == "Block")
                && let Some(at) = &b.at
                && let Ok(f) = a.found_at(at).await
            {
                let _ = a.press(&f).await;
            }
        }
        None => v.check("a site's location prompt", false, "no prompt in front"),
    }
    // Firefox's own tool bars.
    let tree = a.app_tree(pid, details).await.unwrap();
    let frame = all(&tree.root)
        .into_iter()
        .find(|e| e.role == "frame")
        .unwrap();
    let page_at = all(frame)
        .into_iter()
        .find(|e| e.document.as_deref() == Some(top.as_str()))
        .and_then(|e| e.at.clone())
        .unwrap();
    let p = parts::parts(frame, &page_at, None);
    let left = p.left;
    let mut lines = Vec::new();
    for r in &p.read {
        parts::lines(&r.root, 0, &mut lines);
    }
    v.check(
        "Firefox's tool bars read, the tab strip left out",
        lines.iter().any(|l| l.contains("button \"Reload\""))
            && !lines
                .iter()
                .any(|l| l.contains("page tab \"Kai live back\""))
            && left >= 1,
        format!("{} lines, {left} left out", lines.len()),
    );
    // about:preferences in front: its page is the one read; the top page no longer is.
    v.check("the Settings tab pressed", select("Settings").await, "");
    let prefs = until(5_000, async || {
        a.app_tree(pid, details).await.is_ok_and(|t| {
            all(&t.root).iter().any(|e| {
                e.document
                    .as_deref()
                    .is_some_and(|d| d.starts_with("about:preferences"))
            })
        })
    })
    .await;
    // Firefox takes SHOWING from the page before a moment after the new page shows it.
    let switched = Instant::now();
    let left = until(5_000, async || {
        a.app_tree(pid, details).await.is_ok_and(|t| {
            !all(&t.root)
                .iter()
                .any(|e| e.document.as_deref() == Some(top.as_str()))
        })
    })
    .await;
    v.check(
        "the page before no longer shown after the switch",
        left,
        format!(
            "{} ms after the new page showed",
            switched.elapsed().as_millis()
        ),
    );
    let loose = Target {
        exact: false,
        ..target.clone()
    };
    let gone = a
        .find(&loose, &Query::parse("button \"Ask\"").unwrap())
        .await
        .is_err();
    let shown: Vec<String> = a
        .app_tree(pid, details)
        .await
        .map(|t| {
            all(&t.root)
                .iter()
                .filter_map(|e| e.document.clone())
                .collect()
        })
        .unwrap_or_default();
    v.check(
        "about:preferences in front; the top page in the background is not pressed in",
        prefs && gone,
        format!("shown {shown:?}, prefs {prefs}, the top page's button not found {gone}"),
    );
    // An error page: its DocURL is the address that failed, and its window is found by it.
    let known = ["Kai live top", "Kai live back", "Settings", "Opened"];
    let error_tab = a.app_tree(pid, details).await.ok().and_then(|t| {
        all(&t.root)
            .into_iter()
            .find(|e| e.role == "page tab" && !known.iter().any(|k| e.name.starts_with(k)))
            .map(|e| e.name.clone())
    });
    let mut error_page = Err("no error page's tab".to_owned());
    if let Some(title) = &error_tab {
        let _ = select(title).await;
        let tab = Tab {
            id: 4,
            url: refused.clone(),
            title: title.clone(),
            active: true,
            ..Default::default()
        };
        until(5_000, async || {
            error_page = match a.app_tree(pid, details).await {
                Ok(t) => window::of(&t, &tab).map(|w| w.page.document.unwrap_or_default()),
                Err(e) => Err(e.to_string()),
            };
            error_page.is_ok()
        })
        .await;
    }
    v.check(
        "an error page's window found by the address that failed",
        error_page.as_deref() == Ok(refused.as_str()),
        format!("{error_tab:?}: {error_page:?}"),
    );
    // Another extension's page in front (a temporary add-on, installed through WebDriver BiDi):
    // never read or pressed in, the strictest rule.
    let vault = vault_url();
    let installed = install(ff.bidi, &ff.dir.join("vault")).await;
    let mut shown = None;
    if installed.is_ok() {
        until(15_000, async || {
            if let Ok(t) = a.app_tree(pid, details).await {
                shown = all(&t.root)
                    .into_iter()
                    .filter_map(|e| e.document.clone())
                    .find(|d| d.starts_with("moz-extension:"));
            }
            shown.is_some()
        })
        .await;
    }
    let rules = crate::browser::sites::SiteRules::read(&crate::browser::settings::Sites {
        none: vec!["bank.example".into()],
        ..Default::default()
    })
    .unwrap();
    v.check(
        "another extension's page in front: never read or pressed in, the strictest rule",
        shown.as_deref() == Some(vault.as_str())
            && never::never_read(&vault) == Some("another extension's page (moz-extension:)")
            && rules.page_rule(&vault) == crate::browser::sites::Rule::None,
        format!("{installed:?}, shown {shown:?}"),
    );
    drop(ff);
    assert!(v.0.is_empty(), "failed: {:?}", v.0);
}
