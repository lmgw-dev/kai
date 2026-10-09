//! `overlay_weather {place}` (ruling 30, K19, §16 Q19): DWD's open data through Bright Sky
//! (`https://api.brightsky.dev`, no key), the one weather origin Kai fetches from.
//!
//! - **The place** is one plain parameter: a saved place's name from `[weather] places` (the
//!   default `home`), or `latitude,longitude`. Bright Sky has no place-name search; a geocoding
//!   service would be a new external service (the owner's call, §15 Later).
//! - **The data**: `/current_weather` (the nearest station's latest observation) and `/weather`
//!   (the day's hourly forecast, for the range and the rain). Nothing is cached beyond what HTTP
//!   itself does; every show asks again.
//! - **The attribution** Bright Sky asks for is on the widget: DWD as the source, Bright Sky as the
//!   way, and the station's name.
//! - Fetching follows `fetch.rs`'s rules (the size bound, the deadline, same-origin redirects).
//!
//! ```toml
//! [weather]
//! places = { home = "52.52,13.405", work = "48.137,11.575" }   # name = "latitude,longitude"
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fetch::{self, Policy};
use super::kinds::Body;

/// The only weather origin (K19).
pub const BRIGHT_SKY: &str = "https://api.brightsky.dev";

const TOOL: &str = "overlay_weather";

/// What a call without `place` means.
pub const DEFAULT_PLACE: &str = "home";

/// `[weather]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WeatherConfig {
    /// Saved places: a name and its `latitude,longitude`.
    pub places: BTreeMap<String, String>,
}

impl WeatherConfig {
    /// `(key, value, rule)` for the first place that is not `latitude,longitude`.
    pub fn validate(&self) -> Result<(), (&'static str, String, String)> {
        for (name, value) in &self.places {
            if name.trim().is_empty() {
                return Err((
                    "weather.places",
                    "\"\"".into(),
                    "a place needs a name".into(),
                ));
            }
            if let Err(rule) = coordinates(value) {
                return Err(("weather.places", format!("{name} = {value:?}"), rule));
            }
        }
        Ok(())
    }

    /// The coordinates `place` means: `latitude,longitude`, or a saved name (any case).
    pub fn resolve(&self, place: Option<&str>) -> Result<(f64, f64), String> {
        let asked = place.map(str::trim).filter(|p| !p.is_empty());
        let name = asked.unwrap_or(DEFAULT_PLACE);
        if asked.is_some()
            && let Ok(c) = coordinates(name)
        {
            return Ok(c);
        }
        let saved = self
            .places
            .iter()
            .find(|(n, _)| n.trim().eq_ignore_ascii_case(name));
        match saved {
            Some((_, value)) => {
                coordinates(value).map_err(|why| format!("{TOOL}: [weather] places {name}: {why}"))
            }
            None => {
                let known: Vec<&str> = self.places.keys().map(String::as_str).collect();
                let known = if known.is_empty() {
                    "none are saved; add some under [weather] places in the configuration"
                        .to_owned()
                } else {
                    format!("saved: {}", known.join(", "))
                };
                Err(format!(
                    "{TOOL}: place {name:?} is neither a saved place nor latitude,longitude ({known})"
                ))
            }
        }
    }
}

/// `latitude,longitude` in degrees.
pub fn coordinates(s: &str) -> Result<(f64, f64), String> {
    let bad = || "must be latitude,longitude in degrees (e.g. 52.52,13.405)".to_owned();
    let (la, lo) = s.split_once(',').ok_or_else(bad)?;
    let (la, lo) = (
        la.trim().parse::<f64>().map_err(|_| bad())?,
        lo.trim().parse::<f64>().map_err(|_| bad())?,
    );
    if !(-90.0..=90.0).contains(&la) {
        return Err(format!("latitude {la} is outside -90 to 90"));
    }
    if !(-180.0..=180.0).contains(&lo) {
        return Err(format!("longitude {lo} is outside -180 to 180"));
    }
    Ok((la, lo))
}

/// `overlay_weather`: both requests, then the document. `base` is [`BRIGHT_SKY`] outside tests.
pub async fn weather(
    place: Option<&str>,
    places: &WeatherConfig,
    base: &str,
    policy: &Policy,
) -> Result<Body, String> {
    let (lat, lon) = places.resolve(place)?;
    let current = json(
        &format!("{base}/current_weather?lat={lat}&lon={lon}"),
        policy,
    )
    .await?;
    // The forecast's day is the observation's own (Bright Sky's timestamps carry its time zone).
    let date = current["weather"]["timestamp"]
        .as_str()
        .and_then(|t| t.get(..10))
        .ok_or_else(|| format!("{TOOL}: Bright Sky's current weather has no timestamp"))?;
    if num(&current["weather"], "temperature").is_none() {
        return Err(format!(
            "{TOOL}: Bright Sky has no temperature for this place right now"
        ));
    }
    let day = json(
        &format!("{base}/weather?lat={lat}&lon={lon}&date={date}"),
        policy,
    )
    .await?;
    Ok(Body::Document(page(&current, &day, date)?))
}

async fn json(url: &str, policy: &Policy) -> Result<Value, String> {
    let got = fetch::get(TOOL, url, policy).await?;
    serde_json::from_slice(&got.bytes)
        .map_err(|e| format!("{TOOL}: Bright Sky's answer is not JSON ({e})"))
}

fn esc(s: &str) -> String {
    s.chars()
        .fold(String::with_capacity(s.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c => out.push(c),
            }
            out
        })
}

/// Bright Sky's icon word, as text.
fn sky(icon: &str) -> &'static str {
    match icon {
        "clear-day" => "Clear",
        "clear-night" => "Clear night",
        "partly-cloudy-day" => "Partly cloudy",
        "partly-cloudy-night" => "Partly cloudy night",
        "cloudy" => "Cloudy",
        "fog" => "Fog",
        "wind" => "Windy",
        "rain" => "Rain",
        "sleet" => "Sleet",
        "snow" => "Snow",
        "hail" => "Hail",
        "thunderstorm" => "Thunderstorm",
        _ => "Weather",
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn page(current: &Value, day: &Value, date: &str) -> Result<String, String> {
    let now = &current["weather"];
    let Some(temp) = num(now, "temperature") else {
        return Err(format!(
            "{TOOL}: Bright Sky has no temperature for this place right now"
        ));
    };
    let mut hourly = Vec::new();
    for h in day["weather"].as_array().into_iter().flatten() {
        if h["timestamp"].as_str().is_some_and(|t| t.starts_with(date)) {
            hourly.push(h);
        }
    }
    let temps: Vec<f64> = hourly
        .iter()
        .filter_map(|h| num(h, "temperature"))
        .collect();
    let low = temps.iter().copied().fold(f64::INFINITY, f64::min);
    let high = temps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let rain: f64 = hourly.iter().filter_map(|h| num(h, "precipitation")).sum();

    let mut rows = String::new();
    let mut row = |k: &str, v: String| {
        let _ = write!(rows, "<tr><td>{}</td><td>{}</td></tr>", esc(k), esc(&v));
    };
    if !temps.is_empty() {
        row("Today", format!("{low:.0} to {high:.0} \u{b0}C"));
        row("Rain today", format!("{rain:.1} mm"));
    }
    if let Some(w) = num(now, "wind_speed_10") {
        row("Wind", format!("{w:.0} km/h"));
    }
    if let Some(h) = num(now, "relative_humidity") {
        row("Humidity", format!("{h:.0} %"));
    }
    let station = current["sources"][0]["station_name"].as_str().unwrap_or("");
    let station = if station.is_empty() {
        String::new()
    } else {
        format!(" Station: {}.", esc(station))
    };
    let label = now["icon"].as_str().map(sky).unwrap_or("Weather");
    Ok(format!(
        "<!doctype html><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;height:100%;background:transparent;font-family:sans-serif}}\
         .card{{box-sizing:border-box;height:100%;padding:14px 18px;border-radius:10px;\
         background:#232629;color:#eff0f1;border:2px solid #3daee9}}\
         .t{{font-size:44px;line-height:1;color:#3daee9}}.c{{font-size:16px;margin:2px 0 8px}}\
         table{{font-size:14px;border-collapse:collapse}}td{{padding:1px 14px 1px 0}}\
         td:first-child{{color:#bdc3c7}}.s{{font-size:10px;color:#bdc3c7;margin-top:8px}}\
         </style><div class=\"card\"><div class=\"t\">{temp:.0}\u{b0}C</div>\
         <div class=\"c\">{}</div><table>{rows}</table>\
         <div class=\"s\">Source: Deutscher Wetterdienst (DWD), via Bright Sky.{station}</div></div>",
        esc(label)
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::super::fetch::stand_in::{response, serve};
    use super::*;

    fn policy() -> Policy {
        Policy {
            max_bytes: 1024 * 1024,
            max_mb: 1,
            private_addresses: true,
            timeout: Duration::from_secs(5),
            timeout_s: 5,
        }
    }

    fn places() -> WeatherConfig {
        WeatherConfig {
            places: [
                ("home".to_owned(), "52.52,13.405".to_owned()),
                ("Work".to_owned(), "48.137, 11.575".to_owned()),
            ]
            .into(),
        }
    }

    fn ok(v: Value) -> Vec<u8> {
        response(
            "200 OK",
            &[("Content-Type", "application/json")],
            v.to_string().as_bytes(),
        )
    }

    #[test]
    fn a_place_is_a_saved_name_or_coordinates_and_a_bad_one_is_named() {
        let p = places();
        assert_eq!(p.resolve(None), Ok((52.52, 13.405)));
        assert_eq!(p.resolve(Some(" ")), Ok((52.52, 13.405)));
        assert_eq!(p.resolve(Some("work")), Ok((48.137, 11.575)));
        assert_eq!(p.resolve(Some("-33.87, 151.21")), Ok((-33.87, 151.21)));
        let e = p.resolve(Some("Atlantis")).unwrap_err();
        assert!(
            e.contains("\"Atlantis\"") && e.contains("saved: Work, home"),
            "{e}"
        );
        let e = p.resolve(Some("95,10")).unwrap_err();
        assert!(e.contains("\"95,10\""), "{e}");
        let e = WeatherConfig::default().resolve(None).unwrap_err();
        assert!(
            e.contains("\"home\"") && e.contains("[weather] places"),
            "{e}"
        );
        assert!(coordinates("1,2,3").is_err());
        assert!(coordinates("10,181").unwrap_err().contains("longitude"));
    }

    #[test]
    fn the_configuration_reads_and_checks_its_places() {
        let c: WeatherConfig = toml::from_str("places = { home = \"52.5,13.4\" }").unwrap();
        assert_eq!(c.validate(), Ok(()));
        let c: WeatherConfig = toml::from_str("places = { home = \"Berlin\" }").unwrap();
        let (key, value, rule) = c.validate().unwrap_err();
        assert_eq!(
            (key, value.as_str()),
            ("weather.places", "home = \"Berlin\"")
        );
        assert!(rule.contains("latitude,longitude"), "{rule}");
        assert!(toml::from_str::<WeatherConfig>("origin = \"x\"").is_err());
    }

    #[tokio::test]
    async fn the_widget_shows_the_data_the_attribution_and_nothing_unescaped() {
        let s = serve(vec![
            (
                "/current_weather?lat=52.52&lon=13.405",
                ok(json!({"weather": {"timestamp": "2026-10-09T14:00:00+02:00", "temperature": 14.4,
                    "icon": "partly-cloudy-day", "wind_speed_10": 12.0, "relative_humidity": 71},
                    "sources": [{"station_name": "Berlin <b>Tempelhof</b>"}]})),
            ),
            (
                "/weather?lat=52.52&lon=13.405&date=2026-10-09",
                ok(json!({"weather": [
                    {"timestamp": "2026-10-09T00:00:00+02:00", "temperature": 8.1, "precipitation": 0.0},
                    {"timestamp": "2026-10-09T15:00:00+02:00", "temperature": 16.9, "precipitation": 1.5},
                    {"timestamp": "2026-10-10T00:00:00+02:00", "temperature": 2.0, "precipitation": 9.0}]})),
            ),
        ])
        .await;
        let base = format!("http://127.0.0.1:{}", s.port);
        let Body::Document(d) = weather(None, &places(), &base, &policy()).await.unwrap() else {
            panic!("not a document")
        };
        assert!(
            d.contains(">14\u{b0}C<") && d.contains("Partly cloudy"),
            "{d}"
        );
        assert!(
            d.contains("8 to 17 \u{b0}C") && d.contains("1.5 mm"),
            "the day only: {d}"
        );
        assert!(d.contains("12 km/h") && d.contains("71 %"));
        assert!(
            d.contains("Deutscher Wetterdienst (DWD), via Bright Sky"),
            "attribution"
        );
        assert!(d.contains("Berlin &lt;b&gt;Tempelhof"), "escaped: {d}");
        assert_eq!(
            s.seen.lock().unwrap().len(),
            2,
            "two requests, nothing cached or extra"
        );
    }

    #[tokio::test]
    async fn errors_name_the_place_the_status_or_the_missing_data() {
        let s = serve(vec![(
            "/current_weather?lat=1&lon=2",
            ok(json!({"weather": {"timestamp": "2026-10-09T14:00:00+02:00"}})),
        )])
        .await;
        let base = format!("http://127.0.0.1:{}", s.port);
        let e = weather(Some("nowhere"), &places(), &base, &policy())
            .await
            .unwrap_err();
        assert!(e.contains("\"nowhere\""), "{e}");
        assert!(s.seen.lock().unwrap().is_empty(), "a bad place asks nobody");
        let e = weather(Some("1,2"), &places(), &base, &policy())
            .await
            .unwrap_err();
        assert!(e.contains("no temperature"), "{e}");
        let e = weather(Some("5,6"), &places(), &base, &policy())
            .await
            .unwrap_err();
        assert!(e.contains("404"), "{e}");
    }
}
