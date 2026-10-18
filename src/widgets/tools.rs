//! The overlay tools' provider (§10.1): `contract/overlay.rs`'s entries, each call handed to the
//! GTK thread as `UiCmd::Widgets` and answered from there. Off the core (PD3).
//!
//! Answers (K22: compact, no echo of the input): a show says `id: <n>` and, a line each, what the
//! caller should know (a size fitted to the output, text that scrolls); `overlay_hide` says which
//! ids it hid; `overlay_list` one line per overlay.

use std::sync::Arc;
use std::time::Duration;

use kai_desktop::contract::{ToolSpec, overlay::AREA};
use tokio::sync::oneshot;

use super::app_bridge::Apps;
use super::gtk_host::{Look, ShowRequest, Shown, WidgetsCmd};
use super::kinds::{Body, Kind, Sources, parse_position};
use super::settings;
use super::table::ids;
use crate::config::OverlayConfig;
use crate::host::registry::{Call, Deps, Entry, Handler, Provider, Reply};
use crate::ui::UiCmd;

pub struct WidgetTools {
    ui: async_channel::Sender<UiCmd>,
    overlay: Arc<OverlayConfig>,
    /// The fetch rules, saved places and weather origin (K19).
    from: Arc<Sources>,
    /// `overlay_app`'s way to lmgw's `/mcp` and the owner's dialog (WP12).
    apps: Arc<Apps>,
}

/// The widgets' provider: the configuration's `[overlay]` as read at start, the GTK channel, and
/// for `overlay_app` the pairing's credentials and K15's dialog.
pub fn provider(deps: &Deps) -> WidgetTools {
    let connect = deps.config.lmgw.timeout();
    super::set_held_margin(settings::of(&deps.config.overlay, Kind::App).margin);
    let tools = WidgetTools {
        ui: deps.ui.clone(),
        overlay: Arc::new(deps.config.overlay.clone()),
        from: Arc::new(Sources::of(&deps.config.overlay, &deps.config.weather)),
        apps: Arc::new(Apps {
            creds: deps.creds.clone(),
            connect,
            bound: app_bound(&deps.config.overlay),
            calls: app_calls(&deps.config.overlay),
            confirm: Arc::new(crate::host::confirm::Dialog(deps.ui.clone())),
            policy: deps.policy.clone(),
            lists: Default::default(),
            screen: deps.screen.clone(),
        }),
    };
    // The views of the calls in Kai's own conversation open the same way (`thread_apps`).
    super::thread_apps::set(super::thread_apps::Ctx {
        apps: tools.apps.clone(),
        ui: tools.ui.clone(),
        overlay: tools.overlay.clone(),
    });
    tools
}

/// `overlay.app_max_mb` as the bridge bounds by it.
pub fn app_bound(overlay: &OverlayConfig) -> crate::lmgw_link::mcp::Bound {
    crate::lmgw_link::mcp::Bound {
        bytes: overlay.app_max_mb.saturating_mul(1 << 20),
        says: format!("overlay.app_max_mb = {}", overlay.app_max_mb),
    }
}

/// `overlay.app_max_calls` as the bridge bounds by it.
pub fn app_calls(overlay: &OverlayConfig) -> super::app_bridge::Calls {
    super::app_bridge::Calls {
        max: overlay.app_max_calls,
        says: format!("overlay.app_max_calls = {}", overlay.app_max_calls),
    }
}

impl Provider for WidgetTools {
    fn entries(&self) -> Vec<Entry> {
        AREA.tools
            .iter()
            .map(|spec| Entry {
                area: AREA.name,
                spec,
                handler: self.handler(spec),
                prepare: None,
            })
            .collect()
    }
}

const QUITTING: &str = "Kai is quitting; the overlays did not answer";

impl WidgetTools {
    fn handler(&self, spec: &'static ToolSpec) -> Handler {
        let (ui, overlay) = (self.ui.clone(), self.overlay.clone());
        if let Some(kind) = Kind::of_tool(spec.name) {
            let (from, apps) = (self.from.clone(), self.apps.clone());
            return Arc::new(move |call| {
                let (ui, overlay, from, apps) =
                    (ui.clone(), overlay.clone(), from.clone(), apps.clone());
                Box::pin(async move { show(kind, call, &ui, &overlay, &from, &apps).await })
            });
        }
        match spec.name {
            "overlay_hide" => Arc::new(move |call| {
                let ui = ui.clone();
                Box::pin(async move { hide(call, &ui).await })
            }),
            "overlay_list" => Arc::new(move |_| {
                let ui = ui.clone();
                Box::pin(async move {
                    let (reply, answer) = oneshot::channel();
                    if ui
                        .send(UiCmd::Widgets(WidgetsCmd::List { reply }))
                        .await
                        .is_err()
                    {
                        return Reply::error(QUITTING);
                    }
                    match answer.await {
                        Ok(lines) => Reply::text(lines.join("\n")),
                        Err(_) => Reply::error(QUITTING),
                    }
                })
            }),
            other => {
                // The provider test (`every contract area's tools have a provider`) catches this.
                tracing::error!(tool = other, "widgets: a contract entry with no handler");
                Arc::new(move |_| {
                    Box::pin(async move { Reply::error(format!("Kai has no handler for {other}")) })
                })
            }
        }
    }
}

/// The look of a show of `kind`: the kind's `[overlay.<kind>]`, the call's `position` over its
/// default.
pub fn look(kind: Kind, position: Option<&str>, overlay: &OverlayConfig) -> Result<Look, String> {
    let c = settings::of(overlay, kind);
    let position = match position {
        Some(p) => parse_position(kind.tool(), p)?,
        None => c.position,
    };
    Ok(Look {
        position,
        margin: c.margin,
        width: c.width,
        height: c.height,
        timeout: (c.timeout_s > 0).then(|| Duration::from_secs(c.timeout_s)),
        interactive: c.interactive,
        output: overlay.output.clone(),
        show_timeout: Duration::from_secs(overlay.show_timeout_s),
        unresponsive: Duration::from_secs(overlay.unresponsive_s),
    })
}

/// A show's answer text.
pub fn shown_text(s: &Shown) -> String {
    std::iter::once(format!("id: {}", s.id))
        .chain(s.notes.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn show(
    kind: Kind,
    call: Call,
    ui: &async_channel::Sender<UiCmd>,
    overlay: &OverlayConfig,
    from: &Sources,
    apps: &Apps,
) -> Reply {
    let look = match look(kind, call.args.text("position"), overlay) {
        Ok(l) => l,
        Err(e) => return Reply::error(e),
    };
    let body = if kind == Kind::App {
        let (server, uri) = (
            call.args.text("server").unwrap_or_default(),
            call.args.text("uri").unwrap_or_default(),
        );
        apps.open(server, uri, app_size(&look)).await.map(Body::App)
    } else {
        kind.body(&call.args, from).await
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => return Reply::error(e),
    };
    match present(ui, kind, body, look).await {
        Ok(shown) => Reply::text(shown_text(&shown)),
        Err(e) => Reply::error(e),
    }
}

/// An MCP Apps view's own size (`hostContext.containerDimensions`): the configured one, less Kai's
/// strip above an interactive view (`super::view::STRIP_PX`, §10.1).
pub fn app_size(look: &Look) -> (u32, u32) {
    let strip = if look.interactive {
        super::view::STRIP_PX
    } else {
        0
    };
    (look.width, look.height.saturating_sub(strip))
}

/// Hands a show to the GTK side and waits for its answer; the error says why (`<tool>: not shown:
/// …`, or Kai quitting).
pub async fn present(
    ui: &async_channel::Sender<UiCmd>,
    kind: Kind,
    body: Body,
    look: Look,
) -> Result<Shown, String> {
    let (reply, answer) = oneshot::channel();
    let req = ShowRequest {
        kind,
        body,
        look,
        reply,
    };
    if ui
        .send(UiCmd::Widgets(WidgetsCmd::Show(req)))
        .await
        .is_err()
    {
        return Err(QUITTING.into());
    }
    match answer.await {
        Ok(Ok(shown)) => Ok(shown),
        Ok(Err(why)) => Err(format!("{}: not shown: {why}", kind.tool())),
        Err(_) => Err(QUITTING.into()),
    }
}

async fn hide(call: Call, ui: &async_channel::Sender<UiCmd>) -> Reply {
    let id = match call.args.integer("id") {
        Some(i) if i < 1 => return Reply::error("overlay_hide: id must be at least 1"),
        Some(i) => Some(i as u64),
        None => None,
    };
    let (reply, answer) = oneshot::channel();
    if ui
        .send(UiCmd::Widgets(WidgetsCmd::Hide {
            id,
            reply: Some(reply),
        }))
        .await
        .is_err()
    {
        return Reply::error(QUITTING);
    }
    match answer.await {
        Ok(Ok(hidden)) if hidden.is_empty() => Reply::text("no overlay was shown"),
        Ok(Ok(hidden)) => Reply::text(format!("hidden: {}", ids(&hidden))),
        Ok(Err(e)) => Reply::error(format!("overlay_hide: {e}")),
        Err(_) => Reply::error(QUITTING),
    }
}

#[cfg(test)]
mod tests {
    use kai_desktop::contract::Level;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::host::policy::{Caller, Decision, Policy};
    use crate::host::registry::Args;
    use crate::host::settings::{Named, PolicyConfig, ToolState};
    use crate::overlay::placement::Anchor;
    use crate::widgets::kinds::Body;

    fn tools(overlay: OverlayConfig) -> (WidgetTools, async_channel::Receiver<UiCmd>) {
        let (ui, rx) = async_channel::unbounded();
        (
            WidgetTools {
                apps: Arc::new(Apps {
                    creds: tokio::sync::watch::channel(None).1,
                    connect: Duration::from_secs(10),
                    bound: app_bound(&overlay),
                    calls: app_calls(&overlay),
                    confirm: Arc::new(crate::host::confirm::Dialog(ui.clone())),
                    policy: Default::default(),
                    lists: Default::default(),
                    screen: Default::default(),
                }),
                ui,
                from: Arc::new(Sources::of(&overlay, &Default::default())),
                overlay: Arc::new(overlay),
            },
            rx,
        )
    }

    fn entry(t: &WidgetTools, name: &str) -> Entry {
        t.entries()
            .into_iter()
            .find(|e| e.spec.name == name)
            .unwrap()
    }

    fn call(e: &Entry, args: serde_json::Value) -> Call {
        Call::new(
            e.spec,
            Args::read(e.spec, args.as_object().cloned()).unwrap(),
            Caller::Owner,
            CancellationToken::new(),
        )
    }

    async fn next(rx: &async_channel::Receiver<UiCmd>) -> WidgetsCmd {
        match rx.recv().await.unwrap() {
            UiCmd::Widgets(c) => c,
            _ => panic!("not a widgets command"),
        }
    }

    #[test]
    fn the_provider_offers_the_overlay_area_in_its_order() {
        let (t, _) = tools(OverlayConfig::default());
        let names: Vec<(&str, &str)> = t.entries().iter().map(|e| (e.area, e.spec.name)).collect();
        assert_eq!(
            names,
            [
                ("overlay", "overlay_text"),
                ("overlay", "overlay_html"),
                ("overlay", "overlay_image"),
                ("overlay", "overlay_emote"),
                ("overlay", "overlay_weather"),
                ("overlay", "overlay_app"),
                ("overlay", "overlay_hide"),
                ("overlay", "overlay_list")
            ]
        );
    }

    #[tokio::test]
    async fn a_show_takes_the_kinds_defaults_and_answers_the_id() {
        let mut overlay = OverlayConfig {
            output: Some("DP-1".into()),
            ..Default::default()
        };
        overlay.text.timeout_s = 20;
        overlay.text.interactive = true;
        let (t, rx) = tools(overlay);
        let e = entry(&t, "overlay_text");
        let pending = (e.handler)(call(&e, json!({"text": "Hello"})));
        let side = async {
            let WidgetsCmd::Show(req) = next(&rx).await else {
                panic!("not a show")
            };
            assert_eq!(req.kind, Kind::Text);
            assert_eq!(req.body, Body::Text("Hello".into()));
            assert_eq!(
                req.look,
                Look {
                    position: Anchor::TopRight,
                    margin: 24,
                    width: 360,
                    height: 0,
                    timeout: Some(Duration::from_secs(20)),
                    interactive: true,
                    output: Some("DP-1".into()),
                    show_timeout: Duration::from_secs(10),
                    unresponsive: Duration::from_secs(3),
                }
            );
            req.reply
                .send(Ok(Shown {
                    id: 7,
                    notes: vec!["the text is 900 px tall …: it scrolls".into()],
                }))
                .unwrap();
        };
        let (reply, ()) = tokio::join!(pending, side);
        assert_eq!(
            reply,
            Reply::text("id: 7\nthe text is 900 px tall …: it scrolls")
        );
    }

    #[tokio::test]
    async fn a_position_overrides_the_default_and_a_bad_one_never_reaches_the_screen() {
        let (t, rx) = tools(OverlayConfig::default());
        let e = entry(&t, "overlay_html");
        let pending = (e.handler)(call(
            &e,
            json!({"html": "<b>x</b>", "position": "bottom left"}),
        ));
        let side = async {
            let WidgetsCmd::Show(req) = next(&rx).await else {
                panic!()
            };
            assert_eq!(req.look.position, Anchor::BottomLeft);
            assert_eq!((req.look.width, req.look.height), (480, 320));
            assert_eq!(req.look.timeout, None, "0: until hidden");
            req.reply
                .send(Err("no output is connected".into()))
                .unwrap();
        };
        let (reply, ()) = tokio::join!(pending, side);
        assert_eq!(
            reply,
            Reply::error("overlay_html: not shown: no output is connected")
        );
        let bad = (e.handler)(call(&e, json!({"html": "<b>x</b>", "position": "upper"}))).await;
        assert!(bad.error);
        assert!(rx.is_empty(), "nothing was sent to the GTK side");
        let empty = (e.handler)(call(&e, json!({"html": " "}))).await;
        assert_eq!(empty, Reply::error("overlay_html: html is empty"));
        assert!(rx.is_empty());
    }

    #[tokio::test]
    async fn hide_and_list_answer_in_lines() {
        let (t, rx) = tools(OverlayConfig::default());
        let hide = entry(&t, "overlay_hide");
        let list = entry(&t, "overlay_list");
        let side = async {
            type Answer = (Option<u64>, Result<Vec<u64>, String>);
            let answers: [Answer; 4] = [
                (Some(3), Ok(vec![3])),
                (None, Ok(vec![4, 5])),
                (None, Ok(vec![])),
                (Some(9), Err("no overlay 9 is shown (shown: 3)".into())),
            ];
            for (want, answer) in answers {
                let WidgetsCmd::Hide { id, reply } = next(&rx).await else {
                    panic!()
                };
                assert_eq!(id, want);
                reply.unwrap().send(answer).unwrap();
            }
            let WidgetsCmd::List { reply } = next(&rx).await else {
                panic!()
            };
            reply
                .send(vec![
                    "1 text top-right shown 12:00:00 until hidden click-through".into(),
                ])
                .unwrap();
        };
        let calls = async {
            let a = (hide.handler)(call(&hide, json!({"id": 3}))).await;
            let b = (hide.handler)(call(&hide, json!({}))).await;
            let c = (hide.handler)(call(&hide, json!({"id": null}))).await;
            let d = (hide.handler)(call(&hide, json!({"id": 9}))).await;
            let l = (list.handler)(call(&list, json!({}))).await;
            (a, b, c, d, l)
        };
        let ((a, b, c, d, l), ()) = tokio::join!(calls, side);
        assert_eq!(a, Reply::text("hidden: 3"));
        assert_eq!(b, Reply::text("hidden: 4, 5"));
        assert_eq!(c, Reply::text("no overlay was shown"), "a null id is no id");
        assert_eq!(
            d,
            Reply::error("overlay_hide: no overlay 9 is shown (shown: 3)")
        );
        assert_eq!(
            l,
            Reply::text("1 text top-right shown 12:00:00 until hidden click-through")
        );
        let neg = (hide.handler)(call(&hide, json!({"id": 0}))).await;
        assert_eq!(neg, Reply::error("overlay_hide: id must be at least 1"));
    }

    /// overlay_app: a URI outside the server's namespace, and an unpaired Kai, are refused before
    /// anything reaches lmgw or the screen; the place is `[overlay.app]`'s.
    #[tokio::test]
    async fn an_app_view_is_refused_before_lmgw_when_it_cannot_be_the_servers() {
        let (t, rx) = tools(OverlayConfig::default());
        let e = entry(&t, "overlay_app");
        let r = (e.handler)(call(&e, json!({"server": "fx", "uri": "ui://other__card"}))).await;
        assert!(r.error);
        assert!(
            matches!(&r.blocks[0], crate::host::registry::Block::Text(t) if t.contains("namespace")),
            "{r:?}"
        );
        let r = (e.handler)(call(&e, json!({"server": "fx", "uri": "ui://fx__card"}))).await;
        assert_eq!(r, Reply::error("overlay_app: Kai is not paired with lmgw"));
        assert!(rx.is_empty(), "nothing was sent to the GTK side");
        let l = look(Kind::App, None, &OverlayConfig::default()).unwrap();
        assert_eq!((l.position, l.width, l.height), (Anchor::Center, 480, 360));
        assert!(
            l.interactive,
            "an app's view takes clicks by default (decided by the owner, 2026-10-09)"
        );
    }

    #[tokio::test]
    async fn without_the_gtk_side_a_call_says_so() {
        let (t, rx) = tools(OverlayConfig::default());
        drop(rx);
        for (name, args) in [
            ("overlay_text", json!({"text": "x"})),
            ("overlay_hide", json!({})),
            ("overlay_list", json!({})),
        ] {
            let e = entry(&t, name);
            assert_eq!((e.handler)(call(&e, args)).await, Reply::error(QUITTING));
        }
    }

    /// PD4: the overlays run for the owner, the gateway and Kai's own key without a question, are
    /// asked about for any other caller, and every one can be set to `allow` (ruling 28).
    #[test]
    fn the_policy_defaults_are_pd4s() {
        let default = Policy::new(&PolicyConfig::default());
        let other = Caller::Named(Named {
            kind: "key".into(),
            name: "laptop".into(),
        });
        for spec in AREA.tools {
            assert_eq!(spec.level, Level::Any, "{}", spec.name);
            for trusted in [Caller::Owner, Caller::Gateway, Caller::Own] {
                assert_eq!(
                    default.decide(spec, &trusted, None),
                    Decision::Allow,
                    "{} for {trusted:?}",
                    spec.name
                );
            }
            assert!(
                matches!(default.decide(spec, &other, None), Decision::Confirm { .. }),
                "{}: an untrusted caller is asked",
                spec.name
            );
            assert!(
                matches!(
                    default.decide(spec, &Caller::Unknown(String::new()), None),
                    Decision::Confirm { .. }
                ),
                "{}",
                spec.name
            );
            let mut c = PolicyConfig::default();
            c.tools.insert(spec.name.into(), ToolState::Allow);
            assert_eq!(
                Policy::new(&c).decide(spec, &other, None),
                Decision::Allow,
                "{} can be set to allow",
                spec.name
            );
            let mut c = PolicyConfig::default();
            c.trusted_callers.push("key:laptop".into());
            assert_eq!(
                Policy::new(&c).decide(spec, &other, None),
                Decision::Allow,
                "{}: a trusted caller",
                spec.name
            );
        }
    }
}
