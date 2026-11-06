//! Site rules (kai-firefox design §6.4, `[browser.sites]`): per site `act` (the default), `read`
//! (every tool that acts on the page is refused) or `none` (no tool reaches it: Kai does not read,
//! act on or name its tabs). Checked before every call of a browser tool, by the provider
//! (`super::tools`).
//!
//! An entry is a site: a host (`mail.example`), `*.domain` for the domain and every subdomain
//! (WebExtension match patterns' meaning), `*` for every host, each optionally with a scheme
//! (`https://`) and a port (`:8443`); no path, query or fragment. Hosts are compared as the URL
//! parser normalizes them (lowercase, internationalized names as punycode, IPv6 in brackets).
//!
//! **Which entry decides:** the most specific that matches a tab's URL: a host over `*.domain`, a
//! longer domain over a shorter one, `*.domain` over `*`, then an entry with a scheme or a port over
//! one without. Among equally specific matches the strictest rule wins, so a site listed both ways
//! by accident is not acted on.
//!
//! **The page's site** is that of the URL it comes from ([`effective`]): a `blob:` URL's origin,
//! the page `view-source:` shows, the page Reader view shows (`about:reader?url=`), each as deep
//! as they nest. A host's trailing dot (`mail.example.`, the same site in DNS) is dropped, in a
//! URL and in an entry. A URL that still has no host (`about:`, `file:`, `data:`) matches no entry
//! and is act's; Firefox's own pages are out of the extension's reach anyway (design §3). The
//! desktop's tools and Kai's reading of Firefox's own UI, which see whatever Firefox's window
//! shows, judge a page by an allow-list instead ([`SiteRules::page_rule`]): only an `http:`,
//! `https:` or `file:` page with a host gets its site's rule, every other page the strictest rule
//! configured (another extension's `moz-extension:` page, which has a host that is no site, too).
//!
//! **A child frame's own origin decides for the frame** ([`SiteRules::frame_rule`], kai-firefox
//! WP5b's design decision): a `none` site framed in an `act` page is not read. A page or frame
//! whose site Kai cannot tell (an empty URL, an opaque origin) gets the strictest rule any entry
//! gives ([`SiteRules::strictest`]).

use super::settings::Sites;

/// The URL whose site a page is (module doc): `blob:`, `view-source:` and Reader view's
/// `about:reader?url=` unwrapped; `None` when `url` does not parse.
pub fn effective(url: &str) -> Option<url::Url> {
    let mut u = url::Url::parse(url).ok()?;
    loop {
        let inner = match u.scheme() {
            "view-source" => u
                .as_str()
                .strip_prefix("view-source:")
                .and_then(|rest| url::Url::parse(rest).ok()),
            "blob" => url::Url::parse(u.path()).ok(),
            "about" if u.path() == "reader" => u
                .query_pairs()
                .find(|(k, _)| k == "url")
                .and_then(|(_, v)| url::Url::parse(&v).ok()),
            _ => None,
        };
        match inner {
            Some(i) => u = i,
            None => return Some(u),
        }
    }
}

/// `host` without its trailing dot.
fn bare(host: &str) -> &str {
    host.strip_suffix('.').unwrap_or(host)
}

/// A site's rule, ordered by strictness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    Act,
    Read,
    None,
}

impl Rule {
    pub fn word(self) -> &'static str {
        match self {
            Rule::Act => "act",
            Rule::Read => "read",
            Rule::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    /// `*`.
    Any,
    /// `*.domain`: the domain and its subdomains.
    Domain(String),
    Exact(String),
}

/// One entry, read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    scheme: Option<String>,
    host: Host,
    port: Option<u16>,
}

impl Pattern {
    /// How specific it is (module doc): compared as a tuple, larger is more specific.
    fn rank(&self) -> (u8, usize, bool, bool) {
        let (kind, labels) = match &self.host {
            Host::Any => (0, 0),
            Host::Domain(d) => (1, d.split('.').count()),
            Host::Exact(h) => (2, h.split('.').count()),
        };
        (kind, labels, self.scheme.is_some(), self.port.is_some())
    }

    fn matches(&self, url: &url::Url) -> bool {
        let Some(host) = url.host_str().map(bare) else {
            return false;
        };
        let host_fits = match &self.host {
            Host::Any => true,
            Host::Exact(h) => host == h,
            Host::Domain(d) => {
                host == d
                    || host
                        .strip_suffix(d.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            }
        };
        host_fits
            && self.scheme.as_deref().is_none_or(|s| s == url.scheme())
            && self
                .port
                .is_none_or(|p| url.port_or_known_default() == Some(p))
    }
}

/// What an entry must look like, for the loader's error.
const SHAPE: &str = "each entry is a site: a host (mail.example), *.domain for the domain and its \
                     subdomains, * for every host, optionally with a scheme and a port \
                     (https://shop.example:8443); no path";

/// `text` as an entry, or why not.
fn pattern(text: &str) -> Result<Pattern, String> {
    let t = text.trim();
    if t.is_empty() || t != text {
        return Err(SHAPE.into());
    }
    let (scheme, rest) = match t.split_once("://") {
        Some((s, rest)) => {
            if s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            {
                return Err(SHAPE.into());
            }
            (Some(s.to_ascii_lowercase()), rest)
        }
        None => (None, t),
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return Err(SHAPE.into());
    }
    if rest == "*" {
        return Ok(Pattern {
            scheme,
            host: Host::Any,
            port: None,
        });
    }
    let (wild, rest) = match rest.strip_prefix("*.") {
        Some(r) => (true, r),
        None => (false, rest),
    };
    if rest.contains('*') {
        return Err(SHAPE.into());
    }
    // The URL parser normalizes the host as it does a tab's URL's. The port comes from a scheme
    // without a default port, so an explicit `:80` stays the entry's.
    let parsed = url::Url::parse(&format!("http://{rest}/")).map_err(|_| SHAPE.to_owned())?;
    let host = bare(parsed.host_str().ok_or_else(|| SHAPE.to_owned())?).to_owned();
    if host.is_empty() {
        return Err(SHAPE.into());
    }
    let port = url::Url::parse(&format!("kai-site://{rest}/"))
        .map_err(|_| SHAPE.to_owned())?
        .port();
    Ok(Pattern {
        scheme,
        host: if wild {
            Host::Domain(host)
        } else {
            Host::Exact(host)
        },
        port,
    })
}

/// The rules as read from `[browser.sites]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteRules {
    entries: Vec<(Pattern, Rule)>,
}

/// An entry that is no site, or a site listed under two rules: the key, the entry, the rule it
/// breaks.
pub type Refusal = (&'static str, String, String);

impl SiteRules {
    /// Every entry that reads, and a refusal for each that does not (the loader's errors, in the
    /// file's order). A site listed under two rules is kept under both (the stricter decides) and
    /// refused too.
    fn entries(sites: &Sites) -> (Self, Vec<Refusal>) {
        let mut entries: Vec<(Pattern, Rule)> = Vec::new();
        let mut refused = Vec::new();
        for (key, list, rule) in [
            ("browser.sites.act", &sites.act, Rule::Act),
            ("browser.sites.read", &sites.read, Rule::Read),
            ("browser.sites.none", &sites.none, Rule::None),
        ] {
            for text in list {
                let p = match pattern(text) {
                    Ok(p) => p,
                    Err(why) => {
                        refused.push((key, format!("{text:?}"), why));
                        continue;
                    }
                };
                if let Some((_, other)) = entries.iter().find(|(q, _)| *q == p) {
                    refused.push((
                        key,
                        format!("{text:?}"),
                        format!(
                            "the site is listed under {} already; one site has one rule",
                            other.word()
                        ),
                    ));
                }
                entries.push((p, rule));
            }
        }
        (Self { entries }, refused)
    }

    /// The rules, or the loader's error for the first entry that is no site, or a site listed
    /// under two rules.
    pub fn read(sites: &Sites) -> Result<Self, Refusal> {
        let (rules, refused) = Self::entries(sites);
        match refused.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(rules),
        }
    }

    /// The rules of a configuration that loaded. The loader refuses an entry that does not read,
    /// so there is none here; should one come anyway, only that entry is skipped, with a note in
    /// the log, and every other rule holds (a `none` list never falls away whole).
    pub fn of(sites: &Sites) -> Self {
        let (rules, refused) = Self::entries(sites);
        for (key, entry, why) in refused {
            tracing::warn!(
                key,
                entry,
                why,
                "browser: a site rule that does not read is skipped"
            );
        }
        rules
    }

    /// The rule for a page at `url` (module doc). An empty URL (an answer that named none) could
    /// be any site: it gets [`SiteRules::strictest`].
    pub fn rule(&self, url: &str) -> Rule {
        if url.trim().is_empty() {
            return self.strictest();
        }
        let Some(url) = effective(url) else {
            return Rule::Act;
        };
        self.entries
            .iter()
            .filter(|(p, _)| p.matches(&url))
            .max_by_key(|(p, rule)| (p.rank(), *rule))
            .map_or(Rule::Act, |(_, rule)| *rule)
    }

    /// The rule for what the desktop shows of a page at `url` (Kai's site rules over the
    /// desktop's tools, `super::shown`, and over Firefox's own UI, `super::tools::chrome`), an
    /// allow-list: [`SiteRules::rule`] for an `http:`, `https:` or `file:` page with a host (after
    /// [`effective`]'s unwrapping, so Reader view and `view-source:` of one too), and the
    /// strictest rule any entry gives for every other page: one whose URL names no host (`about:`
    /// pages such as Firefox View, a new tab, the logins and the history, `about:blank`, `data:`,
    /// a `file:` page without a host, a `blob:` of an opaque origin) and one of another scheme
    /// (Firefox's own `chrome:` and `resource:`, another extension's `moz-extension:`, whose host
    /// is no site): it could show anything, also a site Kai does not reach.
    pub fn page_rule(&self, url: &str) -> Rule {
        match effective(url) {
            Some(u)
                if matches!(u.scheme(), "http" | "https" | "file")
                    && u.host_str().is_some_and(|h| !h.is_empty()) =>
            {
                self.rule(url)
            }
            _ => self.strictest(),
        }
    }

    /// The rule for a child frame's document by its own origin (the contract's Origin: a URL's
    /// origin, `None` when opaque), which decides for the frame whatever its page's rule (kai-firefox
    /// WP5b's design decision): an origin gets its site's rule, an opaque one could be any site and
    /// gets [`SiteRules::strictest`].
    pub fn frame_rule(&self, origin: Option<&str>) -> Rule {
        match origin.filter(|o| !o.trim().is_empty() && *o != "null") {
            Some(o) => self.rule(o),
            None => self.strictest(),
        }
    }

    /// The strictest rule any entry gives: what a page or frame whose site Kai cannot tell gets
    /// (`act` without entries, so nothing is withheld while no site is `read` or `none`).
    pub fn strictest(&self) -> Rule {
        self.entries
            .iter()
            .map(|(_, rule)| *rule)
            .max()
            .unwrap_or(Rule::Act)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(act: &[&str], read: &[&str], none: &[&str]) -> SiteRules {
        let v = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        SiteRules::read(&Sites {
            act: v(act),
            read: v(read),
            none: v(none),
        })
        .unwrap()
    }

    /// The desktop's rule: a page without a site's host, or Firefox's own, gets the strictest.
    #[test]
    fn a_page_without_a_site_gets_the_strictest_rule_on_the_desktop() {
        let r = rules(&[], &["shop.example"], &["mail.example"]);
        for url in [
            "about:firefoxview",
            "about:newtab",
            "about:logins",
            "about:history",
            "about:blank",
            "data:text/html,<p>hi",
            "blob:null/0b1c",
            "file:///home/u/a.html",
            "chrome://browser/content/places/historySidebar.xhtml",
            "resource://pdf.js/web/viewer.html",
            // Another extension's page (a password manager's vault): its host is no site.
            "moz-extension://4f0c/vault.html",
            "view-source:moz-extension://4f0c/vault.html",
            "blob:moz-extension://4f0c/0b1c",
            "about:reader?url=moz-extension%3A%2F%2F4f0c%2Fvault.html",
            "ftp://news.example/",
            "ws://news.example/",
            "",
            "not a url",
        ] {
            assert_eq!(r.page_rule(url), Rule::None, "{url}");
        }
        for (url, want) in [
            ("https://mail.example/inbox", Rule::None),
            ("https://shop.example/", Rule::Read),
            ("https://news.example/", Rule::Act),
            (
                "about:reader?url=https%3A%2F%2Fmail.example%2Fa",
                Rule::None,
            ),
            ("view-source:https://news.example/", Rule::Act),
            ("blob:https://news.example/0b1c", Rule::Act),
            // A file: page with a host (a share) is judged by it; without one, the strictest.
            ("file://news.example/share/a.html", Rule::Act),
            ("file://shop.example/share/a.html", Rule::Read),
        ] {
            assert_eq!(r.page_rule(url), want, "{url}");
        }
        // Without rules nothing is stricter than act.
        assert_eq!(rules(&[], &[], &[]).page_rule("about:blank"), Rule::Act);
        assert_eq!(
            rules(&[], &["shop.example"], &[]).page_rule("moz-extension://4f0c/vault.html"),
            Rule::Read
        );
        assert_eq!(
            rules(&[], &["shop.example"], &[]).page_rule("about:blank"),
            Rule::Read
        );
    }

    #[test]
    fn the_most_specific_entry_decides() {
        let r = rules(
            &["docs.bank.example"],
            &["*.bank.example", "https://shop.example:8443"],
            &["mail.example", "*.tracker.example"],
        );
        for (url, want) in [
            ("https://mail.example/inbox", Rule::None),
            ("http://MAIL.example:8080/", Rule::None),
            ("https://www.mail.example/", Rule::Act),
            ("https://bank.example/", Rule::Read),
            ("https://login.bank.example/x", Rule::Read),
            ("https://docs.bank.example/", Rule::Act),
            ("https://notbank.example/", Rule::Act),
            ("https://shop.example:8443/cart", Rule::Read),
            ("https://shop.example/cart", Rule::Act),
            ("http://shop.example:8443/", Rule::Act),
            ("https://a.b.tracker.example/", Rule::None),
            ("about:preferences", Rule::Act),
            ("file:///home/x", Rule::Act),
            ("not a url", Rule::Act),
        ] {
            assert_eq!(r.rule(url), want, "{url}");
        }
        // A scheme makes an entry more specific; equally specific ones: the strictest.
        let r = rules(&["https://site.example"], &[], &["site.example"]);
        assert_eq!(r.rule("https://site.example/"), Rule::Act);
        assert_eq!(r.rule("http://site.example/"), Rule::None);
        let r = rules(&["*"], &[], &["*.example"]);
        assert_eq!(r.rule("https://a.example/"), Rule::None);
        assert_eq!(r.rule("https://a.test/"), Rule::Act);
        let r = rules(&[], &["192.0.2.1:8080"], &["[2001:db8::1]"]);
        assert_eq!(r.rule("http://192.0.2.1:8080/"), Rule::Read);
        assert_eq!(r.rule("http://192.0.2.1/"), Rule::Act);
        assert_eq!(r.rule("http://[2001:db8::1]:9/"), Rule::None);
        let r = rules(&[], &["bücher.example"], &[]);
        assert_eq!(r.rule("https://xn--bcher-kva.example/"), Rule::Read);
        let r = rules(&[], &["http://plain.example:80"], &[]);
        assert_eq!(r.rule("http://plain.example/"), Rule::Read);
        assert_eq!(r.rule("http://plain.example:8080/"), Rule::Act);
        assert_eq!(rules(&[], &[], &[]).rule("https://x.example/"), Rule::Act);
        // A page or frame whose site Kai cannot tell gets the strictest rule there is.
        let r = rules(&["*"], &[], &["*.example"]);
        assert_eq!(r.rule(""), Rule::None);
        assert_eq!(r.frame_rule(None), Rule::None);
        assert_eq!(r.frame_rule(Some("null")), Rule::None);
        assert_eq!(r.frame_rule(Some("https://a.test")), Rule::Act);
        assert_eq!(r.frame_rule(Some("https://a.example")), Rule::None);
        assert_eq!(rules(&[], &["a.example"], &[]).frame_rule(None), Rule::Read);
        assert_eq!(rules(&[], &[], &[]).rule(""), Rule::Act);
    }

    /// A page's site is the one it comes from, however its URL wraps it.
    #[test]
    fn a_wrapped_url_or_a_trailing_dot_does_not_get_past_none() {
        let r = rules(&[], &[], &["mail.example", "https://*.bank.example"]);
        for url in [
            "blob:https://mail.example/0b6a7f3e-5c1d-4a8e-9f00-1b2c3d4e5f60",
            "view-source:https://mail.example/inbox",
            "view-source:blob:https://mail.example/0b6a7f3e",
            "about:reader?url=https%3A%2F%2Fmail.example%2Fletter%3Fid%3D3",
            "view-source:about:reader?url=https%3A%2F%2Fmail.example%2F",
            "https://mail.example./inbox",
            "https://MAIL.example.:443/",
            "blob:https://login.bank.example./x",
            "about:reader?url=https%3A%2F%2Fwww.bank.example.%2F",
        ] {
            assert_eq!(r.rule(url), Rule::None, "{url}");
        }
        for url in [
            "view-source:https://news.example/",
            "about:reader?url=https%3A%2F%2Fnews.example%2F",
            "about:reader",
            "blob:null/0b6a7f3e",
            "view-source:http://login.bank.example/",
        ] {
            assert_eq!(r.rule(url), Rule::Act, "{url}");
        }
        // An entry with a trailing dot names the same site.
        let r = rules(&[], &[], &["mail.example.", "*.bank.example."]);
        assert_eq!(r.rule("https://mail.example/"), Rule::None);
        assert_eq!(r.rule("https://login.bank.example./"), Rule::None);
        assert_eq!(
            effective("view-source:about:reader?url=https%3A%2F%2Fmail.example%2Fa")
                .unwrap()
                .as_str(),
            "https://mail.example/a"
        );
    }

    /// The configuration's rules never fall away whole: an entry that does not read is skipped
    /// alone.
    #[test]
    fn a_bad_entry_is_skipped_alone() {
        let r = SiteRules::of(&Sites {
            none: vec![
                "mail.example".into(),
                "mail.example/inbox".into(),
                "*.bank.example".into(),
            ],
            read: vec!["shop.example".into()],
            act: vec!["shop.example".into()],
        });
        assert_eq!(r.rule("https://mail.example/"), Rule::None);
        assert_eq!(r.rule("https://login.bank.example/"), Rule::None);
        assert_eq!(
            r.rule("https://shop.example/"),
            Rule::Read,
            "a site under two rules: the stricter"
        );
        assert_eq!(r.rule("https://news.example/"), Rule::Act);
    }

    #[test]
    fn an_entry_that_is_no_site_is_named() {
        for bad in [
            "",
            " mail.example",
            "mail.example/inbox",
            "https://",
            "a.*.example",
            "user@mail.example",
            "mail.example?x",
            "://mail.example",
            ".",
        ] {
            let e = SiteRules::read(&Sites {
                none: vec![bad.into()],
                ..Default::default()
            })
            .unwrap_err();
            assert_eq!(e.0, "browser.sites.none", "{bad:?}");
            assert!(e.2.contains("each entry is a site"), "{bad:?}: {}", e.2);
        }
        let e = SiteRules::read(&Sites {
            read: vec!["Mail.example".into()],
            none: vec!["mail.example".into()],
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(e.0, "browser.sites.none");
        assert!(e.2.contains("listed under read already"), "{}", e.2);
        let c = crate::config::Config::parse(
            "[browser.sites]\nread = [\"*.bank.example\"]\nnone = [\"mail.example\"]\n",
            std::path::Path::new("kai.toml"),
        )
        .unwrap();
        assert_eq!(c.browser.sites.none, ["mail.example"]);
        let e = crate::config::Config::parse(
            "[browser.sites]\nnone = [\"mail.example/x\"]\n",
            std::path::Path::new("kai.toml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("browser.sites.none"), "{e}");
    }
}
