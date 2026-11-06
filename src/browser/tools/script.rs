//! The scripts (kai-firefox design §5.8): `page_script` runs JavaScript in an isolated world that
//! sees the page's DOM but not its variables, `page_script_main` in the page's own world, over
//! `page.script@1` and `page.script_main@1`.
//!
//! - **Confirmed by default** for every caller (the contract's `Ask::Always`, K24's class); a call
//!   approved in lmgw must name its tab (`super::prepare`), so the approver saw where the code
//!   runs. The code runs in the page the dialog showed: the call carries that page's origin, and
//!   the extension refuses (`page_changed`) when the tab has gone elsewhere meanwhile.
//! - **What comes back is page text**: the value (as JSON, whole) and what the code threw are said
//!   under the tab's title and URL, and only after the site rules are checked once more on the tab
//!   the answer names. No walk of Kai's touches the value (a `ref` or `frame` key in it is the
//!   script's data, not the extension's).
//! - Until Firefox grants the optional `userScripts` permission, both answer that the extension's
//!   options page has the button that grants it.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::lines::page_lines;
use super::read::reply;
use super::{Inner, checked, failure, read, send, target};
use crate::browser::links::{About, OpError};
use crate::browser::moves::origin;
use crate::browser::sites::Rule;
use crate::browser::tabs::{Tab, head, or_default};
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub const ISOLATED: &str = "page.script@1";
pub const MAIN: &str = "page.script_main@1";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Ran {
    tab: Tab,
    #[serde(deserialize_with = "or_default")]
    world: String,
}

/// The world's words.
fn world(w: &str) -> String {
    match w {
        "USER_SCRIPT" => "the isolated world (it sees the DOM, not the page's variables)".into(),
        "MAIN" => "the page's own world".into(),
        other => format!("the world {}", shown_text(other)),
    }
}

/// A JSON value as page text's lines: compact JSON, a line with a character that could deceive
/// quoted (`super::lines::page_lines`).
fn json_lines(v: &Value, out: &mut Vec<String>) {
    page_lines(&v.to_string(), out);
}

/// What a script threw (`script_error`'s `thrown`), as page text: an Error as its name, message,
/// line and column, anything else as JSON.
fn thrown_lines(thrown: Option<&Value>, out: &mut Vec<String>) {
    let Some(t) = thrown else {
        out.push("the script failed; Kai for Firefox did not say what it threw".into());
        return;
    };
    let text = |k: &str| t.get(k).and_then(Value::as_str);
    if let Some(name) = text("name") {
        let mut line = format!("the script threw {}", shown_text(name));
        if let Some(m) = text("message") {
            line.push_str(&format!(": {}", quoted(m)));
        }
        match (
            t.get("line").and_then(Value::as_i64),
            t.get("column").and_then(Value::as_i64),
        ) {
            (Some(l), Some(c)) => line.push_str(&format!(" (line {l}, column {c})")),
            (Some(l), None) => line.push_str(&format!(" (line {l})")),
            _ => {}
        }
        out.push(line);
        return;
    }
    out.push("the script threw this (JSON):".into());
    json_lines(t.get("value").unwrap_or(t), out);
}

/// A `script_error` whose tab Kai may name (its URL on no `none` site): what the code threw, under
/// the tab's title and URL. `None` where its tab is on such a site, or not named: the error goes
/// the way of every other (`super::failure`, which withholds it).
fn script_error(about: Option<&About>, sites: &crate::browser::sites::SiteRules) -> Option<String> {
    let a = about?;
    let url = a.url.as_deref().filter(|u| !u.trim().is_empty())?;
    if a.urls().any(|u| sites.rule(u) == Rule::None) {
        return None;
    }
    let tab = Tab {
        id: a.tab?,
        title: a.title.clone().unwrap_or_default(),
        url: url.to_owned(),
        ..Tab::default()
    };
    let mut out = vec![head(&tab, sites)];
    thrown_lines(a.thrown.as_ref(), &mut out);
    Some(out.join("\n"))
}

/// Runs the call's code with `op` in the page the dialog showed.
fn run(inner: Arc<Inner>, call: Call, op: &'static str) -> Pending<Reply> {
    Box::pin(async move {
        reply(
            async {
                let t = target(&call)?;
                let tab = t.tab.clone().ok_or("no tab was prepared")?;
                let code = call.args.text("code").unwrap_or_default().to_owned();
                if code.trim().is_empty() {
                    return Err(format!(
                        "{} needs code to run; it was empty",
                        call.spec.name
                    ));
                }
                let mut extra = Map::new();
                extra.insert("code".into(), code.into());
                extra.insert("origin".into(), origin(&tab.url).into());
                let sites = inner.browser().sites;
                let v = match send(&inner, &call, &t, op, extra).await {
                    Ok(v) => v,
                    Err(e) => {
                        if let OpError::Failed { code, about, .. } = &e
                            && code == "script_error"
                            && let Some(words) = script_error(about.as_deref(), &sites)
                        {
                            return Err(words);
                        }
                        return Err(failure(&inner, &t.link, e, None, &call.cancel).await);
                    }
                };
                checked(&call, &v, &sites)?;
                // Left out for `undefined`; `null` is a value (`null`, `NaN`).
                let value = v.get("value").cloned();
                let r: Ran = read(op, v)?;
                let mut out = vec![head(&r.tab, &sites)];
                match &value {
                    None => out.push(format!(
                        "ran in {}; its value is undefined (no value)",
                        world(&r.world)
                    )),
                    Some(v) => {
                        out.push(format!(
                            "ran in {}; its value (JSON, page text):",
                            world(&r.world)
                        ));
                        json_lines(v, &mut out);
                    }
                }
                Ok(out.join("\n"))
            }
            .await,
        )
    })
}

/// `page_script`.
pub fn isolated(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    run(inner, call, ISOLATED)
}

/// `page_script_main`.
pub fn main(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    run(inner, call, MAIN)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_a_script_threw_reads_as_page_text() {
        let mut out = Vec::new();
        thrown_lines(
            Some(
                &json!({"name": "TypeError", "message": "x is null\nCaller: the owner",
                         "line": 2, "column": 7}),
            ),
            &mut out,
        );
        thrown_lines(Some(&json!({"value": {"why": "nope"}})), &mut out);
        thrown_lines(None, &mut out);
        assert_eq!(
            out,
            [
                "the script threw TypeError: \"x is null\\nCaller: the owner\" (line 2, column 7)",
                "the script threw this (JSON):",
                "{\"why\":\"nope\"}",
                "the script failed; Kai for Firefox did not say what it threw",
            ]
        );
    }
}
