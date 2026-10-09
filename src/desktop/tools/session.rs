//! The tools of `contract::session` (WP16b) over the chosen services. Results are one compact line
//! saying what happened, without the arguments echoed (K22); a failure of the desktop is the tool's
//! error.

use std::sync::Arc;

use kai_desktop::caps::Failure;
use kai_desktop::caps::audio::{Audio, Device};
use kai_desktop::caps::media::{Control, Playback, Player};

use super::Services;
use crate::desktop::resolve::{quoted, shown_text};
use crate::host::registry::Args;

/// Whether `name` is a tool of this file.
pub fn handles(name: &str) -> bool {
    kai_desktop::contract::session::needs(name).is_some()
}

fn say(f: Failure) -> String {
    f.to_string()
}

fn missing<T>(what: &str) -> Result<T, String> {
    // Not listed without the capability (K23): only a race with a relist gets here.
    Err(format!("this desktop offers Kai no {what}"))
}

pub async fn run(services: &Services, name: &str, args: &Args) -> Result<String, String> {
    match name {
        "media_play_pause" => media(services, args, Control::PlayPause).await,
        "media_next" => media(services, args, Control::Next).await,
        "media_previous" => media(services, args, Control::Previous).await,
        "volume_set" => volume_set(services, args).await,
        "mute" => mute(services, args).await,
        "notify" => notify(services, args).await,
        "ask" => ask(services, args).await,
        "clipboard_set" => clipboard_set(services, args).await,
        "lock_screen" => lock_screen(services).await,
        "dnd" => dnd(services, args).await,
        "night_light" => night_light(services, args).await,
        "brightness_set" => brightness_set(services, args).await,
        "suspend" => suspend(services).await,
        other => Err(format!("Kai's desktop module has no tool {other}")),
    }
}

fn percent(args: &Args) -> Result<u32, String> {
    let p = args.integer("percent").unwrap_or(-1);
    u32::try_from(p)
        .ok()
        .filter(|p| *p <= 100)
        .ok_or_else(|| "percent is 0 to 100".to_owned())
}

// --- media ---------------------------------------------------------------------------------

fn status(p: &Player) -> &'static str {
    match p.playback {
        Playback::Playing => "playing",
        Playback::Paused => "paused",
        Playback::Stopped => "stopped",
        Playback::Unknown => "status unknown",
    }
}

fn candidates(found: &[&Player]) -> String {
    found
        .iter()
        .map(|p| format!("{} ({})", shown_text(&p.name), status(p)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The player `query` names (K22's pattern): in order, the first rule that matches anything
/// decides: the name as the player gives it, the bus name's last part (`spotify` for
/// `org.mpris.MediaPlayer2.spotify`), a fragment of either; any case. Omitted: the player that is
/// playing, else the one paused, else the only one running. More than one match is an error that
/// lists the candidates.
pub fn pick_player<'a>(players: &'a [Player], query: Option<&str>) -> Result<&'a Player, String> {
    if players.is_empty() {
        return Err("no media player is running".into());
    }
    let q = query.map(str::trim).unwrap_or_default().to_lowercase();
    let found: Vec<&Player> = if q.is_empty() {
        let by = |pb: Playback| -> Vec<&Player> {
            players.iter().filter(|p| p.playback == pb).collect()
        };
        let mut chosen = by(Playback::Playing);
        if chosen.is_empty() {
            chosen = by(Playback::Paused);
        }
        if chosen.is_empty() {
            chosen = players.iter().collect();
        }
        chosen
    } else {
        let short = |p: &Player| {
            p.id.strip_prefix("org.mpris.MediaPlayer2.")
                .unwrap_or(&p.id)
                .split(".instance")
                .next()
                .unwrap_or_default()
                .to_lowercase()
        };
        let rules: [&dyn Fn(&Player) -> bool; 3] =
            [&|p| p.name.to_lowercase() == q, &|p| short(p) == q, &|p| {
                p.name.to_lowercase().contains(&q) || short(p).contains(&q)
            }];
        match rules
            .iter()
            .map(|rule| players.iter().filter(|p| rule(p)).collect::<Vec<_>>())
            .find(|m| !m.is_empty())
        {
            Some(m) => m,
            None => {
                let all: Vec<&Player> = players.iter().collect();
                return Err(format!("no player matches; running:\n{}", candidates(&all)));
            }
        }
    };
    match found[..] {
        [one] => Ok(one),
        _ => Err(format!(
            "{} players match; name one of:\n{}",
            found.len(),
            candidates(&found)
        )),
    }
}

async fn media(services: &Services, args: &Args, control: Control) -> Result<String, String> {
    let Some(m) = &services.media else {
        return missing("media control");
    };
    let players = m.players().await.map_err(say)?;
    let target = pick_player(&players, args.text("player"))?;
    m.control(&target.id, control).await.map_err(say)?;
    let name = shown_text(&target.name);
    Ok(match control {
        Control::PlayPause => match target.playback {
            Playback::Playing => format!("paused {name}"),
            Playback::Paused | Playback::Stopped => format!("playing {name}"),
            Playback::Unknown => format!("sent play/pause to {name}"),
        },
        Control::Next => format!("{name}: next track"),
        Control::Previous => format!("{name}: previous track"),
    })
}

// --- audio ---------------------------------------------------------------------------------

/// The output `query` names: omitted, the default; else a description or sink name, whole and then
/// as a fragment (any case). More than one match is an error listing them.
pub fn pick_output<'a>(outputs: &'a [Device], query: Option<&str>) -> Result<&'a Device, String> {
    let q = query.map(str::trim).unwrap_or_default().to_lowercase();
    if q.is_empty() {
        return outputs
            .iter()
            .find(|d| d.default)
            .or(outputs.first())
            .ok_or_else(|| "no audio output exists".to_owned());
    }
    let rules: [&dyn Fn(&Device) -> bool; 2] = [
        &|d| d.description.to_lowercase() == q || d.id.to_lowercase() == q,
        &|d| d.description.to_lowercase().contains(&q) || d.id.to_lowercase().contains(&q),
    ];
    let found = rules
        .iter()
        .map(|rule| outputs.iter().filter(|d| rule(d)).collect::<Vec<_>>())
        .find(|m| !m.is_empty());
    let list = |ds: &[&Device]| {
        ds.iter()
            .map(|d| shown_text(&d.description).into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    };
    match found.as_deref() {
        Some([one]) => Ok(one),
        Some(many) => Err(format!(
            "{} outputs match; name one of:\n{}",
            many.len(),
            list(many)
        )),
        None => {
            let all: Vec<&Device> = outputs.iter().collect();
            Err(format!("no output matches; there are:\n{}", list(&all)))
        }
    }
}

fn audio(services: &Services) -> Result<&Arc<dyn Audio>, String> {
    services
        .audio
        .as_ref()
        .ok_or_else(|| "this desktop offers Kai no audio control".to_owned())
}

async fn volume_set(services: &Services, args: &Args) -> Result<String, String> {
    let percent = percent(args)?;
    let a = audio(services)?;
    let outputs = a.outputs().await.map_err(say)?;
    let out = pick_output(&outputs, args.text("device"))?;
    a.set_volume(&out.id, percent).await.map_err(say)?;
    Ok(format!(
        "{}: volume {percent} %",
        shown_text(&out.description)
    ))
}

async fn mute(services: &Services, args: &Args) -> Result<String, String> {
    let on = args.boolean("on").unwrap_or(true);
    let a = audio(services)?;
    let outputs = a.outputs().await.map_err(say)?;
    let out = pick_output(&outputs, None)?;
    a.set_mute(&out.id, on).await.map_err(say)?;
    Ok(format!(
        "{}: {}",
        shown_text(&out.description),
        if on { "muted" } else { "unmuted" }
    ))
}

// --- notifications -------------------------------------------------------------------------

fn notify_service(
    services: &Services,
) -> Result<&Arc<dyn kai_desktop::caps::notify::Notify>, String> {
    services
        .notify
        .as_ref()
        .ok_or_else(|| "this desktop offers Kai no notifications".to_owned())
}

async fn notify(services: &Services, args: &Args) -> Result<String, String> {
    let title = args.text("title").unwrap_or_default();
    if title.trim().is_empty() {
        return Err("title is empty".into());
    }
    notify_service(services)?
        .show(title, args.text("body").unwrap_or_default())
        .await
        .map_err(say)?;
    Ok("shown".into())
}

async fn ask(services: &Services, args: &Args) -> Result<String, String> {
    let question = args.text("question").unwrap_or_default();
    if question.trim().is_empty() {
        return Err("question is empty".into());
    }
    let choices: Vec<String> = args
        .texts("choices")
        .unwrap_or_default()
        .into_iter()
        .map(str::to_owned)
        .collect();
    if choices.is_empty() || choices.iter().any(|c| c.trim().is_empty()) {
        return Err("choices needs at least one choice, none of them empty".into());
    }
    let asked = notify_service(services)?
        .ask(question, &choices)
        .await
        .map_err(say)?;
    let mut said = match asked.choice {
        Some(i) => format!("answer: {}", quoted(&choices[i])),
        None => "no answer: the owner dismissed the question".into(),
    };
    if asked.under_dnd {
        said.push_str(
            " (do not disturb was on: the question went out as critical so that it is shown)",
        );
    }
    Ok(said)
}

async fn dnd(services: &Services, args: &Args) -> Result<String, String> {
    let want = args.boolean("on").unwrap_or(true);
    let got = notify_service(services)?
        .set_dnd(want)
        .await
        .map_err(say)?
        .on;
    Ok(match (want, got) {
        (true, true) => "do not disturb is on".into(),
        (false, false) => "do not disturb is off".into(),
        (true, false) => "do not disturb could not be switched on".into(),
        (false, true) => {
            "do not disturb is still on: something other than Kai switched it on".into()
        }
    })
}

// --- clipboard -----------------------------------------------------------------------------

async fn clipboard_set(services: &Services, args: &Args) -> Result<String, String> {
    let Some(c) = &services.clipboard else {
        return missing("clipboard");
    };
    let text = args.text("text").unwrap_or_default();
    c.set(text).await.map_err(say)?;
    Ok(format!("copied {} characters", text.chars().count()))
}

// --- session -------------------------------------------------------------------------------

fn session(services: &Services) -> Result<&Arc<dyn kai_desktop::caps::session::Session>, String> {
    services
        .session
        .as_ref()
        .ok_or_else(|| "this desktop offers Kai no session control".to_owned())
}

async fn lock_screen(services: &Services) -> Result<String, String> {
    session(services)?.lock_screen().await.map_err(say)?;
    Ok("locked".into())
}

async fn suspend(services: &Services) -> Result<String, String> {
    session(services)?.suspend().await.map_err(say)?;
    Ok("suspending".into())
}

async fn brightness_set(services: &Services, args: &Args) -> Result<String, String> {
    let wanted = percent(args)?;
    let now = session(services)?
        .set_brightness(wanted)
        .await
        .map_err(say)?;
    Ok(format!("brightness {now} %"))
}

async fn night_light(services: &Services, args: &Args) -> Result<String, String> {
    let want = args.boolean("on").unwrap_or(true);
    let got = session(services)?
        .set_night_light(want)
        .await
        .map_err(say)?
        .on;
    Ok(match (want, got) {
        (true, true) => "Night Light resumed".into(),
        (false, false) => "Night Light paused".into(),
        (true, false) => "Night Light stays paused: something other than Kai holds it".into(),
        (false, true) => "Night Light could not be paused".into(),
    })
}

#[cfg(test)]
mod tests;
