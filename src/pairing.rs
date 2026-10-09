//! Pairing (§3.1, K13, api §1.4): the `lmgw-pair:` link, and storing what it carries.
//!
//! ```text
//! lmgw-pair:?v=1&url=http%3A%2F%2F127.0.0.1%3A8001&name=desktop&key=lmgw-device-<64 hex>[&fp=sha256:<64 hex>]
//! ```
//!
//! The link carries a credential. Nothing here logs it, and no error echoes any value taken from
//! it: a mangled link can put the key anywhere (a parameter name, the url).

use std::fmt;

use crate::config::{self, ConfigError};
use crate::paths::Paths;
use crate::secret::{SecretError, SecretStore, Stored};

const SCHEME_PREFIX: &str = "lmgw-pair:";
const KEY_PREFIX: &str = "lmgw-device-";
const FP_PREFIX: &str = "sha256:";

/// A device key (`lmgw-device-<64 hex>`, api §1.1). Its `Debug` is redacted and it has no
/// `Display`, so it cannot reach a log line by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceKey(String);

impl DeviceKey {
    pub fn parse(s: &str) -> Result<Self, LinkError> {
        let ok = s
            .strip_prefix(KEY_PREFIX)
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(LinkError::BadKey {
                len: s.chars().count(),
            })
        }
    }

    /// A key read back from the Secret Service. It was validated when stored; it is not re-checked,
    /// so a format change in lmgw never locks out a key that already works.
    pub fn from_store(s: String) -> Self {
        Self(s)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKey(<redacted>)")
    }
}

/// `fp=sha256:<64 hex>`: the SHA-256 of lmgw's TLS certificate (api §1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(String);

impl Fingerprint {
    fn parse(s: &str) -> Result<Self, LinkError> {
        let hex = s.strip_prefix(FP_PREFIX).ok_or(LinkError::BadFingerprint)?;
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(hex.to_ascii_lowercase()))
        } else {
            Err(LinkError::BadFingerprint)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// The normalized base URL (no trailing slash).
    pub url: String,
    /// The device name lmgw gave this key.
    pub name: String,
    pub key: DeviceKey,
    pub fp: Option<Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("not a pairing link: it must start with `lmgw-pair:?`")]
    NotALink,
    #[error("the link has no `v` parameter; this Kai reads v=1 links")]
    MissingVersion,
    #[error("unsupported link version v={0}; this Kai reads v=1 links")]
    UnsupportedVersion(String),
    #[error("the link has no `{0}` parameter")]
    Missing(&'static str),
    #[error("the link's `{0}` parameter is empty")]
    Empty(&'static str),
    #[error("the link carries `{0}` twice")]
    Duplicate(&'static str),
    #[error("the link carries {0}, which a v=1 link does not have")]
    Unknown(String),
    #[error("the link's url is not usable: {0}")]
    BadUrl(String),
    #[error(
        "the link's key is not a device key: expected `lmgw-device-` and 64 hex digits, got {len} characters"
    )]
    BadKey { len: usize },
    #[error("the link's fp is not a fingerprint: expected `sha256:` and 64 hex digits")]
    BadFingerprint,
}

const KNOWN: [&str; 5] = ["v", "url", "name", "key", "fp"];

impl PairingLink {
    /// Parses a link, strictly: `v` must be `1`, every v=1 parameter appears at most once, `url`,
    /// `name` and `key` are required, and nothing else is accepted. Surrounding whitespace (from a
    /// paste) is ignored; the scheme is case-insensitive.
    pub fn parse(input: &str) -> Result<Self, LinkError> {
        let s = input.trim();
        let rest = s
            .get(..SCHEME_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(SCHEME_PREFIX))
            .map(|_| &s[SCHEME_PREFIX.len()..])
            .ok_or(LinkError::NotALink)?;
        let query = rest.strip_prefix('?').ok_or(LinkError::NotALink)?;

        let mut known: [Option<String>; 5] = Default::default();
        let mut unknown: Option<String> = None;
        for (k, v) in form_urlencoded::parse(query.as_bytes()) {
            match KNOWN.iter().position(|n| *n == k) {
                Some(i) if known[i].is_some() => return Err(LinkError::Duplicate(KNOWN[i])),
                Some(i) => known[i] = Some(v.into_owned()),
                None => {
                    unknown.get_or_insert_with(|| describe_name(&k));
                }
            }
        }
        let [v, url, name, key, fp] = known;

        // The version decides what the rest means, so it is checked first.
        match v.as_deref() {
            None => return Err(LinkError::MissingVersion),
            Some("1") => {}
            Some(other) => return Err(LinkError::UnsupportedVersion(describe_value(other))),
        }
        if let Some(u) = unknown {
            return Err(LinkError::Unknown(u));
        }
        let required = |value: Option<String>, name: &'static str| match value {
            None => Err(LinkError::Missing(name)),
            Some(s) if s.trim().is_empty() => Err(LinkError::Empty(name)),
            Some(s) => Ok(s),
        };
        let url = required(url, "url")?;
        let name = required(name, "name")?;
        let key = required(key, "key")?;
        let url = normalize_base_url(&url).map_err(LinkError::BadUrl)?;
        let key = DeviceKey::parse(&key)?;
        let fp = fp.map(|f| Fingerprint::parse(&f)).transpose()?;
        Ok(Self { url, name, key, fp })
    }
}

/// Names an unknown parameter in an error only when it looks like a parameter name; anything else
/// could be part of the key.
fn describe_name(k: &str) -> String {
    if !k.is_empty() && k.len() <= 16 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        format!("a parameter `{k}`")
    } else {
        "an unnamed or malformed parameter".into()
    }
}

fn describe_value(v: &str) -> String {
    if !v.is_empty() && v.len() <= 8 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') {
        v.to_owned()
    } else {
        "<malformed>".into()
    }
}

/// An lmgw base URL: `http` or `https`, a host, an optional path prefix; no user info, query or
/// fragment. Returned without a trailing slash, so the stored attribute and the config value
/// compare equal however the URL was written. The error is a rule, never the URL itself.
pub fn normalize_base_url(s: &str) -> Result<String, String> {
    let u = url::Url::parse(s.trim()).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("the scheme must be http or https".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("it has no host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("it must not carry a user name or password".into());
    }
    if u.query().is_some() {
        return Err("it must not carry a query (?…)".into());
    }
    if u.fragment().is_some() {
        return Err("it must not carry a fragment (#…)".into());
    }
    Ok(u.as_str().trim_end_matches('/').to_owned())
}

/// §3.1 step 4: a link with `fp` is verified against the server's certificate. This fails closed.
/// lmgw serves no TLS today (api §1.4), and the TLS client that would read the certificate comes
/// with the lmgw link (`lmgw_link`), so every link that carries `fp` is refused, with the reason.
pub fn verify_fingerprint(link: &PairingLink) -> Result<(), String> {
    match &link.fp {
        None => Ok(()),
        Some(_) if link.url.starts_with("http://") => Err(
            "the link carries a certificate fingerprint (fp), but its url is http://, which has no \
             certificate to check it against; refusing the link"
                .into(),
        ),
        Some(_) => Err(
            "the link carries a certificate fingerprint (fp), and this build cannot check one yet \
             (lmgw serves no TLS today); refusing the link rather than trusting it unchecked"
                .into(),
        ),
    }
}

/// How many characters of the device name a dialog shows; a longer name is shortened with "…"
/// (the dialog says so). The name is the link's own text, and the link may come from anyone.
pub const NAME_SHOWN_CHARS: usize = 64;

/// What the owner confirms before Kai stores a link (api §1.4: "a client must confirm before it
/// replaces an existing pairing, showing the new url"). The confirmation dialog shows it for a
/// clicked or handed-over link, the paste dialog before its Pair button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairFacts {
    /// The normalized gateway URL.
    pub url: String,
    /// The device name, safe to show (see [`display_name`]).
    pub name: String,
    /// The pairing this one replaces: the configured `lmgw.url`.
    pub replaces: Option<String>,
    /// `http://` to a host that is not this computer: the key and the voice go unencrypted.
    pub plain_http_remote: bool,
}

/// One line of a pairing dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactLine {
    Plain(String),
    Warning(String),
}

impl PairFacts {
    pub fn new(link: &PairingLink, current: Option<&str>) -> Self {
        Self {
            url: link.url.clone(),
            name: display_name(&link.name),
            replaces: current.map(str::to_owned),
            plain_http_remote: plain_http_remote(&link.url),
        }
    }

    /// The dialog's lines, in order: the gateway, the device, what it replaces, the http warning.
    pub fn lines(&self) -> Vec<FactLine> {
        let mut lines = vec![
            FactLine::Plain(format!("Gateway: {}", self.url)),
            FactLine::Plain(format!("Device name: {}", self.name)),
        ];
        match &self.replaces {
            Some(old) if *old == self.url => lines.push(FactLine::Warning(format!(
                "This replaces the device key Kai keeps for {old}."
            ))),
            Some(old) => lines.push(FactLine::Warning(format!(
                "This replaces Kai's pairing with {old}: Kai stops using that gateway, its voice \
                 session there ends, and Kai deletes the device key it keeps for it."
            ))),
            None => {}
        }
        if self.plain_http_remote {
            lines.push(FactLine::Warning(
                "This gateway is reached over plain http on another computer: the device key and \
                 your voice travel unencrypted over the network."
                    .into(),
            ));
        }
        lines
    }
}

/// `http://` to a host other than this computer (loopback addresses and `localhost` are this
/// computer).
pub fn plain_http_remote(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    if u.scheme() != "http" {
        return false;
    }
    match u.host() {
        Some(url::Host::Ipv4(a)) => !a.is_loopback(),
        Some(url::Host::Ipv6(a)) => !a.is_loopback(),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !(d == "localhost" || d.ends_with(".localhost"))
        }
        None => true,
    }
}

/// Unicode's format characters (general category Cf: bidirectional formatting, zero-width and
/// joiners, the soft hyphen, the BOM, tags) and its line and paragraph separators (Zl, Zp), which
/// GTK breaks lines at.
fn format_or_separator(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}'
        | '\u{0890}'..='\u{0891}' | '\u{08E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// The device name as a dialog shows it: control characters, format characters (bidirectional
/// formatting, zero-width) and line or paragraph separators, which could make one name read as
/// another or add a line under it, become U+FFFD (review KAI-2 K2-14); past [`NAME_SHOWN_CHARS`]
/// it is cut with "… (shortened)".
pub fn display_name(name: &str) -> String {
    let clean: Vec<char> = name
        .chars()
        .map(|c| {
            if c.is_control() || format_or_separator(c) {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect();
    if clean.len() > NAME_SHOWN_CHARS {
        let mut s: String = clean[..NAME_SHOWN_CHARS].iter().collect();
        s.push_str("… (shortened)");
        s
    } else {
        clean.into_iter().collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("{0}")]
    Link(#[from] LinkError),
    #[error("{0}")]
    Fingerprint(String),
    #[error("{0}")]
    Wallet(#[from] SecretError),
    #[error("{0}")]
    Config(#[from] ConfigError),
}

/// What storing a link did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Committed {
    /// The key is in the Secret Service and `lmgw.url` in the config. `first`: Kai had no
    /// `lmgw.url` before, so "Start with Plasma" is switched on (§13).
    Stored { first: bool },
    /// The wallet is locked; nothing was stored and no unlock was asked for.
    WalletLocked,
}

/// Stores a parsed link (§3.1 step 3): the key as a Secret Service item, then the URL as
/// `lmgw.url`. Only the running Kai calls it, and only for a link the owner confirmed (K13).
/// `current_url` is the configured `lmgw.url` before this pairing.
pub async fn commit<S: SecretStore>(
    store: &S,
    paths: &Paths,
    current_url: Option<&str>,
    link: &PairingLink,
) -> Result<Committed, PairError> {
    verify_fingerprint(link).map_err(PairError::Fingerprint)?;
    match store.store(&link.url, &link.name, &link.key).await? {
        Stored::Locked => Ok(Committed::WalletLocked),
        Stored::Stored => {
            config::set_lmgw_url(&paths.config_file, &link.url)?;
            Ok(Committed::Stored {
                first: current_url.is_none(),
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const KEY: &str =
        "lmgw-device-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    pub fn link(params: &str) -> String {
        format!("lmgw-pair:?{params}")
    }

    pub fn good() -> String {
        link(&format!(
            "v=1&url=http%3A%2F%2F127.0.0.1%3A8001&name=desktop&key={KEY}"
        ))
    }

    #[test]
    fn a_good_link_parses() {
        let l = PairingLink::parse(&good()).unwrap();
        assert_eq!(l.url, "http://127.0.0.1:8001");
        assert_eq!(l.name, "desktop");
        assert_eq!(l.key.expose(), KEY);
        assert_eq!(l.fp, None);
        // pasted with whitespace, upper-case scheme, parameters in another order, `+` as a space
        let l = PairingLink::parse(&format!(
            "  LMGW-PAIR:?key={KEY}&name=my+desk&url=https%3A%2F%2Fgateway.example%2Fbase%2F&v=1\n"
        ))
        .unwrap();
        assert_eq!(
            (l.url.as_str(), l.name.as_str()),
            ("https://gateway.example/base", "my desk")
        );
    }

    #[test]
    fn the_version_must_be_1() {
        let k = format!("url=http%3A%2F%2Fh&name=n&key={KEY}");
        assert_eq!(
            PairingLink::parse(&link(&k)),
            Err(LinkError::MissingVersion)
        );
        for v in ["2", "0", "01", "1.0", "", "%201"] {
            let e = PairingLink::parse(&link(&format!("v={v}&{k}"))).unwrap_err();
            assert!(
                matches!(e, LinkError::UnsupportedVersion(_)),
                "v={v}: {e:?}"
            );
        }
        // a v=2 link with parameters this Kai does not know reports the version, not the parameter
        let e = PairingLink::parse(&link(&format!("v=2&{k}&token=x"))).unwrap_err();
        assert_eq!(e, LinkError::UnsupportedVersion("2".into()));
    }

    #[test]
    fn missing_or_empty_required_parameters() {
        let all = [
            ("url", "http%3A%2F%2Fh".to_owned()),
            ("name", "n".to_owned()),
            ("key", KEY.to_owned()),
        ];
        for skip in ["url", "name", "key"] {
            let q: Vec<String> = all
                .iter()
                .filter(|(n, _)| *n != skip)
                .map(|(n, v)| format!("{n}={v}"))
                .collect();
            let e = PairingLink::parse(&link(&format!("v=1&{}", q.join("&")))).unwrap_err();
            assert_eq!(
                e,
                LinkError::Missing(match skip {
                    "url" => "url",
                    "name" => "name",
                    _ => "key",
                })
            );
        }
        let e = PairingLink::parse(&link("v=1&url=http%3A%2F%2Fh&name=n&key=")).unwrap_err();
        assert_eq!(e, LinkError::Empty("key"));
    }

    #[test]
    fn malformed_links_are_refused() {
        assert_eq!(
            PairingLink::parse("https://example.com"),
            Err(LinkError::NotALink)
        );
        assert_eq!(
            PairingLink::parse("lmgw-pair:v=1"),
            Err(LinkError::NotALink)
        );
        assert_eq!(PairingLink::parse("lmgw-pai"), Err(LinkError::NotALink));
        let dup = format!("{}&name=other", good());
        assert_eq!(PairingLink::parse(&dup), Err(LinkError::Duplicate("name")));
        let unknown = format!("{}&label=x", good());
        assert_eq!(
            PairingLink::parse(&unknown),
            Err(LinkError::Unknown("a parameter `label`".into()))
        );
        let bad_url = link(&format!("v=1&url=ftp%3A%2F%2Fh&name=n&key={KEY}"));
        assert!(matches!(
            PairingLink::parse(&bad_url),
            Err(LinkError::BadUrl(_))
        ));
        let short = link("v=1&url=http%3A%2F%2Fh&name=n&key=lmgw-device-0123");
        assert_eq!(
            PairingLink::parse(&short),
            Err(LinkError::BadKey { len: 16 })
        );
        let other_kind = link(&format!(
            "v=1&url=http%3A%2F%2Fh&name=n&key=lmgw-client-{}",
            &KEY[12..]
        ));
        assert!(matches!(
            PairingLink::parse(&other_kind),
            Err(LinkError::BadKey { .. })
        ));
    }

    #[test]
    fn no_error_echoes_the_key() {
        let secret = &KEY[12..];
        let cases = [
            // the key smuggled into a parameter name, the url and the version
            link(&format!("v=1&url=http%3A%2F%2Fh&name=n&{KEY}")),
            link(&format!(
                "v=1&url=http%3A%2F%2Fh%3Fk%3D{KEY}&name=n&key={KEY}"
            )),
            link(&format!("v={KEY}&url=http%3A%2F%2Fh&name=n&key={KEY}")),
            link(&format!("v=1&url=http%3A%2F%2Fh&name=n&key={KEY}0")),
        ];
        for c in cases {
            let e = PairingLink::parse(&c).unwrap_err().to_string();
            assert!(!e.contains(secret), "{e}");
        }
        assert!(!format!("{:?}", PairingLink::parse(&good()).unwrap()).contains(secret));
    }

    #[test]
    fn a_link_with_fp_parses_and_then_fails_closed() {
        let fp = "sha256:".to_owned() + &"AB".repeat(32);
        let l = PairingLink::parse(&format!("{}&fp={fp}", good())).unwrap();
        assert_eq!(l.fp, Some(Fingerprint("ab".repeat(32))));
        let e = verify_fingerprint(&l).unwrap_err();
        assert!(e.contains("http://") && e.contains("refusing"), "{e}");
        let https = link(&format!("v=1&url=https%3A%2F%2Fh&name=n&key={KEY}&fp={fp}"));
        let e = verify_fingerprint(&PairingLink::parse(&https).unwrap()).unwrap_err();
        assert!(e.contains("cannot check") && e.contains("refusing"), "{e}");
        assert_eq!(
            verify_fingerprint(&PairingLink::parse(&good()).unwrap()),
            Ok(())
        );
        let bad = format!("{}&fp=md5:00", good());
        assert_eq!(PairingLink::parse(&bad), Err(LinkError::BadFingerprint));
    }

    #[test]
    fn the_facts_name_the_gateway_the_device_and_what_is_replaced() {
        let l = PairingLink::parse(&good()).unwrap();
        let f = PairFacts::new(&l, None);
        assert_eq!(
            f.lines(),
            [
                FactLine::Plain("Gateway: http://127.0.0.1:8001".into()),
                FactLine::Plain("Device name: desktop".into()),
            ],
            "a first pairing on this computer: no warning"
        );
        let f = PairFacts::new(&l, Some("http://gateway.example:8001"));
        assert!(matches!(&f.lines()[2], FactLine::Warning(w)
            if w.contains("replaces Kai's pairing with http://gateway.example:8001")));
        let f = PairFacts::new(&l, Some("http://127.0.0.1:8001"));
        assert!(matches!(&f.lines()[2], FactLine::Warning(w)
            if w.contains("replaces the device key Kai keeps for http://127.0.0.1:8001")));
        let remote = link(&format!(
            "v=1&url=http%3A%2F%2F192.0.2.20%3A8001&name=desktop&key={KEY}"
        ));
        let f = PairFacts::new(&PairingLink::parse(&remote).unwrap(), None);
        assert!(f.plain_http_remote);
        assert!(
            matches!(f.lines().last(), Some(FactLine::Warning(w)) if w.contains("unencrypted"))
        );
        let text = format!("{:?}", f.lines());
        assert!(!text.contains(&KEY[12..]), "the key is never shown");
    }

    #[test]
    fn plain_http_is_remote_unless_it_is_this_computer() {
        for (url, remote) in [
            ("http://127.0.0.1:8001", false),
            ("http://127.3.4.5", false),
            ("http://[::1]:8001", false),
            ("http://localhost:8001", false),
            ("http://LOCALHOST.", false),
            ("http://kai.localhost", false),
            ("http://192.0.2.20:8001", true),
            ("http://gateway.example", true),
            ("http://[fe80::1]", true),
            ("https://gateway.example", false),
        ] {
            assert_eq!(plain_http_remote(url), remote, "{url}");
        }
    }

    #[test]
    fn a_device_name_is_made_safe_to_show() {
        assert_eq!(display_name("desktop"), "desktop");
        assert_eq!(display_name("a\nb\u{202E}c"), "a\u{FFFD}b\u{FFFD}c");
        // K2-14: a line separator would show a second "Gateway" line; zero-width characters and
        // the soft hyphen hide in a name.
        assert_eq!(
            display_name("desktop\u{2028}Gateway: http://127.0.0.1:8001"),
            "desktop\u{FFFD}Gateway: http://127.0.0.1:8001"
        );
        for c in [
            '\u{2029}',
            '\u{200B}',
            '\u{200D}',
            '\u{FEFF}',
            '\u{00AD}',
            '\u{E0041}',
        ] {
            assert_eq!(
                display_name(&format!("a{c}b")),
                "a\u{FFFD}b",
                "U+{:04X}",
                c as u32
            );
        }
        assert_eq!(
            display_name("Küche — Tür"),
            "Küche — Tür",
            "letters and dashes stay"
        );
        let long = "x".repeat(NAME_SHOWN_CHARS + 5);
        let shown = display_name(&long);
        assert!(shown.ends_with("… (shortened)"), "{shown}");
        assert_eq!(
            shown.chars().filter(|c| *c == 'x').count(),
            NAME_SHOWN_CHARS
        );
    }

    #[test]
    fn base_urls_are_normalized() {
        assert_eq!(
            normalize_base_url("http://127.0.0.1:8001/").unwrap(),
            "http://127.0.0.1:8001"
        );
        assert_eq!(normalize_base_url("HTTP://Host:80").unwrap(), "http://host");
        assert_eq!(
            normalize_base_url("https://h/a/b/").unwrap(),
            "https://h/a/b"
        );
        assert!(normalize_base_url("http://h#x").is_err());
    }
}
