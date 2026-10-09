//! The overlay kinds (ruling 24): what each show tool's arguments become.
//!
//! **A new kind**: a [`Kind`] variant with its arms here,
//! its table in `[overlay]` (`settings::of`), its contract entry (`contract/overlay.rs`). Its body
//! is text Kai's host page renders as text ([`Body::Text`]), a document shown in the sandboxed
//! frame ([`Body::Document`]), or an MCP Apps view with its bridge ([`Body::App`], WP12: made by
//! `super::app_bridge::Apps::open`, not by [`Kind::body`]).

use super::fetch::Policy;
use super::weather::WeatherConfig;
use crate::host::registry::Args;
use crate::overlay::placement::Anchor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Text,
    Html,
    Image,
    Emote,
    Weather,
    App,
}

/// What a kind that fetches needs besides its arguments: the fetch rules, the saved places, and
/// the weather origin (Bright Sky; a test points it at a loopback server).
#[derive(Debug, Clone)]
pub struct Sources {
    pub fetch: Policy,
    pub weather: WeatherConfig,
    pub weather_base: String,
}

impl Sources {
    pub fn of(overlay: &crate::config::OverlayConfig, weather: &WeatherConfig) -> Self {
        Self {
            fetch: Policy::of(overlay),
            weather: weather.clone(),
            weather_base: super::weather::BRIGHT_SKY.to_owned(),
        }
    }
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Text,
        Kind::Html,
        Kind::Image,
        Kind::Emote,
        Kind::Weather,
        Kind::App,
    ];

    /// The kind's word: its table in `[overlay]`, `overlay_list`'s column.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Html => "html",
            Kind::Image => "image",
            Kind::Emote => "emote",
            Kind::Weather => "weather",
            Kind::App => "app",
        }
    }

    /// Its show tool in the contract (`contract/overlay.rs`).
    pub fn tool(self) -> &'static str {
        match self {
            Kind::Text => "overlay_text",
            Kind::Html => "overlay_html",
            Kind::Image => "overlay_image",
            Kind::Emote => "overlay_emote",
            Kind::Weather => "overlay_weather",
            Kind::App => "overlay_app",
        }
    }

    pub fn of_tool(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.tool() == name)
    }

    /// The parameter that carries the content (`overlay_app`'s first of two).
    pub(super) fn content(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Html => "html",
            Kind::Image => "src",
            Kind::Emote => "name",
            Kind::Weather => "place",
            Kind::App => "server",
        }
    }

    /// The host page can tell the content's height, so `height = 0` (follow the content) is
    /// allowed: Kai lays the text out itself. An HTML document is the caller's, in a frame Kai
    /// cannot look into (no bridge, K18).
    pub fn measures(self) -> bool {
        match self {
            Kind::Text => true,
            // A picture, an emote and the weather card have a size of their own (`[overlay.<kind>]`).
            // An app's view is a document like overlay_html's.
            Kind::Html | Kind::Image | Kind::Emote | Kind::Weather | Kind::App => false,
        }
    }

    /// The call's arguments as the content to show; the error goes back to the caller. Async for
    /// the kinds that fetch (K19: images and Bright Sky's weather, nothing else).
    pub async fn body(self, args: &Args, from: &Sources) -> Result<Body, String> {
        let name = self.content();
        let content = args.text(name).unwrap_or_default();
        if content.trim().is_empty() && self != Kind::Weather {
            return Err(format!("{}: {name} is empty", self.tool()));
        }
        match self {
            Kind::Text => Ok(Body::Text(content.to_owned())),
            Kind::Html => Ok(Body::Document(content.to_owned())),
            Kind::Image => super::image::image(content, &from.fetch).await,
            Kind::Emote => super::image::emote(content),
            Kind::Weather => {
                super::weather::weather(
                    Some(content),
                    &from.weather,
                    &from.weather_base,
                    &from.fetch,
                )
                .await
            }
            Kind::App => Err("overlay_app's view is opened by its bridge (app_bridge)".into()),
        }
    }
}

/// What an overlay shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Plain text, rendered by Kai's host page as text, never as markup.
    Text(String),
    /// An HTML document, served to the sandboxed frame under §10.1's CSP.
    Document(String),
    /// An MCP Apps view: its document, served as [`Body::Document`]'s, and its bridge (K18).
    App(super::app_bridge::AppBody),
}

/// The positions' words, as `position` and `[overlay.<kind>] position` take them.
pub const POSITIONS: [(Anchor, &str); 9] = [
    (Anchor::TopLeft, "top-left"),
    (Anchor::Top, "top"),
    (Anchor::TopRight, "top-right"),
    (Anchor::Left, "left"),
    (Anchor::Center, "center"),
    (Anchor::Right, "right"),
    (Anchor::BottomLeft, "bottom-left"),
    (Anchor::Bottom, "bottom"),
    (Anchor::BottomRight, "bottom-right"),
];

pub fn position_key(a: Anchor) -> &'static str {
    POSITIONS
        .iter()
        .find(|(p, _)| *p == a)
        .map(|(_, k)| *k)
        .unwrap_or("bottom-right")
}

/// A `position` argument: the words above, in any case, with a space or an underscore for the
/// dash ("Bottom right"). The error lists the words.
pub fn parse_position(tool: &str, s: &str) -> Result<Anchor, String> {
    let norm: String = s
        .trim()
        .chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    POSITIONS
        .iter()
        .find(|(_, k)| *k == norm)
        .map(|(a, _)| *a)
        .ok_or_else(|| {
            let words: Vec<&str> = POSITIONS.iter().map(|(_, k)| *k).collect();
            format!("{tool}: position must be one of {}", words.join(", "))
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sources() -> Sources {
        Sources::of(&Default::default(), &Default::default())
    }

    fn args(kind: Kind, v: serde_json::Value) -> Args {
        let (_, spec) = kai_desktop::contract::find(kind.tool()).unwrap();
        Args::read(spec, v.as_object().cloned()).unwrap()
    }

    #[test]
    fn every_kind_has_its_contract_entry_and_its_parameter() {
        for k in Kind::ALL {
            let (area, spec) =
                kai_desktop::contract::find(k.tool()).unwrap_or_else(|| panic!("{}", k.tool()));
            assert_eq!(area.name, "overlay");
            assert_eq!(spec.params[0].name, k.content());
            assert_eq!(
                spec.params[0].required,
                k != Kind::Weather,
                "only the weather's place has a default"
            );
            // overlay_app's second names the view; its place is the configuration's (K22).
            let second = if k == Kind::App { "uri" } else { "position" };
            assert_eq!(spec.params[1].name, second);
            assert_eq!(Kind::of_tool(k.tool()), Some(k));
        }
        assert_eq!(Kind::of_tool("overlay_hide"), None);
    }

    #[tokio::test]
    async fn arguments_become_the_body_never_shortened() {
        let long = "x".repeat(200_000);
        assert_eq!(
            Kind::Text
                .body(&args(Kind::Text, json!({"text": long})), &sources())
                .await,
            Ok(Body::Text("x".repeat(200_000)))
        );
        assert_eq!(
            Kind::Html
                .body(&args(Kind::Html, json!({"html": "<p>hi</p>"})), &sources())
                .await,
            Ok(Body::Document("<p>hi</p>".into()))
        );
        assert_eq!(
            Kind::Text
                .body(&args(Kind::Text, json!({"text": "  \n"})), &sources())
                .await,
            Err("overlay_text: text is empty".into())
        );
    }

    #[test]
    fn positions_read_leniently_and_the_error_lists_them() {
        assert_eq!(
            parse_position("overlay_text", "bottom-right"),
            Ok(Anchor::BottomRight)
        );
        assert_eq!(
            parse_position("overlay_text", " Top Left "),
            Ok(Anchor::TopLeft)
        );
        assert_eq!(
            parse_position("overlay_text", "bottom_left"),
            Ok(Anchor::BottomLeft)
        );
        assert_eq!(parse_position("overlay_text", "CENTER"), Ok(Anchor::Center));
        let e = parse_position("overlay_html", "middle").unwrap_err();
        assert_eq!(
            e,
            "overlay_html: position must be one of top-left, top, top-right, left, center, right, \
             bottom-left, bottom, bottom-right"
        );
        for (a, k) in POSITIONS {
            assert_eq!(position_key(a), k);
            // The configuration's words are the same (`placement::Anchor`'s kebab-case).
            let read: Anchor = serde_json::from_value(json!(k)).unwrap();
            assert_eq!(read, a);
        }
    }
}
