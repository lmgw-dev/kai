//! The overlays' bookkeeping (§10.1), pure: ids, show, hide by id and all, expiry, `overlay_list`'s
//! lines and the tray's entries. The GTK side ([`super::host`]) carries it out.
//!
//! - **Ids** count up from 1 for Kai's run and are never reused, so a stale id hides nothing else.
//! - **No update** (ruling 24): showing always adds an overlay; nothing replaces one.
//! - An overlay is **loading** from the call until its surface maps, then **shown**; a hide while it
//!   loads ends its show call with an error.
//! - **Expiry** counts from the moment it shows; no timeout keeps it until it is hidden.
//! - A shown overlay that **goes without a call** (its web process ended or stopped responding, its
//!   output went away) is said once by the next `overlay_list`, with why.
//! - **Refused navigations and permission requests** are counted per overlay (the first of each is
//!   logged, the rest only counted) and the counts are in its `overlay_list` line.

use std::time::{Duration, Instant};

use super::kinds::{Kind, position_key};
use crate::overlay::placement::Anchor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Loading,
    Shown {
        /// The wall clock at the show, `HH:MM:SS`.
        at: String,
        /// When it goes: the instant, and the wall clock for the list.
        until: Option<(Instant, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub kind: Kind,
    pub position: Anchor,
    pub interactive: bool,
    /// The configured timeout, armed when it shows.
    pub timeout: Option<Duration>,
    pub state: State,
    /// What the caller was told beyond the id (a fitted size, scrolling text), kept for the list.
    pub notes: Vec<String>,
    /// Refused navigations.
    pub navigations: u64,
    /// Denied permission requests.
    pub permissions: u64,
}

/// What [`Table::count`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Navigation,
    Permission,
}

#[derive(Debug)]
pub struct Table {
    next: u64,
    entries: Vec<Entry>,
    /// Overlays that went without a call, one line each, until the next list says them.
    ended: Vec<String>,
}

impl Default for Table {
    fn default() -> Self {
        Self {
            next: 1,
            entries: Vec::new(),
            ended: Vec::new(),
        }
    }
}

/// `3 navigations` / `1 permission request`.
fn counted(n: u64, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// `hidden: 3, 5` style id lists.
pub fn ids(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Table {
    /// A new overlay, loading; its id.
    pub fn add(
        &mut self,
        kind: Kind,
        position: Anchor,
        interactive: bool,
        timeout: Option<Duration>,
    ) -> u64 {
        let id = self.next;
        self.next += 1;
        self.entries.push(Entry {
            id,
            kind,
            position,
            interactive,
            timeout,
            state: State::Loading,
            notes: Vec::new(),
            navigations: 0,
            permissions: 0,
        });
        id
    }

    /// One more refused navigation or denied permission request of overlay `id`: the count now
    /// (`None`: no such overlay).
    pub fn count(&mut self, id: u64, what: Count) -> Option<u64> {
        let e = self.entries.iter_mut().find(|e| e.id == id)?;
        let n = match what {
            Count::Navigation => &mut e.navigations,
            Count::Permission => &mut e.permissions,
        };
        *n += 1;
        Some(*n)
    }

    /// Overlay `id` went without a call (`clock` on the wall), for `why`: taken out, and said by
    /// the next [`Table::list`].
    pub fn ended(&mut self, id: u64, clock: &Clock, why: &str) {
        if let Some(e) = self.remove(id) {
            self.ended.push(format!(
                "{} {} {} ended {}: {why}",
                e.id,
                e.kind.key(),
                position_key(e.position),
                clock.at(Duration::ZERO)
            ));
        }
    }

    /// `overlay_list`: [`Table::lines`], then the overlays that went without a call since the last
    /// list (said once).
    pub fn list(&mut self) -> Vec<String> {
        let mut lines = self.lines();
        if !self.ended.is_empty() {
            if self.entries.is_empty() {
                lines.clear();
            }
            lines.append(&mut self.ended);
        }
        lines
    }

    /// It mapped at `now` (`clock` on the wall): shown, its expiry armed. `false` when it is gone
    /// (hidden while it loaded).
    pub fn shown(&mut self, id: u64, now: Instant, clock: &Clock, notes: Vec<String>) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        e.state = State::Shown {
            at: clock.at(Duration::ZERO),
            until: e.timeout.map(|t| (now + t, clock.at(t))),
        };
        e.notes = notes;
        true
    }

    /// `overlay_hide`: the one with `id`, or every one (loading ones too). The ids hidden, or an
    /// error naming the ones there are.
    pub fn hide(&mut self, id: Option<u64>) -> Result<Vec<u64>, String> {
        match id {
            None => Ok(self.entries.drain(..).map(|e| e.id).collect()),
            Some(id) => match self.remove(id) {
                Some(e) => Ok(vec![e.id]),
                None if self.entries.is_empty() => {
                    Err(format!("no overlay {id} is shown (none is)"))
                }
                None => {
                    let there: Vec<u64> = self.entries.iter().map(|e| e.id).collect();
                    Err(format!("no overlay {id} is shown (shown: {})", ids(&there)))
                }
            },
        }
    }

    /// Takes one out (expired, its web process ended, its output went away, its show failed).
    pub fn remove(&mut self, id: u64) -> Option<Entry> {
        let i = self.entries.iter().position(|e| e.id == id)?;
        Some(self.entries.remove(i))
    }

    /// The shown overlays whose time is up at `now`, taken out.
    pub fn expire(&mut self, now: Instant) -> Vec<u64> {
        let due: Vec<u64> = self
            .entries
            .iter()
            .filter(|e| matches!(&e.state, State::Shown { until: Some((t, _)), .. } if *t <= now))
            .map(|e| e.id)
            .collect();
        self.entries.retain(|e| !due.contains(&e.id));
        due
    }

    /// The next expiry, for the timer.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter_map(|e| match &e.state {
                State::Shown {
                    until: Some((t, _)),
                    ..
                } => Some(*t),
                _ => None,
            })
            .min()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn ids(&self) -> Vec<u64> {
        self.entries.iter().map(|e| e.id).collect()
    }

    /// `overlay_list` (§10.1): one line per overlay: id, kind, position, shown and expiry times,
    /// interactive, notes. K22: compact, nothing shortened.
    pub fn lines(&self) -> Vec<String> {
        if self.entries.is_empty() {
            return vec!["no overlays".into()];
        }
        self.entries
            .iter()
            .map(|e| {
                let mut l = format!("{} {} {}", e.id, e.kind.key(), position_key(e.position));
                match &e.state {
                    State::Loading => l.push_str(" loading"),
                    State::Shown { at, until } => {
                        l.push_str(&format!(" shown {at}"));
                        match until {
                            Some((_, u)) => l.push_str(&format!(" until {u}")),
                            None => l.push_str(" until hidden"),
                        }
                    }
                }
                l.push_str(if e.interactive {
                    " interactive"
                } else {
                    " click-through"
                });
                if !e.notes.is_empty() {
                    l.push_str(&format!("; {}", e.notes.join("; ")));
                }
                let refused: Vec<String> = [
                    (e.navigations, "navigation"),
                    (e.permissions, "permission request"),
                ]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, one)| counted(n, one))
                .collect();
                if !refused.is_empty() {
                    l.push_str(&format!("; refused: {}", refused.join(", ")));
                }
                l
            })
            .collect()
    }

    /// The tray's Overlays entries: `(id, label)`.
    pub fn tray(&self) -> Vec<(u64, String)> {
        self.entries
            .iter()
            .map(|e| {
                let state = match &e.state {
                    State::Loading => " (loading)".to_owned(),
                    State::Shown { at, .. } => format!(" since {at}"),
                };
                (
                    e.id,
                    format!(
                        "{} {}, {}{state}",
                        e.id,
                        e.kind.key(),
                        position_key(e.position)
                    ),
                )
            })
            .collect()
    }
}

/// The wall clock as the list shows it: a fixed reading in the tests, the local time in Kai.
pub enum Clock {
    Local,
    #[cfg(test)]
    Fixed(u64),
}

impl Clock {
    /// `HH:MM:SS`, `after` from now.
    pub fn at(&self, after: Duration) -> String {
        match self {
            Clock::Local => gtk::glib::DateTime::now_local()
                .and_then(|t| t.add_seconds(after.as_secs_f64()))
                .and_then(|t| t.format("%H:%M:%S"))
                .map(|t| t.to_string())
                .unwrap_or_else(|_| "?".into()),
            #[cfg(test)]
            Clock::Fixed(s) => {
                let s = s + after.as_secs();
                format!("{:02}:{:02}:{:02}", s / 3600 % 24, s / 60 % 60, s % 60)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOON: Clock = Clock::Fixed(12 * 3600);

    #[test]
    fn ids_count_up_and_are_never_reused() {
        let mut t = Table::default();
        let a = t.add(Kind::Text, Anchor::TopRight, false, None);
        let b = t.add(Kind::Html, Anchor::Center, true, None);
        assert_eq!((a, b), (1, 2));
        assert_eq!(t.hide(Some(2)), Ok(vec![2]));
        assert_eq!(t.hide(None), Ok(vec![1]));
        let c = t.add(Kind::Text, Anchor::TopRight, false, None);
        assert_eq!(c, 3, "a hidden id is never given again");
        // A stale id hides nothing else and says what there is.
        assert_eq!(
            t.hide(Some(1)),
            Err("no overlay 1 is shown (shown: 3)".into())
        );
        assert_eq!(t.ids(), [3]);
        t.hide(None).unwrap();
        assert_eq!(
            t.hide(Some(3)),
            Err("no overlay 3 is shown (none is)".into())
        );
        assert_eq!(
            t.hide(None),
            Ok(vec![]),
            "hiding all of nothing is no error"
        );
    }

    #[test]
    fn showing_adds_and_never_replaces() {
        let mut t = Table::default();
        let now = Instant::now();
        let a = t.add(Kind::Text, Anchor::TopRight, false, None);
        assert!(t.shown(a, now, &NOON, vec![]));
        let b = t.add(Kind::Text, Anchor::TopRight, false, None);
        assert!(t.shown(b, now, &NOON, vec![]));
        assert_eq!(
            t.ids(),
            [1, 2],
            "the same kind at the same place is a second overlay"
        );
    }

    #[test]
    fn a_hide_while_loading_wins_over_the_late_map() {
        let mut t = Table::default();
        let a = t.add(Kind::Html, Anchor::Center, false, None);
        assert_eq!(t.lines(), ["1 html center loading click-through"]);
        assert_eq!(t.hide(None), Ok(vec![a]));
        assert!(
            !t.shown(a, Instant::now(), &NOON, vec![]),
            "gone: not shown"
        );
        assert!(t.is_empty());
    }

    #[test]
    fn expiry_counts_from_the_show() {
        let mut t = Table::default();
        let now = Instant::now();
        let a = t.add(
            Kind::Text,
            Anchor::TopRight,
            false,
            Some(Duration::from_secs(60)),
        );
        let b = t.add(Kind::Text, Anchor::Top, false, None);
        // Loading: no expiry yet, however long it takes.
        assert_eq!(t.next_expiry(), None);
        assert!(t.expire(now + Duration::from_secs(3600)).is_empty());
        t.shown(a, now, &NOON, vec![]);
        t.shown(b, now, &NOON, vec![]);
        assert_eq!(t.next_expiry(), Some(now + Duration::from_secs(60)));
        assert!(t.expire(now + Duration::from_secs(59)).is_empty());
        assert_eq!(t.expire(now + Duration::from_secs(60)), [a]);
        assert_eq!(t.ids(), [b], "no timeout: until hidden");
        assert_eq!(t.next_expiry(), None);
    }

    #[test]
    fn refusals_are_counted_and_an_overlay_gone_without_a_call_is_said_once() {
        let mut t = Table::default();
        let now = Instant::now();
        let a = t.add(Kind::Html, Anchor::TopLeft, false, None);
        let b = t.add(Kind::Text, Anchor::TopRight, false, None);
        t.shown(a, now, &NOON, vec![]);
        t.shown(b, now, &NOON, vec![]);
        assert_eq!(t.count(a, Count::Navigation), Some(1));
        assert_eq!(t.count(a, Count::Navigation), Some(2));
        assert_eq!(t.count(a, Count::Permission), Some(1));
        assert_eq!(t.count(b, Count::Navigation), Some(1));
        assert_eq!(t.count(9, Count::Navigation), None);
        assert_eq!(
            t.list(),
            [
                "1 html top-left shown 12:00:00 until hidden click-through; refused: 2 \
                 navigations, 1 permission request",
                "2 text top-right shown 12:00:00 until hidden click-through; refused: 1 navigation",
            ]
        );
        t.ended(a, &NOON, "its web process ended");
        t.ended(7, &NOON, "never there");
        assert_eq!(
            t.list(),
            [
                "2 text top-right shown 12:00:00 until hidden click-through; refused: 1 navigation",
                "1 html top-left ended 12:00:00: its web process ended",
            ]
        );
        assert_eq!(t.ids(), [b]);
        t.ended(b, &NOON, "its output went away");
        assert_eq!(
            t.list(),
            ["2 text top-right ended 12:00:00: its output went away"],
            "said, not \"no overlays\""
        );
        assert_eq!(t.list(), ["no overlays"], "said once");
    }

    #[test]
    fn the_list_has_one_line_per_overlay() {
        let mut t = Table::default();
        assert_eq!(t.lines(), ["no overlays"]);
        let now = Instant::now();
        let a = t.add(
            Kind::Text,
            Anchor::TopRight,
            false,
            Some(Duration::from_secs(90)),
        );
        let b = t.add(Kind::Html, Anchor::BottomLeft, true, None);
        let _c = t.add(Kind::Text, Anchor::Center, false, None);
        t.shown(a, now, &NOON, vec![]);
        t.shown(
            b,
            now,
            &NOON,
            vec!["480x320 px fitted to 400x300 (the output less the margins)".into()],
        );
        assert_eq!(
            t.lines(),
            [
                "1 text top-right shown 12:00:00 until 12:01:30 click-through",
                "2 html bottom-left shown 12:00:00 until hidden interactive; 480x320 px fitted \
                 to 400x300 (the output less the margins)",
                "3 text center loading click-through",
            ]
        );
        assert_eq!(
            t.tray(),
            [
                (1, "1 text, top-right since 12:00:00".to_owned()),
                (2, "2 html, bottom-left since 12:00:00".to_owned()),
                (3, "3 text, center (loading)".to_owned()),
            ]
        );
    }
}
