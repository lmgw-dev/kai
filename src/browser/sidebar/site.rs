//! The sidebar's question about sites (kai-firefox design §8.1: the site strip and "Attach this
//! page"; the wire is kai-firefox's `docs/protocol/operations.md`, "The sidebar's own questions"):
//! the extension asks `{"t": "site", "id": …, "urls": [<URL or origin>, …]}` and Kai answers
//! `{"t": "site", "id": …, "rules": [{"site": …, "rule": "act" | "read" | "none"}, …]}` in the same
//! order, each by Kai's site rules as the browser tools apply them to a frame
//! (`SiteRules::frame_rule`): a page's URL its site's rule, a frame's origin its own site's, and
//! `""` or `"null"` (an opaque origin, a page whose site cannot be told) the strictest rule any
//! entry gives.
//!
//! - `site` is the host (and port) the rule was read for, after the rules' unwrapping (Reader view,
//!   `view-source:`, `blob:`); `""` for an address without one.
//! - A question that does not read (no id, `urls` no list of texts) gets an `error` frame
//!   `bad_site` with its `id` where it has one, which the extension ends the question with.
//! - Nothing goes to lmgw, and the answer says only what Kai's settings window shows anyway: the
//!   rules are the owner's, and the sidebar is Kai (decision 9).

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::browser::sites::{SiteRules, effective};

/// Kai's site rules now (`[browser.sites]` as the policy holds it, which a write of the settings
/// changes).
pub type Rules = Arc<dyn Fn() -> SiteRules + Send + Sync>;

/// The host (with its port) a rule is read for, `""` without one.
fn site_of(url: &str) -> String {
    let Some(u) = effective(url) else {
        return String::new();
    };
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{}:{p}", h.trim_end_matches('.')),
        (Some(h), None) => h.trim_end_matches('.').to_owned(),
        _ => String::new(),
    }
}

/// The answer to a `site` question (its fields without `t`), or the `error` frame for one that
/// does not read.
pub fn answer(frame: &Map<String, Value>, rules: &SiteRules) -> Value {
    let id = frame.get("id").cloned();
    let urls = frame.get("urls").and_then(Value::as_array).and_then(|l| {
        l.iter()
            .map(|u| u.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    });
    let (Some(id), Some(urls)) = (id.clone(), urls) else {
        return json!({
            "t": "error",
            "code": "bad_site",
            "id": id,
            "message": "a site question needs an id and urls, a list of texts",
        });
    };
    let answered: Vec<Value> = urls
        .iter()
        .map(|u| json!({"site": site_of(u), "rule": rules.frame_rule(Some(u)).word()}))
        .collect();
    json!({"t": "site", "id": id, "rules": answered})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::settings::Sites;

    fn rules(read: &[&str], none: &[&str]) -> SiteRules {
        let v = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        SiteRules::read(&Sites {
            act: Vec::new(),
            read: v(read),
            none: v(none),
        })
        .unwrap()
    }

    fn ask(v: Value, r: &SiteRules) -> Value {
        answer(v.as_object().unwrap(), r)
    }

    #[test]
    fn each_address_gets_its_site_and_rule_in_order() {
        let r = rules(&["news.example"], &["*.bank.example"]);
        let a = ask(
            json!({"id": 7, "urls": [
                "https://news.example/today",
                "https://login.bank.example:8443/",
                "https://shop.example/cart",
                "about:reader?url=https%3A%2F%2Fwww.bank.example%2Fa",
                "",
                "null",
                "moz-extension://0a1b/sidebar/sidebar.html",
            ]}),
            &r,
        );
        assert_eq!(
            a,
            json!({"t": "site", "id": 7, "rules": [
                {"site": "news.example", "rule": "read"},
                {"site": "login.bank.example:8443", "rule": "none"},
                {"site": "shop.example", "rule": "act"},
                {"site": "www.bank.example", "rule": "none"},
                // An opaque origin could be any site: the strictest rule any entry gives.
                {"site": "", "rule": "none"},
                // So is an opaque origin as a browser serializes it.
                {"site": "", "rule": "none"},
                // Another extension's page: the extension never asks about one (it reads web
                // pages and local files only); its host is no site, and no entry matches it.
                {"site": "0a1b", "rule": "act"},
            ]})
        );
        // Without rules every site is act's, an opaque origin too.
        let a = ask(
            json!({"id": "x", "urls": ["", "null", "https://a.example"]}),
            &SiteRules::default(),
        );
        assert_eq!(
            a["rules"],
            json!([
                {"site": "", "rule": "act"},
                {"site": "", "rule": "act"},
                {"site": "a.example", "rule": "act"},
            ])
        );
        // With only read entries, "null" is read, as "" is: not act.
        let a = ask(
            json!({"id": 8, "urls": ["null", ""]}),
            &rules(&["news.example"], &[]),
        );
        assert_eq!(
            a["rules"],
            json!([{"site": "", "rule": "read"}, {"site": "", "rule": "read"}])
        );
    }

    #[test]
    fn a_question_that_does_not_read_is_an_error_frame_with_its_id() {
        let r = SiteRules::default();
        assert_eq!(
            ask(json!({"id": 3, "urls": "https://a.example"}), &r),
            json!({"t": "error", "code": "bad_site", "id": 3,
                   "message": "a site question needs an id and urls, a list of texts"})
        );
        assert_eq!(ask(json!({"urls": []}), &r)["id"], Value::Null);
        assert_eq!(ask(json!({"id": 1, "urls": [1]}), &r)["code"], "bad_site");
    }
}
