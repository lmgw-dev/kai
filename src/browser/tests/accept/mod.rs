//! kai-firefox WP10's acceptance runs (kai-firefox design §6.6, §11): Eyes and Hands on the fixture
//! site through Kai's real tool path, with a scripted model. `just browser-accept` runs them.
//!
//! - **Kai's side is real**: the browser server on a socket in `target/browser-accept/<time>/`
//!   with the peer check (`browser.peer_scopes` at its default), the browser provider with its
//!   real routes, policy, focus rule, site rules, commit guard, network gate and the
//!   hands-and-eyes switch, and every call through the host's dispatch (`Local::call_timed`: K15's
//!   policy, the tool's preparation, the owner's confirmation, the handler) as Kai's own voice
//!   session's call through lmgw (`Caller::Own`, lmgw's deadline). The configuration is a real
//!   user's: the switch on, the fixture site's two origins `act`, defaults otherwise.
//! - **The seams** are the ones Kai has no desktop for here: the owner's dialog ([`stage::Owner`],
//!   which answers as the script says), the speech ([`stage::Tts`]: the announcements' and the
//!   questions' text is kept, nothing is played), the owner's microphone and the transcription of
//!   a spoken answer ([`voice`]: Kai's own questions go through the real `VoiceConfirmer` and the
//!   real approvals bookkeeping, and the owner answers some by voice) and the trusted path's
//!   desktop (`NoDesk`: a press that needs a real key fails; the trusted path needs KWin and is
//!   not run headless).
//! - **Firefox's side is real**: kai-firefox's `tests/accept/launch.mjs` starts the fixture site and
//!   a headless Firefox (its profile under the kai-firefox checkout's `target/`) with the extension,
//!   in a systemd scope named as Plasma names a Firefox it starts, whose `kai-browser-host` (this
//!   checkout's build) links to the socket. Nobody sends a page a mouse or key event.
//! - **The script** ([`eyes`], [`hands`], [`free`]) calls tools as a model would, reads their
//!   answers' lines for refs, and checks what Kai answered and said; the report goes to the run
//!   folder (`report.txt`) and the test's output.
//!
//! Needs `KAI_FIREFOX` (the kai-firefox checkout), `node` and Firefox 159; `KAI_BROWSER_HOST`
//! names the host (default `target/debug/kai-browser-host`).

mod eyes;
mod free;
mod hands;
mod launcher;
mod stage;
mod voice;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::browser::links::Links;
use crate::browser::provider::BrowserTools;
use crate::browser::refs::Refs;
use crate::browser::server::{Server, serve};
use crate::browser::spans::Spans;
use crate::host::HostLink;
use crate::host::policy::{Policy, SharedPolicy};
use crate::host::registry::Changes;
use launcher::Launcher;
use stage::{Outcome, Owner, Stage, Tts};

/// The owner's configuration (module doc): the switch on, the fixture's origins `act`.
fn config(port: &str) -> crate::config::Config {
    let text = format!(
        "[browser]\nhands_and_eyes = true\n\n[browser.sites]\nact = [\"127.0.0.1:{port}\", \
         \"localhost:{port}\"]\n"
    );
    crate::config::Config::parse(&text, std::path::Path::new("kai.toml"))
        .expect("the run's configuration reads")
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Firefox, node and the kai-firefox checkout (KAI_FIREFOX): just browser-accept"]
async fn hands_and_eyes_on_the_fixture_site() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let firefox = env_path("KAI_FIREFOX").expect("KAI_FIREFOX names the kai-firefox checkout");
    let host_bin =
        env_path("KAI_BROWSER_HOST").unwrap_or_else(|| root.join("target/debug/kai-browser-host"));
    assert!(host_bin.exists(), "no {}", host_bin.display());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let dir = root.join(format!("target/browser-accept/{stamp}"));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("browser.sock");

    // Kai's browser server and host (module doc).
    let (changes, changed) = Changes::new();
    let links = Links::new(changes);
    let server = tokio::spawn(serve(
        socket.clone(),
        Server {
            links: links.clone(),
            config_file: None,
            scopes: crate::browser::settings::BrowserConfig::default().peer_scopes,
            peer_of: crate::browser::peer::proc_peer,
            sidebar: None,
            sites: None,
        },
    ));
    let policy: SharedPolicy = Arc::new(std::sync::RwLock::new(Policy::of(&config("0"))));
    let owner = Arc::new(Owner::default());
    let tts = Tts::default();
    // Kai's own questions go to the owner's dialog and to the voice (kai-firefox WP10b).
    let ears = voice::Ears::start(tts.clone());
    let confirmer = Arc::new(crate::approvals::asked::VoiceConfirmer::new(
        owner.clone(),
        ears.voice(),
    ));
    let spans = Spans::new(
        links.clone(),
        policy.clone(),
        crate::browser::moves::Moves::default(),
    );
    let tools = BrowserTools::new(
        links.clone(),
        policy.clone(),
        Refs::default(),
        confirmer.clone(),
        Arc::new(crate::browser::tools::trusted::NoDesk),
        spans,
        Arc::new(tts.clone()),
    );
    let host = HostLink::new(
        crate::host::tests::settings(5),
        policy.clone(),
        vec![Box::new(tools)],
        changed,
        confirmer,
        tokio::sync::watch::channel(None).0,
        crate::screen_lock::ScreenLock::default(),
        crate::jobs::Jobs::default(),
    );
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(!server.is_finished(), "browser.sock was not served");
        assert!(std::time::Instant::now() < end, "no socket");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let launcher = Launcher::start(&firefox, &socket, &host_bin)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let profile = launcher.ready["profile"].as_str().unwrap_or("").to_owned();
    let firefox_pid = launcher.ready["firefox"]["pid"].as_i64().unwrap_or(-1);
    let port = launcher.ready["site"]
        .as_str()
        .and_then(|s| s.rsplit(':').next())
        .unwrap_or("")
        .trim_end_matches('/')
        .to_owned();
    host.set_policy(Policy::of(&config(&port)));
    let end = std::time::Instant::now() + Duration::from_secs(30);
    while links.list().len() != 1 {
        assert!(
            std::time::Instant::now() < end,
            "Kai for Firefox did not link within 30 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut st = Stage::new(host.local(), owner, tts, ears, launcher, dir.clone());
    eyes::run(&mut st).await;
    hands::run(&mut st).await;
    free::run(&mut st).await;
    let shots: Vec<String> = st
        .tools_called()
        .into_iter()
        .filter(|t| t == "page_see" || t == "element_see")
        .collect();
    st.record(
        "Eyes",
        "no screenshot tool was used",
        Ok(if shots.is_empty() {
            Outcome::Pass
        } else {
            Outcome::Fail(format!("called {shots:?}"))
        }),
    );

    let Stage {
        launcher, steps, ..
    } = st;
    let stopped = launcher.stop().await;
    server.abort();
    // The run's Firefox: what names its profile, and its content processes by their parent.
    let parent = format!("-parentPid {firefox_pid} ");
    let leftovers: Vec<String> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("cmdline")).ok())
        .map(|c| String::from_utf8_lossy(&c).replace('\0', " "))
        .filter(|c| (!profile.is_empty() && c.contains(&profile)) || c.contains(&parent))
        .collect();
    let report = report::write(&steps, &stopped, &leftovers);
    std::fs::write(dir.join("report.txt"), &report).unwrap();
    println!("{report}\nreport: {}", dir.join("report.txt").display());
    assert!(
        leftovers.is_empty(),
        "processes of the run's Firefox are left: {leftovers:?}"
    );
    let failed: Vec<&str> = steps
        .iter()
        .filter(|s| matches!(s.outcome, Outcome::Fail(_)))
        .map(|s| s.name)
        .collect();
    assert!(failed.is_empty(), "failed steps: {failed:?}");
}

mod report {
    use serde_json::Value;

    use super::stage::{Outcome, Step};

    /// The run's report: every step with its outcome, its calls and answers, the owner's
    /// questions and what Kai said.
    pub fn write(steps: &[Step], stopped: &Result<Value, String>, leftovers: &[String]) -> String {
        let mut out = String::new();
        let mut summary = String::new();
        for s in steps {
            let (mark, why) = match &s.outcome {
                Outcome::Pass => ("PASS", String::new()),
                Outcome::Fail(w) => ("FAIL", w.clone()),
            };
            summary.push_str(&format!("{mark:8} {:6} {}\n", s.run, s.name));
            out.push_str(&format!("\n=== {mark} {}: {}\n", s.run, s.name));
            if !why.is_empty() {
                out.push_str(&format!("  {why}\n"));
            }
            for c in &s.calls {
                out.push_str(&format!(
                    "  > {} {}\n",
                    c.tool,
                    serde_json::to_string(&c.args).unwrap_or_default()
                ));
                for l in c.answer.text.lines() {
                    out.push_str(&format!(
                        "    {}{l}\n",
                        if c.answer.error { "! " } else { "" }
                    ));
                }
            }
            for q in &s.questions {
                out.push_str(&format!("  asked the owner: {}\n", q.said()));
            }
            for h in &s.heard {
                out.push_str(&format!(
                    "  spoken ({:?}, focused {:?}{}): {}\n",
                    h.kind,
                    h.focused,
                    h.dropped
                        .as_ref()
                        .map(|d| format!(", dropped: {d}"))
                        .unwrap_or_default(),
                    h.text
                ));
            }
        }
        format!(
            "kai-firefox WP10 acceptance runs\n\n{summary}\nlauncher stopped: {stopped:?}\nleft \
             processes of the run's profile: {leftovers:?}\n{out}"
        )
    }
}
