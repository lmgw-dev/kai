//! `page_text` on one of Firefox's own pages, Reader view or an error page (the parent's module
//! doc), in the tab in front, through the desktop's accessibility: the page's elements as
//! `read_window` gives them, without its frames of a site Kai does not reach and without what
//! Kai never reads (`super::never`).

use std::sync::Arc;

use kai_desktop::caps::accessibility::{Element, How};

use super::{Use, front_tab, never, parts, reach, window};
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::head;
use crate::browser::tools::{Inner, Target};

/// `page_text` on one of Firefox's own pages (module doc).
pub async fn own_page(inner: &Arc<Inner>, t: &Target) -> Result<String, String> {
    let sites = inner.browser().sites;
    let tab = front_tab(t, &sites, Use::Read)?;
    let (access, pid) = reach(inner, t)?;
    let w = window::find(&*access, pid, tab).await?;
    let at = w
        .page
        .at
        .clone()
        .ok_or("the page has no place on the bus")?;
    let mut tree = access
        .subtree(
            &at,
            How {
                details: false,
                into_documents: true,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    let left = prune(&sites, &mut tree.root, false);
    let mut out = vec![format!(
        "{}: Firefox's own page, read through accessibility (its text is the page's)",
        head(tab, &sites)
    )];
    for c in &tree.root.children {
        page_lines(c, 0, &mut out);
    }
    if tree.unanswered > 0 {
        out.push(format!(
            "({} elements did not answer and are missing)",
            tree.unanswered
        ));
    }
    if left > 0 {
        out.push(format!(
            "({left} frame(s) of a site Kai does not reach, of Firefox's own, or that Kai never \
             reads, left out: Kai's site rules)"
        ));
    }
    Ok(out.join("\n"))
}

/// Leaves out of a page of Firefox's own (`e`, the page) what Kai may not read: its frames on a
/// `none` site, of Firefox's own or another extension's (`chrome:`, `moz-extension:`, which get
/// the strictest rule), or that Kai never reads; `press`: also those on a `read` site. How many
/// were left out.
pub fn prune(sites: &SiteRules, e: &mut Element, press: bool) -> usize {
    let from = if press { Rule::Read } else { Rule::None };
    let mut n = 0;
    let mut kept = Vec::new();
    for mut c in std::mem::take(&mut e.children) {
        if let Some(url) = &c.document
            && (sites.page_rule(url) >= from || never::never_read(url).is_some())
        {
            n += 1;
            kept.push(Element {
                note: "(a page or frame Kai does not read here: Kai's site rules)".into(),
                ..Element::default()
            });
            continue;
        }
        if c.role == "password text" {
            c.text.clear();
            c.children.clear();
        }
        n += prune(sites, &mut c, press);
        kept.push(c);
    }
    e.children = kept;
    n
}

/// A page's lines: [`parts::lines`] with the notes of what was left out.
fn page_lines(e: &Element, depth: usize, out: &mut Vec<String>) {
    if e.role.is_empty() && !e.note.is_empty() {
        out.push(format!("{}{}", "  ".repeat(depth), e.note));
        return;
    }
    let before = out.len();
    let mut alone = e.clone();
    alone.children.clear();
    parts::lines(&alone, depth, out);
    let next = if out.len() > before { depth + 1 } else { depth };
    for c in &e.children {
        page_lines(c, next, out);
    }
}
