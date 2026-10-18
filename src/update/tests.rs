//! The updater's feed resolution and its state flow, without the desktop: the notifications go to
//! a fake desk, the feed's side is a fake (no network, no pkexec, no dnf), the owner's clicks come
//! in as the notification server's `ActionInvoked` would, in tokio's paused time. The real feed's
//! requests (`FeedUpdates`) run against a minimal HTTP server on loopback.

// The updater's own requests (to a loopback feed), never lmgw's: device link D15's rule
// (`clippy.toml`) is lmgw's connections'.
#![allow(clippy::disallowed_methods)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use lmgw_update::ReleaseManifest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::*;

const SERVER: &str = ":1.42";

fn info(version: &str, notes: &str) -> UpdateInfo {
    UpdateInfo {
        current: "0.1.0+7".into(),
        manifest: ReleaseManifest {
            version: version.into(),
            notes: notes.into(),
            pub_date: String::new(),
            rpm: RpmArtifact {
                file: format!("kai-{version}-1.fc44.x86_64.rpm"),
                url: format!("https://registry.invalid/kai/{version}/kai.rpm"),
                sha256: String::new(),
            },
        },
    }
}

/// A check's answer.
type Answer = Result<Option<UpdateInfo>, String>;

/// The feed's side: checks answer from a queue (empty: up to date), the download writes the file,
/// the install records whether it found it.
#[derive(Clone)]
struct FakeFeed {
    checks: Arc<Mutex<VecDeque<Answer>>>,
    download: Arc<Mutex<Result<(), String>>>,
    install: Arc<Mutex<Result<(), String>>>,
    rpm_host: bool,
    log: Arc<Mutex<Vec<String>>>,
}

impl FakeFeed {
    fn new(rpm_host: bool) -> Self {
        Self {
            checks: Arc::default(),
            download: Arc::new(Mutex::new(Ok(()))),
            install: Arc::new(Mutex::new(Ok(()))),
            rpm_host,
            log: Arc::default(),
        }
    }
    fn answer(&self, a: Answer) {
        self.checks.lock().unwrap().push_back(a);
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn checks(&self) -> usize {
        self.log().iter().filter(|l| l.starts_with("check")).count()
    }
}

impl Updates for FakeFeed {
    async fn check(&self, current: &str) -> Result<Option<UpdateInfo>, String> {
        self.log.lock().unwrap().push(format!("check {current}"));
        self.checks.lock().unwrap().pop_front().unwrap_or(Ok(None))
    }
    async fn download(&self, rpm: &RpmArtifact, dest: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!(
            "download {} (leftover: {})",
            rpm.url,
            dest.exists()
        ));
        std::fs::write(dest, b"rpm").unwrap();
        self.download.lock().unwrap().clone()
    }
    fn install(&self, rpm: &Path) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("install (file there: {})", rpm.exists()));
        self.install.lock().unwrap().clone()
    }
    fn can_self_install(&self) -> bool {
        self.rpm_host
    }
}

/// One notification the updater showed: what, in place of which, and the id it got.
#[derive(Debug)]
struct Said {
    note: Note,
    replaces: Option<Shown>,
    shown: Shown,
}

struct FakeDesk {
    next: Mutex<u32>,
    said: UnboundedSender<Said>,
    /// The notifications the updater closed, in order.
    closed: Arc<Mutex<Vec<Shown>>>,
}

impl Desk for FakeDesk {
    async fn show(&self, note: Note, replaces: Option<Shown>) -> Option<Shown> {
        let id = {
            let mut next = self.next.lock().unwrap();
            *next += 1;
            *next
        };
        let shown = Shown {
            id,
            server: SERVER.into(),
        };
        let _ = self.said.send(Said {
            note,
            replaces,
            shown: shown.clone(),
        });
        Some(shown)
    }
    async fn close(&self, shown: &Shown) {
        self.closed.lock().unwrap().push(shown.clone());
    }
}

struct Rig {
    feed: FakeFeed,
    requests: UnboundedSender<Request>,
    actions: UnboundedSender<Invoked>,
    said: UnboundedReceiver<Said>,
    cmds: UnboundedReceiver<Command>,
    closed: Arc<Mutex<Vec<Shown>>>,
    dest: PathBuf,
    _dir: tempfile::TempDir,
}

fn settings(dir: &Path, background_check: bool) -> UpdateSettings {
    UpdateSettings {
        background_check,
        first_check: Duration::from_secs(20),
        interval: Duration::from_secs(6 * 60 * 60),
        dest: dir.join("run").join("kai-update.rpm"),
    }
}

fn rig_with(feed: Result<FakeFeed, String>, background_check: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings(dir.path(), background_check);
    let dest = settings.dest.clone();
    let (said_tx, said) = unbounded_channel();
    let (cmds_tx, cmds) = unbounded_channel();
    let (requests, requests_rx) = unbounded_channel();
    let (actions, actions_rx) = unbounded_channel();
    let closed = Arc::new(Mutex::new(Vec::new()));
    let desk = FakeDesk {
        next: Mutex::new(0),
        said: said_tx,
        closed: closed.clone(),
    };
    let fake = feed.clone().unwrap_or_else(|_| FakeFeed::new(true));
    tokio::spawn(
        Updater::new(feed, desk, settings, "0.1.0+7", cmds_tx).run(requests_rx, actions_rx),
    );
    Rig {
        feed: fake,
        requests,
        actions,
        said,
        cmds,
        closed,
        dest,
        _dir: dir,
    }
}

fn rig(rpm_host: bool) -> Rig {
    rig_with(Ok(FakeFeed::new(rpm_host)), true)
}

impl Rig {
    async fn said(&mut self) -> Said {
        tokio::time::timeout(Duration::from_secs(60), self.said.recv())
            .await
            .expect("a notification")
            .unwrap()
    }
    /// Lets everything that is ready run, in paused time.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    fn closed(&self) -> Vec<Shown> {
        self.closed.lock().unwrap().clone()
    }
    fn nothing_said(&mut self) {
        assert!(self.said.try_recv().is_err(), "no notification expected");
    }
    fn click(&self, shown: &Shown, key: &str) {
        self.actions
            .send(Invoked {
                server: shown.server.clone(),
                id: shown.id,
                key: key.into(),
            })
            .unwrap();
    }
    async fn tray_check(&mut self) -> Said {
        self.requests.send(Request::CheckNow).unwrap();
        self.said().await
    }
}

const PUBLIC_URL: &str = "https://github.com/lmgw-dev/kai/releases/latest/download/latest.json";

fn built(url: Option<&'static str>, deploy_token: Option<&'static str>) -> BuildFeed<'static> {
    BuildFeed {
        url,
        deploy_token,
        public: false,
    }
}

#[test]
fn the_feed_is_the_endpoint_or_the_compiled_url_and_none_without_either() {
    assert_eq!(
        resolve_feed(&built(None, Some("dep")), None, Some("tok")),
        None
    );
    assert_eq!(
        resolve_feed(&built(None, None), Some("  "), None),
        None,
        "blank is unset"
    );

    let r = resolve_feed(&built(Some("https://ci/latest.json"), None), None, None).unwrap();
    assert_eq!(
        (r.feed.url.as_str(), &r.feed.auth, r.from),
        ("https://ci/latest.json", &FeedAuth::None, "compiled in")
    );
    let r = resolve_feed(
        &built(Some("https://ci/latest.json"), None),
        Some(" http://test/latest.json "),
        None,
    )
    .unwrap();
    assert_eq!(
        (r.feed.url.as_str(), r.from),
        ("http://test/latest.json", ENDPOINT_VAR),
        "the run-time override wins"
    );
    let r = resolve_feed(&built(None, None), Some("http://test/latest.json"), None).unwrap();
    assert_eq!(
        r.feed.url, "http://test/latest.json",
        "an override alone is a feed"
    );
    let r = resolve_feed(&built(Some("https://ci/l.json"), None), Some(""), None).unwrap();
    assert_eq!(r.from, "compiled in", "a blank override is unset");
}

/// Each token goes only to the feed it belongs to: the user's with the user's endpoint, the
/// deploy token with the compiled-in feed; neither reaches a log.
#[test]
fn each_token_goes_only_to_its_own_feed_and_never_reaches_a_log() {
    let ci = Some("https://git.example/api/v4/projects/7/packages/generic/kai/latest/latest.json");
    let mirror = Some("https://mirror.example/kai/latest.json");

    // The private endpoint with the user's token: the token goes along.
    let r = resolve_feed(&built(ci, Some("gldt-ci")), mirror, Some("glpat-mine")).unwrap();
    assert_eq!(r.feed.auth, FeedAuth::PrivateToken("glpat-mine".into()));
    assert_eq!(r.unused_token, None);
    // The endpoint never gets the compiled-in deploy token.
    let r = resolve_feed(&built(ci, Some("gldt-ci")), mirror, None).unwrap();
    assert_eq!(r.feed.auth, FeedAuth::None);
    // The compiled-in feed gets its deploy token, never the user's (which is said in the log).
    let r = resolve_feed(&built(ci, Some("gldt-ci")), None, Some("glpat-mine")).unwrap();
    assert_eq!(r.feed.auth, FeedAuth::DeployToken("gldt-ci".into()));
    assert!(r.unused_token.unwrap().contains("KAI_UPDATE_ENDPOINT"));
    let r = resolve_feed(&built(ci, None), None, Some("glpat-mine")).unwrap();
    assert_eq!(r.feed.auth, FeedAuth::None);
    assert!(r.unused_token.is_some());
    // Blank is unset.
    let r = resolve_feed(&built(ci, Some("gldt-ci")), mirror, Some(" ")).unwrap();
    assert_eq!(r.feed.auth, FeedAuth::None);
    assert_eq!(r.unused_token, None);

    let r = resolve_feed(&built(None, None), mirror, Some("glpat-mine")).unwrap();
    let shown = format!("{:?}", r.feed.auth);
    assert!(!shown.contains("glpat-mine"), "{shown}");
}

/// The public feed never gets a token: not the user's from the environment (a public RPM must not
/// send a private registry's token to github.com), not a deploy token a public build was given by
/// mistake, not one with GitHub named as the endpoint.
#[test]
fn the_public_feed_never_gets_a_token() {
    let public = BuildFeed {
        url: Some(PUBLIC_URL),
        deploy_token: None,
        public: true,
    };
    let r = resolve_feed(&public, None, Some("glpat-mine")).unwrap();
    assert_eq!(
        (r.feed.url.as_str(), &r.feed.auth),
        (PUBLIC_URL, &FeedAuth::None)
    );
    assert!(r.unused_token.is_some(), "the dropped token is logged");

    let stray = BuildFeed {
        deploy_token: Some("gldt-stray"),
        ..public
    };
    assert_eq!(
        resolve_feed(&stray, None, Some("glpat-mine"))
            .unwrap()
            .feed
            .auth,
        FeedAuth::None
    );
    // The public URL without the marker (a build that forgot it) is known by its host.
    assert_eq!(
        resolve_feed(&built(Some(PUBLIC_URL), Some("gldt-stray")), None, None)
            .unwrap()
            .feed
            .auth,
        FeedAuth::None
    );
    // GitHub named at run time, token or not.
    for url in [
        PUBLIC_URL,
        "https://GitHub.com./x/latest.json",
        "https://objects.githubusercontent.com/x",
    ] {
        let r = resolve_feed(&built(None, None), Some(url), Some("glpat-mine")).unwrap();
        assert_eq!(r.feed.auth, FeedAuth::None, "{url}");
        assert!(r.unused_token.is_some(), "{url}");
    }
    // A private endpoint on a public build still gets the user's token.
    let r = resolve_feed(
        &public,
        Some("https://mirror.example/kai/latest.json"),
        Some("glpat-mine"),
    )
    .unwrap();
    assert_eq!(r.feed.auth, FeedAuth::PrivateToken("glpat-mine".into()));

    assert!(!is_public_host("https://notgithub.com/x"));
    assert!(!is_public_host("https://github.com.example/x"));
    assert!(!is_public_host("not a url"));
}

fn rpm(url: &str, sha256: &str) -> RpmArtifact {
    RpmArtifact {
        file: "kai.rpm".into(),
        url: url.into(),
        sha256: sha256.into(),
    }
}

/// A manifest's RPM must carry a 64-hex sha256 and an https URL (plain http only to loopback, and
/// only where the caller allows it: the tests' server).
#[test]
fn a_manifest_without_a_real_sha256_or_https_is_refused() {
    let good = "ab".repeat(32);
    assert_eq!(
        vet_rpm(&rpm("https://x.example/k.rpm", &good), false),
        Ok(())
    );
    assert_eq!(
        vet_rpm(&rpm("https://x.example/k.rpm", &good.to_uppercase()), false),
        Ok(())
    );
    let (g64, a63, a65) = ("g".repeat(64), "a".repeat(63), "a".repeat(65));
    for sha in ["", "  ", "abc", &g64, &a63, &a65] {
        let e = vet_rpm(&rpm("https://x.example/k.rpm", sha), true).unwrap_err();
        assert!(e.contains("sha256"), "{sha:?}: {e}");
    }
    for url in [
        "http://x.example/k.rpm",
        "ftp://x.example/k.rpm",
        "file:///tmp/k.rpm",
    ] {
        let e = vet_rpm(&rpm(url, &good), true).unwrap_err();
        assert!(e.contains("https only"), "{url}: {e}");
    }
    assert!(vet_rpm(&rpm("not a url", &good), true).is_err());
    for url in [
        "http://127.0.0.1:9/k.rpm",
        "http://[::1]:9/k.rpm",
        "http://localhost/k.rpm",
    ] {
        assert_eq!(vet_rpm(&rpm(url, &good), true), Ok(()), "{url}");
        assert!(vet_rpm(&rpm(url, &good), false).is_err(), "{url}");
    }
}

/// Redirects never go from https down to http (with or without a token); a token's stay within the
/// feed's origin; at most [`MAX_REDIRECTS`].
#[test]
fn redirects_never_go_down_from_https() {
    let u = |s: &str| reqwest::Url::parse(s).unwrap();
    let gh = u(PUBLIC_URL);
    let assets = u("https://objects.githubusercontent.com/kai/latest.json");
    assert_eq!(
        redirect_verdict(std::slice::from_ref(&gh), &assets, false),
        Ok(())
    );
    let e = redirect_verdict(
        std::slice::from_ref(&gh),
        &u("http://objects.githubusercontent.com/x"),
        false,
    )
    .unwrap_err();
    assert!(e.contains("down from https"), "{e}");
    let e = redirect_verdict(
        &[gh.clone(), assets.clone()],
        &u("http://evil.example/x"),
        false,
    )
    .unwrap_err();
    assert!(e.contains("down from https"), "{e}");
    let e =
        redirect_verdict(std::slice::from_ref(&gh), &u("http://github.com/x"), true).unwrap_err();
    assert!(e.contains("another origin"), "{e}");
    assert!(redirect_verdict(std::slice::from_ref(&gh), &assets, true).is_err());
    assert_eq!(
        redirect_verdict(std::slice::from_ref(&gh), &u("https://github.com/y"), true),
        Ok(())
    );
    // Plain http that stays plain http (the tests' loopback server) is not a downgrade.
    let local = u("http://127.0.0.1:9/a");
    assert_eq!(
        redirect_verdict(&[local], &u("http://127.0.0.2:9/b"), false),
        Ok(())
    );
    let many = vec![gh.clone(); MAX_REDIRECTS + 1];
    assert!(
        redirect_verdict(&many, &gh, false)
            .unwrap_err()
            .contains("more than")
    );
}

#[tokio::test(start_paused = true)]
async fn a_build_without_a_feed_never_polls_and_the_tray_check_says_so() {
    let mut r = rig_with(Err(NO_FEED.to_owned()), true);
    tokio::time::sleep(Duration::from_secs(7 * 60 * 60)).await;
    r.nothing_said();
    let s = r.tray_check().await;
    assert_eq!(s.note.summary, "Kai cannot check for updates");
    assert!(
        s.note.body.contains("this build has no update feed"),
        "{}",
        s.note.body
    );
    assert!(s.note.actions.is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_background_check_runs_after_the_first_delay_then_every_interval_and_is_quiet() {
    let mut r = rig(true);
    r.feed.answer(Ok(None));
    r.feed.answer(Err("connection refused".into()));
    tokio::time::sleep(Duration::from_secs(19)).await;
    assert_eq!(r.feed.checks(), 0, "update.first_check_s");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        r.feed.log(),
        ["check 0.1.0+7"],
        "the running version is compared"
    );
    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
    assert_eq!(r.feed.checks(), 2, "update.interval_s");
    r.settle().await;
    r.nothing_said();
}

#[tokio::test(start_paused = true)]
async fn background_check_off_only_the_tray_checks() {
    let mut r = rig_with(Ok(FakeFeed::new(true)), false);
    tokio::time::sleep(Duration::from_secs(7 * 60 * 60)).await;
    assert_eq!(r.feed.checks(), 0);
    let s = r.tray_check().await;
    assert_eq!(s.note.summary, "Kai is up to date");
    assert_eq!(s.note.body, "Kai 0.1.0+7 is the latest version.");
}

#[tokio::test(start_paused = true)]
async fn the_tray_check_always_answers() {
    let mut r = rig_with(Ok(FakeFeed::new(true)), false);
    let s = r.tray_check().await;
    assert_eq!(s.note.summary, "Kai is up to date");
    r.feed
        .answer(Err("HTTP status client error (401 Unauthorized)".into()));
    let e = r.tray_check().await;
    assert_eq!(e.note.summary, "Kai couldn't check for updates");
    assert_eq!(e.note.body, "HTTP status client error (401 Unauthorized)");
    assert_eq!(e.replaces, None, "the tray's answer shows fresh");
    assert_eq!(r.closed(), [s.shown], "after closing the last one");
}

/// A tray check pops up even when the background's offer already expired into the history: the
/// offer is closed and the answer is a new notification, which the buttons then belong to.
#[tokio::test(start_paused = true)]
async fn a_tray_check_closes_the_background_offer_and_shows_fresh() {
    let mut r = rig(true);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let background = r.said().await;
    assert_eq!(background.replaces, None);
    assert!(r.closed().is_empty());

    let tray = r.tray_check().await;
    assert_eq!(tray.note.actions, [INSTALL, LATER]);
    assert_eq!(tray.replaces, None, "fresh, not a replacement");
    assert_eq!(r.closed(), std::slice::from_ref(&background.shown));

    r.click(&background.shown, "install");
    r.settle().await;
    r.nothing_said();
    assert_eq!(
        r.feed.log().len(),
        2,
        "the closed offer's button is ignored"
    );
    r.click(&tray.shown, "install");
    let installing = r.said().await;
    assert_eq!(installing.note.summary, "Installing Kai 0.1.0+9");
    assert_eq!(
        installing.replaces,
        Some(tray.shown),
        "an action's follow-up replaces in place"
    );
}

/// The background's repeat stays quiet: in place, nothing closed.
#[tokio::test(start_paused = true)]
async fn a_background_repeat_replaces_in_place() {
    let mut r = rig(true);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let first = r.said().await;
    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
    let second = r.said().await;
    assert_eq!(second.replaces, Some(first.shown));
    assert!(r.closed().is_empty());
}

/// An "up to date" answer after an earlier note closes it and shows fresh.
#[tokio::test(start_paused = true)]
async fn a_tray_up_to_date_after_an_earlier_note_closes_it_and_shows_fresh() {
    let mut r = rig_with(Ok(FakeFeed::new(true)), false);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.tray_check().await;
    let latest = r.tray_check().await;
    assert_eq!(latest.note.summary, "Kai is up to date");
    assert_eq!(latest.replaces, None);
    assert_eq!(r.closed(), [offer.shown]);
}

/// The whole flow: offered, Install, the download to the fixed path, the install that found it, the
/// file deleted, installed with Restart, and Restart asks the core for its quit sequence.
#[tokio::test(start_paused = true)]
async fn install_downloads_installs_deletes_and_restart_asks_the_core() {
    let mut r = rig(true);
    r.feed
        .answer(Ok(Some(info("0.1.0+9", "Automated build of abc on main."))));
    let offer = r.said().await;
    assert_eq!(offer.note.summary, "Kai update available");
    assert!(
        offer
            .note
            .body
            .starts_with("Kai 0.1.0+9 is available (you have 0.1.0+7).\n\nAutomated build of abc"),
        "{}",
        offer.note.body
    );
    assert_eq!(offer.note.actions, [INSTALL, LATER]);

    r.click(&offer.shown, "install");
    let installing = r.said().await;
    assert_eq!(installing.note.summary, "Installing Kai 0.1.0+9");
    assert_eq!(installing.replaces, Some(offer.shown.clone()));
    let done = r.said().await;
    assert_eq!(done.note.summary, "Updated to Kai 0.1.0+9");
    assert_eq!(done.note.actions, [RESTART]);
    assert_eq!(
        r.feed.log()[1..],
        [
            "download https://registry.invalid/kai/0.1.0+9/kai.rpm (leftover: false)",
            "install (file there: true)"
        ]
    );
    assert!(!r.dest.exists(), "the RPM is deleted after the install");

    assert!(r.cmds.try_recv().is_err());
    r.click(&done.shown, "restart");
    r.settle().await;
    assert!(matches!(r.cmds.try_recv(), Ok(Command::Restart)));
}

#[tokio::test(start_paused = true)]
async fn only_the_offers_own_buttons_count() {
    let mut r = rig(true);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.said().await;
    // Another notification's id, and the right id from another peer of the session.
    r.actions
        .send(Invoked {
            server: SERVER.into(),
            id: offer.shown.id + 100,
            key: "install".into(),
        })
        .unwrap();
    r.actions
        .send(Invoked {
            server: ":1.99".into(),
            id: offer.shown.id,
            key: "install".into(),
        })
        .unwrap();
    r.settle().await;
    r.nothing_said();
    assert_eq!(r.feed.log(), ["check 0.1.0+7"]);

    r.click(&offer.shown, "later");
    r.settle().await;
    r.click(&offer.shown, "install");
    r.settle().await;
    r.nothing_said();
    assert_eq!(r.feed.log(), ["check 0.1.0+7"], "Later ends the offer");
    // The next check offers it again.
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let again = r.tray_check().await;
    assert_eq!(again.note.actions, [INSTALL, LATER]);
}

#[tokio::test(start_paused = true)]
async fn a_host_without_dnf_logs_in_the_background_and_says_how_in_the_tray() {
    let mut r = rig(false);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(r.feed.checks(), 1);
    r.nothing_said();
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let s = r.tray_check().await;
    assert!(s.note.actions.is_empty(), "no Install that could only fail");
    assert!(
        s.note
            .body
            .starts_with("Kai 0.1.0+9 is available (you have 0.1.0+7).")
            && s.note.body.contains("not RPM-based")
            && s.note.body.contains("self-install is RPM-only"),
        "{}",
        s.note.body
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_download_or_install_says_why_deletes_the_file_and_offers_again() {
    let mut r = rig(true);
    *r.feed.download.lock().unwrap() = Err("sha256 mismatch: expected aa, got bb".into());
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.said().await;
    r.click(&offer.shown, "install");
    r.said().await; // Installing
    let failed = r.said().await;
    assert_eq!(failed.note.summary, "Kai's update failed");
    assert_eq!(
        failed.note.body,
        "Download failed: sha256 mismatch: expected aa, got bb"
    );
    assert!(!r.dest.exists());
    assert!(!r.feed.log().iter().any(|l| l.starts_with("install")));

    *r.feed.download.lock().unwrap() = Ok(());
    *r.feed.install.lock().unwrap() = Err("dnf exited with exit status: 1".into());
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.tray_check().await;
    assert_eq!(offer.note.actions, [INSTALL, LATER], "offered again");
    r.click(&offer.shown, "install");
    r.said().await;
    let failed = r.said().await;
    assert_eq!(
        failed.note.body,
        "Install failed: dnf exited with exit status: 1"
    );
    assert!(!r.dest.exists());
}

/// Installed, this process is still the old version: a check that finds what was installed says
/// so with Restart instead of offering it again; a newer one is offered.
#[tokio::test(start_paused = true)]
async fn after_the_install_the_same_version_is_not_offered_again() {
    let mut r = rig(true);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.said().await;
    r.click(&offer.shown, "install");
    r.said().await;
    r.said().await; // Updated to Kai 0.1.0+9

    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    tokio::time::sleep(Duration::from_secs(6 * 60 * 60 + 1)).await;
    assert_eq!(r.feed.checks(), 2);
    r.nothing_said();
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let s = r.tray_check().await;
    assert_eq!(s.note.summary, "Updated to Kai 0.1.0+9");
    assert_eq!(s.note.actions, [RESTART]);
    r.click(&s.shown, "restart");
    r.settle().await;
    assert!(matches!(r.cmds.try_recv(), Ok(Command::Restart)));

    r.feed.answer(Ok(Some(info("0.1.0+10", ""))));
    let newer = r.tray_check().await;
    assert_eq!(newer.note.summary, "Kai update available");
    assert_eq!(newer.note.actions, [INSTALL, LATER]);
}

/// A Restart once the core has gone (Kai quits) is only a log line.
#[tokio::test(start_paused = true)]
async fn a_restart_while_kai_quits_does_nothing() {
    let mut r = rig(true);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.said().await;
    r.click(&offer.shown, "install");
    r.said().await;
    let done = r.said().await;
    let (_, closed) = unbounded_channel::<Command>();
    r.cmds = closed;
    r.click(&done.shown, "restart");
    r.settle().await;
    // Still running: it answers the tray.
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let s = r.tray_check().await;
    assert_eq!(s.note.summary, "Updated to Kai 0.1.0+9");
}

/// The download goes to the fixed path in a private directory (0700, made when missing), and what
/// an install that was cut short left there goes first.
#[tokio::test(start_paused = true)]
async fn a_leftover_download_is_removed_first_and_the_directory_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let mut r = rig(true);
    std::fs::create_dir_all(r.dest.parent().unwrap()).unwrap();
    std::fs::write(&r.dest, b"half an rpm").unwrap();
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.said().await;
    r.click(&offer.shown, "install");
    r.said().await;
    assert_eq!(r.said().await.note.summary, "Updated to Kai 0.1.0+9");
    assert_eq!(
        r.feed.log()[1],
        "download https://registry.invalid/kai/0.1.0+9/kai.rpm (leftover: false)"
    );
    let mode = std::fs::metadata(r.dest.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(!r.dest.exists());
}

/// Installed, later outcomes (an error, a newer offer) are notifications of their own: the
/// installed update's notification keeps its Restart.
#[tokio::test(start_paused = true)]
async fn later_outcomes_keep_the_installed_updates_restart() {
    let mut r = rig_with(Ok(FakeFeed::new(true)), false);
    r.feed.answer(Ok(Some(info("0.1.0+9", ""))));
    let offer = r.tray_check().await;
    r.click(&offer.shown, "install");
    let installing = r.said().await;
    let done = r.said().await;
    assert_eq!(
        done.replaces,
        Some(installing.shown),
        "it takes the place of Installing"
    );

    r.feed.answer(Err("connection refused".into()));
    let e = r.tray_check().await;
    assert_ne!(e.replaces, Some(done.shown.clone()));
    r.feed.answer(Ok(Some(info("0.1.0+10", ""))));
    let newer = r.tray_check().await;
    assert_eq!(newer.note.actions, [INSTALL, LATER]);
    assert_ne!(newer.replaces, Some(done.shown.clone()));

    r.click(&done.shown, "restart");
    r.settle().await;
    assert!(matches!(r.cmds.try_recv(), Ok(Command::Restart)));
}

/// A minimal HTTP/1.1 server on loopback: each connection's request head goes to `heads`, and
/// `respond` gives the status line's code and the body for its path (for a 3xx, the body is the
/// `Location`). `None`: the connection is held open and never answered.
async fn serve(
    respond: impl Fn(&str) -> Option<(u16, Vec<u8>)> + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = heads.clone();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let seen = seen.clone();
            let respond = respond.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                seen.lock().unwrap().push(head);
                let Some((code, body)) = respond(&path) else {
                    std::future::pending::<()>().await;
                    return;
                };
                let (location, body) = if (300..400).contains(&code) {
                    let to = String::from_utf8(body).unwrap();
                    (format!("location: {to}\r\n"), Vec::new())
                } else {
                    (String::new(), body)
                };
                let reply = format!(
                    "HTTP/1.1 {code} X\r\n{location}content-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            });
        }
    });
    (base, heads)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The real feed through `lmgw-update`: the manifest and the RPM with the deploy token's header,
/// the version order (a `+BUILD` after its release), the download's hash checked and a mismatch's
/// file removed.
#[tokio::test]
async fn the_real_feed_checks_and_downloads_with_its_token() {
    let rpm = b"not really an rpm".to_vec();
    let good = sha256_hex(&rpm);
    let rpm_served = rpm.clone();
    let base_cell: Arc<Mutex<String>> = Arc::default();
    let base_for = base_cell.clone();
    let (base, heads) = serve(move |path| match path {
        "/latest.json" => {
            let base = base_for.lock().unwrap().clone();
            let json = format!(
                r#"{{"version":"0.1.0+9","notes":"n","pub_date":"","rpm":{{"file":"kai.rpm","url":"{base}/kai.rpm","sha256":"{good}"}}}}"#
            );
            Some((200, json.into_bytes()))
        }
        "/kai.rpm" => Some((200, rpm_served.clone())),
        _ => Some((404, Vec::new())),
    })
    .await;
    *base_cell.lock().unwrap() = base.clone();
    let feed = Feed {
        url: format!("{base}/latest.json"),
        auth: FeedAuth::DeployToken("gldt-test".into()),
    };
    let u = FeedUpdates::with(
        reqwest::Client::builder().no_proxy(),
        feed,
        Duration::from_secs(5),
    )
    .unwrap();

    assert!(
        u.check("0.1.0+9").await.unwrap().is_none(),
        "the same build"
    );
    assert!(
        u.check("0.1.1").await.unwrap().is_none(),
        "the next release"
    );
    let found = u.check("0.1.0").await.unwrap().expect("0.1.0+9 > 0.1.0");
    assert_eq!(found.manifest.version, "0.1.0+9");

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("kai-update.rpm");
    u.download(&found.manifest.rpm, &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), rpm);

    let mut bad = found.manifest.rpm.clone();
    bad.sha256 = "00".repeat(32);
    let e = u.download(&bad, &dest).await.unwrap_err();
    assert!(e.starts_with("sha256 mismatch"), "{e}");
    assert!(!dest.exists(), "a mismatch's file is removed");

    let heads = heads.lock().unwrap();
    assert!(
        heads
            .iter()
            .all(|h| h.to_ascii_lowercase().contains("deploy-token: gldt-test")),
        "{heads:?}"
    );
}

/// A feed that takes the connection and never answers is given up after `update.timeout_s`,
/// which the error names; a refused status says its code.
#[tokio::test]
async fn a_silent_feed_times_out_naming_the_setting() {
    let (base, _) = serve(|path| (path == "/missing.json").then(|| (404, Vec::new()))).await;
    let feed = |path: &str| Feed {
        url: format!("{base}{path}"),
        auth: FeedAuth::None,
    };
    let u = FeedUpdates::with(
        reqwest::Client::builder().no_proxy(),
        feed("/latest.json"),
        Duration::from_secs(1),
    )
    .unwrap();
    let e = u.check("0.1.0").await.unwrap_err();
    assert!(e.contains("no answer within 1 s (update.timeout_s)"), "{e}");

    let u = FeedUpdates::with(
        reqwest::Client::builder().no_proxy(),
        feed("/missing.json"),
        Duration::from_secs(1),
    )
    .unwrap();
    let e = u.check("0.1.0").await.unwrap_err();
    assert!(e.contains("404"), "{e}");
}

/// Redirects are followed within the feed's origin only: the token's headers would go along to
/// another origin, so that redirect is an error that says why, and the other origin never sees a
/// request.
#[tokio::test]
async fn redirects_stay_within_the_feeds_origin() {
    let (elsewhere, elsewhere_heads) = serve(|_| Some((200, b"{}".to_vec()))).await;
    let (base, heads) = serve(move |path| match path {
        "/moved.json" => Some((302, b"/latest.json".to_vec())),
        "/away.json" => Some((302, format!("{elsewhere}/latest.json").into_bytes())),
        "/latest.json" => Some((
            200,
            format!(
                r#"{{"version":"0.1.0+9","rpm":{{"file":"k.rpm","url":"https://x.example/k.rpm","sha256":"{}"}}}}"#,
                "0".repeat(64)
            )
            .into_bytes(),
        )),
        _ => Some((404, Vec::new())),
    })
    .await;
    let u = |path: &str| {
        FeedUpdates::with(
            reqwest::Client::builder().no_proxy(),
            Feed {
                url: format!("{base}{path}"),
                auth: FeedAuth::PrivateToken("glpat-secret".into()),
            },
            Duration::from_secs(5),
        )
        .unwrap()
    };
    let found = u("/moved.json").check("0.1.0").await.unwrap();
    assert_eq!(found.unwrap().manifest.version, "0.1.0+9");
    let e = u("/away.json").check("0.1.0").await.unwrap_err();
    assert!(e.contains("another origin"), "{e}");
    assert!(elsewhere_heads.lock().unwrap().is_empty(), "never asked");
    assert_eq!(heads.lock().unwrap().len(), 3);
}

/// A feed without a token follows a redirect to another origin, as the public feed needs (GitHub's
/// `releases/latest/download/latest.json` redirects to its asset host); still within the bound.
#[tokio::test]
async fn a_feed_without_a_token_follows_redirects_to_another_origin() {
    let (assets, assets_heads) = serve(|path| match path {
        "/latest.json" => Some((
            200,
            format!(
                r#"{{"version":"0.1.1","rpm":{{"file":"k.rpm","url":"https://x.example/k.rpm","sha256":"{}"}}}}"#,
                "0".repeat(64)
            )
            .into_bytes(),
        )),
        _ => Some((404, Vec::new())),
    })
    .await;
    let (base, _) = serve(move |path| match path {
        "/releases/latest/download/latest.json" => {
            Some((302, format!("{assets}/latest.json").into_bytes()))
        }
        "/loop.json" => Some((302, b"/loop.json".to_vec())),
        _ => Some((404, Vec::new())),
    })
    .await;
    let u = |path: &str| {
        FeedUpdates::with(
            reqwest::Client::builder().no_proxy(),
            Feed {
                url: format!("{base}{path}"),
                auth: FeedAuth::None,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    };
    let found = u("/releases/latest/download/latest.json")
        .check("0.1.0")
        .await
        .unwrap();
    assert_eq!(found.unwrap().manifest.version, "0.1.1");
    assert_eq!(assets_heads.lock().unwrap().len(), 1);
    let e = u("/loop.json").check("0.1.0").await.unwrap_err();
    assert!(e.contains("more than"), "{e}");
}

/// End to end against the loopback server: a public build with `KAI_REGISTRY_TOKEN` in the
/// environment sends no token header at all; a private endpoint with it sends `PRIVATE-TOKEN`; a
/// manifest without a sha256 is refused by the check, before any download.
#[tokio::test]
async fn the_wire_carries_a_token_only_to_a_private_endpoint() {
    let (base, heads) = serve(|path| match path {
        "/latest.json" => Some((
            200,
            format!(
                r#"{{"version":"0.1.1","rpm":{{"file":"k.rpm","url":"https://x.example/k.rpm","sha256":"{}"}}}}"#,
                "0".repeat(64)
            )
            .into_bytes(),
        )),
        "/nosha.json" => Some((
            200,
            br#"{"version":"0.1.1","rpm":{"file":"k.rpm","url":"https://x.example/k.rpm"}}"#
                .to_vec(),
        )),
        _ => Some((404, Vec::new())),
    })
    .await;
    let url = format!("{base}/latest.json");
    let updates = |r: Resolved| {
        FeedUpdates::with(
            reqwest::Client::builder().no_proxy(),
            r.feed,
            Duration::from_secs(5),
        )
        .unwrap()
    };
    let public = BuildFeed {
        url: Some(&url),
        deploy_token: None,
        public: true,
    };
    let r = resolve_feed(&public, None, Some("glpat-mine")).unwrap();
    assert!(updates(r).check("0.1.0").await.unwrap().is_some());
    let r = resolve_feed(&built(None, None), Some(&url), Some("glpat-mine")).unwrap();
    assert!(updates(r).check("0.1.0").await.unwrap().is_some());
    {
        let heads: Vec<String> = heads
            .lock()
            .unwrap()
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect();
        assert_eq!(heads.len(), 2, "{heads:?}");
        assert!(!heads[0].contains("private-token"), "{}", heads[0]);
        assert!(!heads[0].contains("glpat-mine"), "{}", heads[0]);
        assert!(
            heads[1].contains("private-token: glpat-mine"),
            "{}",
            heads[1]
        );
    }

    let r = resolve_feed(
        &built(None, None),
        Some(&format!("{base}/nosha.json")),
        None,
    )
    .unwrap();
    let e = updates(r).check("0.1.0").await.unwrap_err();
    assert!(e.contains("no sha256"), "{e}");
}
