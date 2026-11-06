//! The frame with the keyboard focus in a linked Firefox's window (the parent's module doc):
//! before Kai's keys go to it, `context.get@1` names the focused element of the user's tab and,
//! where it is in a child frame, that frame's origin (the contract's Element and Origin), so a
//! frame of a `read` or `none` site inside an `act` page refuses the keys by its own rule, as the
//! browser tools judge a frame.
//!
//! Kai cannot tell, and so takes the strictest rule, when: the link does not serve
//! `context.get@1`; it fails; Firefox does not say that the user's tab's window has the focus (a
//! Private Browsing window, another profile's, or the focus elsewhere); the page cannot be read
//! for a reason other than being one of Firefox's own pages, Reader view, the PDF viewer or an
//! error page; or nothing in the page has the focus while one of its frames could not be read.
//! **Not covered yet:** a focus inside Firefox's own controls (the address bar, the search bar,
//! the sidebar) cannot be told from a focus on the page's body (`context.get@1` names neither),
//! so keys there are judged by the page's rule; and the focus can move between this check and
//! the key (a page's script, a key that moves it), which the next key's check sees.

use kai_desktop::caps::windows::Window;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Reach, Shown, cannot_tell, rule_refusal, window_name};
use crate::browser::tabs::or_default;

/// The operation that names the focused element.
pub const GET: &str = "context.get@1";

/// The part of `context.get@1`'s answer this check reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Context {
    tab: Option<Page>,
    window: Option<FocusWindow>,
    /// The focused element: an Element, with `frame` and `origin` in a child frame.
    focus: Option<Value>,
    frames: Option<Frames>,
    page_error: Option<PageError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Page {
    #[serde(deserialize_with = "or_default")]
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FocusWindow {
    focused: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Frames {
    unread: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageError {
    #[serde(deserialize_with = "or_default")]
    code: String,
}

/// The page errors whose page Kai judges by the tab's own address: Firefox's own pages, Reader
/// view, the PDF viewer and error pages hold no other site's frame.
const TAB_DECIDES: &[&str] = &["firefox_page", "reader_view", "pdf_viewer", "error_page"];

impl Shown {
    /// Why Kai's keys may not go to the linked Firefox's window `w` by the frame that has the
    /// keyboard focus in it (module doc); `None` when they may, or when `w` is no linked
    /// Firefox's (its window-level check decides).
    pub async fn focus_refusal(&self, w: &Window) -> Option<String> {
        let reach = Reach::Act;
        let sites = self.sites();
        let link = self.link_of(w)?;
        let name = window_name(w);
        let unknown = |why: &str| {
            cannot_tell(
                &sites,
                reach,
                &format!("the frame with the keyboard focus in {name}"),
                why,
            )
        };
        if !link.ops.iter().any(|o| o == GET) {
            return unknown(&format!(
                "that Kai for Firefox does not serve {GET}, which names the focused frame"
            ));
        }
        let answer = self
            .links
            .op(link.id, GET, json!({}), &CancellationToken::new())
            .await;
        let ctx = match answer {
            Ok(v) => match serde_json::from_value::<Context>(v) {
                Ok(c) => c,
                Err(e) => {
                    return unknown(&format!("{GET} answered something Kai does not read: {e}"));
                }
            },
            Err(e) => {
                return unknown(&format!(
                    "{GET} failed: {}",
                    crate::browser::tools::said(&e)
                ));
            }
        };
        if !ctx.window.as_ref().is_some_and(|x| x.focused) {
            return unknown(
                "Firefox does not say that the window of its user's tab has the focus: a Private \
                 Browsing window, another profile's, or a focus that moved",
            );
        }
        let url = ctx.tab.map(|t| t.url).unwrap_or_default();
        let what = format!("the page with the keyboard focus in {name}");
        if let Some(r) = rule_refusal(sites.page_rule(&url), reach, &what) {
            return Some(r);
        }
        if let Some(e) = &ctx.page_error
            && !TAB_DECIDES.contains(&e.code.as_str())
        {
            return unknown(&format!(
                "Kai for Firefox could not read the page ({})",
                e.code
            ));
        }
        match &ctx.focus {
            Some(Value::Object(el)) => match el.get("origin") {
                // In the top document: the tab's rule, checked above.
                None => None,
                Some(origin) => {
                    let what = format!("the frame with the keyboard focus in {name}");
                    rule_refusal(
                        sites.page_rule(origin.as_str().unwrap_or_default()),
                        reach,
                        &what,
                    )
                }
            },
            _ if ctx.frames.as_ref().is_some_and(|f| f.unread > 0) => unknown(
                "nothing in the page has the focus while one of its frames could not be read, so \
                 the focus may be in that frame",
            ),
            _ => None,
        }
    }
}
