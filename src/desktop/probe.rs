//! The probe's result and the choice per capability (K23): per capability the highest-priority
//! provider whose probe passes wins (`plasma` before `fdo`), unless `[desktop]` names one; the
//! capability table says what was chosen and why, in the log and the tray.

use std::sync::Arc;

use kai_desktop::caps::Capability;
use kai_desktop::caps::accessibility::Accessibility;
use kai_desktop::caps::audio::Audio;
use kai_desktop::caps::capture::Capture;
use kai_desktop::caps::clipboard::Clipboard;
use kai_desktop::caps::input::Input;
use kai_desktop::caps::launch::Launch;
use kai_desktop::caps::media::Media;
use kai_desktop::caps::notify::Notify;
use kai_desktop::caps::search::Search;
use kai_desktop::caps::session::Session;
use kai_desktop::caps::windows::Windows;

use super::settings::{Choice, DesktopConfig};

/// A provider, in priority order (K23: the desktop's own before the freedesktop base).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderName {
    Plasma,
    Fdo,
}

impl ProviderName {
    pub fn key(self) -> &'static str {
        match self {
            ProviderName::Plasma => "plasma",
            ProviderName::Fdo => "fdo",
        }
    }

    fn of(choice: Choice) -> Option<Self> {
        match choice {
            Choice::Plasma => Some(ProviderName::Plasma),
            Choice::Fdo => Some(ProviderName::Fdo),
            Choice::None => None,
        }
    }
}

/// A capability's implementation, ready to call.
#[derive(Clone)]
pub enum Service {
    Windows(Arc<dyn Windows>),
    Session(Arc<dyn Session>),
    Media(Arc<dyn Media>),
    Notify(Arc<dyn Notify>),
    Audio(Arc<dyn Audio>),
    Capture(Arc<dyn Capture>),
    Accessibility(Arc<dyn Accessibility>),
    Search(Arc<dyn Search>),
    Clipboard(Arc<dyn Clipboard>),
    Launch(Arc<dyn Launch>),
    Input(Arc<dyn Input>),
}

impl Service {
    pub fn capability(&self) -> Capability {
        match self {
            Service::Windows(_) => Capability::Windows,
            Service::Session(_) => Capability::Session,
            Service::Media(_) => Capability::Media,
            Service::Notify(_) => Capability::Notify,
            Service::Audio(_) => Capability::Audio,
            Service::Capture(_) => Capability::Capture,
            Service::Accessibility(_) => Capability::Accessibility,
            Service::Search(_) => Capability::Search,
            Service::Clipboard(_) => Capability::Clipboard,
            Service::Launch(_) => Capability::Launch,
            Service::Input(_) => Capability::Input,
        }
    }
}

/// What a provider's probe found for one capability it implements: the service with a note on
/// how it works (`KWin scripting`), or why it is not available here.
pub struct Offer {
    pub capability: Capability,
    pub found: Result<(Service, String), String>,
}

impl Offer {
    pub fn ready(service: Service, note: impl Into<String>) -> Self {
        Self {
            capability: service.capability(),
            found: Ok((service, note.into())),
        }
    }

    pub fn absent(capability: Capability, why: impl Into<String>) -> Self {
        Self {
            capability,
            found: Err(why.into()),
        }
    }
}

/// One provider's probe: an offer per capability it implements.
pub struct Probed {
    pub provider: ProviderName,
    pub offers: Vec<Offer>,
}

/// The choice for one capability: the provider and its service, or none, with the table's note.
#[derive(Clone)]
pub struct Chosen {
    pub capability: Capability,
    pub provider: Option<ProviderName>,
    pub service: Option<Service>,
    pub note: String,
}

/// The choice for every capability: the providers' probes (in priority order) and the owner's
/// `[desktop]`.
pub fn choose(probes: &[Probed], config: &DesktopConfig) -> Vec<Chosen> {
    Capability::ALL
        .iter()
        .map(|&cap| choose_one(cap, probes, config.choice(cap)))
        .collect()
}

fn offer(probe: &Probed, cap: Capability) -> Option<&Offer> {
    probe.offers.iter().find(|o| o.capability == cap)
}

fn choose_one(cap: Capability, probes: &[Probed], choice: Option<Choice>) -> Chosen {
    let none = |note: String| Chosen {
        capability: cap,
        provider: None,
        service: None,
        note,
    };
    let key = cap.key();
    let take = |provider: ProviderName, found: &Result<(Service, String), String>| match found {
        Ok((service, note)) => Some(Chosen {
            capability: cap,
            provider: Some(provider),
            service: Some(service.clone()),
            note: note.clone(),
        }),
        Err(_) => None,
    };
    match choice {
        Some(Choice::None) => none(format!("desktop.{key} = none")),
        Some(named) => {
            let name = ProviderName::of(named).expect("not none");
            match probes.iter().find(|p| p.provider == name) {
                None => none(format!(
                    "desktop.{key} = {}: that provider is not in this build",
                    name.key()
                )),
                Some(p) => match offer(p, cap) {
                    None => none(format!(
                        "desktop.{key} = {}: it does not serve {key} in this build",
                        name.key()
                    )),
                    Some(o) => take(name, &o.found).unwrap_or_else(|| {
                        none(format!(
                            "desktop.{key} = {}: {}",
                            name.key(),
                            o.found
                                .as_ref()
                                .err()
                                .map(String::as_str)
                                .unwrap_or_default()
                        ))
                    }),
                },
            }
        }
        None => {
            let mut why = Vec::new();
            for p in probes {
                if let Some(o) = offer(p, cap) {
                    if let Some(c) = take(p.provider, &o.found) {
                        return c;
                    }
                    if let Err(e) = &o.found {
                        why.push(format!("{}: {e}", p.provider.key()));
                    }
                }
            }
            if why.is_empty() {
                none("no provider serves it in this build".into())
            } else {
                none(why.join("; "))
            }
        }
    }
}

/// The capabilities some provider of this build implements (the others have no tools yet).
pub fn built(probes: &[Probed]) -> Vec<Capability> {
    Capability::ALL
        .into_iter()
        .filter(|&c| probes.iter().any(|p| offer(p, c).is_some()))
        .collect()
}

/// The table's lines for the log: one per capability.
pub fn log_lines(chosen: &[Chosen]) -> Vec<String> {
    chosen
        .iter()
        .map(|c| match c.provider {
            Some(p) => format!("{}: {} ({})", c.capability.key(), p.key(), c.note),
            None => format!("{}: none ({})", c.capability.key(), c.note),
        })
        .collect()
}

/// The tray's line: the capabilities this build has tools for, each with its provider.
pub fn tray_line(chosen: &[Chosen], built: &[Capability]) -> String {
    let parts: Vec<String> = chosen
        .iter()
        .filter(|c| built.contains(&c.capability))
        .map(|c| {
            format!(
                "{} {}",
                c.capability.key(),
                c.provider.map(ProviderName::key).unwrap_or("none")
            )
        })
        .collect();
    format!("desktop: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use kai_desktop::caps::Fut;
    use kai_desktop::caps::media::Player;

    use super::*;

    struct NoMedia;
    impl Media for NoMedia {
        fn players(&self) -> Fut<'_, Vec<Player>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn control<'a>(&'a self, _: &'a str, _: kai_desktop::caps::media::Control) -> Fut<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn media() -> Service {
        Service::Media(Arc::new(NoMedia))
    }

    fn probes(plasma: Option<&str>, fdo: Option<&str>) -> Vec<Probed> {
        let offer = |found: Option<&str>| match found {
            None => Offer::ready(media(), "ready"),
            Some(why) => Offer::absent(Capability::Media, why),
        };
        vec![
            Probed {
                provider: ProviderName::Plasma,
                offers: plasma
                    .map(|p| offer(Some(p).filter(|w| *w != "ok")))
                    .into_iter()
                    .collect(),
            },
            Probed {
                provider: ProviderName::Fdo,
                offers: fdo
                    .map(|f| offer(Some(f).filter(|w| *w != "ok")))
                    .into_iter()
                    .collect(),
            },
        ]
    }

    fn media_choice(probes: &[Probed], choice: Option<Choice>) -> (Option<ProviderName>, String) {
        let config = DesktopConfig {
            media: choice,
            ..Default::default()
        };
        let c = choose(probes, &config)
            .into_iter()
            .find(|c| c.capability == Capability::Media)
            .unwrap();
        assert_eq!(c.service.is_some(), c.provider.is_some());
        (c.provider, c.note)
    }

    /// K23: the highest-priority provider whose probe passes wins; the configuration overrides it,
    /// but never to a provider whose probe failed; the note says why.
    #[test]
    fn the_first_provider_whose_probe_passes_wins_unless_the_owner_names_one() {
        use ProviderName::*;
        let both = probes(Some("ok"), Some("ok"));
        assert_eq!(media_choice(&both, None), (Some(Plasma), "ready".into()));
        let plasma_down = probes(Some("no KWin"), Some("ok"));
        assert_eq!(media_choice(&plasma_down, None).0, Some(Fdo));
        assert_eq!(media_choice(&both, Some(Choice::Fdo)).0, Some(Fdo));
        assert_eq!(
            media_choice(&both, Some(Choice::None)),
            (None, "desktop.media = none".into())
        );
        assert_eq!(
            media_choice(&plasma_down, Some(Choice::Plasma)),
            (None, "desktop.media = plasma: no KWin".into())
        );
        let neither = probes(Some("not Plasma"), Some("no bus"));
        assert_eq!(
            media_choice(&neither, None),
            (None, "plasma: not Plasma; fdo: no bus".into())
        );
        let fdo_only = probes(None, Some("ok"));
        assert_eq!(
            media_choice(&fdo_only, Some(Choice::Plasma)),
            (
                None,
                "desktop.media = plasma: it does not serve media in this build".into()
            )
        );
        let chosen = choose(&fdo_only, &DesktopConfig::default());
        let capture = chosen
            .iter()
            .find(|c| c.capability == Capability::Capture)
            .unwrap();
        assert_eq!(capture.note, "no provider serves it in this build");
        assert_eq!(built(&fdo_only), [Capability::Media]);
        assert_eq!(tray_line(&chosen, &built(&fdo_only)), "desktop: media fdo");
        assert!(log_lines(&chosen).contains(&"media: fdo (ready)".to_owned()));
    }
}
