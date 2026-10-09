//! The README's desktop shots (`scripts/readme-shots.sh`, `just readme-shots`): Kai's overlays as
//! a user sees them, from stand-in content only, in the nested KWin of scripts/nested-kwin.sh (one
//! output at 125 %, a plain backdrop from scripts/readme-backdrop.py), inside a private PipeWire
//! instance. Nothing reaches a real host: the weather comes from a loopback stand-in, the ring's
//! levels from a synthetic tone, the approval card's call is made up here.
//!
//! - `shots/desktop.png`: the ring while Kai speaks, a weather card and a text overlay;
//! - `shots/approval.png`: the approval card for a `type_text` call, the ring listening.

use std::path::Path;
use std::time::Duration;

use gtk::glib;
use serde_json::json;
use tokio::sync::mpsc;

use super::{Run, guard, look, new_run, sleep, wait_ring};
use crate::approvals::card::{CardCmd, CardModel, Look as CardLook, arg_lines};
use crate::audio::LevelTaps;
use crate::host::registry::Args;
use crate::overlay::placement::Anchor;
use crate::overlay::{self, OverlayCmd, Place, RingInput, RingSettings, RingState};
use crate::widgets::fetch::stand_in::{response, serve};
use crate::widgets::gtk_host::{WidgetsCmd, start};
use crate::widgets::kinds::{Body, Kind, Sources};

/// Spectacle's capture of the whole nested screen.
async fn capture(path: &Path) -> bool {
    let f = path.to_path_buf();
    let took = gtk::gio::spawn_blocking(move || {
        std::process::Command::new("spectacle")
            .args(["-b", "-n", "-f", "-o"])
            .arg(&f)
            .env("QT_QPA_PLATFORM", "wayland")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    })
    .await;
    matches!(took, Ok(Ok(s)) if s.success()) && path.exists()
}

/// The weather card's document, from a loopback stand-in of Bright Sky.
fn weather_body() -> Result<Body, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let ok = |v: serde_json::Value| {
            response(
                "200 OK",
                &[("Content-Type", "application/json")],
                v.to_string().as_bytes(),
            )
        };
        let s = serve(vec![
            (
                "/current_weather?lat=52.52&lon=13.405",
                ok(json!({"weather": {"timestamp": "2026-10-09T14:00:00+02:00", "temperature": 17.2,
                    "icon": "partly-cloudy-day", "wind_speed_10": 9.0, "relative_humidity": 58},
                    "sources": [{"station_name": "Berlin-Tempelhof"}]})),
            ),
            (
                "/weather?lat=52.52&lon=13.405&date=2026-10-09",
                ok(json!({"weather": [
                    {"timestamp": "2026-10-09T05:00:00+02:00", "temperature": 9.4, "precipitation": 0.0},
                    {"timestamp": "2026-10-09T15:00:00+02:00", "temperature": 18.1, "precipitation": 0.4}]})),
            ),
        ])
        .await;
        let mut from = Sources::of(&Default::default(), &Default::default());
        from.weather_base = format!("http://127.0.0.1:{}", s.port);
        from.weather
            .places
            .insert("home".into(), "52.52,13.405".into());
        let (_, spec) = kai_desktop::contract::find(Kind::Weather.tool()).unwrap();
        let args = Args::read(spec, json!({"place": ""}).as_object().cloned()).unwrap();
        Kind::Weather.body(&args, &from).await
    })
}

/// A voice-like signal on the playback's tap while `on` holds, so the ring moves as Kai speaks.
fn speak(taps: LevelTaps) -> std::rc::Rc<std::cell::Cell<bool>> {
    let on = std::rc::Rc::new(std::cell::Cell::new(true));
    let still = on.clone();
    let mut t = 0u64;
    glib::timeout_add_local(Duration::from_millis(20), move || {
        let rate = crate::audio::RATE as f64;
        let samples: Vec<i16> = (0..480)
            .map(|i| {
                let x = (t + i) as f64 / rate;
                // Syllables at about 4 per second over a 140 Hz voice with its harmonics.
                let env = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 3.7 * x).sin()).powi(2);
                let f0 = 140.0 + 12.0 * (2.0 * std::f64::consts::PI * 0.8 * x).sin();
                let v: f64 = (1..=8)
                    .map(|h| (2.0 * std::f64::consts::PI * f0 * h as f64 * x).sin() / h as f64)
                    .sum();
                (v * env * 9000.0) as i16
            })
            .collect();
        t += 480;
        taps.feed_output(&samples);
        if still.get() {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
    on
}

fn ring_input(state: RingState, turn: bool) -> RingInput {
    RingInput {
        bound: true,
        active: true,
        turn,
        state,
        muted: false,
        loading: None,
        held: false,
    }
}

async fn drive(r: &mut Run) {
    let shots = r.out.join("shots");
    let _ = std::fs::create_dir_all(&shots);
    let defaults = crate::config::Config::default();
    let handle = start();
    let (ptx, probes) = async_channel::unbounded();
    handle.send(WidgetsCmd::Probe(ptx));
    r.handle = Some(handle);
    r.probes = probes;

    let taps = LevelTaps::silent();
    let ring = overlay::start(
        RingSettings {
            place: Place {
                anchor: defaults.ring.anchor,
                margin: defaults.ring.margin,
                size: defaults.ring.size,
            },
            output: None,
            over_fullscreen: true,
        },
        taps.clone(),
        Box::new(|_| {}),
    );
    let (rtx, ring_probes) = async_channel::unbounded();
    ring.send(OverlayCmd::Probe(rtx));
    let speaking = speak(taps);
    ring.send(OverlayCmd::Ring(ring_input(RingState::Speaking, true)));
    let up = wait_ring(&ring_probes, 10, |p| {
        matches!(p, overlay::Probe::Mapped { .. })
    })
    .await;
    r.check(up, "the ring is up");

    // ---- desktop.png: the ring speaking, a weather card and a text overlay.
    let weather = weather_body();
    r.check(
        weather.is_ok(),
        &format!("the weather card's body: {weather:?}"),
    );
    if let Ok(body) = weather {
        let shown = r
            .show(Kind::Weather, body, look(Anchor::TopRight, None, 300, 230))
            .await;
        r.check(shown.is_ok(), &format!("the weather card: {shown:?}"));
    }
    let text = "Spaghetti aglio e olio, for four\n\n400 g spaghetti\n4 cloves of garlic, thinly sliced\n\
                100 ml olive oil\n1 dried chili\na handful of parsley";
    let shown = r
        .show(
            Kind::Text,
            Body::Text(text.into()),
            look(Anchor::TopLeft, None, 380, 0),
        )
        .await;
    r.check(shown.is_ok(), &format!("the text overlay: {shown:?}"));
    sleep(2500).await;
    let took = capture(&shots.join("desktop.png")).await;
    r.check(took, "shots/desktop.png");
    let _ = r.hide(None).await;

    // ---- approval.png: the card for a call of Kai's own conversation, the ring listening.
    speaking.set(false);
    ring.send(OverlayCmd::Ring(ring_input(RingState::Listening, false)));
    let card = crate::approvals::overlay::start();
    let (events, mut heard) = mpsc::unbounded_channel();
    let arguments =
        json!({"text": "Sounds good, see you Saturday at seven!", "window": "Messages"})
            .to_string();
    let a = &defaults.approvals;
    crate::approvals::overlay::send(CardCmd::Show {
        model: Box::new(CardModel {
            call: "readme-1".into(),
            heading: "Approve kai__type_text?".into(),
            asked: "asked by Kai's voice session in thread 'Weekend plans'".into(),
            args: arg_lines(&arguments),
            count: None,
            note: None,
            busy: None,
            hint: "Enter approves, Esc declines; or hold the talk key and say yes or no.".into(),
            typed: String::new(),
        }),
        look: CardLook {
            anchor: a.anchor,
            margin: a.margin,
            width: a.width,
            arm: a.arm(),
            output: None,
        },
        events,
    });
    // Until it armed, and a moment for the page to settle.
    let end = std::time::Instant::now() + Duration::from_secs(10);
    let mut armed = false;
    while !armed && std::time::Instant::now() < end {
        sleep(100).await;
        while let Ok(e) = heard.try_recv() {
            armed |= matches!(e, crate::approvals::card::CardEvent::Armed { .. });
        }
    }
    r.check(armed, "the approval card armed");
    sleep(1500).await;
    let took = capture(&shots.join("approval.png")).await;
    r.check(took, "shots/approval.png");
    crate::approvals::overlay::send(CardCmd::Hide);
    card.quit().await;
    ring.quit().await;
    if let Some(h) = r.handle.take() {
        h.quit().await;
    }
}

#[test]
#[ignore = "needs the nested KWin of scripts/readme-shots.sh"]
fn readme_desktop_nested() {
    if let Err(e) = guard() {
        panic!("refused: {e}");
    }
    let out = std::path::PathBuf::from(std::env::var("KAI_LIVE_OUT").expect("KAI_LIVE_OUT"));
    gtk::init().expect("GTK");
    let r = glib::MainContext::default().block_on(async move {
        let mut r = new_run(out);
        drive(&mut r).await;
        r
    });
    assert_eq!(r.failed, 0, "{} checks failed", r.failed);
}
