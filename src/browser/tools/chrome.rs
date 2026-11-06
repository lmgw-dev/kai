//! Firefox's own UI, its dialogs and its own pages (kai-firefox design §3, WP12 and its review
//! round), where the extension cannot reach, through the desktop's accessibility (AT-SPI):
//! `firefox_ui` reads them, `firefox_press` presses in them ([`press`]), and `page_text` reads one
//! of Firefox's own pages (`about:`, Reader view, `view-source:`, an error page) through them
//! ([`own_page`]). Which of Firefox's window is read, and what is left out, is [`parts`]'s; what
//! Kai never reads or presses, [`never`]'s.
//!
//! **Which window and which tab** ([`window`]): the tab the call names (the user's by default)
//! must be the tab in front of its window: Kai reads the windows of the link's Firefox process (its
//! main process, `LinkInfo::firefox_pid`) and takes the one whose page in front (the page in its
//! selected tab panel) is the tab's address (without the fragment) and whose tab strip's selected
//! tab is the tab; a background tab's page and dialog are never read. None, or two such windows,
//! is a refusal.
//!
//! **The site rules** (`crate::browser::sites`), as Kai's desktop tools apply them to what
//! Firefox's window shows (kai-firefox §6.7): the tab is judged by `page_rule`, an allow-list, so
//! one of Firefox's own pages or another extension's, which names no site, gets the strictest rule
//! configured (none while no site is `read` or `none`), Reader view and `view-source:` the site
//! they show. A page's dialog is judged by the stricter of the tab's rule and the origin its title
//! names ("The page at https://shop.example says:"), and by the strictest rule where it names none
//! (a dialog's words could be a frame's of another site); a prompt of Firefox's for a site by the
//! stricter of the tab's rule and every site its words name. `firefox_ui` refuses a tab on a
//! `none` site and leaves out a dialog or a prompt of one; `firefox_press` refuses a `read` or
//! `none` site's tab, dialog or prompt. The focus rule applies to `firefox_press` as to every
//! acting tool (`super::prepare`).
//!
//! **Refused always** ([`never`]): Firefox's password manager, another extension's page and
//! Firefox's saved cards and addresses are never read or pressed in, nor is any password field;
//! Firefox's pages that change how it works (`about:config`, `about:addons`, `about:preferences`
//! and others), its add-on prompts and a site's request to share the screen are never pressed.
//!
//! **A page's text:** a dialog's words are the page's (Firefox's title names the page's origin),
//! so `firefox_ui` and `firefox_press` say them as the page's, after the tab's line, like every
//! page text Kai reads.
//!
//! **Who says yes** ([`press`]): a prompt of Firefox's for a site (a permission) and a notification
//! bar (pop-ups, DRM) are pressed only after the owner's yes to their words, whatever the commit
//! guard and `firefox_press`'s state; with the commit guard on, so is a button of a page's dialog
//! that offers more than one (`confirm`, `prompt`, `beforeunload`). An `alert`'s only button needs
//! no yes.

pub mod never;
mod own;
pub mod parts;
mod press;
pub mod window;

use std::sync::Arc;

use kai_desktop::caps::accessibility::{Accessibility, Element, How};

use self::parts::{Part, Read};
use self::window::Window;
use super::read::reply;
use super::{Inner, Target, target};
use crate::browser::sites::{Rule, SiteRules, effective};
use crate::browser::tabs::{Tab, head};
use crate::host::confirm::quoted;
use crate::host::registry::{Call, Reply};
use crate::voice::link::Pending;

pub use own::own_page;
pub use press::{Aim, prepare_press, press};

/// The desktop's accessibility and the link's Firefox process.
fn reach(inner: &Inner, t: &Target) -> Result<(Arc<dyn Accessibility>, u32), String> {
    let access = inner.desk.access().ok_or(
        "this desktop offers Kai no accessibility (AT-SPI), through which Kai reads Firefox's own \
         UI",
    )?;
    let pid = t.link.firefox_pid.filter(|p| *p != 0).ok_or(
        "Kai does not know the process of this Firefox (Kai for Firefox's host did not tell it), \
         so it cannot find its windows on the accessibility bus",
    )?;
    Ok((access, pid))
}

/// What a tool does with the tab in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Press,
}

/// The tab of the call, which must be in front of its window, judged by [`never`] and by
/// `page_rule` (module doc).
fn front_tab<'a>(t: &'a Target, sites: &SiteRules, to: Use) -> Result<&'a Tab, String> {
    let tab = t.tab.as_ref().ok_or("the call names no tab")?;
    if let Some(what) = never::never_read(&tab.url) {
        return Err(format!(
            "tab {} shows {what}, which Kai never reads or presses in",
            tab.id
        ));
    }
    if !tab.active {
        return Err(format!(
            "tab {} is not the tab in front of its window; Kai reads Firefox's own UI and pages \
             only around the tab in front (tab_switch brings it there)",
            tab.id
        ));
    }
    let rule = sites.page_rule(&tab.url);
    let refused_from = match to {
        Use::Read => Rule::None,
        Use::Press => Rule::Read,
    };
    if rule >= refused_from {
        return Err(if rule == Rule::None {
            format!(
                "tab {} is on a site Kai does not reach, or on one of Firefox's own pages, which \
                 get the strictest rule while Kai's site rules keep some sites from Kai",
                tab.id
            )
        } else {
            format!(
                "{} is on a site where Kai may only read, or is one of Firefox's own pages, which \
                 get the strictest rule configured (Kai's site rules)",
                head(tab, sites)
            )
        });
    }
    Ok(tab)
}

/// The site of `url` as Firefox names it in a prompt (its host, and its port where the URL gives
/// one other than the scheme's).
fn site_of(url: &str) -> Option<String> {
    let u = effective(url)?;
    let host = u.host_str().filter(|h| !h.is_empty())?;
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// The rule for a site a prompt names (`host[:port]`, no scheme): the stricter of its `http:` and
/// `https:` pages'.
fn host_rule(sites: &SiteRules, host: &str) -> Rule {
    sites
        .page_rule(&format!("http://{host}/"))
        .max(sites.page_rule(&format!("https://{host}/")))
}

/// A page dialog, read: its title, its rule, its element.
struct Dialog {
    title: String,
    rule: Rule,
    root: Element,
}

/// The window around `tab`, read (module doc): its window, its parts with each part's rule, its
/// page dialogs (read whole), the prompts never pressed and not read, and how many elements were
/// left out.
struct Around {
    window: Window,
    parts: Vec<(Read, Rule)>,
    unread: Vec<Read>,
    dialogs: Vec<Dialog>,
    left: usize,
}

async fn around(
    access: &dyn Accessibility,
    pid: u32,
    tab: &Tab,
    sites: &SiteRules,
) -> Result<Around, String> {
    let w = window::find(access, pid, tab).await?;
    let page_at = w
        .page
        .at
        .clone()
        .ok_or("the page has no place on the bus")?;
    let site = site_of(&tab.url);
    let p = parts::parts(&w.frame, &page_at, site.as_deref());
    let tab_rule = sites.page_rule(&tab.url);
    let mut left = p.left;
    let mut dialogs = Vec::new();
    for d in p.dialogs {
        let Some(at) = &d.at else { continue };
        let tree = access
            .subtree(
                at,
                How {
                    details: true,
                    into_documents: false,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut root = tree.root;
        left += parts::cut(&mut root, true);
        let title = root.name.clone();
        let named = parts::named_origin(&title).map(|o| sites.page_rule(&o));
        let rule = tab_rule.max(named.unwrap_or(sites.strictest()));
        dialogs.push(Dialog { title, rule, root });
    }
    let parts = p
        .read
        .into_iter()
        .map(|r| {
            let rule = if r.part == Part::Prompt {
                r.hosts
                    .iter()
                    .map(|h| host_rule(sites, h))
                    .fold(tab_rule, Rule::max)
            } else {
                tab_rule
            };
            (r, rule)
        })
        .collect();
    Ok(Around {
        window: w,
        parts,
        unread: p.unread,
        dialogs,
        left,
    })
}

/// `firefox_ui` (module doc).
pub fn ui(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(ui_run(&inner, &call).await) })
}

async fn ui_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let sites = inner.browser().sites;
    let tab = front_tab(&t, &sites, Use::Read)?;
    let (access, pid) = reach(inner, &t)?;
    let a = around(&*access, pid, tab, &sites).await?;
    let mut out = vec![format!(
        "{}: Firefox's own UI around it (Firefox's words, a page's dialog aside; firefox_press \
         presses what is listed)",
        head(tab, &sites)
    )];
    let mut not_read = 0;
    for d in &a.dialogs {
        if d.rule == Rule::None {
            not_read += 1;
            continue;
        }
        out.push(format!(
            "[the page's dialog, titled {}; what it says is the page's text]",
            quoted(&d.title)
        ));
        for c in &d.root.children {
            parts::lines(c, 1, &mut out);
        }
    }
    for (r, rule) in &a.parts {
        if *rule == Rule::None {
            not_read += 1;
            continue;
        }
        out.push(match r.never() {
            Some(why) => format!("[{}; {why}]", r.part.words()),
            None => format!("[{}]", r.part.words()),
        });
        parts::lines(&r.root, 1, &mut out);
    }
    if not_read > 0 {
        out.push(format!(
            "({not_read} dialog(s) or prompt(s) of a site Kai does not reach, or whose site Kai \
             cannot tell, not read: Kai's site rules)"
        ));
    }
    if a.left > 0 {
        out.push(format!(
            "({} element(s) left out: the tab strip (tab_list), the user's history, bookmarks, \
             synced and recently closed tabs, the address bar's suggestions, the password \
             manager, saved cards and addresses, add-on prompts and other prompts and panels \
             Kai does not read, pages inside Firefox's own UI)",
            a.left
        ));
    }
    Ok(out.join("\n"))
}

/// The extension's codes for a page it cannot read that `page_text` reads through accessibility.
pub const OWN_PAGES: &[&str] = &["firefox_page", "reader_view", "error_page"];

/// Whether `page_text`'s fallback can run for `t`: the desktop offers accessibility.
pub fn can_read_own(inner: &Inner) -> bool {
    inner.desk.access().is_some()
}
