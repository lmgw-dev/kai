//! The live check of WP11's kinds: `overlay_image`, `overlay_emote` and `overlay_weather` shown in
//! the nested KWin, their bodies made by [`Kind::body`] against a loopback stand-in server (never a
//! real host, never Bright Sky). The image is a solid Breeze-blue PNG the size of the widget, so
//! its crop must be blue throughout: it got through fetch, decode, re-encode and the sandbox's
//! `data:` load. Crops of all three land in `<out>/shots/`.

use serde_json::json;

use super::{OUTPUTS, Run, crops, look, sleep};
use crate::host::registry::Args;
use crate::overlay::placement::Anchor;
use crate::widgets::fetch::stand_in::{response, serve};
use crate::widgets::kinds::{Body, Kind, Sources};

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(480, 320, image::Rgba([61, 174, 233, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn args(kind: Kind, key: &str, value: &str) -> Args {
    let (_, spec) = kai_desktop::contract::find(kind.tool()).unwrap();
    Args::read(spec, json!({ key: value }).as_object().cloned()).unwrap()
}

/// The three bodies, made on a tokio runtime beside the GTK loop.
fn bodies(rt: &tokio::runtime::Runtime) -> Vec<(Kind, Result<Body, String>)> {
    rt.block_on(async {
        let ok = |v: serde_json::Value| response("200 OK", &[("Content-Type", "application/json")], v.to_string().as_bytes());
        let s = serve(vec![
            ("/p.png", response("200 OK", &[("Content-Type", "image/png")], &png())),
            (
                "/current_weather?lat=52.52&lon=13.405",
                ok(json!({"weather": {"timestamp": "2026-10-09T14:00:00+02:00", "temperature": 14.4,
                    "icon": "partly-cloudy-day", "wind_speed_10": 12.0, "relative_humidity": 71},
                    "sources": [{"station_name": "Test Station"}]})),
            ),
            (
                "/weather?lat=52.52&lon=13.405&date=2026-10-09",
                ok(json!({"weather": [
                    {"timestamp": "2026-10-09T03:00:00+02:00", "temperature": 8.1, "precipitation": 0.0},
                    {"timestamp": "2026-10-09T15:00:00+02:00", "temperature": 16.9, "precipitation": 1.5}]})),
            ),
        ])
        .await;
        let mut from = Sources::of(&Default::default(), &Default::default());
        from.weather_base = format!("http://127.0.0.1:{}", s.port);
        from.weather.places.insert("home".into(), "52.52,13.405".into());
        let url = format!("http://127.0.0.1:{}/p.png", s.port);
        vec![
            (Kind::Image, Kind::Image.body(&args(Kind::Image, "src", &url), &from).await),
            (Kind::Emote, Kind::Emote.body(&args(Kind::Emote, "name", "heart"), &from).await),
            (Kind::Weather, Kind::Weather.body(&args(Kind::Weather, "place", ""), &from).await),
        ]
    })
}

/// The mean colour of a crop, as ImageMagick prints it.
fn mean(path: &std::path::Path) -> Option<(u32, u32, u32)> {
    let out = std::process::Command::new("magick")
        .arg(path)
        .args(["-resize", "1x1!", "-depth", "8", "txt:-"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let rgb = text
        .lines()
        .nth(1)?
        .split("srgb(")
        .nth(1)?
        .split(')')
        .next()?
        .to_owned();
    let mut it = rgb
        .split(',')
        .map(|c| c.trim().trim_end_matches('%').parse::<f64>().ok());
    Some((it.next()?? as u32, it.next()?? as u32, it.next()?? as u32))
}

pub async fn run(r: &mut Run) {
    r.line("== 5c. overlay_image, overlay_emote, overlay_weather".into());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let made = bodies(&rt);
    for (kind, body) in made {
        let name = kind.key();
        let body = match body {
            Ok(b) => b,
            Err(e) => {
                r.check(false, &format!("{name}: no body ({e})"));
                continue;
            }
        };
        let c = crate::widgets::settings::of(&Default::default(), kind).clone();
        let l = look(Anchor::Center, Some(OUTPUTS[0]), c.width, c.height);
        let shown = r.show(kind, body, l).await;
        let Ok(s) = shown else {
            r.check(
                false,
                &format!("{name}: not shown ({})", shown.unwrap_err()),
            );
            continue;
        };
        r.check(true, &format!("{name}: shown as {}", s.id));
        sleep(1200).await;
        let tag = format!("kind-{name}");
        crops(r, &tag).await;
        let crop = r.out.join("shots").join(format!("{tag}-00.png"));
        if kind == Kind::Image {
            match mean(&crop) {
                Some((red, g, b)) => r.check(
                    b > 200 && red < 90 && g > 130,
                    &format!("the image's crop is Breeze blue throughout (mean {red},{g},{b})"),
                ),
                None => r.line("skipped: no crop to measure the image's colour".into()),
            }
        }
        let _ = r.hide(Some(s.id)).await;
        sleep(200).await;
    }
    rt.shutdown_background();
}
