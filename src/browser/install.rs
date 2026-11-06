//! "Install Kai for Firefox" in the settings window's Firefox section (kai-firefox design §10, Kai's
//! side): where the signed extension is, whether one is linked to Kai now, and the click that opens
//! it in the browser.
//!
//! **The link is the build's.** Firefox installs signed XPIs only, and where the signed file lives
//! depends on the channel Kai was built for: a private channel's XPI URL needs a deploy token
//! (it is a link with the token in it), a public GitHub release's is open. So the link is decided at
//! compile time from `KAI_FIREFOX_INSTALL_URL`, which Kai's own CI sets per channel, as it sets the
//! updater's feed (`crate::update`); a build without it has no link, and the section says how to
//! install by hand (about:addons, Install Add-on From File). Never in the source.
//!
//! **The link never leaves the core.** It may carry a token, so the page learns only whether the
//! build has one ([`Facts::install`]), the log names the host and nothing else, and no answer or
//! error repeats it. The page's click asks the core to open it: `xdg-open`, as Kai opens
//! lmgw's dashboard, so the desktop starts the browser in its own app scope (Kai takes the link
//! only from a Firefox in its own systemd unit, `browser.peer_scopes`); the default browser must be
//! Firefox for Firefox to offer the installation, which the page says.
//!
//! **Connected now** is the browser server's link count (`Command::BrowserLinked`): one per Firefox
//! profile whose extension said hello and was welcomed.

use serde::Serialize;

/// The build variable (a CI/CD variable of Kai's project, read at compile time): the signed XPI's
/// URL for this build's channel.
pub const URL_VAR: &str = "KAI_FIREFOX_INSTALL_URL";

/// The link compiled in, from CI only. No default: a build without it has no link.
const BUILD_URL: Option<&str> = match option_env!("KAI_FIREFOX_INSTALL_URL") {
    Some(url) if !url.is_empty() => Some(url),
    _ => None,
};

/// This build's link, as compiled in.
pub fn build_url() -> Option<&'static str> {
    BUILD_URL
}

/// What the section shows (`GET /api/browser`), and after a click (`POST /api/browser/install`)
/// that it opened the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Facts {
    /// This build has a link that reads as one Firefox can install from. Never the link itself.
    pub install: bool,
    /// Why the build's link is of no use (it does not read as a web address Firefox installs
    /// from), when it is set but unusable; the rule only, never the link.
    pub problem: Option<String>,
    /// How many Firefox extensions are linked to Kai now.
    pub linked: usize,
    /// `None` for a look; after a click, whether the link was handed to the desktop.
    pub opened: Option<bool>,
}

/// The link as the click hands it to `xdg-open`: an `https` URL with a host (Firefox installs an
/// add-on only from a secure origin), or an `http` one to the machine itself (Firefox counts it
/// secure; for a try with a local server). User information is allowed: a private channel's link
/// carries its deploy token there. The error is the rule, without the link.
pub fn parse(link: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(link.trim()).map_err(|e| format!("it is not a web address ({e})"))?;
    // An http or https URL always has a host: Url::parse refuses an empty one.
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&url) => Ok(url),
        "http" => Err("Firefox installs an add-on only from https (or from this computer)".into()),
        _ => Err("the scheme must be https".into()),
    }
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    }
}

/// What the section shows for `link` (this build's, or a test's) and `linked` extensions.
pub fn look(link: Option<&str>, linked: usize) -> Facts {
    let (install, problem) = match link.map(parse) {
        None => (false, None),
        Some(Ok(_)) => (true, None),
        Some(Err(rule)) => (false, Some(rule)),
    };
    Facts {
        install,
        problem,
        linked,
        opened: None,
    }
}

/// The click: `open` is handed the link (it returns why it could not start the browser, in words
/// without the link). A build without a usable link says so.
pub fn install(
    link: Option<&str>,
    linked: usize,
    open: impl FnOnce(&url::Url) -> Result<(), String>,
) -> Result<Facts, String> {
    let url = match link.map(parse) {
        None => {
            return Err(format!(
                "this build of Kai has no download link for Kai for Firefox ({URL_VAR} was not \
                 set when it was built); install the extension by hand"
            ));
        }
        Some(Err(rule)) => {
            tracing::warn!(%rule, "{URL_VAR} does not read as a link Firefox installs from");
            return Err(format!(
                "this build's download link for Kai for Firefox cannot be used: {rule}; install \
                 the extension by hand"
            ));
        }
        Some(Ok(url)) => url,
    };
    tracing::info!(
        host = url.host_str().unwrap_or_default(),
        "Install Kai for Firefox"
    );
    open(&url)?;
    Ok(Facts {
        opened: Some(true),
        ..look(link, linked)
    })
}

/// Hands `url` to `xdg-open`, which starts the desktop's browser (KIO puts it in its own app
/// scope) and returns; its exit is reaped beside Kai, never waited on. Nothing here names the URL:
/// it may hold a token.
pub fn open_in_browser(url: &url::Url) -> Result<(), String> {
    let host = url.host_str().unwrap_or_default().to_owned();
    match std::process::Command::new("xdg-open")
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || match child.wait() {
                Ok(st) if !st.success() => tracing::warn!("xdg-open of {host}: {st}"),
                Err(e) => tracing::warn!("xdg-open of {host}: {e}"),
                Ok(_) => {}
            });
            Ok(())
        }
        Err(e) => {
            tracing::warn!("cannot run xdg-open: {e}");
            Err(format!(
                "Kai could not start xdg-open to open the link ({e}); install the extension by hand"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "glpat-s3cret-token";

    fn private_link() -> String {
        format!(
            "https://deploy-user:{TOKEN}@git.example/api/v4/projects/7/packages/generic/kai-firefox/\
             0.1.0.52/kai-firefox-0.1.0.52.xpi"
        )
    }

    #[test]
    fn a_link_must_be_one_firefox_installs_from() {
        let private = private_link();
        for ok in [
            "https://github.com/lmgw-dev/kai-firefox/releases/latest/download/kai-firefox.xpi",
            private.as_str(),
            "  https://example.com/a.xpi\n",
            "http://localhost:8000/kai-firefox.xpi",
            "http://127.0.0.1/kai-firefox.xpi",
            "http://[::1]:9/kai-firefox.xpi",
        ] {
            assert!(parse(ok).is_ok(), "{ok}");
        }
        for (bad, why) in [
            ("", "not a web address"),
            ("not a url", "not a web address"),
            ("https://", "not a web address"),
            ("http://example.com/a.xpi", "only from https"),
            ("http://192.0.2.7/a.xpi", "only from https"),
            ("ftp://example.com/a.xpi", "must be https"),
            ("file:///tmp/a.xpi", "must be https"),
            ("javascript:alert(1)", "must be https"),
            ("-a://x", "not a web address"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    #[test]
    fn the_page_learns_whether_there_is_a_link_and_never_the_link() {
        let none = look(None, 0);
        assert_eq!(
            none,
            Facts {
                install: false,
                problem: None,
                linked: 0,
                opened: None
            }
        );
        let private = private_link();
        let some = look(Some(&private), 2);
        assert!(some.install && some.problem.is_none() && some.linked == 2);
        let json = serde_json::to_string(&some).unwrap();
        for secret in [TOKEN, "deploy-user", "git.example", "xpi"] {
            assert!(!json.contains(secret), "{json}");
        }
        let broken = look(Some(&format!("ftp://u:{TOKEN}@git.example/x.xpi")), 0);
        assert!(!broken.install);
        let problem = broken.problem.clone().unwrap();
        assert!(problem.contains("https"), "{problem}");
        let json = serde_json::to_string(&broken).unwrap();
        assert!(
            !json.contains(TOKEN) && !json.contains("git.example"),
            "{json}"
        );
    }

    #[test]
    fn a_click_hands_the_link_to_the_desktop_once() {
        let private = private_link();
        let mut given = Vec::new();
        let facts = install(Some(&private), 1, |url| {
            given.push(url.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(given, [private]);
        assert_eq!(
            facts,
            Facts {
                install: true,
                problem: None,
                linked: 1,
                opened: Some(true)
            }
        );
    }

    #[test]
    fn a_click_without_a_usable_link_or_a_browser_says_why_and_never_the_link() {
        let none = install(None, 0, |_| panic!("nothing to open")).unwrap_err();
        assert!(
            none.contains("no download link") && none.contains(URL_VAR) && none.contains("by hand"),
            "{none}"
        );
        let broken = format!("http://u:{TOKEN}@git.example/x.xpi");
        let e = install(Some(&broken), 0, |_| panic!("nothing to open")).unwrap_err();
        assert!(
            e.contains("only from https") && e.contains("by hand"),
            "{e}"
        );
        assert!(!e.contains(TOKEN) && !e.contains("git.example"), "{e}");
        let private = private_link();
        let e = install(Some(&private), 0, |_| Err("xdg-open did not start".into())).unwrap_err();
        assert_eq!(e, "xdg-open did not start");
    }
}
