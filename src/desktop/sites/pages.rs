//! The pages a desktop tool read or presses in, inside a Firefox window (the parent's module doc):
//! AT-SPI names each document below the window with the address it shows (`Element::document`,
//! `Found::documents`), so the tree that was actually read, and the element actually pressed, is
//! judged by its own pages, whichever window the accessibility bus took for the one the window
//! list named.
//!
//! - **Reading** ([`prune`]): every page or frame of a Firefox window's tree whose rule refuses
//!   reading (`SiteRules::page_rule`: a site Kai does not reach, or a page without a site while
//!   some site is `none`, such as an `about:blank` frame, whose origin AT-SPI does not say) is left
//!   out with everything in it, its place said by one line; the answer says how many.
//! - **Pressing** ([`found`]): an element of a Firefox window is pressed only inside a page, and
//!   only while neither that page nor the frame the element is in is on a `read` or `none` site.
//!   Firefox's own controls (its tabs, the address bar, back and forward, the bookmarks) are not
//!   pressed under the rules: the browser tools (`tab_switch`, `tab_go`, `tab_back`, …) do that
//!   with the rules applied.

use kai_desktop::caps::accessibility::{Element, Found, Tree};
use kai_desktop::caps::windows::Window;

use super::{Checked, Services, rules};
use crate::browser::shown::{Reach, rule_refusal, window_name};
use crate::browser::sites::SiteRules;

/// The line in place of a page or frame left out.
pub const LEFT_OUT: &str = "(a page or frame of a site Kai does not reach, not read)";

/// `tree`, read from the window `checked` let through, without the pages and frames Kai may not
/// read (module doc); how many were left out. Nothing is left out of another app's window, or
/// while no site is `none`.
pub fn prune(services: &Services, checked: Option<&Checked>, tree: &mut Tree) -> usize {
    let Some((_, sites)) = rules(services, Reach::See) else {
        return 0;
    };
    if !checked.is_some_and(Checked::firefox) {
        return 0;
    }
    let mut out = 0;
    for c in &mut tree.root.children {
        out += cut(&sites, c);
    }
    out
}

/// [`prune`] below `e`.
fn cut(sites: &SiteRules, e: &mut Element) -> usize {
    if let Some(url) = &e.document
        && sites.page_rule(url) >= Reach::See.refused_from()
    {
        *e = Element {
            note: LEFT_OUT.into(),
            ..Element::default()
        };
        return 1;
    }
    e.children.iter_mut().map(|c| cut(sites, c)).sum()
}

/// The line an answer adds for `n` pages or frames left out.
pub fn left_out_note(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some(
            "1 page or frame of a site Kai does not reach, or one Kai cannot tell, was left out \
             (Kai's site rules)"
                .into(),
        ),
        n => Some(format!(
            "{n} pages or frames of sites Kai does not reach, or ones Kai cannot tell, were left \
             out (Kai's site rules)"
        )),
    }
}

/// Checks the element `found` in the window `w` before `tool` presses it (module doc); nothing
/// is checked while no site is `read` or `none`, or in another app's window.
pub fn found(services: &Services, w: &Window, found: &Found, tool: &str) -> Result<(), String> {
    let Some((shown, sites)) = rules(services, Reach::Act) else {
        return Ok(());
    };
    if !shown.firefox(w) {
        return Ok(());
    }
    let name = window_name(w);
    let (Some(page), Some(frame)) = (found.documents.first(), found.documents.last()) else {
        return Err(format!(
            "the element is one of Firefox's own controls in {name} (its tabs, its address bar, \
             its buttons), outside any page; while Kai's site rules let Kai only read some sites \
             or keep them from Kai, Firefox's own controls are worked by the browser tools \
             (tab_switch, tab_go, tab_back and the others, and firefox_press for its dialogs, \
             prompts and menus), which apply the rules; {tool} did not run"
        ));
    };
    for (url, what) in [
        (page, format!("the page of the element in {name}")),
        (frame, format!("the frame of the element in {name}")),
    ] {
        if let Some(r) = rule_refusal(sites.page_rule(url), Reach::Act, &what) {
            return Err(format!("{r}; {tool} did not run"));
        }
    }
    Ok(())
}
