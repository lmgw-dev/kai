//! The ring page (§9.2): Kai's embedded assets, served from `kai://app/` (K10) under a strict CSP,
//! and both directions of the bridge: calls into `window.__kai` (assets/overlay/boot.js) and the
//! page's messages.

use serde_json::{Value, json};

use super::lifecycle::PageState;
use crate::audio::Levels;

pub const SCHEME: &str = "kai";

/// No inline script (the boot is an external module), nothing but the page's own files.
pub const CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'self'; base-uri 'none'; form-action 'none'";

/// The page, with reduced motion as GTK says it now (`engine.js` reads it at mount).
pub fn page_uri(reduced: bool) -> String {
    format!(
        "{SCHEME}://app/ring.html?reduced={}",
        if reduced { 1 } else { 0 }
    )
}

struct Asset {
    path: &'static str,
    mime: &'static str,
    body: &'static [u8],
}

macro_rules! asset {
    ($path:literal, $mime:literal) => {
        Asset {
            path: concat!("/", $path),
            mime: $mime,
            body: include_bytes!(concat!("../../assets/overlay/", $path)),
        }
    };
}

/// The files under assets/overlay. `viz/` is lmgw's, vendored byte-identical (`just viz-check`).
const ASSETS: &[Asset] = &[
    asset!("ring.html", "text/html"),
    asset!("ring.css", "text/css"),
    asset!("boot.js", "text/javascript"),
    asset!("adapter.js", "text/javascript"),
    asset!("palette.js", "text/javascript"),
    asset!("viz/engine.js", "text/javascript"),
    asset!("viz/ring.js", "text/javascript"),
];

/// The content type and body for `kai://app<path>`; `None` is a 404.
pub fn asset(host: &str, path: &str) -> Option<(&'static str, &'static [u8])> {
    if host != "app" {
        return None;
    }
    ASSETS
        .iter()
        .find(|a| a.path == path)
        .map(|a| (a.mime, a.body))
}

/// What the page posts to the `kai` message handler.
#[derive(Debug, Clone, PartialEq)]
pub enum PageMsg {
    Ready,
    /// The engine drew its first frame since the page became visible.
    Drawn,
    /// The idle state of send `seq` was eased into and drawn.
    Settled {
        seq: u64,
    },
    Stats(Value),
    Error(String),
    Csp {
        directive: String,
        blocked: String,
    },
    /// Anything else, kept for the log.
    Other(String),
}

pub fn parse(text: &str) -> PageMsg {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return PageMsg::Other(text.to_owned());
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    match v.get("ev").and_then(Value::as_str) {
        Some("ready") => PageMsg::Ready,
        Some("drawn") => PageMsg::Drawn,
        Some("settled") => match v.get("seq").and_then(Value::as_u64) {
            Some(seq) => PageMsg::Settled { seq },
            None => PageMsg::Other(text.to_owned()),
        },
        Some("stats") => PageMsg::Stats(v),
        Some("error") => PageMsg::Error(s("msg")),
        Some("csp") => PageMsg::Csp {
            directive: s("directive"),
            blocked: s("blocked"),
        },
        _ => PageMsg::Other(text.to_owned()),
    }
}

pub fn set_state_js(p: &PageState) -> String {
    let info = json!({"muted": p.muted, "loading": p.loading, "held": p.held});
    format!(
        "__kai.setState({},{info},{})",
        Value::from(p.state.key()),
        p.seq
    )
}

/// One feed: the playback's and the microphone's level and bands, to a thousandth of the range.
pub fn feed_js(output: &Levels, input: &Levels) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(720);
    let _ = write!(s, "__kai.feed({:.3},{:.3},", output.level, input.level);
    for (i, levels) in [output, input].into_iter().enumerate() {
        s.push('[');
        for (b, v) in levels.bands.iter().enumerate() {
            if b > 0 {
                s.push(',');
            }
            let _ = write!(s, "{v:.3}");
        }
        s.push(']');
        s.push(if i == 0 { ',' } else { ')' });
    }
    s
}

pub fn timing_js(timing: Option<&Value>) -> String {
    format!("__kai.setTiming({})", timing.unwrap_or(&Value::Null))
}

pub fn reduced_js(reduced: bool) -> String {
    format!("__kai.setReduced({reduced})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::levels::BANDS;
    use crate::overlay::lifecycle::RingState;

    /// Every file the page names (`href`, `src`, `import … from`) is served.
    #[test]
    fn every_file_the_page_loads_is_served() {
        let mut wanted = vec!["/ring.html".to_owned()];
        for a in ASSETS {
            let text = std::str::from_utf8(a.body).unwrap();
            let dir = a.path.rsplit_once('/').unwrap().0;
            for (pat, end) in [("href=\"", '"'), ("src=\"", '"'), ("from \"./", '"')] {
                for part in text.split(pat).skip(1) {
                    let rel = part.split(end).next().unwrap().trim_start_matches("./");
                    wanted.push(format!("{dir}/{rel}"));
                }
            }
        }
        for p in &wanted {
            assert!(asset("app", p).is_some(), "{p} is not served");
        }
        assert!(
            wanted.iter().any(|p| p == "/viz/engine.js"),
            "the scan saw the imports: {wanted:?}"
        );
    }

    #[test]
    fn nothing_else_is_served() {
        for (host, path) in [
            ("app", "/"),
            ("app", "/../Cargo.toml"),
            ("app", "/viz/orb.js"),
            ("other", "/ring.html"),
        ] {
            assert!(asset(host, path).is_none(), "{host}{path}");
        }
        let (mime, body) = asset("app", "/ring.html").unwrap();
        assert_eq!(mime, "text/html");
        assert!(
            std::str::from_utf8(body).unwrap().contains("boot.js"),
            "boot is an external module (K10)"
        );
        assert_eq!(page_uri(true), "kai://app/ring.html?reduced=1");
    }

    #[test]
    fn page_messages_are_parsed() {
        assert_eq!(parse(r#"{"ev":"ready","hidden":true}"#), PageMsg::Ready);
        assert_eq!(parse(r#"{"ev":"drawn","frames":3}"#), PageMsg::Drawn);
        assert_eq!(
            parse(r#"{"ev":"settled","seq":7}"#),
            PageMsg::Settled { seq: 7 }
        );
        assert_eq!(
            parse(r#"{"ev":"error","msg":"x is not defined"}"#),
            PageMsg::Error("x is not defined".into())
        );
        assert_eq!(
            parse(r#"{"ev":"csp","directive":"script-src","blocked":"inline"}"#),
            PageMsg::Csp {
                directive: "script-src".into(),
                blocked: "inline".into()
            }
        );
        assert!(matches!(
            parse(r#"{"ev":"stats","frames":1}"#),
            PageMsg::Stats(_)
        ));
        assert!(matches!(parse("not json"), PageMsg::Other(_)));
        assert!(matches!(parse(r#"{"ev":"settled"}"#), PageMsg::Other(_)));
    }

    #[test]
    fn calls_into_the_page_are_well_formed() {
        let p = PageState {
            state: RingState::Thinking,
            muted: true,
            loading: Some("a \"quoted\" alias".into()),
            held: false,
            seq: 4,
        };
        assert_eq!(
            set_state_js(&p),
            r#"__kai.setState("thinking",{"held":false,"loading":"a \"quoted\" alias","muted":true},4)"#
        );
        let mut out = Levels::SILENT;
        out.level = 0.5;
        out.bands[0] = 1.0;
        out.bands[47] = 0.12345;
        let js = feed_js(&out, &Levels::SILENT);
        assert!(
            js.starts_with("__kai.feed(0.500,0.000,[1.000,0.000,"),
            "{js}"
        );
        assert!(js.ends_with(",0.000])"), "{js}");
        assert!(js.contains(",0.123],[0.000,"), "{js}");
        assert_eq!(js.matches(',').count(), 2 + 2 * (BANDS - 1) + 1, "{js}");
        assert!(js.len() < 720, "{} bytes", js.len());
        assert_eq!(timing_js(None), "__kai.setTiming(null)");
        assert_eq!(
            timing_js(Some(&json!({"first_audio_ms": 800}))),
            r#"__kai.setTiming({"first_audio_ms":800})"#
        );
        assert_eq!(reduced_js(true), "__kai.setReduced(true)");
    }
}
