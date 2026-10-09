//! Kai's self-updater (§13, the owner's decision 2026-10-08): Kai polls the manifest its CI
//! publishes with every build (`latest.json` in its own package registry), compares it to the
//! running build's version ([`crate::VERSION`]) and, on a newer one, offers it in a notification:
//! Install downloads the RPM to a fixed path, hands it to `pkexec dnf install` and deletes it, and
//! the notification that says it is installed offers Restart. The mechanism (the manifest, the
//! version order, the sha256-checked download, the install step) is lmgw's `lmgw-update` crate,
//! shared with lmgw's own updater; the feed and every word shown here are Kai's.
//!
//! **The feed** ([`resolve_feed`]): `KAI_UPDATE_ENDPOINT` at run time, else
//! `KAI_UPDATE_MANIFEST_URL` compiled in (a CI build bakes its feed's `latest/latest.json`,
//! `packaging/ci-build.sh`; a GitHub release bakes the public
//! `https://github.com/lmgw-dev/kai/releases/latest/download/latest.json`, `packaging/gh-build.sh`).
//! There is no built-in URL: a build without either has no feed, says so once in the log and never
//! polls; the tray's "Check for Updates…" says "this build has no update feed". Auth, each token
//! bound to the feed it belongs to: `KAI_REGISTRY_TOKEN` (a user's token, `PRIVATE-TOKEN`) only
//! together with `KAI_UPDATE_ENDPOINT`, the feed the same user chose; `KAI_UPDATE_DEPLOY_TOKEN`
//! compiled in (a read-only deploy token, `Deploy-Token`) only with the compiled-in feed it was
//! built for. Neither ever goes to the public feed: a build marked public
//! (`KAI_UPDATE_PUBLIC_FEED`, `packaging/gh-build.sh`) never sends its compiled-in feed a token, and
//! no token goes to a feed on GitHub's hosts ([`is_public_host`]), whichever variable named it. A
//! token that is set but not sent says why in the log. A feed with a token follows redirects within
//! its origin only, so the token's headers never reach another host; a feed without one follows
//! them anywhere (the public feed is a redirect from github.com to GitHub's asset host), but never
//! from https down to http.
//!
//! **What a manifest must say** ([`vet_rpm`]): the RPM's sha256 as 64 hex digits and an https URL;
//! anything else is refused before a download starts, so an RPM is never installed unchecked.
//!
//! **Who is told what.** The background check (`update.background_check`, first after
//! `update.first_check_s`, then every `update.interval_s`) only logs "up to date" and errors, and
//! notifies only an update it can install. The tray's check always answers: up to date, the error,
//! or the update. On a host without `pkexec` and `dnf` (`lmgw_update::can_self_install`) the
//! background check only logs a newer version, and the tray's says how to update instead of
//! offering Install.
//!
//! **Restart** runs Kai's quit sequence first ([`Command::Restart`]: exactly what the tray's Quit
//! does), then `main` execs the installed binary in place, so the new Kai keeps the app scope.

#[cfg(test)]
mod tests;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use lmgw_update::{Feed, FeedAuth, RpmArtifact, UpdateInfo, is_newer};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config::UpdateConfig;
use crate::core::Command;
use crate::notify::{Action, Invoked, Shown};

/// The run-time override of the feed's URL.
pub const ENDPOINT_VAR: &str = "KAI_UPDATE_ENDPOINT";
/// A user's token for a private feed, at run time (sent as `PRIVATE-TOKEN`).
pub const TOKEN_VAR: &str = "KAI_REGISTRY_TOKEN";

/// The feed compiled in (`KAI_UPDATE_MANIFEST_URL`), from CI only. No default: a build without it
/// has no feed.
const BUILD_MANIFEST_URL: Option<&str> = match option_env!("KAI_UPDATE_MANIFEST_URL") {
    Some(url) if !url.is_empty() => Some(url),
    _ => None,
};

/// A read-only deploy token for the compiled-in feed (`KAI_UPDATE_DEPLOY_TOKEN`, a masked CI
/// variable, never in the source), sent as `Deploy-Token`, to that feed only.
const BUILD_DEPLOY_TOKEN: Option<&str> = match option_env!("KAI_UPDATE_DEPLOY_TOKEN") {
    Some(token) if !token.is_empty() => Some(token),
    _ => None,
};

/// The public build's marker (`KAI_UPDATE_PUBLIC_FEED`, set by `packaging/gh-build.sh`): its
/// compiled-in feed is public and never gets a token, whatever else was compiled in.
const BUILD_PUBLIC_FEED: bool = match option_env!("KAI_UPDATE_PUBLIC_FEED") {
    Some(v) => !v.is_empty(),
    None => false,
};

/// What this build was compiled with for its feed.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildFeed<'a> {
    /// `KAI_UPDATE_MANIFEST_URL`.
    pub url: Option<&'a str>,
    /// `KAI_UPDATE_DEPLOY_TOKEN`.
    pub deploy_token: Option<&'a str>,
    /// `KAI_UPDATE_PUBLIC_FEED`: the compiled-in feed is the public one.
    pub public: bool,
}

/// This build's values.
pub const BUILD_FEED: BuildFeed<'static> = BuildFeed {
    url: BUILD_MANIFEST_URL,
    deploy_token: BUILD_DEPLOY_TOKEN,
    public: BUILD_PUBLIC_FEED,
};

/// The fixed download path's file name, under Kai's runtime directory (`$XDG_RUNTIME_DIR/kai`):
/// never one from the manifest.
const RPM_FILE: &str = "kai-update.rpm";

/// The redirects one request may follow (all within the feed's origin); more is an error that says
/// so. reqwest's own default is the same number.
const MAX_REDIRECTS: usize = 10;

/// The notifications' actions.
const INSTALL: Action = ("install", "Install");
const LATER: Action = ("later", "Later");
const RESTART: Action = ("restart", "Restart");

/// The feed this process polls, where its URL came from (for the log line), and why a token that
/// is set is not sent (said in the log, so a dropped token is never a silent 401).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub feed: Feed,
    pub from: &'static str,
    pub unused_token: Option<&'static str>,
}

/// A host of the public feed: github.com and GitHub's asset hosts. No token is ever sent there.
/// An unparseable URL is not one (and `lmgw-update` sends no token to it either).
pub fn is_public_host(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ["github.com", "githubusercontent.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

const USER_TOKEN_NEEDS_ENDPOINT: &str = "KAI_REGISTRY_TOKEN is set but not sent: it goes only to     a feed named by KAI_UPDATE_ENDPOINT, never to the compiled-in one";
const NO_TOKEN_TO_PUBLIC_FEED: &str =
    "a token is set but not sent: the feed is the public one     (GitHub), which needs none";

/// The feed from the run-time override `endpoint` (`KAI_UPDATE_ENDPOINT`) or the compiled-in
/// `build.url`; `None`: neither URL, so no feed. Blank values count as unset. The run-time
/// `user_token` (`KAI_REGISTRY_TOKEN`) goes with `endpoint` only, the compiled-in deploy token with
/// the compiled-in URL only, and no token with a public feed (`build.public` for the compiled-in
/// one, [`is_public_host`] for either).
pub fn resolve_feed(
    build: &BuildFeed<'_>,
    endpoint: Option<&str>,
    user_token: Option<&str>,
) -> Option<Resolved> {
    fn set(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    let user_token = set(user_token);
    let (url, from, auth, unused_token) = if let Some(url) = set(endpoint) {
        match user_token {
            Some(_) if is_public_host(url) => (
                url,
                ENDPOINT_VAR,
                FeedAuth::None,
                Some(NO_TOKEN_TO_PUBLIC_FEED),
            ),
            Some(token) => (
                url,
                ENDPOINT_VAR,
                FeedAuth::PrivateToken(token.to_owned()),
                None,
            ),
            None => (url, ENDPOINT_VAR, FeedAuth::None, None),
        }
    } else {
        let url = set(build.url)?;
        let public = build.public || is_public_host(url);
        match set(build.deploy_token) {
            Some(_) if public => (
                url,
                "compiled in",
                FeedAuth::None,
                Some(NO_TOKEN_TO_PUBLIC_FEED),
            ),
            Some(token) => (
                url,
                "compiled in",
                FeedAuth::DeployToken(token.to_owned()),
                user_token.map(|_| USER_TOKEN_NEEDS_ENDPOINT),
            ),
            None => (
                url,
                "compiled in",
                FeedAuth::None,
                user_token.map(|_| USER_TOKEN_NEEDS_ENDPOINT),
            ),
        }
    };
    Some(Resolved {
        feed: Feed {
            url: url.to_owned(),
            auth,
        },
        from,
        unused_token,
    })
}

/// [`resolve_feed`] with this build's values and this process's environment.
pub fn feed_from_env() -> Option<Resolved> {
    let var = |k: &str| std::env::var(k).ok();
    resolve_feed(
        &BUILD_FEED,
        var(ENDPOINT_VAR).as_deref(),
        var(TOKEN_VAR).as_deref(),
    )
}

/// The RPM a manifest names, vetted before anything is downloaded: its sha256 is 64 hex digits (so
/// `lmgw-update`, which skips the check for an empty one, always checks it) and its URL is https.
/// `loopback_http`: an http URL to a loopback host passes too (the tests' local server only).
pub fn vet_rpm(rpm: &RpmArtifact, loopback_http: bool) -> Result<(), String> {
    let sha = rpm.sha256.trim();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(if sha.is_empty() {
            "the feed's manifest gives no sha256 for the RPM; Kai installs only a checked download"
                .to_owned()
        } else {
            format!(
                "the feed's manifest gives \"{sha}\" as the RPM's sha256, not 64 hex digits; Kai \
                 installs only a checked download"
            )
        });
    }
    let url = reqwest::Url::parse(&rpm.url)
        .map_err(|e| format!("the feed's manifest gives an RPM URL that is not one ({e})"))?;
    let loopback = || match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback_http && loopback() => Ok(()),
        scheme => Err(format!(
            "the feed's manifest gives the RPM's URL with {scheme}://, not https://; Kai \
             downloads updates over https only"
        )),
    }
}

/// [`vet_rpm`]'s `loopback_http` for the real feed: only the tests serve over plain http.
const LOOPBACK_HTTP: bool = cfg!(test);

/// Why this process cannot check, said by the tray's check.
pub const NO_FEED: &str = "this build has no update feed (it was built without \
    KAI_UPDATE_MANIFEST_URL, and KAI_UPDATE_ENDPOINT is not set)";

/// The feed's side of an update, behind a seam for the tests.
pub trait Updates: Send + Sync + 'static {
    /// The newer build than `current`, or `None` when up to date.
    fn check(
        &self,
        current: &str,
    ) -> impl Future<Output = Result<Option<UpdateInfo>, String>> + Send;
    /// The RPM to `dest`, sha256-checked.
    fn download(
        &self,
        rpm: &RpmArtifact,
        dest: &Path,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// `pkexec dnf install -y <rpm>`; blocks until dnf is done (the password prompt included).
    fn install(&self, rpm: &Path) -> Result<(), String>;
    /// This host has `pkexec` and `dnf`.
    fn can_self_install(&self) -> bool;
}

/// The real feed: `lmgw-update` over an HTTP client of its own.
pub struct FeedUpdates {
    http: reqwest::Client,
    feed: Feed,
    timeout: Duration,
}

impl FeedUpdates {
    /// `timeout` (`update.timeout_s`) bounds the connect and each read, not a download as a whole.
    /// The system's proxy settings apply: the feed is a registry, not the gateway. A feed with a
    /// token follows redirects only within the origin they start from ([`same_origin_redirects`]),
    /// one without follows them to any origin ([`any_origin_redirects`]).
    pub fn new(feed: Feed, timeout: Duration) -> Result<Self, String> {
        Self::with(reqwest::Client::builder(), feed, timeout)
    }

    fn with(
        builder: reqwest::ClientBuilder,
        feed: Feed,
        timeout: Duration,
    ) -> Result<Self, String> {
        let redirects = match feed.auth {
            FeedAuth::None => any_origin_redirects(),
            FeedAuth::PrivateToken(_) | FeedAuth::DeployToken(_) => same_origin_redirects(),
        };
        let http = builder
            .redirect(redirects)
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .build()
            .map_err(|e| {
                format!(
                    "cannot build the HTTP client: {}",
                    crate::lmgw_link::describe(&e)
                )
            })?;
        Ok(Self {
            http,
            feed,
            timeout,
        })
    }

    fn explain(&self, e: lmgw_update::Error) -> String {
        explain(&e, self.timeout)
    }
}

/// Redirects within the request's origin (scheme, host and port) only. The feed's token travels
/// in `PRIVATE-TOKEN` or `Deploy-Token`, which reqwest does not strip when a redirect leaves the
/// host (it strips only the standard auth headers), so a redirect to another origin is stopped with
/// an error that says why. A token-protected feed is expected not to redirect.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        match redirect_verdict(attempt.previous(), attempt.url(), true) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    })
}

/// Redirects to any origin, for a feed without a token: nothing secret goes along. The public
/// feed needs it: `releases/latest/download/latest.json` on github.com redirects to the release's
/// asset, which GitHub serves from another host. Never from https down to http.
fn any_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        match redirect_verdict(attempt.previous(), attempt.url(), false) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    })
}

/// Whether a redirect to `next` after the requests to `previous` (the first is the feed's own
/// request) is followed: never from https down to http, at most [`MAX_REDIRECTS`] times, and with
/// `same_origin` only within the origin the requests started from.
fn redirect_verdict(
    previous: &[reqwest::Url],
    next: &reqwest::Url,
    same_origin: bool,
) -> Result<(), String> {
    let from = previous.first().map(|u| u.origin());
    if same_origin && from.as_ref() != Some(&next.origin()) {
        let to = next.origin().ascii_serialization();
        let from = from.map_or_else(String::new, |o| o.ascii_serialization());
        return Err(format!(
            "the feed redirected from {from} to {to}, another origin: Kai follows redirects only \
             within the feed's origin, so its token never goes elsewhere"
        ));
    }
    if next.scheme() != "https" && previous.iter().any(|u| u.scheme() == "https") {
        return Err(format!(
            "the feed redirected from https to {}://{}: Kai never follows a redirect down from \
             https",
            next.scheme(),
            next.host_str().unwrap_or("")
        ));
    }
    if previous.len() > MAX_REDIRECTS {
        return Err(format!(
            "the feed redirected more than {MAX_REDIRECTS} times; Kai stops there"
        ));
    }
    Ok(())
}

/// `e` as the crate words it, with the cause underneath a request's failure and the setting a
/// timeout comes from.
fn explain(e: &lmgw_update::Error, timeout: Duration) -> String {
    match e {
        lmgw_update::Error::Http(re) if re.is_timeout() => format!(
            "{re}: no answer within {} s (update.timeout_s)",
            timeout.as_secs()
        ),
        lmgw_update::Error::Http(re) if re.status().is_none() => {
            let cause = crate::lmgw_link::describe(re);
            if cause == re.to_string() {
                cause
            } else {
                format!("{re}: {cause}")
            }
        }
        e => e.to_string(),
    }
}

impl Updates for FeedUpdates {
    async fn check(&self, current: &str) -> Result<Option<UpdateInfo>, String> {
        let found = lmgw_update::check(&self.http, &self.feed, current)
            .await
            .map_err(|e| self.explain(e))?;
        if let Some(info) = &found {
            vet_rpm(&info.manifest.rpm, LOOPBACK_HTTP)
                .map_err(|e| format!("Kai {}: {e}", info.manifest.version))?;
        }
        Ok(found)
    }

    async fn download(&self, rpm: &RpmArtifact, dest: &Path) -> Result<(), String> {
        vet_rpm(rpm, LOOPBACK_HTTP)?;
        lmgw_update::download_rpm(&self.http, &self.feed, rpm, dest)
            .await
            .map_err(|e| self.explain(e))
    }

    fn install(&self, rpm: &Path) -> Result<(), String> {
        lmgw_update::install_rpm(rpm).map_err(|e| e.to_string())
    }

    fn can_self_install(&self) -> bool {
        lmgw_update::can_self_install()
    }
}

/// A notification the updater shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub summary: String,
    pub body: String,
    pub actions: &'static [Action],
}

/// The desktop's side, behind a seam for the tests.
pub trait Desk: Send + Sync + 'static {
    /// Shows `note`, in the place of `replaces` when that is still shown; what was shown, if the
    /// server took it.
    fn show(
        &self,
        note: Note,
        replaces: Option<Shown>,
    ) -> impl Future<Output = Option<Shown>> + Send;
}

/// The desktop's notification server.
pub struct BusDesk {
    pub bus: zbus::Connection,
    /// `dbus.timeout_s`.
    pub timeout: Duration,
}

impl Desk for BusDesk {
    async fn show(&self, note: Note, replaces: Option<Shown>) -> Option<Shown> {
        crate::notify::notify(
            &self.bus,
            &note.summary,
            &note.body,
            note.actions,
            replaces.as_ref(),
            self.timeout,
        )
        .await
    }
}

/// What the updater takes from the core: the tray's check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    CheckNow,
}

/// Who asked for a check: the background's is quiet, the tray's always answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Background,
    Tray,
}

/// The updater's state.
#[derive(Debug)]
enum Stage {
    Idle,
    /// The notification with Install and Later shows `info`.
    Offered {
        info: UpdateInfo,
        note: Shown,
    },
    /// The download and the install (its password prompt included) run.
    Installing {
        version: String,
    },
}

/// An update that is installed while this process is still the old version: its notification
/// offers Restart. Kept apart from [`Stage`], so a later offer, check or failure neither replaces
/// that notification nor loses its Restart.
#[derive(Debug)]
struct Installed {
    version: String,
    note: Option<Shown>,
}

/// The settings the updater runs with (`[update]`).
#[derive(Debug, Clone)]
pub struct UpdateSettings {
    pub background_check: bool,
    pub first_check: Duration,
    pub interval: Duration,
    /// Where the RPM is downloaded to: a fixed path in Kai's own runtime directory (the user's,
    /// 0700; not the shared temp directory), never one from the manifest.
    pub dest: PathBuf,
}

impl UpdateSettings {
    /// `runtime_dir`: Kai's runtime directory (`Paths::runtime_dir`, `$XDG_RUNTIME_DIR/kai`).
    pub fn from_config(c: &UpdateConfig, runtime_dir: &Path) -> Self {
        Self {
            background_check: c.background_check,
            first_check: c.first_check(),
            interval: c.interval(),
            dest: runtime_dir.join(RPM_FILE),
        }
    }
}

type Pending<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A check on its way.
type Checking = Pending<Result<Option<UpdateInfo>, String>>;

/// The step of an install that failed, and why.
type Failed = (&'static str, String);

pub struct Updater<U, D> {
    /// The feed's side, or why this process has none.
    updates: Result<Arc<U>, String>,
    desk: D,
    settings: UpdateSettings,
    current: String,
    /// The core's commands: Restart.
    cmds: mpsc::UnboundedSender<Command>,
    stage: Stage,
    installed: Option<Installed>,
    /// The last notification of an outcome, which the next one takes the place of (never the
    /// installed update's, [`Installed`]).
    said: Option<Shown>,
}

impl<U: Updates, D: Desk> Updater<U, D> {
    pub fn new(
        updates: Result<U, String>,
        desk: D,
        settings: UpdateSettings,
        current: &str,
        cmds: mpsc::UnboundedSender<Command>,
    ) -> Self {
        Self {
            updates: updates.map(Arc::new),
            desk,
            settings,
            current: current.to_owned(),
            cmds,
            stage: Stage::Idle,
            installed: None,
            said: None,
        }
    }

    /// Runs until `requests` closes (the core quit).
    pub async fn run(
        mut self,
        mut requests: mpsc::UnboundedReceiver<Request>,
        mut actions: mpsc::UnboundedReceiver<Invoked>,
    ) {
        let mut next = match &self.updates {
            Err(why) => {
                tracing::info!("update: {why}; Kai does not look for updates");
                None
            }
            Ok(_) if !self.settings.background_check => {
                tracing::info!(
                    "update: update.background_check is off; Kai looks for updates only from the \
                     tray's \"Check for Updates…\""
                );
                None
            }
            Ok(_) => Some(Instant::now() + self.settings.first_check),
        };
        let mut check: Option<(Origin, Checking)> = None;
        let mut install: Option<Pending<Result<(), Failed>>> = None;
        loop {
            tokio::select! {
                req = requests.recv() => match req {
                    None => break,
                    Some(Request::CheckNow) => match (&mut check, &self.updates) {
                        // The check that runs answers the tray.
                        (Some((origin, _)), _) => *origin = Origin::Tray,
                        (None, Ok(u)) => check = Some((Origin::Tray, self.checking(u.clone()))),
                        (None, Err(why)) => {
                            let why = why.clone();
                            self.say("Kai cannot check for updates", why, &[]).await;
                        }
                    },
                },
                Some(action) = actions.recv() => {
                    if let Some(fut) = self.on_action(action).await {
                        install = Some(fut);
                    }
                }
                () = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                    next = Some(Instant::now() + self.settings.interval);
                    if check.is_none()
                        && let Ok(u) = &self.updates
                    {
                        check = Some((Origin::Background, self.checking(u.clone())));
                    }
                }
                result = async {
                    match check.as_mut() {
                        Some((_, f)) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let origin = check.take().map_or(Origin::Background, |(o, _)| o);
                    self.on_checked(origin, result).await;
                }
                result = async {
                    match install.as_mut() {
                        Some(f) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    install = None;
                    self.on_installed(result).await;
                }
            }
        }
    }

    fn checking(&self, u: Arc<U>) -> Checking {
        let current = self.current.clone();
        Box::pin(async move { u.check(&current).await })
    }

    async fn on_checked(&mut self, origin: Origin, result: Result<Option<UpdateInfo>, String>) {
        let tray = origin == Origin::Tray;
        match result {
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                if tray {
                    self.say("Kai couldn't check for updates", e, &[]).await;
                }
            }
            Ok(None) => {
                tracing::info!("update check: up to date ({})", self.current);
                if tray {
                    let body = format!("Kai {} is the latest version.", self.current);
                    self.say("Kai is up to date", body, &[]).await;
                }
            }
            Ok(Some(info)) => self.found(origin, info).await,
        }
    }

    async fn found(&mut self, origin: Origin, info: UpdateInfo) {
        let tray = origin == Origin::Tray;
        let v = info.manifest.version.clone();
        if let Stage::Installing { version } = &self.stage {
            tracing::info!("update check: Kai {v} is available; Kai {version} is being installed");
            if tray {
                let body = format!("Kai {version} is being installed.");
                self.say("Kai is installing an update", body, &[]).await;
            }
            return;
        }
        if let Some(installed) = &self.installed
            && !is_newer(&v, &installed.version)
        {
            let version = installed.version.clone();
            tracing::info!(
                "update check: Kai {version} is installed; Kai restarts into it on Restart"
            );
            if tray {
                self.say_installed(version).await;
            }
            return;
        }
        let can_install = self.updates.as_ref().is_ok_and(|u| u.can_self_install());
        let notes = info.manifest.notes.trim();
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("\n\n{notes}")
        };
        if !can_install {
            // A host that cannot install what it found gets a log line from the background check,
            // not a notification whose only button would fail.
            tracing::info!(
                "Kai {v} is available (running {}), but this host is not RPM-based: self-install \
                 is RPM-only; update Kai the way it was installed",
                self.current
            );
            if tray {
                let body = format!(
                    "Kai {v} is available (you have {}).{notes}\n\nThis host is not RPM-based, so \
                     Kai can't install it for you: self-install is RPM-only. Update it the way you \
                     installed it, e.g. rebuild from source (git pull, then cargo build --release).",
                    self.current
                );
                self.say("Kai update available", body, &[]).await;
            }
            return;
        }
        tracing::info!("update: Kai {v} is available (running {})", self.current);
        let body = format!(
            "Kai {v} is available (you have {}).{notes}\n\nInstall downloads it and asks for your \
             password to install the package.",
            self.current
        );
        if let Some(note) = self
            .say("Kai update available", body, &[INSTALL, LATER])
            .await
        {
            self.stage = Stage::Offered { info, note };
        }
    }

    /// The installed version's notification, with Restart, in the place of an earlier one of its
    /// own; the next outcome does not take its place.
    async fn say_installed(&mut self, version: String) {
        let note = Note {
            summary: format!("Updated to Kai {version}"),
            body: "Restart Kai to use the new version.".to_owned(),
            actions: &[RESTART],
        };
        // In the place of its own earlier one, else of the outcome before it ("Installing").
        let earlier = self
            .installed
            .take()
            .and_then(|i| i.note)
            .or_else(|| self.said.take());
        let note = self.desk.show(note, earlier).await;
        self.installed = Some(Installed { version, note });
    }

    /// An action of one of the updater's notifications: Install starts the install (its future),
    /// Restart asks the core for it. Any other notification's, or an older one's, is ignored.
    async fn on_action(&mut self, action: Invoked) -> Option<Pending<Result<(), Failed>>> {
        if let Some(Installed {
            version,
            note: Some(note),
        }) = &self.installed
            && action.of(note)
        {
            if action.key == RESTART.0 {
                tracing::info!("update: restarting into Kai {version}");
                if self.cmds.send(Command::Restart).is_err() {
                    tracing::warn!(
                        "update: Kai is already quitting, so it ends without the restart; start \
                         it again to run Kai {version}"
                    );
                }
            }
            return None;
        }
        match &self.stage {
            Stage::Offered { info, note } if action.of(note) => {
                let info = info.clone();
                match action.key.as_str() {
                    k if k == INSTALL.0 => return self.install(info).await,
                    k if k == LATER.0 => {
                        tracing::info!(
                            "update: Kai {} later (offered again at the next check)",
                            info.manifest.version
                        );
                        self.stage = Stage::Idle;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// Download, install, delete: the future that does it.
    async fn install(&mut self, info: UpdateInfo) -> Option<Pending<Result<(), Failed>>> {
        let Ok(u) = self.updates.clone() else {
            return None;
        };
        let version = info.manifest.version.clone();
        tracing::info!("update: installing Kai {version}");
        self.stage = Stage::Installing {
            version: version.clone(),
        };
        self.say(
            format!("Installing Kai {version}"),
            "Kai downloads the update, then asks for your password to install the package."
                .to_owned(),
            &[],
        )
        .await;
        let dest = self.settings.dest.clone();
        let rpm = info.manifest.rpm;
        Some(Box::pin(async move {
            // What an install cut short left (Kai quit mid-download) goes first.
            if let Err(e) = fresh_download_path(&dest).await {
                return Err(("Download", e));
            }
            if let Err(e) = u.download(&rpm, &dest).await {
                let _ = tokio::fs::remove_file(&dest).await;
                return Err(("Download", e));
            }
            let path = dest.clone();
            let installer = u.clone();
            let installed = tokio::task::spawn_blocking(move || installer.install(&path))
                .await
                .unwrap_or_else(|e| Err(format!("the install task failed: {e}")));
            let _ = tokio::fs::remove_file(&dest).await;
            installed.map_err(|e| ("Install", e))
        }))
    }

    async fn on_installed(&mut self, result: Result<(), Failed>) {
        let Stage::Installing { version } = std::mem::replace(&mut self.stage, Stage::Idle) else {
            return;
        };
        match result {
            Ok(()) => {
                tracing::info!("update: Kai {version} is installed");
                self.say_installed(version).await;
            }
            Err((step, e)) => {
                tracing::error!(
                    "update: {} of Kai {version} failed: {e}",
                    step.to_lowercase()
                );
                self.say("Kai's update failed", format!("{step} failed: {e}"), &[])
                    .await;
            }
        }
    }

    /// Shows a notification in the place of the updater's last one.
    async fn say(
        &mut self,
        summary: impl Into<String>,
        body: String,
        actions: &'static [Action],
    ) -> Option<Shown> {
        let note = Note {
            summary: summary.into(),
            body,
            actions,
        };
        let shown = self.desk.show(note, self.said.take()).await;
        self.said = shown.clone();
        shown
    }
}

/// Makes `dest`'s directory (0700, the user's) and removes a file a download or install that was
/// cut short left there.
async fn fresh_download_path(dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|e| format!("cannot make {} private: {e}", dir.display()))?;
    }
    match tokio::fs::remove_file(dest).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove the old {}: {e}", dest.display())),
    }
}

/// The updater on the real feed and desktop, with the tray's checks from `requests`, until the
/// core quits. Its notifications' buttons come through [`crate::notify::actions`]. `runtime_dir`
/// holds the download (`Paths::runtime_dir`).
pub async fn run(
    config: UpdateConfig,
    runtime_dir: PathBuf,
    bus: zbus::Connection,
    dbus_timeout: Duration,
    cmds: mpsc::UnboundedSender<Command>,
    requests: mpsc::UnboundedReceiver<Request>,
) {
    let updates = match feed_from_env() {
        None => Err(NO_FEED.to_owned()),
        Some(Resolved {
            feed,
            from,
            unused_token,
        }) => {
            tracing::info!(url = %feed.url, auth = ?feed.auth, "update feed ({from})");
            if let Some(why) = unused_token {
                tracing::warn!("update: {why}");
            }
            FeedUpdates::new(feed, config.timeout())
        }
    };
    let (actions_tx, actions_rx) = mpsc::unbounded_channel();
    let watch = updates.is_ok().then(|| {
        tokio::spawn(crate::notify::actions(
            bus.clone(),
            actions_tx,
            dbus_timeout,
        ))
    });
    let desk = BusDesk {
        bus,
        timeout: dbus_timeout,
    };
    let settings = UpdateSettings::from_config(&config, &runtime_dir);
    // A download an earlier Kai left (it quit or was killed mid-install) is of no use.
    match tokio::fs::remove_file(&settings.dest).await {
        Ok(()) => tracing::info!(
            "update: removed {}, left by an install that was cut short",
            settings.dest.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("update: cannot remove {}: {e}", settings.dest.display()),
    }
    Updater::new(updates, desk, settings, crate::VERSION, cmds)
        .run(requests, actions_rx)
        .await;
    if let Some(w) = watch {
        w.abort();
    }
}
