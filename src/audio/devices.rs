//! What the PipeWire thread knows about the graph: the audio devices and the defaults, from the
//! registry and the `default` metadata. Pure, so the tray's lists and the AEC view are testable.

use std::collections::BTreeMap;

use super::aec::{self, GraphView};

/// A device the tray offers (§7: Microphone, Speaker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `node.name`: what Kai stores and targets with `target.object`.
    pub name: String,
    /// `node.description` (else the nick, else the name): what the menu shows.
    pub label: String,
}

/// The devices and defaults as the tray shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceList {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
    /// The effective defaults (`default.audio.source` / `.sink`).
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

impl DeviceList {
    pub fn label_of<'a>(devices: &'a [Device], name: &str) -> Option<&'a str> {
        devices
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.label.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    name: String,
    label: String,
    kind: Option<Kind>,
}

/// Nodes by id, and the four default keys.
#[derive(Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<u32, Node>,
    pub configured_sink: Option<String>,
    pub configured_source: Option<String>,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
}

/// The device side of a `media.class`: plain sources and sinks only (`…/Internal` nodes are
/// hidden from programs on purpose, monitors are not nodes).
fn kind_of(class: &str) -> Option<Kind> {
    match class {
        "Audio/Source" | "Audio/Source/Virtual" => Some(Kind::Input),
        "Audio/Sink" => Some(Kind::Output),
        _ => None,
    }
}

impl Graph {
    /// A node global with its `media.class`, `node.name`, `node.description` and `node.nick`.
    pub fn add_node(
        &mut self,
        id: u32,
        class: Option<&str>,
        name: Option<&str>,
        description: Option<&str>,
        nick: Option<&str>,
    ) {
        let Some(name) = name else { return };
        let label = description.or(nick).unwrap_or(name).to_owned();
        self.nodes.insert(
            id,
            Node {
                name: name.to_owned(),
                label,
                kind: class.and_then(kind_of),
            },
        );
    }

    /// A global went away; true when it was a node.
    pub fn remove(&mut self, id: u32) -> bool {
        self.nodes.remove(&id).is_some()
    }

    pub fn has_node(&self, name: &str) -> bool {
        self.nodes.values().any(|n| n.name == name)
    }

    /// A `default` metadata property on subject 0. Returns true when it was one of the four keys.
    pub fn set_default(&mut self, key: &str, value: Option<&str>) -> bool {
        let slot = match key {
            "default.configured.audio.sink" => &mut self.configured_sink,
            "default.configured.audio.source" => &mut self.configured_source,
            "default.audio.sink" => &mut self.default_sink,
            "default.audio.source" => &mut self.default_source,
            _ => return false,
        };
        *slot = value.and_then(name_of_json);
        true
    }

    /// The metadata was cleared (a `None` key).
    pub fn clear_defaults(&mut self) {
        self.configured_sink = None;
        self.configured_source = None;
        self.default_sink = None;
        self.default_source = None;
    }

    pub fn devices(&self) -> DeviceList {
        let list = |kind| {
            let mut v: Vec<Device> = self
                .nodes
                .values()
                .filter(|n| n.kind == Some(kind) && !aec::is_own_node(&n.name))
                .map(|n| Device {
                    name: n.name.clone(),
                    label: n.label.clone(),
                })
                .collect();
            v.sort_by(|a, b| a.label.cmp(&b.label).then(a.name.cmp(&b.name)));
            v.dedup_by(|a, b| a.name == b.name);
            v
        };
        DeviceList {
            inputs: list(Kind::Input),
            outputs: list(Kind::Output),
            default_input: self.default_source.clone(),
            default_output: self.default_sink.clone(),
        }
    }

    pub fn aec_view(&self) -> GraphView {
        GraphView {
            configured_sink: self.configured_sink.clone(),
            configured_source: self.configured_source.clone(),
            default_sink: self.default_sink.clone(),
            default_source: self.default_source.clone(),
            has_sink: self.has_node(aec::SINK),
            has_source: self.has_node(aec::SOURCE),
        }
    }
}

/// A default's value: SPA-JSON `{"name": "<node.name>"}`.
pub fn name_of_json(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// The value Kai writes for a default.
pub fn json_of_name(name: &str) -> String {
    serde_json::json!({ "name": name }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_hold_plain_devices_sorted_by_label_without_kais_own() {
        let mut g = Graph::default();
        g.add_node(1, Some("Audio/Sink"), Some("spk"), Some("Speakers"), None);
        g.add_node(2, Some("Audio/Source"), Some("mic"), None, Some("Mic nick"));
        g.add_node(
            3,
            Some("Audio/Source/Internal"),
            Some("pre"),
            Some("Hidden"),
            None,
        );
        g.add_node(4, Some("Stream/Output/Audio"), Some("Firefox"), None, None);
        g.add_node(5, Some("Audio/Sink"), Some(aec::SINK), Some("Kai"), None);
        g.add_node(6, Some("Audio/Sink"), Some("hdmi"), Some("HDMI"), None);
        g.add_node(
            7,
            Some("Audio/Source/Virtual"),
            Some("v"),
            Some("A virtual"),
            None,
        );
        g.add_node(8, Some("Audio/Sink"), None, Some("nameless"), None);
        let d = g.devices();
        let names = |v: &[Device]| v.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&d.outputs), ["hdmi", "spk"]);
        assert_eq!(names(&d.inputs), ["v", "mic"]);
        assert_eq!(DeviceList::label_of(&d.inputs, "mic"), Some("Mic nick"));
        assert!(g.remove(6));
        assert!(!g.remove(6));
        assert_eq!(names(&g.devices().outputs), ["spk"]);
    }

    #[test]
    fn defaults_come_from_the_metadata_json() {
        let mut g = Graph::default();
        assert!(g.set_default("default.audio.sink", Some(r#"{ "name": "spk" }"#)));
        assert!(g.set_default(
            "default.configured.audio.source",
            Some(&json_of_name("a \"b\""))
        ));
        assert!(!g.set_default("default.video.source", Some(r#"{"name":"cam"}"#)));
        assert_eq!(g.default_sink.as_deref(), Some("spk"));
        assert_eq!(g.configured_source.as_deref(), Some("a \"b\""));
        assert!(g.set_default("default.audio.sink", None));
        assert_eq!(g.default_sink, None);
        assert!(g.set_default("default.audio.source", Some("not json")));
        assert_eq!(g.default_source, None);
        g.configured_sink = Some("x".into());
        g.clear_defaults();
        assert_eq!(g.aec_view(), GraphView::default());
    }

    #[test]
    fn the_aec_view_sees_kais_nodes() {
        let mut g = Graph::default();
        g.add_node(5, Some("Audio/Sink"), Some(aec::SINK), None, None);
        assert!(g.aec_view().has_sink && !g.aec_view().has_source);
        g.add_node(6, Some("Audio/Source"), Some(aec::SOURCE), None, None);
        assert!(g.aec_view().has_source);
    }
}
