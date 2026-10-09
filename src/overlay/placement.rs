//! Where the ring goes (§9.1): the anchor and margins from the configuration, the layer from
//! `overlay.ring_over_fullscreen` (§16 answer 5), and the output, named explicitly.
//!
//! The output is `ring.output` when it is connected, else KWin's active output, else the
//! compositor's choice (nil). With no output listed at all there is nothing to choose from
//! ([`Listed::NoOutput`]). GDK3 knows the outputs only by geometry and model; their connector
//! names come from wl_output (v4 `name`), and [`monitor_for`] matches the two by position.

use serde::{Deserialize, Serialize};

/// The corner, edge or centre of the output the ring sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

/// The layer-shell edges an anchor pins: a corner pins two, an edge one, the centre none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Anchor {
    pub fn edges(self) -> Edges {
        use Anchor::*;
        Edges {
            top: matches!(self, TopLeft | Top | TopRight),
            bottom: matches!(self, BottomLeft | Bottom | BottomRight),
            left: matches!(self, TopLeft | Left | BottomLeft),
            right: matches!(self, TopRight | Right | BottomRight),
        }
    }
}

/// The layer the surface is created on: OVERLAY stays above a fullscreen window, TOP is hidden by
/// one (WP0 (b)). Chosen each time the surface is mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Overlay,
    Top,
}

impl Layer {
    pub fn for_setting(ring_over_fullscreen: bool) -> Layer {
        if ring_over_fullscreen {
            Layer::Overlay
        } else {
            Layer::Top
        }
    }
}

/// What the compositor's output list leaves to the output decision (§9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    /// No output is listed at all: nothing to choose from, so nothing is mapped. A map with no
    /// output named would leave the choice to a compositor that has none to give (review KAI-6
    /// R6-2, R6-10).
    NoOutput,
    /// The connectors to choose from ([`choose_output`]), and whether KWin's active output is
    /// needed for the choice: no `ring.output`, or one that is not among them.
    Outputs {
        names: Vec<String>,
        need_active: bool,
    },
}

impl Listed {
    /// `configured` is `ring.output`, `outputs` what wl_output lists.
    pub fn of(configured: Option<&str>, outputs: &[WlOutput]) -> Listed {
        if outputs.is_empty() {
            return Listed::NoOutput;
        }
        let names: Vec<String> = outputs.iter().map(|o| o.name.clone()).collect();
        let need_active = configured.is_none_or(|c| !names.iter().any(|n| n == c));
        Listed::Outputs { names, need_active }
    }
}

/// The output decision: a connector name, or `None` when none can be named, and why. The note
/// gives only the cause; what follows from it depends on the show ([`Placing::line`]). A name with
/// a note is KWin's active output standing in for a `ring.output` that is not connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChoice {
    pub name: Option<String>,
    pub note: Option<String>,
}

/// `configured` is the setting `key` (`ring.output`, or the widgets' `overlay.output`), `active`
/// KWin's active output, `connected` the connectors.
pub fn choose_output(
    key: &str,
    configured: Option<&str>,
    active: Result<&str, &str>,
    connected: &[String],
) -> OutputChoice {
    let present = |n: &str| connected.iter().any(|c| c == n);
    let mut note = None;
    if let Some(c) = configured {
        if present(c) {
            return OutputChoice {
                name: Some(c.to_owned()),
                note: None,
            };
        }
        note = Some(format!(
            "{key} '{c}' is not connected (connected: {})",
            list(connected)
        ));
    }
    match active {
        Ok(a) if present(a) => OutputChoice {
            name: Some(a.to_owned()),
            note,
        },
        Ok(a) => OutputChoice {
            name: None,
            note: Some(join(
                note,
                format!(
                    "KWin's active output '{a}' is not among the outputs ({})",
                    list(connected)
                ),
            )),
        },
        Err(e) => OutputChoice {
            name: None,
            note: Some(join(note, format!("KWin's active output is unknown ({e})"))),
        },
    }
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

fn join(a: Option<String>, b: String) -> String {
    match a {
        Some(a) => format!("{a}; {b}"),
        None => b,
    }
}

/// What a show does with the output (§9.1): the resolved one; for a ring that is up and nothing
/// resolved, the output it is on (review KAI-5 R5-5); else the compositor's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placing {
    Resolved,
    Keep,
    Compositor,
}

impl Placing {
    pub fn of(resolved: bool, mapped: bool) -> Placing {
        match (resolved, mapped) {
            (true, _) => Placing::Resolved,
            (false, true) => Placing::Keep,
            (false, false) => Placing::Compositor,
        }
    }

    /// The log line of a show: the resolution's note, then what happens, once (review KAI-5
    /// R5-12, KAI-6 R6-4). `output` is the resolved connector; a resolved output with a note is
    /// the active output standing in for a missing `ring.output` ([`choose_output`]).
    pub fn line(self, note: Option<&str>, output: Option<&str>) -> Option<String> {
        let cause = note.unwrap_or("no output was resolved");
        match self {
            Placing::Resolved => note.map(|n| match output {
                Some(o) => format!("{n}; the ring goes to the active output '{o}'"),
                None => n.to_owned(),
            }),
            Placing::Keep => Some(format!(
                "{cause}; the ring that is up stays on the output it is on"
            )),
            Placing::Compositor => Some(format!("{cause}; the compositor chooses")),
        }
    }
}

/// An output as wl_output describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WlOutput {
    /// The connector (`DP-1`), from wl_output v4's `name`.
    pub name: String,
    /// The position in the compositor's (logical) space.
    pub x: i32,
    pub y: i32,
    pub make: String,
    pub model: String,
}

/// A GDK monitor as GDK3 describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub make: String,
    pub model: String,
}

/// The index of the GDK monitor showing connector `name`: the one at the output's position (outputs
/// never share a top-left corner), or else the only one of the output's make and model.
pub fn monitor_for(name: &str, outputs: &[WlOutput], monitors: &[Monitor]) -> Option<usize> {
    let out = outputs.iter().find(|o| o.name == name)?;
    if let Some(i) = monitors.iter().position(|m| (m.x, m.y) == (out.x, out.y)) {
        return Some(i);
    }
    let same: Vec<usize> = monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| (m.make.as_str(), m.model.as_str()) == (&out.make, &out.model))
        .map(|(i, _)| i)
        .collect();
    match same.as_slice() {
        [i] => Some(*i),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn anchors_pin_their_edges() {
        let e = Anchor::BottomRight.edges();
        assert!(e.bottom && e.right && !e.top && !e.left);
        let e = Anchor::Top.edges();
        assert!(e.top && !e.bottom && !e.left && !e.right);
        assert_eq!(Anchor::Center.edges(), Edges::default());
        assert_eq!(Anchor::default(), Anchor::BottomRight, "§9.1's default");
    }

    #[test]
    fn the_setting_chooses_the_layer() {
        assert_eq!(Layer::for_setting(true), Layer::Overlay);
        assert_eq!(Layer::for_setting(false), Layer::Top);
    }

    #[test]
    fn a_connected_ring_output_wins() {
        let c = choose_output(
            "ring.output",
            Some("HDMI-A-1"),
            Ok("DP-1"),
            &names(&["DP-1", "HDMI-A-1"]),
        );
        assert_eq!(c.name.as_deref(), Some("HDMI-A-1"));
        assert_eq!(c.note, None);
    }

    #[test]
    fn without_ring_output_the_active_output_is_named() {
        let c = choose_output(
            "ring.output",
            None,
            Ok("DP-1"),
            &names(&["DP-1", "HDMI-A-1"]),
        );
        assert_eq!((c.name.as_deref(), c.note), (Some("DP-1"), None));
    }

    #[test]
    fn a_missing_ring_output_falls_back_to_the_active_output_and_says_so() {
        let c = choose_output(
            "ring.output",
            Some("HDMI-A-1"),
            Ok("DP-1"),
            &names(&["DP-1"]),
        );
        assert_eq!(c.name.as_deref(), Some("DP-1"));
        let note = c.note.unwrap();
        assert!(
            note.contains("ring.output 'HDMI-A-1' is not connected (connected: DP-1)"),
            "{note}"
        );
    }

    #[test]
    fn no_active_output_leaves_the_choice_to_the_compositor() {
        let c = choose_output(
            "ring.output",
            None,
            Err("org.kde.KWin is not on the bus"),
            &names(&["DP-1"]),
        );
        assert_eq!(c.name, None);
        assert!(c.note.unwrap().contains("org.kde.KWin is not on the bus"));
        let c = choose_output("ring.output", Some("X"), Ok("Y"), &[]);
        assert_eq!(c.name, None);
        let note = c.note.unwrap();
        assert!(
            note.contains("'X' is not connected (connected: none)") && note.contains("'Y'"),
            "{note}"
        );
    }

    #[test]
    fn a_show_says_what_follows_from_an_unresolved_output() {
        // Review KAI-5 R5-12: a ring that is up keeps its output; only a new map leaves the choice
        // to the compositor, and each line says only what happens.
        let c = choose_output("ring.output", None, Err("no reply"), &names(&["DP-1"]));
        let keep = Placing::of(false, true)
            .line(c.note.as_deref(), None)
            .unwrap();
        assert_eq!(
            keep,
            "KWin's active output is unknown (no reply); the ring that is up stays on the output \
             it is on"
        );
        let new = Placing::of(false, false)
            .line(c.note.as_deref(), None)
            .unwrap();
        assert_eq!(
            new,
            "KWin's active output is unknown (no reply); the compositor chooses"
        );
        // Review KAI-6 R6-4: a configured output that is missing, and no active output either,
        // still says one outcome.
        let c = choose_output(
            "ring.output",
            Some("DP-1"),
            Err("no reply"),
            &names(&["HDMI-A-1"]),
        );
        assert_eq!(c.name, None);
        let keep = Placing::of(false, true)
            .line(c.note.as_deref(), None)
            .unwrap();
        assert_eq!(
            keep,
            "ring.output 'DP-1' is not connected (connected: HDMI-A-1); KWin's active output is \
             unknown (no reply); the ring that is up stays on the output it is on"
        );
        let new = Placing::of(false, false)
            .line(c.note.as_deref(), None)
            .unwrap();
        assert_eq!(
            new,
            "ring.output 'DP-1' is not connected (connected: HDMI-A-1); KWin's active output is \
             unknown (no reply); the compositor chooses"
        );
        // The active output standing in for the configured one says where the ring goes, once.
        let c = choose_output(
            "ring.output",
            Some("HDMI-A-1"),
            Ok("DP-1"),
            &names(&["DP-1"]),
        );
        let line = Placing::of(true, true)
            .line(c.note.as_deref(), c.name.as_deref())
            .unwrap();
        assert_eq!(
            line,
            "ring.output 'HDMI-A-1' is not connected (connected: DP-1); the ring goes to the \
             active output 'DP-1'"
        );
        // A resolved output with nothing that fell back has no line.
        assert_eq!(Placing::of(true, false).line(None, Some("DP-1")), None);
    }

    fn out(name: &str, x: i32, model: &str) -> WlOutput {
        WlOutput {
            name: name.into(),
            x,
            y: 0,
            make: "Vendor".into(),
            model: model.into(),
        }
    }

    fn mon(x: i32, model: &str) -> Monitor {
        Monitor {
            x,
            y: 0,
            make: "Vendor".into(),
            model: model.into(),
        }
    }

    #[test]
    fn monitors_are_matched_by_position_then_by_a_unique_model() {
        let outputs = [out("DP-1", 0, "A"), out("DP-2", 2560, "A")];
        let monitors = [mon(2560, "A"), mon(0, "A")];
        assert_eq!(monitor_for("DP-2", &outputs, &monitors), Some(0));
        assert_eq!(monitor_for("DP-1", &outputs, &monitors), Some(1));
        assert_eq!(monitor_for("HDMI-A-1", &outputs, &monitors), None);
        // Positions disagree (a scale change in between): only a unique model decides.
        let outputs = [out("DP-1", 0, "A"), out("DP-2", 9999, "B")];
        let monitors = [mon(10, "A"), mon(20, "B")];
        assert_eq!(monitor_for("DP-2", &outputs, &monitors), Some(1));
        let monitors = [mon(10, "B"), mon(20, "B")];
        assert_eq!(monitor_for("DP-2", &outputs, &monitors), None, "ambiguous");
    }

    #[test]
    fn an_empty_output_list_leaves_nothing_to_choose_from() {
        // Review KAI-6 R6-2, R6-10: nothing listed is no output, whatever `ring.output` names,
        // never a choice that names none and leaves the map to the compositor.
        assert_eq!(Listed::of(None, &[]), Listed::NoOutput);
        assert_eq!(Listed::of(Some("DP-2"), &[]), Listed::NoOutput);
        let outputs = [out("DP-1", 0, "A"), out("DP-2", 2560, "A")];
        let listed = |need_active| Listed::Outputs {
            names: names(&["DP-1", "DP-2"]),
            need_active,
        };
        assert_eq!(Listed::of(Some("DP-2"), &outputs), listed(false));
        assert_eq!(Listed::of(Some("HDMI-A-1"), &outputs), listed(true));
        assert_eq!(Listed::of(None, &outputs), listed(true));
    }
}
