//! The overlays' configuration (§10.1: output, margin, size, timeout and interactive come from the
//! configuration's defaults, not from parameters). `crate::config`'s `[overlay]` holds the shared
//! output and one table per kind:
//!
//! ```toml
//! [overlay]
//! output = "DP-1"         # the overlays' output, a connector name; absent: KWin's active output
//!                         # at each show
//! show_timeout_s = 10     # how long a show may take from the call to the map; past it the call
//!                         # fails, naming this key. At least 3: the output's resolution alone may
//!                         # take 2 s (1 s to list the outputs, 1 s for KWin's active output), and
//!                         # the widgets' web process starts in about 0.3 s (measured, WP10)
//! unresponsive_s = 3      # Kai pings the widgets' web process this often; a ping still
//!                         # unanswered at the next one (a script that never yields) ends the
//!                         # process and every widget in it, each listed once by overlay_list with
//!                         # why. 3 is WebKit's own responsiveness timeout
//!
//! [overlay.text]          # overlay_text
//! position = "top-right"  # where a show without `position` puts it (the ring's anchors)
//! margin = 24             # px from the anchored edges
//! width = 360             # px
//! height = 0              # px; 0: as tall as the text, up to the output's height less the
//!                         # margins (taller text scrolls, and the tool's answer says so)
//! timeout_s = 0           # hidden this long after it shows; 0: until it is hidden
//! interactive = false     # true: it takes clicks on its rectangle (no keyboard); false: clicks
//!                         # go through to what is under it
//!
//! [overlay.html]          # overlay_html: the same keys; height must be set (an HTML view has no
//! position = "top-left"   # way to tell its size, K18)
//! width = 480
//! height = 320
//!
//! [overlay.image]         # overlay_image: the same keys, height must be set
//! position = "bottom-right"
//! width = 480
//! height = 320
//!
//! [overlay.emote]         # overlay_emote
//! position = "bottom-right"
//! width = 160
//! height = 160
//!
//! [overlay.weather]       # overlay_weather
//! position = "top-right"
//! width = 300
//! height = 230
//!
//! [overlay.app]           # MCP Apps views (WP12): overlay_app's, and a tool call's in Kai's
//! position = "center"     # conversation; no position parameter (K22: overlay_app's two name
//! width = 480             # the view), so this places every one
//! height = 360
//! interactive = true      # an app's view takes clicks by default (decided by the owner,
//!                         # 2026-10-09); MCP Apps has no flag of its own for it
//! auto_show = true        # a tool call in Kai's conversation whose tool names a UI resource
//!                         # shows that view by itself, with the call's input and result
//!                         # (`super::thread_apps`); false: only overlay_app shows views. A key of
//!                         # [overlay.app] only
//!
//! app_max_mb = 64         # the most a view's message or one of lmgw's /mcp answers to its
//!                         # bridge may be (the view's document included); a bigger one fails
//!                         # naming this. 64 is lmgw's own MCP message default
//! app_max_calls = 16      # the most requests one view may have in flight (a tool call waiting
//!                         # for lmgw or for the owner's yes counts until its answer is handed
//!                         # to the view); one more is refused naming this. Each answer may be
//!                         # app_max_mb, so this also bounds what waits for the view
//!
//! # The fetch rules of overlay_image and overlay_weather (K19, `super::fetch`):
//! image_max_mb = 32               # the most a fetch may download, and the most pixel memory
//!                                 # (RGBA) a picture may unpack to; a bigger one fails naming this
//! fetch_private_addresses = true  # false: loopback, link-local and LAN addresses are refused
//! fetch_timeout_s = 15            # the whole request's deadline
//! ```
//!
//! A size larger than the output (less the margins) is fitted to it, and the tool's answer says so.

use serde::{Deserialize, Deserializer};

use super::kinds::Kind;
use crate::config::OverlayConfig;
use crate::overlay::outputs::{KWIN_TIMEOUT, OUTPUTS_TIMEOUT};
use crate::overlay::placement::Anchor;

/// `overlay.show_timeout_s`'s default.
pub const SHOW_TIMEOUT_S: u64 = 10;

/// `overlay.unresponsive_s`'s default: WebKit's own responsiveness timeout.
pub const UNRESPONSIVE_S: u64 = 3;

/// `overlay.show_timeout_s`'s least value: the output's resolution's own bounds (the outputs'
/// list, KWin's active output), and a second for the web process's cold start (0.3 s measured).
pub fn least_show_timeout_s() -> u64 {
    (OUTPUTS_TIMEOUT + KWIN_TIMEOUT).as_secs_f64().ceil() as u64 + 1
}

/// `overlay.image_max_mb`'s default (K19).
pub const IMAGE_MAX_MB: u64 = 32;

/// `overlay.app_max_mb`'s default: lmgw's own `mcp.host_max_message_mb` default.
pub const APP_MAX_MB: u64 = 64;

/// `overlay.app_max_calls`'s default: room for a view that loads several things at once, while
/// its waiting answers stay bounded (16 × `app_max_mb`).
pub const APP_MAX_CALLS: usize = 16;

/// `overlay.fetch_timeout_s`'s default.
pub const FETCH_TIMEOUT_S: u64 = 15;

/// One kind's defaults.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KindConfig {
    pub position: Anchor,
    pub margin: u32,
    pub width: u32,
    /// 0: the content's height, for a kind that can tell it ([`Kind::measures`]).
    pub height: u32,
    /// 0: until hidden.
    pub timeout_s: u64,
    pub interactive: bool,
    /// `[overlay.app]`'s only: a tool call in Kai's conversation shows its view by itself
    /// (`super::thread_apps`). `None` for every other kind, which has no such key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_show: Option<bool>,
}

impl KindConfig {
    pub fn text() -> Self {
        Self {
            position: Anchor::TopRight,
            margin: 24,
            width: 360,
            height: 0,
            timeout_s: 0,
            interactive: false,
            auto_show: None,
        }
    }

    pub fn html() -> Self {
        Self {
            position: Anchor::TopLeft,
            margin: 24,
            width: 480,
            height: 320,
            timeout_s: 0,
            interactive: false,
            auto_show: None,
        }
    }

    pub fn image() -> Self {
        Self {
            position: Anchor::BottomRight,
            margin: 24,
            width: 480,
            height: 320,
            timeout_s: 0,
            interactive: false,
            auto_show: None,
        }
    }

    pub fn emote() -> Self {
        Self {
            position: Anchor::BottomRight,
            margin: 24,
            width: 160,
            height: 160,
            timeout_s: 0,
            interactive: false,
            auto_show: None,
        }
    }

    pub fn weather() -> Self {
        Self {
            position: Anchor::TopRight,
            margin: 24,
            width: 300,
            height: 230,
            timeout_s: 0,
            interactive: false,
            auto_show: None,
        }
    }

    /// Interactive by default (decided by the owner, 2026-10-09): a view is made to be used.
    pub fn app() -> Self {
        Self {
            position: Anchor::Center,
            margin: 24,
            width: 480,
            height: 360,
            timeout_s: 0,
            interactive: true,
            auto_show: Some(true),
        }
    }

    /// `(key, value, rule)` for the first value that breaks a rule; `measures` is whether the
    /// kind can size itself to its content.
    fn validate(
        &self,
        width_key: &'static str,
        height_key: &'static str,
        measures: bool,
    ) -> Result<(), (&'static str, String, String)> {
        if self.width == 0 {
            return Err((width_key, "0".into(), "must be at least 1 (px)".into()));
        }
        if self.height == 0 && !measures {
            return Err((
                height_key,
                "0".into(),
                "must be at least 1 (px): this kind cannot tell its content's height".into(),
            ));
        }
        Ok(())
    }
}

/// A kind's table as written: each key it leaves out takes the kind's own default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    position: Option<Anchor>,
    margin: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    timeout_s: Option<u64>,
    interactive: Option<bool>,
    auto_show: Option<bool>,
}

impl Written {
    fn over(self, d: KindConfig) -> Result<KindConfig, String> {
        if self.auto_show.is_some() && d.auto_show.is_none() {
            return Err("auto_show is a key of [overlay.app] only".into());
        }
        Ok(KindConfig {
            position: self.position.unwrap_or(d.position),
            margin: self.margin.unwrap_or(d.margin),
            width: self.width.unwrap_or(d.width),
            height: self.height.unwrap_or(d.height),
            timeout_s: self.timeout_s.unwrap_or(d.timeout_s),
            interactive: self.interactive.unwrap_or(d.interactive),
            auto_show: self.auto_show.or(d.auto_show),
        })
    }
}

fn read<'de, D: Deserializer<'de>>(d: D, base: KindConfig) -> Result<KindConfig, D::Error> {
    Written::deserialize(d).and_then(|w| w.over(base).map_err(serde::de::Error::custom))
}

/// `[overlay.text]` over [`KindConfig::text`] (`#[serde(deserialize_with)]` in `crate::config`).
pub fn text<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::text())
}

/// `[overlay.html]` over [`KindConfig::html`].
pub fn html<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::html())
}

/// `[overlay.image]` over [`KindConfig::image`].
pub fn image<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::image())
}

/// `[overlay.emote]` over [`KindConfig::emote`].
pub fn emote<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::emote())
}

/// `[overlay.weather]` over [`KindConfig::weather`].
pub fn weather<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::weather())
}

/// `[overlay.app]` over [`KindConfig::app`].
pub fn app<'de, D: Deserializer<'de>>(d: D) -> Result<KindConfig, D::Error> {
    read(d, KindConfig::app())
}

/// The kind's table in `[overlay]`. A new kind adds its field to `OverlayConfig`
/// (`crate::config`), a `KindConfig` constructor and reader above, an arm here and a line in
/// [`validate`].
pub fn of(overlay: &OverlayConfig, kind: Kind) -> &KindConfig {
    match kind {
        Kind::Text => &overlay.text,
        Kind::Html => &overlay.html,
        Kind::Image => &overlay.image,
        Kind::Emote => &overlay.emote,
        Kind::Weather => &overlay.weather,
        Kind::App => &overlay.app,
    }
}

/// `(key, value, rule)` for the first value of the widgets' part of `[overlay]` that breaks a rule.
pub fn validate(overlay: &OverlayConfig) -> Result<(), (&'static str, String, String)> {
    if overlay
        .output
        .as_deref()
        .is_some_and(|o| o.trim().is_empty())
    {
        return Err((
            "overlay.output",
            "\"\"".into(),
            "a connector name (DP-1), or leave the key out for KWin's active output".into(),
        ));
    }
    let least = least_show_timeout_s();
    if overlay.show_timeout_s < least {
        return Err((
            "overlay.show_timeout_s",
            overlay.show_timeout_s.to_string(),
            format!(
                "must be at least {least} (seconds: the output's resolution alone may take {} ms, \
                 and the widgets' web process has to start)",
                (OUTPUTS_TIMEOUT + KWIN_TIMEOUT).as_millis()
            ),
        ));
    }
    if overlay.unresponsive_s == 0 {
        return Err((
            "overlay.unresponsive_s",
            "0".into(),
            "must be at least 1 (seconds)".into(),
        ));
    }
    if overlay.image_max_mb == 0 {
        return Err((
            "overlay.image_max_mb",
            "0".into(),
            "must be at least 1 (MB)".into(),
        ));
    }
    if overlay.app_max_mb == 0 {
        return Err((
            "overlay.app_max_mb",
            "0".into(),
            "must be at least 1 (MB)".into(),
        ));
    }
    if overlay.app_max_calls == 0 {
        return Err((
            "overlay.app_max_calls",
            "0".into(),
            "must be at least 1 (a view with none could call nothing)".into(),
        ));
    }
    if overlay.fetch_timeout_s == 0 {
        return Err((
            "overlay.fetch_timeout_s",
            "0".into(),
            "must be at least 1 (seconds)".into(),
        ));
    }
    for (kind, c) in [
        (Kind::Text, &overlay.text),
        (Kind::Html, &overlay.html),
        (Kind::Image, &overlay.image),
        (Kind::Emote, &overlay.emote),
        (Kind::Weather, &overlay.weather),
        (Kind::App, &overlay.app),
    ] {
        let k = kind.key();
        // The key names are static; a table per kind (one line each in `of`'s arms).
        let (w, h) = match kind {
            Kind::Text => ("overlay.text.width", "overlay.text.height"),
            Kind::Html => ("overlay.html.width", "overlay.html.height"),
            Kind::Image => ("overlay.image.width", "overlay.image.height"),
            Kind::Emote => ("overlay.emote.width", "overlay.emote.height"),
            Kind::Weather => ("overlay.weather.width", "overlay.weather.height"),
            Kind::App => ("overlay.app.width", "overlay.app.height"),
        };
        debug_assert!(w.contains(k) && h.contains(k));
        c.validate(w, h, kind.measures())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_and_the_rules() {
        let o = OverlayConfig::default();
        assert_eq!(validate(&o), Ok(()));
        assert_eq!(o.text.height, 0, "text follows its lines");
        assert_eq!(
            o.text.timeout_s, 0,
            "no timeout: shown until hidden (§10.1)"
        );
        assert!(
            !o.text.interactive && !o.html.interactive,
            "click-through by default"
        );
        assert_eq!(of(&o, Kind::Html), &KindConfig::html());
        let mut bad = o.clone();
        bad.html.height = 0;
        let (key, _, rule) = validate(&bad).unwrap_err();
        assert_eq!(key, "overlay.html.height");
        assert!(rule.contains("cannot tell"), "{rule}");
        let mut bad = o.clone();
        bad.text.width = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.text.width");
        let mut bad = o.clone();
        bad.output = Some(" ".into());
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.output");
        assert_eq!(o.show_timeout_s, 10);
        assert_eq!(least_show_timeout_s(), 3);
        let mut bad = o;
        bad.show_timeout_s = 2;
        let (key, value, rule) = validate(&bad).unwrap_err();
        assert_eq!((key, value.as_str()), ("overlay.show_timeout_s", "2"));
        assert!(rule.starts_with("must be at least 3 (seconds"), "{rule}");
        bad.show_timeout_s = 3;
        assert_eq!(validate(&bad), Ok(()));
        bad.unresponsive_s = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.unresponsive_s");
    }

    #[test]
    fn a_kind_table_reads_with_its_defaults_and_refuses_unknown_keys() {
        let o: OverlayConfig =
            toml::from_str("output = \"DP-1\"\n[text]\nposition = \"bottom\"\ntimeout_s = 20\n")
                .unwrap();
        assert_eq!(o.output.as_deref(), Some("DP-1"));
        assert_eq!(o.text.position, Anchor::Bottom);
        assert_eq!(o.text.timeout_s, 20);
        assert_eq!(o.text.width, 360);
        assert_eq!(o.html, KindConfig::html());
        assert!(o.ring_over_fullscreen);
        // A key left out takes its own kind's default, not another kind's.
        let o: OverlayConfig = toml::from_str("[html]\nwidth = 500\n").unwrap();
        assert_eq!((o.html.width, o.html.height), (500, 320));
        assert_eq!(validate(&o), Ok(()));
        assert!(toml::from_str::<OverlayConfig>("[text]\nsize = 3\n").is_err());
        assert!(toml::from_str::<OverlayConfig>("[app]\nsize = 3\n").is_err());
        let d = OverlayConfig::default();
        assert!(
            d.app.interactive,
            "an app's view takes clicks by default (the owner, 2026-10-09)"
        );
        assert_eq!(d.app.auto_show, Some(true));
        assert_eq!(d.text.auto_show, None);
        let o: OverlayConfig =
            toml::from_str("app_max_mb = 8\n[app]\ninteractive = false\nauto_show = false\n")
                .unwrap();
        assert_eq!(o.app_max_mb, 8);
        assert!(!o.app.interactive);
        assert_eq!(o.app.auto_show, Some(false));
        let e = toml::from_str::<OverlayConfig>("[html]\nauto_show = true\n").unwrap_err();
        assert!(
            e.to_string()
                .contains("auto_show is a key of [overlay.app] only"),
            "{e}"
        );
        assert_eq!(
            (o.app.position, o.app.width, o.app.height),
            (Anchor::Center, 480, 360)
        );
        assert_eq!(o.app_max_calls, APP_MAX_CALLS);
        let mut bad = o.clone();
        bad.app_max_mb = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.app_max_mb");
        bad = o.clone();
        bad.app_max_calls = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.app_max_calls");
        let o4: OverlayConfig = toml::from_str("app_max_calls = 4\n").unwrap();
        assert_eq!(o4.app_max_calls, 4);
        bad = o;
        bad.app.height = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.app.height");
        let o: OverlayConfig = toml::from_str(
            "image_max_mb = 8\n[image]\nposition = \"top\"\n[weather]\nwidth = 400\n",
        )
        .unwrap();
        assert_eq!(
            (o.image_max_mb, o.image.position, o.image.width),
            (8, Anchor::Top, 480)
        );
        assert_eq!((o.weather.width, o.weather.height), (400, 230));
        assert!(o.fetch_private_addresses && o.fetch_timeout_s == 15);
        let mut bad = o.clone();
        bad.image_max_mb = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.image_max_mb");
        bad = o.clone();
        bad.emote.height = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.emote.height");
        bad = o;
        bad.fetch_timeout_s = 0;
        assert_eq!(validate(&bad).unwrap_err().0, "overlay.fetch_timeout_s");
    }
}
