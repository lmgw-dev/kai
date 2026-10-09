//! MPRIS (`org.mpris.MediaPlayer2.*` on the session bus): each player's name (`Identity`), its
//! playback status and the track's title and artists. A player that does not answer in time is
//! still listed, as not saying what it plays. The controls (`PlayPause`, `Next`, `Previous`) go to
//! the player's bus name, which only `Player::id` carries.

use std::collections::HashMap;
use std::time::Duration;

use kai_desktop::caps::media::{Control, Media, Playback, Player};
use kai_desktop::caps::{Failure, Fut};
use zbus::zvariant::OwnedValue;

use crate::desktop::{call, property};
use crate::host::registry::SessionBus;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";

pub struct Mpris {
    pub bus: SessionBus,
    pub timeout: Duration,
}

impl Media for Mpris {
    fn players(&self) -> Fut<'_, Vec<Player>> {
        Box::pin(async move {
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let names: Vec<String> = call(
                &bus,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListNames",
                &(),
                self.timeout,
            )
            .await?;
            let mut names: Vec<String> = names
                .into_iter()
                .filter(|n| n.starts_with(PREFIX))
                .collect();
            names.sort();
            let reads = names.iter().map(|n| player(&bus, n, self.timeout));
            Ok(futures_util::future::join_all(reads).await)
        })
    }

    fn control<'a>(&'a self, id: &'a str, control: Control) -> Fut<'a, ()> {
        Box::pin(async move {
            if !id.starts_with(PREFIX) {
                return Err(Failure::Refused("that is not a media player".into()));
            }
            let bus = self.bus.get().await.map_err(Failure::Failed)?;
            let method = match control {
                Control::PlayPause => "PlayPause",
                Control::Next => "Next",
                Control::Previous => "Previous",
            };
            call::<_, ()>(
                &bus,
                id,
                PATH,
                "org.mpris.MediaPlayer2.Player",
                method,
                &(),
                self.timeout,
            )
            .await
        })
    }
}

async fn player(bus: &zbus::Connection, name: &str, timeout: Duration) -> Player {
    let fallback = name
        .strip_prefix(PREFIX)
        .unwrap_or(name)
        .split(".instance")
        .next()
        .unwrap_or(name)
        .to_owned();
    let identity: Option<String> = property(
        bus,
        name,
        PATH,
        "org.mpris.MediaPlayer2",
        "Identity",
        timeout,
    )
    .await
    .ok();
    let props: Result<HashMap<String, OwnedValue>, Failure> = call(
        bus,
        name,
        PATH,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &("org.mpris.MediaPlayer2.Player",),
        timeout,
    )
    .await;
    let mut p = Player {
        id: name.to_owned(),
        name: identity
            .filter(|i| !i.trim().is_empty())
            .unwrap_or(fallback),
        playback: Playback::Unknown,
        title: String::new(),
        artists: Vec::new(),
    };
    let Ok(props) = props else {
        return p;
    };
    p.playback = match props
        .get("PlaybackStatus")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .as_deref()
    {
        Some("Playing") => Playback::Playing,
        Some("Paused") => Playback::Paused,
        Some("Stopped") => Playback::Stopped,
        _ => Playback::Unknown,
    };
    if let Some(meta) = props
        .get("Metadata")
        .and_then(|v| HashMap::<String, OwnedValue>::try_from(v.try_clone().ok()?).ok())
    {
        if let Some(t) = meta
            .get("xesam:title")
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        {
            p.title = t;
        }
        if let Some(a) = meta
            .get("xesam:artist")
            .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        {
            p.artists = a.into_iter().filter(|a| !a.trim().is_empty()).collect();
        }
    }
    p
}
