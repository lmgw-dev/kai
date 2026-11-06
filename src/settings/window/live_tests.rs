//! The window's live check (`scripts/live-check-window.sh`): the real window, WebKit and page in a
//! nested KWin (scripts/nested-kwin.sh: its own bus and XDG directories under target/, nothing on
//! the session's screen). Ignored by default; it refuses to run unless `KAI_LIVE_NESTED=1` and the
//! display, the bus and the runtime directory are that nested KWin's.
//!
//! Against a stand-in core that answers the window's commands with the real operations on a
//! temporary configuration file, the page is driven through its own DOM: the policy table's
//! selects, the provider table, the settings list with an accepted value and a refused one (the
//! file is read after each). The page may reach nothing else: a fetch of the ring's origin and of a
//! loopback server fail. KWin's frame is the window's (no GTK title bar), the zoom is 125 %. The
//! page's console is on stdout (`KAI_WINDOW_CONSOLE`), and a CSP violation fails the run.
//!
//! The Personality page ([`personality`]) runs against a dev lmgw the script started: the stand-in
//! core forwards the page's profile-editor requests over a pairing with that lmgw as Kai's core
//! does (`crate::lmgw_link::proxy`), unpaired first, then paired.
//!
//! Env: `KAI_LIVE_OUT` the report directory (screenshots in `<out>/shots/`, spectacle's capture of
//! the active window); `KAI_LIVE_LMGW` the file naming a pairing with the dev lmgw (two lines: a
//! `device-link.json` and the device's key as PEM).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use tokio::sync::mpsc;
use webkit2gtk::WebViewExt;

use super::SettingsWindow;
use crate::core::Command;
use crate::lmgw_link::Credentials;
use crate::lmgw_link::proxy::{self, Limits, Proxied};
use crate::settings;

mod personality;

const CONFIG: &str = "# Kai's configuration: these notes stay.\n[voice]\n# how long a hold-to-talk session stays open\ngrace_s = 30   # half a minute\nmode = \"automatic\"\n\n[audio]\necho_mode = \"app\"   # the headset echoes\n";

fn guard() -> Result<(), String> {
    if std::env::var("KAI_LIVE_NESTED").as_deref() != Ok("1") {
        return Err("KAI_LIVE_NESTED is not 1".into());
    }
    let log = std::env::var_os("KAI_NESTED_KWIN_LOG").ok_or("no KAI_NESTED_KWIN_LOG")?;
    let dir = Path::new(&log).parent().ok_or("no directory")?;
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    if Path::new(&display) != dir.join("run/kai-nested") {
        return Err("WAYLAND_DISPLAY is not the nested KWin's socket".into());
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let private = std::fs::read_to_string(dir.join("bus")).unwrap_or_default();
    if bus.is_empty()
        || bus != private.trim()
        || !bus.contains(&format!("path={}/run/", dir.display()))
    {
        return Err("not the nested KWin's private bus".into());
    }
    if Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap_or_default()) != dir.join("run") {
        return Err("XDG_RUNTIME_DIR is not the nested one".into());
    }
    if std::env::var_os("KAI_LIVE_OUT").is_none() {
        return Err("no KAI_LIVE_OUT".into());
    }
    if crate::nvidia::nvidia_present() && std::env::var_os(crate::nvidia::VAR).is_none() {
        return Err(format!(
            "NVIDIA is present: {} must be set",
            crate::nvidia::VAR
        ));
    }
    Ok(())
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

struct Run {
    out: PathBuf,
    config: PathBuf,
    window: Option<SettingsWindow>,
    lines: Vec<String>,
    failed: usize,
    lmgw: std::rc::Rc<Lmgw>,
}

/// The dev lmgw the stand-in core forwards to, and whether it says Kai is paired.
struct Lmgw {
    creds: Option<Credentials>,
    paired: std::cell::Cell<bool>,
    rt: tokio::runtime::Runtime,
}

impl Lmgw {
    fn new() -> Self {
        let creds = std::env::var_os("KAI_LIVE_LMGW")
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| {
                let mut lines = text.lines();
                let paired =
                    crate::pairing::Paired::load(Path::new(lines.next()?.trim())).ok()??;
                let pem = std::fs::read_to_string(lines.next()?.trim()).ok()?;
                let key = crate::pairing::TlsKey::from_pem(&pem).ok()?;
                Some(Credentials {
                    reach: crate::lmgw_link::tls::Reach::new(paired.credentials.urls.clone()),
                    tls: crate::lmgw_link::tls::Tls::paired(
                        &paired.credentials,
                        &key,
                        std::time::Duration::from_secs(10),
                    )
                    .ok()?,
                })
            });
        Self {
            creds,
            paired: std::cell::Cell::new(false),
            rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    /// What Kai's core answers the page's request with (`crate::core::window`).
    fn forward(&self, forward: proxy::Forward, reply: tokio::sync::oneshot::Sender<Proxied>) {
        let creds = match (&self.creds, self.paired.get()) {
            (Some(c), true) => c.clone(),
            _ => {
                let why = crate::core::WINDOW_NOT_PAIRED;
                let _ = reply.send(Proxied::refused(503, "not_paired", why));
                return;
            }
        };
        let limits = Limits::from_config(&crate::config::WindowConfig::default());
        self.rt.spawn(async move {
            let _ = reply.send(proxy::send(&creds, forward, limits).await);
        });
    }

    fn conversation(&self) -> super::profiles::Conversation {
        let paired = self.paired.get();
        super::profiles::Conversation {
            paired,
            why: (!paired).then(|| crate::core::WINDOW_NOT_PAIRED.to_owned()),
            gateway: self.creds.as_ref().map(|c| c.url()),
            folder: paired.then(|| "Kai".to_owned()),
            thread_id: None,
            model: None,
            profile: paired.then(|| "Default".to_owned()),
        }
    }
}

impl Run {
    fn win(&self) -> &SettingsWindow {
        self.window.as_ref().expect("the window is open")
    }

    fn ok(&mut self, what: &str, good: bool, detail: impl std::fmt::Display) {
        let tag = if good { "ok  " } else { "FAIL" };
        if !good {
            self.failed += 1;
        }
        let line = format!("{tag} {what}: {detail}");
        println!("{line}");
        self.lines.push(line);
    }

    async fn js(&self, js: &str) -> String {
        self.win()
            .eval(js)
            .await
            .unwrap_or_else(|e| format!("js error: {e}"))
    }

    /// Polls `js` (a boolean expression) up to `secs`.
    async fn until(&self, js: &str, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if self.js(js).await == "true" {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            sleep(150).await;
        }
    }

    fn file(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap_or_default()
    }

    async fn file_has(&self, needle: &str, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if self.file().contains(needle) {
                return true;
            }
            sleep(150).await;
        }
        false
    }

    /// Spectacle's capture of the active window (KWin's frame and the page).
    async fn shot(&mut self, name: &str) {
        let dir = self.out.join("shots");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{name}.png"));
        let f = file.clone();
        let took = gtk::gio::spawn_blocking(move || {
            std::process::Command::new("spectacle")
                .args(["-a", "-b", "-n", "-o"])
                .arg(&f)
                .env("QT_QPA_PLATFORM", "wayland")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
        })
        .await;
        let good = matches!(took, Ok(Ok(s)) if s.success()) && file.exists();
        self.ok(&format!("screenshot {name}"), good, file.display());
    }

    /// A row of the page's table or list whose text starts with `key`: runs `body` on it.
    fn row_js(selector: &str, key: &str, body: &str) -> String {
        format!(
            "(() => {{ const rows = [...document.querySelectorAll('{selector}')]; \
             const row = rows.find(r => r.textContent.trim().startsWith('{key}')); \
             if (!row) return 'no row {key}'; {body} }})()"
        )
    }

    /// The Settings page's row of `key` (its `data-key`): runs `body` on it.
    fn setting_js(key: &str, body: &str) -> String {
        format!(
            "(() => {{ const row = document.querySelector('.settings .setting[data-key=\"{key}\"]'); \
             if (!row) return 'no row {key}'; {body} }})()"
        )
    }

    async fn pick(&mut self, selector: &str, key: &str, which: usize, value: &str) -> String {
        let body = format!(
            "const s = row.querySelectorAll('select')[{which}]; s.value = '{value}'; \
             s.dispatchEvent(new Event('change', {{bubbles: true}})); return 'done';"
        );
        self.js(&Self::row_js(selector, key, &body)).await
    }

    async fn page(&self, label: &str) {
        let js = format!(
            "(() => {{ const b = [...document.querySelectorAll('.rail-item')].find(b => b.textContent === '{label}'); \
             if (!b) return 'no rail item {label}'; b.click(); return 'done'; }})()"
        );
        self.js(&js).await;
        sleep(500).await;
    }
}

/// The stand-in core's state directory, beside the temporary configuration: the remote-control
/// token the Desktop page forgets lives there.
fn remote_state(config: &std::path::Path) -> PathBuf {
    config
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join("state"))
        .unwrap_or_default()
}

/// The stand-in core: the window's commands answered by the real operations on the file, and the
/// Personality page's by the dev lmgw.
fn spawn_core(config: PathBuf, lmgw: std::rc::Rc<Lmgw>) -> mpsc::UnboundedSender<Command> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    glib::MainContext::default().spawn_local(async move {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::Setting { request, reply } => {
                    // As the core's window path: a refusal in the catalog's words.
                    let _ = reply.send(
                        settings::apply(&config, request)
                            .map(|o| o.response)
                            .map_err(|r| settings::Refusal::plain(settings::catalog::in_words(&r))),
                    );
                }
                Command::WindowConversation { reply } => {
                    let _ = reply.send(lmgw.conversation());
                }
                Command::WindowDevices { reply } => {
                    use super::devices::{Choice, Devices};
                    let choice = |name: &str, label: &str| Choice {
                        name: name.into(),
                        label: label.into(),
                    };
                    let _ = reply.send(Devices {
                        microphones: vec![choice("alsa_input.test", "Test USB microphone")],
                        speakers: vec![choice("alsa_output.test", "Test speakers")],
                        screens: vec![choice("DP-1", "Test Monitor 27 (DP-1)")],
                    });
                }
                Command::WindowProxy { forward, reply } => lmgw.forward(forward, reply),
                Command::WindowRemoteControl { forget, reply } => {
                    let file = crate::desktop::fdo::input::token_file(&remote_state(&config));
                    let _ = reply.send(super::remote::answer(&file, forget));
                }
                _ => {}
            }
        }
    });
    tx
}

#[test]
#[ignore = "needs a nested KWin: scripts/live-check-window.sh"]
fn live_window_nested() {
    if let Err(e) = guard() {
        panic!("refused: {e}");
    }
    gtk::init().expect("GTK");
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").unwrap());
    let config = out.join("config").join("kai").join("config.toml");
    let _ = std::fs::remove_dir_all(out.join("config"));
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, CONFIG).unwrap();
    // A stored remote-control permission for the Desktop page to forget.
    let _ = std::fs::remove_dir_all(remote_state(&config));
    crate::desktop::fdo::input::token_file(&remote_state(&config))
        .store("live-check-token")
        .unwrap();
    let lmgw = std::rc::Rc::new(Lmgw::new());
    let cmds = spawn_core(config.clone(), lmgw.clone());
    let ctx = glib::MainContext::default();
    let failed = ctx.block_on(async move {
        let window = SettingsWindow::open(cmds, || {});
        let mut r = Run {
            out,
            config,
            window: Some(window),
            lines: vec![],
            failed: 0,
            lmgw,
        };
        drive(&mut r).await;
        close(&mut r).await;
        std::fs::write(r.out.join("report.txt"), r.lines.join("\n") + "\n").unwrap();
        r.failed
    });
    assert_eq!(failed, 0, "{failed} checks failed");
}

/// This process's WebKit children: (count, their summed PSS in kB, each one's name and PSS in MB).
fn web_children() -> (usize, u64, String) {
    let me = std::process::id();
    let (mut n, mut pss, mut each) = (0, 0, String::new());
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else {
            continue;
        };
        let ppid: u32 = stat[b + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        if ppid != me || !stat[a + 1..b].starts_with("WebKit") {
            continue;
        }
        n += 1;
        let rollup =
            std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
        let one = rollup
            .lines()
            .find_map(|l| l.strip_prefix("Pss:"))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0);
        pss += one;
        each.push_str(&format!(" {} {} MB;", &stat[a + 1..b], one / 1024));
    }
    (n, pss, each)
}

/// K1's RAM rule: the open window costs what a view costs, and closing it leaves no web process.
async fn close(r: &mut Run) {
    let (n, pss, each) = web_children();
    r.ok(
        "open: the web processes and their PSS",
        n > 0,
        format!(
            "{n} processes, {} MB:{each} (K1 measured 66 to 72 MB for an unrelated view)",
            pss / 1024
        ),
    );
    drop(r.window.take());
    let end = Instant::now() + Duration::from_secs(8);
    while web_children().0 > 0 && Instant::now() < end {
        sleep(200).await;
    }
    let left = web_children().0;
    r.ok(
        "closed: no web process is left",
        left == 0,
        format!("{left} left"),
    );
}

async fn drive(r: &mut Run) {
    let rows = "document.querySelectorAll('table.tools tbody tr').length";
    let up = r.until(&format!("{rows} > 20"), 40).await;
    let n = r.js(rows).await;
    r.ok(
        "the page loaded and listed the tools",
        up,
        format!("{n} tool rows"),
    );
    if !up {
        let body = r.js("document.body.innerText.slice(0, 400)").await;
        r.ok("what the page shows instead", false, body);
        return;
    }
    r.ok(
        "the zoom is 125 %",
        r.win().view.zoom_level() == 1.25,
        r.win().view.zoom_level(),
    );
    r.ok(
        "no GTK header bar: the window has no titlebar widget",
        r.win().window.titlebar().is_none(),
        "KWin's frame",
    );
    let (w, h) = r.win().window.size();
    r.ok(
        "the window fits the 1280x720 output",
        w <= 1280 && h <= 720,
        format!("{w}x{h}"),
    );
    r.shot("tools").await;

    // Tools: a state, then a level, written to the file at once and said "Saved" for a moment.
    let said = r
        .pick("table.tools tbody tr", "run_command", 1, "allow")
        .await;
    r.ok("the policy select answers", said == "done", said);
    let wrote = r.file_has("run_command = \"allow\"", 5).await;
    r.ok(
        "the tool's state is in the file",
        wrote,
        "policy.tools.run_command",
    );
    let note = r
        .until(
            &Run::row_js(
                "table.tools tbody tr",
                "run_command",
                "return row.querySelector('.note.ok')?.textContent === 'Saved';",
            ),
            5,
        )
        .await;
    r.ok("the row says Saved", note, "");
    r.pick("table.tools tbody tr", "run_command", 0, "owner")
        .await;
    let wrote = r.file_has("run_command = \"owner\"", 5).await;
    r.ok(
        "the tool's level is in the file",
        wrote,
        "policy.levels.run_command",
    );
    r.pick("table.tools tbody tr", "run_command", 1, "").await;
    sleep(400).await;
    r.ok(
        "the default removes the state, the file's other lines stay",
        !r.file().contains("run_command = \"allow\"") && r.file().contains("# the headset echoes"),
        "",
    );

    // Desktop.
    r.page("Desktop").await;
    let providers = r
        .js("document.querySelectorAll('table.providers tbody tr').length")
        .await;
    r.ok(
        "the capabilities are listed",
        providers == "11",
        format!("{providers} rows"),
    );
    r.pick("table.providers tbody tr", "Capture", 0, "fdo")
        .await;
    let wrote = r.file_has("capture = \"fdo\"", 5).await;
    r.ok("the provider is in the file", wrote, "desktop.capture");
    let restart = r
        .until("document.querySelector('.page-foot') !== null", 5)
        .await;
    r.ok("a restart-only change says so in the foot", restart, "");
    r.shot("desktop").await;
    // The remote-control permission (WP16c): said stored, forgotten after the confirmation.
    let token = crate::desktop::fdo::input::token_file(&remote_state(&r.config));
    let said = r
        .until(
            "(document.querySelector('.remote-status')?.textContent ?? '')\
             .startsWith('A remote-control permission is stored')",
            5,
        )
        .await;
    r.ok(
        "the Desktop page says a remote-control permission is stored",
        said,
        "",
    );
    let clicked = r
        .js(
            "(() => { const b = [...document.querySelectorAll('.remote-control button')]\
             .find(b => b.textContent.trim() === 'Forget remote-control permission'); \
             if (!b || b.disabled) return 'no enabled button'; b.click(); return 'done'; })()",
        )
        .await;
    let asked = r
        .until(
            "[...document.querySelectorAll('dialog[open]')].some(d => \
             d.textContent.includes('Forget the remote-control permission?'))",
            5,
        )
        .await;
    r.ok(
        "Forget asks for a confirmation first, and nothing is forgotten yet",
        clicked == "done" && asked && token.load().is_some(),
        &clicked,
    );
    r.shot("desktop-forget-confirm").await;
    r.js(
        "(() => { const b = [...document.querySelectorAll('dialog[open] button')]\
         .find(b => b.textContent.trim() === 'Forget'); if (b) b.click(); return 'done'; })()",
    )
    .await;
    let gone = r
        .until(
            "(document.querySelector('.remote-status')?.textContent ?? '')\
             .startsWith('No remote-control permission is stored')",
            5,
        )
        .await;
    r.ok(
        "the confirmed Forget removes the token and the page says none is stored",
        gone && token.load().is_none() && !token.0.exists(),
        token.0.display(),
    );

    // Settings: an accepted value (the comments stay), a refused one (the file stays).
    r.page("Settings").await;
    let fields = r
        .js("document.querySelectorAll('.settings .field').length")
        .await;
    r.ok(
        "every setting is listed",
        fields.parse::<usize>().unwrap_or(0) > 30,
        format!("{fields} fields"),
    );
    // The owner's rule (2026-10-19): a setting's label is its words, never its key.
    let keyed = r
        .js("[...document.querySelectorAll('.settings .field > label')].map(l => l.textContent.trim()) \
             .filter(t => /^[a-z_]+(\\.[a-z0-9_-]+)+$/.test(t) || t.includes('_')).join(', ')")
        .await;
    r.ok(
        "no setting is labelled with its key",
        keyed.is_empty(),
        &keyed,
    );
    // The polish pass of 2026-10-19: plain checkboxes (no On/Off word), no raw folder id, labels
    // on one line at 125 %, the pickers in place of identifier fields.
    let words = r
        .js("[...document.querySelectorAll('.settings .switch-line')].map(l => l.textContent.trim()) \
             .filter(t => t !== '').join(', ')")
        .await;
    r.ok(
        "a switch is a plain checkbox, with no On/Off word",
        words.is_empty(),
        &words,
    );
    let folder_id = r
        .js("document.querySelector('.settings .setting[data-key=\"conversation.folder_id\"]') === null")
        .await;
    r.ok(
        "the bare folder id is not shown",
        folder_id == "true",
        &folder_id,
    );
    let wrapped = r
        .js(
            "[...document.querySelectorAll('.settings .field > label')] \
             .filter(l => l.getBoundingClientRect().height > 30) \
             .map(l => l.textContent.trim()).join(' | ')",
        )
        .await;
    r.ok(
        "no label wraps onto a second line",
        wrapped.is_empty(),
        &wrapped,
    );
    let lower = r
        .js("[...document.querySelectorAll('.settings .field-hint')].map(h => h.textContent.trim()) \
             .filter(t => /^(On|Off),/.test(t)).join(' | ')")
        .await;
    r.ok(
        "no help line starts with On, or Off,",
        lower.is_empty(),
        &lower,
    );
    let pickers = r
        .js(&Run::setting_js(
            "audio.input",
            "const s = row.querySelector('select'); if (!s) return 'no select'; \
             return [...s.options].map(o => o.textContent.trim()).join(' | ');",
        ))
        .await;
    r.ok(
        "the microphone is a picker with System default and the device's name",
        pickers == "System default | Test USB microphone",
        &pickers,
    );
    r.js(&Run::setting_js(
        "audio.input",
        "const s = row.querySelector('select'); s.value = 'alsa_input.test'; \
         s.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let mic = r.file_has("input = \"alsa_input.test\"", 5).await;
    r.ok("picking a microphone stores its node name", mic, "");
    let reset = r
        .until(
            &Run::setting_js(
                "audio.input",
                "return row.querySelector('.ctl-reset button') !== null;",
            ),
            5,
        )
        .await;
    r.ok("Reset shows for a value that is not the default", reset, "");
    r.js(&Run::setting_js(
        "audio.input",
        "row.querySelector('.ctl-reset button').click(); return 'done';",
    ))
    .await;
    let unset = r
        .until(
            &Run::setting_js(
                "audio.input",
                "return row.querySelector('select').value === '' \
                 && row.querySelector('.ctl-reset button') === null;",
            ),
            5,
        )
        .await;
    r.ok(
        "Reset puts System default back and the button goes",
        unset && !r.file().contains("alsa_input.test"),
        "",
    );
    let set = |key: &str, value: &str| {
        Run::setting_js(
            key,
            &format!(
                "const i = row.querySelector('input'); i.value = '{value}'; \
                 i.dispatchEvent(new Event('input', {{bubbles: true}})); \
                 i.dispatchEvent(new Event('change', {{bubbles: true}})); return 'done';"
            ),
        )
    };
    r.js(&set("voice.grace_s", "45")).await;
    let wrote = r.file_has("grace_s = 45   # half a minute", 5).await;
    r.ok(
        "an accepted value is written and the comment stays",
        wrote,
        "voice.grace_s = 45",
    );
    // It applies at the next start: the footer says which setting waits and offers the restart.
    let footer = r
        .until(
            "(() => { const f = document.querySelector('.page-foot'); return !!f \
             && f.textContent.includes('“Keep a hold-to-talk session open”') \
             && [...f.querySelectorAll('button')].some(b => b.textContent.trim() === 'Restart Kai now'); })()",
            5,
        )
        .await;
    r.ok(
        "the footer lists the setting waiting for a restart and offers Restart Kai now",
        footer,
        "",
    );
    // Reset sits in one column whatever the control: compare a number's with a choice's.
    r.js(&Run::setting_js(
        "ring.anchor",
        "const s = row.querySelector('select'); s.value = 'top'; \
         s.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let aligned = r
        .until(
            "(() => { const a = document.querySelector('.setting[data-key=\"voice.grace_s\"] .ctl-reset button'); \
             const b = document.querySelector('.setting[data-key=\"ring.anchor\"] .ctl-reset button'); \
             return !!a && !!b && Math.abs(a.getBoundingClientRect().left - b.getBoundingClientRect().left) < 1; })()",
            5,
        )
        .await;
    r.ok("Reset lines up across a number and a choice", aligned, "");
    r.js(&Run::setting_js(
        "ring.anchor",
        "row.querySelector('.ctl-reset button').click(); return 'done';",
    ))
    .await;
    r.js(&Run::setting_js(
        "voice.grace_s",
        "row.scrollIntoView({block: 'center'}); return 'done';",
    ))
    .await;
    sleep(300).await;
    r.shot("settings-reset").await;
    r.js(&Run::setting_js(
        "voice.on_at_start",
        "row.scrollIntoView(); return 'done';",
    ))
    .await;
    sleep(300).await;
    r.shot("settings-switches").await;
    let before = r.file();
    // Below a bound the catalog knows, the page says so itself and sends nothing.
    r.js(&set("host.keepalive_s", "0")).await;
    let bounded = r
        .until(
            &Run::setting_js(
                "host.keepalive_s",
                "return (row.querySelector('.field-err')?.textContent ?? '') === 'At least 1 s.';",
            ),
            5,
        )
        .await;
    r.ok(
        "a value below its bound is said inline before it is sent",
        bounded,
        "",
    );
    // A rule only the loader knows (the buffer holds at least the connect timeout's audio): its
    // sentence, naming the settings by their labels.
    r.js(&set("voice.send_queue_s", "5")).await;
    let refused = r
        .until(
            &Run::setting_js(
                "voice.send_queue_s",
                "return (row.querySelector('.field-err')?.textContent ?? '') \
                 .startsWith('Outgoing audio buffer: must be at least 15 (seconds: “Connect timeout”');",
            ),
            5,
        )
        .await;
    r.ok(
        "a refused value shows the loader's sentence inline, naming the settings by their labels",
        refused,
        "",
    );
    r.ok("and writes nothing", r.file() == before, "");
    // The Conversation (2026-10-19): shared by default. "Only on this computer" only shows the
    // name field (prefilled with this computer's name) and a button; nothing is written until the
    // button is pressed, and then once.
    let shared = r
        .js(&Run::setting_js(
            "conversation.folder_name",
            "const [s, o] = row.querySelectorAll('input[type=radio]'); \
             return `${s.checked} ${o.checked} ${row.querySelector('.own-name') === null}`;",
        ))
        .await;
    r.ok(
        "the conversation starts shared, with no name field",
        shared == "true false true",
        &shared,
    );
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "row.scrollIntoView(); return 'done';",
    ))
    .await;
    sleep(300).await;
    r.shot("settings-conversation-shared").await;
    let before = r.file();
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "const o = row.querySelectorAll('input[type=radio]')[1]; o.checked = true; \
         o.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let host = crate::settings::catalog::own_conversation_name();
    let field = r
        .until(
            &Run::setting_js(
                "conversation.folder_name",
                &format!(
                    "return row.querySelector('.own-name input')?.value === '{host}' \
                     && row.querySelector('.own-name button')?.textContent.trim() === 'Use this conversation';"
                ),
            ),
            5,
        )
        .await;
    sleep(800).await;
    r.ok(
        "only on this computer shows the name field with this computer's name and the button",
        field,
        &host,
    );
    r.ok("and writes nothing yet", r.file() == before, "");
    r.shot("settings-conversation-own").await;
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "row.querySelector('.own-name button').click(); return 'done';",
    ))
    .await;
    let own = r.file_has(&format!("folder_name = \"{host}\""), 5).await;
    r.ok("the button writes the name, once", own, "");
    let said = r
        .until(
            &Run::setting_js(
                "conversation.folder_name",
                &format!(
                    "return (row.querySelector('.note.ok')?.textContent ?? '').startsWith('Now using “{host}”');"
                ),
            ),
            5,
        )
        .await;
    r.ok("and says what happened", said, "");
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "const s = row.querySelectorAll('input[type=radio]')[0]; s.checked = true; \
         s.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let mut back = false;
    for _ in 0..50 {
        if !r.file().contains("folder_name") {
            back = true;
            break;
        }
        sleep(100).await;
    }
    r.ok("shared again removes the name from the file", back, "");
    // And back to the own name just used: it returns to it at once (after the page took Kai's
    // answer to the removal).
    sleep(600).await;
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "const o = row.querySelectorAll('input[type=radio]')[1]; o.checked = true; \
         o.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let again = r.file_has(&format!("folder_name = \"{host}\""), 5).await;
    r.ok(
        "choosing the own conversation again returns to the name used",
        again,
        "",
    );
    sleep(600).await;
    r.js(&Run::setting_js(
        "conversation.folder_name",
        "const s = row.querySelectorAll('input[type=radio]')[0]; s.checked = true; \
         s.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
    ))
    .await;
    let mut back = false;
    for _ in 0..50 {
        if !r.file().contains("folder_name") {
            back = true;
            break;
        }
        sleep(100).await;
    }
    r.ok("shared once more removes it", back, "");
    r.js("document.querySelector('.page-body').scrollTop = 0; 'done'")
        .await;
    sleep(300).await;
    r.shot("settings-top").await;
    overlay_position(r).await;

    personality::drive(r).await;

    // The page reaches nothing else.
    let probe = "window.__probe = {}; \
        const t = (k, u) => fetch(u).then(r => window.__probe[k] = 'status ' + r.status).catch(e => window.__probe[k] = 'blocked'); \
        t('ring', 'kai://app/ring.html'); t('http', 'http://127.0.0.1:9/'); t('tools', '/api/tools'); \
        'started'";
    r.js(probe).await;
    sleep(1500).await;
    let probe = r.js("JSON.stringify(window.__probe)").await;
    let v: serde_json::Value = serde_json::from_str(&probe).unwrap_or_default();
    let blocked = |k: &str| {
        matches!(
            v[k].as_str(),
            Some("blocked") | Some("status 403") | Some("status 404")
        )
    };
    r.ok(
        "the ring's origin is not reachable from the page",
        blocked("ring"),
        &v["ring"],
    );
    r.ok(
        "an http server is not reachable from the page",
        blocked("http"),
        &v["http"],
    );
    r.ok("its own route is", v["tools"] == "status 200", &v["tools"]);
}

/// A fixed-word setting is a select of the words Kai's core sent, under its table's heading: the
/// weather overlay's position picked there is in the file and loads as that anchor.
async fn overlay_position(r: &mut Run) {
    let key = "overlay.weather.position";
    let shape = r
        .js(&Run::setting_js(
            key,
            "const s = row.querySelector('select'); if (!s) return 'no select'; \
             const head = row.closest('.subsec')?.querySelector('.subsec-head')?.textContent; \
             const words = [...s.options].map(o => o.value).join(','); \
             const names = [...s.options].map(o => o.textContent).join(','); \
             return `${head} ${s.value} ${words} ${names}`;",
        ))
        .await;
    let words: Vec<&str> = crate::widgets::kinds::POSITIONS
        .iter()
        .map(|(_, w)| *w)
        .collect();
    r.ok(
        "the weather overlay's position is a select of the nine named positions under its heading",
        shape
            == format!(
                "Weather card top-right {} Top left,Top,Top right,Left,Center,Right,Bottom left,Bottom,Bottom right",
                words.join(",")
            ),
        &shape,
    );
    let text = r
        .js(&Run::setting_js(
            "overlay.weather.width",
            "return (row.querySelector('select') ? 'select' : row.querySelector('input')?.inputMode) \
             + ' ' + row.querySelector('.unit')?.textContent;",
        ))
        .await;
    r.ok(
        "a number beside it is a number field with its unit",
        text == "numeric px",
        &text,
    );
    let said = r
        .js(&Run::setting_js(
            key,
            "const s = row.querySelector('select'); s.value = 'bottom-left'; \
             s.dispatchEvent(new Event('change', {bubbles: true})); return 'done';",
        ))
        .await;
    r.ok("the position's select answers", said == "done", &said);
    let wrote = r.file_has("position = \"bottom-left\"", 5).await;
    let loaded = crate::config::Config::parse(&r.file(), &r.config)
        .map(|c| c.overlay.weather.position)
        .map_err(|e| e.to_string());
    r.ok(
        "the picked position is in the file and loads as that anchor",
        wrote && loaded == Ok(crate::overlay::placement::Anchor::BottomLeft),
        format!("{loaded:?}"),
    );
    let chip = r
        .until(
            &Run::setting_js(
                key,
                "return [...row.querySelectorAll('button')].some(b => b.textContent === 'Reset') \
                 && row.querySelector('select').value === 'bottom-left';",
            ),
            5,
        )
        .await;
    r.ok("the row offers Reset, showing bottom-left", chip, "");
    r.js(&Run::setting_js(
        key,
        "row.closest('.subsec').scrollIntoView({block: 'center'}); return 'done';",
    ))
    .await;
    sleep(300).await;
    r.shot("settings-overlay").await;
}

/// The README's shot of this window (`scripts/readme-shots.sh`, `just readme-shots`): the Tools page
/// on a configuration with Kai's defaults, unpaired, in a nested KWin.
/// `<out>/shots/window-tools.png`, spectacle's capture of the window.
#[test]
#[ignore = "needs a nested KWin: scripts/readme-shots.sh"]
fn readme_window_nested() {
    if let Err(e) = guard() {
        panic!("refused: {e}");
    }
    glib::set_prgname(Some(crate::APP_ID));
    gtk::init().expect("GTK");
    let out = PathBuf::from(std::env::var("KAI_LIVE_OUT").unwrap());
    let config = out.join("config").join("kai").join("config.toml");
    let _ = std::fs::remove_dir_all(out.join("config"));
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "").unwrap();
    let lmgw = std::rc::Rc::new(Lmgw::new());
    let cmds = spawn_core(config.clone(), lmgw.clone());
    let ctx = glib::MainContext::default();
    let failed = ctx.block_on(async move {
        let window = SettingsWindow::open(cmds, || {});
        let mut r = Run {
            out,
            config,
            window: Some(window),
            lines: vec![],
            failed: 0,
            lmgw,
        };
        let up = r
            .until(
                "document.querySelectorAll('table.tools tbody tr').length > 20",
                40,
            )
            .await;
        r.ok("the page listed the tools", up, "");
        sleep(800).await;
        r.shot("window-tools").await;
        drop(r.window.take());
        sleep(500).await;
        r.failed
    });
    assert_eq!(failed, 0, "{failed} checks failed");
}
