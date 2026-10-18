//! `/mcp`'s tool list as Kai last fetched it, so a tool call in Kai's conversation whose tool has
//! no view opens no `/mcp` session (`initialize`, `tools/list`, `DELETE`) to learn that again
//! (`super::Apps::open_for_call`).
//!
//! - **What is kept:** each tool's exposed name and whether it names a UI resource; not the list.
//!   A tool the list names a view for is still opened with a list fetched in the view's own session
//!   (its bridge's rules are that list's), which refreshes this one.
//! - **When it goes:** every fetch replaces it (`overlay_app`'s, a call's view's); another pairing's
//!   credentials do not read it; Kai's voice session ending forgets it (`super::super::thread_apps`).
//!   A tool it does not hold (a server connected since) is looked up afresh, and a result frame
//!   naming a view for a tool it said has none opens that view anyway (the frame is lmgw's word).
//!   lmgw's `notifications/tools/list_changed` never reaches Kai: its `/mcp` sessions only POST
//!   (no `GET` stream), so these are the invalidations there are.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::lmgw_link::Credentials;

/// What the kept list says of one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// No list kept for these credentials, or the tool is not in it: ask `/mcp`.
    Unknown,
    /// Listed without a UI resource.
    NoView,
    /// Listed with a UI resource (`_meta.ui.resourceUri`, or the deprecated flat key).
    View,
}

/// The kept list: per exposed tool name, whether it names a UI resource.
#[derive(Default)]
pub struct Lists(Mutex<Option<Kept>>);

struct Kept {
    /// Whose list it is (the pairing: its device certificate, hashed).
    whose: [u8; 32],
    tools: HashMap<String, bool>,
}

fn whose(c: &Credentials) -> [u8; 32] {
    *c.tls.identity()
}

impl Lists {
    /// A `tools/list` result fetched under `c`: it replaces what was kept.
    pub fn put(&self, c: &Credentials, list: &Value) {
        let tools = list["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_owned();
                Some((name, super::resource_uri(t).is_some()))
            })
            .collect();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(Kept {
            whose: whose(c),
            tools,
        });
    }

    /// What the list kept for `c` says of the tool `exposed`.
    pub fn seen(&self, c: &Credentials, exposed: &str) -> Seen {
        let kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match kept.as_ref().filter(|k| k.whose == whose(c)) {
            Some(k) => match k.tools.get(exposed) {
                Some(true) => Seen::View,
                Some(false) => Seen::NoView,
                None => Seen::Unknown,
            },
            None => Seen::Unknown,
        }
    }

    /// Kai's voice session ended: the next call asks `/mcp` again.
    pub fn forget(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    use crate::lmgw_link::test_pki::{FOREIGN, PKI};

    #[test]
    fn the_kept_list_answers_for_its_own_credentials_until_forgotten() {
        let l = Lists::default();
        let a = PKI.creds("https://a");
        assert_eq!(l.seen(&a, "fx__show"), Seen::Unknown);
        l.put(
            &a,
            &json!({"tools": [
                {"name": "fx__show", "_meta": {"ui": {"resourceUri": "ui://fx__card"}}},
                {"name": "fx__old", "_meta": {"ui/resourceUri": "ui://fx__old"}},
                {"name": "fx__plain"},
            ]}),
        );
        assert_eq!(l.seen(&a, "fx__show"), Seen::View);
        assert_eq!(l.seen(&a, "fx__old"), Seen::View);
        assert_eq!(l.seen(&a, "fx__plain"), Seen::NoView);
        assert_eq!(l.seen(&a, "fx__new"), Seen::Unknown, "not in the list");
        // Another pairing does not read it; the same pairing at another of its URLs does.
        assert_eq!(
            l.seen(&FOREIGN.creds("https://a"), "fx__plain"),
            Seen::Unknown
        );
        assert_eq!(l.seen(&PKI.creds("https://b"), "fx__plain"), Seen::NoView);
        // A fetch replaces it.
        l.put(&a, &json!({"tools": [{"name": "fx__plain", "_meta": {"ui": {"resourceUri": "ui://fx__p"}}}]}));
        assert_eq!(l.seen(&a, "fx__plain"), Seen::View);
        assert_eq!(l.seen(&a, "fx__show"), Seen::Unknown);
        l.forget();
        assert_eq!(l.seen(&a, "fx__plain"), Seen::Unknown);
    }
}
