//! `firefox_press` (the parent's module doc): a control of Firefox's own UI, a page's dialog, a
//! prompt of Firefox's for a site or one of Firefox's own pages, pressed through AT-SPI's
//! `doAction`.
//!
//! - **Before the confirmation** ([`prepare_press`]): the control is found, so the dialog names it
//!   and what it is in ([`Aim`]): a page dialog's title and words, a prompt's words ("Firefox's
//!   prompt for a site says: “Allow example.com to use your camera and microphone?”"), each check
//!   box with its state, the sites the prompt names. A control in a part Kai never presses
//!   (`super::never`) is refused there, before anyone is asked.
//! - **Who says yes:** a prompt for a site, and a notification bar (Firefox's bars carry a site's
//!   pop-ups, DRM and other permissions), is pressed only after the owner's yes to its words: the
//!   call's own confirmation, which shows them, or, where the call's policy let it run unasked, a
//!   question of its own, whatever the commit guard's state. With the commit guard on, a page
//!   dialog's button where it offers more than one gets the same.
//! - **Found again, twice** ([`locate`]): after the confirmation (and before that question), and
//!   once more right before `doAction`: the tab (still on its address, still the tab in front of
//!   its window, by the extension's listing and by the window's selected tab), the window, the
//!   control (shown on the screen, of the role and name the confirmation named, in the same part,
//!   the dialog's or the prompt's words the same). Any difference refuses and says what changed;
//!   the control pressed is the one the last look found.
//! - **The press:** the input turn, the spans (Kai's input is never the user's,
//!   `crate::browser::spans`), the network gate on the tab's page (`gate.open@1` with the tab:
//!   what the page sends after a dialog's OK is held and asked about as after any acting tool),
//!   the last look, the press, the gate read, the spans' end. A page `prompt()`'s text is not
//!   given yet: a headless Firefox 159 answered EditableText's `SetTextContents` on its field with
//!   yes and kept the field as it was (kai-firefox WP12).

use std::sync::Arc;

use kai_desktop::caps::accessibility::{Accessibility, Candidate, Element, How, Match, Query};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::parts::{self, Part, Prompt};
use super::{Around, Use, around, front_tab, never, own, reach, window};
use crate::browser::moves::now_ms;
use crate::browser::sites::{Rule, SiteRules};
use crate::browser::tabs::{Seen, Tab, head};
use crate::browser::tools::guard::{self, Question, Questions, gate_arg};
use crate::browser::tools::read::reply;
use crate::browser::tools::{Inner, Target, answered, failure, look, target};
use crate::host::confirm::{quoted, shown_text};
use crate::host::registry::{Args, Call, Reply};
use crate::voice::link::Pending;

/// The control `firefox_press` presses, as found before the confirmation (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aim {
    part: Part,
    role: String,
    name: String,
    /// A page dialog's title.
    title: Option<String>,
    /// What the dialog or the prompt says, quoted, a check box with its state.
    says: Vec<String>,
    /// The sites a prompt names.
    sites: Vec<String>,
    /// A page dialog's button where it offers more than one: asked with the commit guard on.
    choice: bool,
    /// A prompt for a site or a notification bar (a site's permission, DRM, pop-ups): always
    /// asked.
    always: bool,
}

impl Aim {
    /// The confirmation's lines for it.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "control: {} {} in {}",
            shown_text(&self.role),
            quoted(&self.name),
            self.part.words()
        )];
        if let Some(title) = &self.title {
            out.push(format!("the dialog's title: {}", quoted(title)));
        }
        let speaker = match self.part {
            Part::Dialog => "the page's dialog says",
            Part::Bar => "Firefox's notification bar says",
            _ => "Firefox's prompt for a site says",
        };
        for w in &self.says {
            out.push(format!("{speaker}: {w}"));
        }
        if !self.sites.is_empty() {
            out.push(format!(
                "the sites it names: {}",
                self.sites
                    .iter()
                    .map(|s| shown_text(s).into_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out
    }

    /// What differs in `now` from the aim, in words.
    fn changes(&self, now: &Aim) -> Vec<String> {
        let mut out = Vec::new();
        if now.part != self.part {
            out.push(format!(
                "the control is now in {}, not in {}",
                now.part.words(),
                self.part.words()
            ));
        }
        if now.role != self.role || now.name != self.name {
            out.push(format!(
                "the control is now {} {}",
                shown_text(&now.role),
                quoted(&now.name)
            ));
        }
        if now.title != self.title {
            out.push(format!(
                "the dialog's title is now {}",
                now.title.as_deref().map(quoted).unwrap_or_default()
            ));
        }
        if now.says != self.says {
            out.push(format!(
                "{} now says: {} (the confirmation named: {})",
                now.part.words(),
                now.says.join(" "),
                self.says.join(" ")
            ));
        }
        if now.sites != self.sites {
            out.push(format!(
                "the sites it names are now: {}",
                now.sites.join(", ")
            ));
        }
        out
    }
}

/// A pressable control, where it is.
struct Hit {
    part: Part,
    element: Element,
    /// The dialog it is in (its index in `Around::dialogs`).
    dialog: Option<usize>,
    /// The part it is in (its index in `Around::parts`).
    read: Option<usize>,
    /// Why Kai never presses it (`super::never`).
    never: Option<String>,
}

/// The pressable controls around the tab (and, for one of Firefox's own pages, in it), those Kai
/// never presses marked.
async fn hits(
    access: &dyn Accessibility,
    pid: u32,
    tab: &Tab,
    sites: &SiteRules,
) -> Result<(Around, Vec<Hit>), String> {
    let a = around(access, pid, tab, sites).await?;
    let mut out = Vec::new();
    let mut take = |root: &Element, part: Part, dialog, read, never: Option<String>| {
        let mut found = Vec::new();
        parts::pressables(root, &mut found);
        out.extend(found.into_iter().map(|e| Hit {
            part,
            element: e.clone(),
            dialog,
            read,
            never: never.clone(),
        }));
    };
    for (i, d) in a.dialogs.iter().enumerate() {
        let never = parts::has_password(&d.root).then(|| {
            "a dialog with a password field (a site's sign-in), which Kai never presses".to_owned()
        });
        take(&d.root, Part::Dialog, Some(i), None, never);
    }
    for (i, (r, _)) in a.parts.iter().enumerate() {
        take(&r.root, r.part, None, Some(i), r.never().map(str::to_owned));
    }
    for r in &a.unread {
        take(&r.root, r.part, None, None, r.never().map(str::to_owned));
    }
    if own_page(&tab.url) {
        let at = a
            .window
            .page
            .at
            .clone()
            .ok_or("the page has no place on the bus")?;
        let mut tree = access
            .subtree(
                &at,
                How {
                    details: true,
                    into_documents: true,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        own::prune(sites, &mut tree.root, true);
        let never = never::never_pressed(&tab.url)
            .map(|why| format!("{why}, a page of Firefox's that Kai never presses in"));
        take(&tree.root, Part::OwnPage, None, None, never);
    }
    Ok((a, out))
}

/// One of Firefox's own pages, whose controls `firefox_press` presses (Reader view's included).
fn own_page(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    (u.starts_with("about:") && !u.starts_with("about:blank") && !u.starts_with("about:srcdoc"))
        || u.starts_with("view-source:")
        || u.starts_with("chrome:")
}

fn candidates(hits: &[&Hit]) -> Vec<Candidate> {
    hits.iter()
        .map(|h| Candidate {
            role: h.element.role.clone(),
            name: h.element.name.clone(),
        })
        .collect()
}

/// The one control `query` names among `hits`, as `ui_press` picks (`Query::select`); one Kai
/// never presses is named only where the query names no other, and refused.
fn pick<'a>(hits: &'a [Hit], query: &Query) -> Result<&'a Hit, String> {
    let open: Vec<&Hit> = hits.iter().filter(|h| h.never.is_none()).collect();
    match query.select(&candidates(&open)) {
        Match::One(i) => Ok(open[i]),
        Match::Many(v) => {
            let mut text = format!(
                "{} controls match; name one with its role, as in button \"OK\":",
                v.len()
            );
            for i in v {
                text.push_str(&format!(
                    "\n{} {} in {}",
                    shown_text(&open[i].element.role),
                    quoted(&open[i].element.name),
                    open[i].part.words()
                ));
            }
            Err(text)
        }
        Match::None => {
            let closed: Vec<&Hit> = hits.iter().filter(|h| h.never.is_some()).collect();
            let first = match query.select(&candidates(&closed)) {
                Match::One(i) => Some(closed[i]),
                Match::Many(v) => v.first().map(|i| closed[*i]),
                Match::None => None,
            };
            Err(match first {
                Some(h) => format!(
                    "{} {} is in {}, whatever Kai's site rules, the commit guard and \
                     firefox_press's state say",
                    shown_text(&h.element.role),
                    quoted(&h.element.name),
                    h.never.as_deref().unwrap_or_default()
                ),
                None => "no control of Firefox's own UI around the tab matches (firefox_ui lists \
                         them)"
                    .into(),
            })
        }
    }
}

/// The aim for `hit` (module doc); a dialog or a prompt of a site where Kai may only read, or
/// that it does not reach, is refused.
fn aim_of(hit: &Hit, a: &Around) -> Result<Aim, String> {
    if let Some(why) = &hit.never {
        return Err(format!(
            "{} {} is in {why}",
            shown_text(&hit.element.role),
            quoted(&hit.element.name)
        ));
    }
    let mut aim = Aim {
        part: hit.part,
        role: hit.element.role.clone(),
        name: hit.element.name.clone(),
        title: None,
        says: Vec::new(),
        sites: Vec::new(),
        choice: false,
        always: false,
    };
    if let Some(i) = hit.dialog {
        let d = &a.dialogs[i];
        if d.rule >= Rule::Read {
            return Err(
                "the dialog is a site's where Kai may only read or that Kai does not reach, or one \
                 whose site Kai cannot tell (Kai's site rules)"
                    .into(),
            );
        }
        parts::says(&d.root, &mut aim.says);
        aim.title = Some(d.title.clone());
        aim.choice = parts::buttons(&d.root) > 1;
    }
    if let Some((r, rule)) = hit.read.map(|i| &a.parts[i])
        && r.part == Part::Prompt
    {
        if *rule >= Rule::Read {
            return Err(
                "Firefox's prompt is for a site where Kai may only read or that Kai does not reach \
                 (Kai's site rules)"
                    .into(),
            );
        }
        parts::says(&r.root, &mut aim.says);
        aim.sites = r.hosts.clone();
        aim.always = r.prompt == Some(Prompt::Permission);
    }
    if let Some((r, _)) = hit.read.map(|i| &a.parts[i])
        && r.part == Part::Bar
    {
        if !r.root.name.is_empty() {
            aim.says.push(quoted(&r.root.name));
        }
        parts::says(&r.root, &mut aim.says);
        aim.always = true;
    }
    Ok(aim)
}

/// The aim of `firefox_press`'s call, found before the confirmation (module doc).
pub async fn prepare_press(
    inner: &Inner,
    t: &Target,
    args: &Args,
) -> Result<(Aim, Vec<String>), String> {
    let sites = inner.browser().sites;
    let tab = front_tab(t, &sites, Use::Press)?;
    let (access, pid) = reach(inner, t)?;
    let query = Query::parse(args.text("control").unwrap_or_default())?;
    let (a, hits) = hits(&*access, pid, tab, &sites).await?;
    let aim = aim_of(pick(&hits, &query)?, &a)?;
    let lines = aim.lines();
    Ok((aim, lines))
}

/// The tab and the control as a look finds them now (module doc).
struct Located {
    /// The call's target with the tab as listed now.
    target: Target,
    /// The control's place.
    at: (String, String),
}

/// Finds the tab and the control again in `seen` and on the bus, and compares the control with
/// `aim` (module doc); a refusal says what changed.
async fn locate(
    inner: &Inner,
    t: &Target,
    seen: &[Seen],
    aim: &Aim,
    query: &Query,
) -> Result<Located, String> {
    let sites = inner.browser().sites;
    let was = t.tab.as_ref().ok_or("the call names no tab")?;
    let tab = seen
        .iter()
        .filter(|s| s.link.id == t.link.id)
        .find_map(|s| s.listing.as_ref().ok())
        .and_then(|l| l.tabs.iter().find(|x| x.id == was.id))
        .cloned()
        .ok_or("the tab closed after the confirmation")?;
    if window::bare(&was.url) != window::bare(&tab.url) {
        return Err(format!(
            "tab {} went to another page after the confirmation",
            tab.id
        ));
    }
    let now = Target {
        tab: Some(tab),
        ..t.clone()
    };
    let tab = front_tab(&now, &sites, Use::Press)?;
    let (access, pid) = reach(inner, &now)?;
    let (a, hits) = hits(&*access, pid, tab, &sites).await?;
    let hit = pick(&hits, query)?;
    let found = aim_of(hit, &a)?;
    let changed = aim.changes(&found);
    if !changed.is_empty() {
        return Err(format!(
            "{} {} is no longer what the confirmation named: {}",
            shown_text(&aim.role),
            quoted(&aim.name),
            changed.join("; ")
        ));
    }
    if !hit.element.showing {
        return Err(format!(
            "{} {} is not shown on the screen now",
            shown_text(&aim.role),
            quoted(&aim.name)
        ));
    }
    let at = hit
        .element
        .at
        .clone()
        .ok_or("the control has no place on the bus")?;
    Ok(Located { target: now, at })
}

/// `e` with "nothing was pressed" said.
fn nothing(e: String) -> String {
    if e.ends_with("nothing was pressed") {
        e
    } else {
        format!("{e}; nothing was pressed")
    }
}

/// `firefox_press` (module doc).
pub fn press(inner: Arc<Inner>, call: Call) -> Pending<Reply> {
    Box::pin(async move { reply(press_run(&inner, &call).await) })
}

async fn press_run(inner: &Arc<Inner>, call: &Call) -> Result<String, String> {
    let t = target(call)?;
    let aim = t
        .chrome
        .clone()
        .ok_or("firefox_press was prepared without its control")?;
    let query = Query::parse(call.args.text("control").unwrap_or_default())?;
    // The tab and the control again: the dialog may have waited.
    let seen = look(inner, &call.cancel).await;
    let first = locate(inner, &t, &seen, &aim, &query)
        .await
        .map_err(nothing)?;
    let sites = inner.browser().sites;
    let tab = first.target.tab.clone().ok_or("the call names no tab")?;
    let confirmed = call.prepared.as_ref().is_some_and(|p| p.confirmed);
    let policy = inner.browser();
    let asks = if aim.always {
        Some((
            "firefox_press (a site's permission prompt or a notification bar)",
            "Kai presses a button of Firefox's prompt for a site or of a notification bar: what \
             it grants or changes (a site's permission, DRM, pop-ups, as its words say) follows \
             from it.",
        ))
    } else if aim.choice && policy.commit_guard {
        Some((
            "firefox_press (a choice the page asks for)",
            "Kai presses a button of a page's dialog: what follows is the page's to act on.",
        ))
    } else {
        None
    };
    if let Some((tool, description)) = asks
        && !confirmed
    {
        let mut lines = vec![format!("tab: {}", head(&tab, &sites))];
        lines.extend(aim.lines());
        let q = Question {
            tool: tool.into(),
            description: description.into(),
            lines,
            notes: Vec::new(),
        };
        // Not spoken yet for a call of Kai's own voice: the card shows it (Kai's words for these
        // questions, `crate::browser::announce::words::asked`, do not cover them yet).
        let a = guard::ask_owner_spoken(
            &*inner.confirmer,
            &call.caller,
            q,
            None,
            call.deadline,
            &call.cancel,
        )
        .await;
        if a != guard::Answered::Yes {
            return Err(format!(
                "{} {} was not pressed: {} ({})",
                shown_text(&aim.role),
                quoted(&aim.name),
                a.why(),
                if aim.always {
                    "the question every prompt for a site and every notification bar gets"
                } else {
                    "the commit guard's question"
                }
            ));
        }
    }
    let turn = inner.desk.turn(call).await?;
    let span = inner.spans.begin("firefox_press", &call.cancel).await;
    let result = pressed(inner, call, &first.target, &aim, &query, &policy).await;
    drop(turn);
    span.end(0).await;
    let after = look(inner, &CancellationToken::new()).await;
    inner.moves().raised(&seen, &after, now_ms());
    let requests = result?;
    let mut out = vec![format!(
        "{}: pressed {} {} in {}",
        head(&tab, &sites),
        shown_text(&aim.role),
        quoted(&aim.name),
        aim.part.words()
    )];
    out.extend(requests);
    Ok(out.join("\n"))
}

/// The gate, the last look and the press (module doc); the requests' lines.
async fn pressed(
    inner: &Arc<Inner>,
    call: &Call,
    t: &Target,
    aim: &Aim,
    query: &Query,
    policy: &crate::browser::policy::BrowserPolicy,
) -> Result<Vec<String>, String> {
    let tab = t.tab.as_ref().ok_or("the call names no tab")?;
    let link = t.link.id;
    let serves = t
        .link
        .ops
        .iter()
        .any(|o| o == crate::browser::tools::trusted::GATE_OPEN);
    let mut gate = None;
    let mut watching = None;
    let mut answer_ms = None;
    if serves {
        let g = gate_arg(policy, call.deadline)?;
        answer_ms = g.get("answer_ms").and_then(Value::as_u64);
        let how = if policy.commit_guard {
            Questions::Held
        } else {
            Questions::Allowed
        };
        let (id, w) = guard::watch(inner, call, t, how);
        watching = Some(w);
        let v = inner
            .links
            .op_as(
                id,
                link,
                crate::browser::tools::trusted::GATE_OPEN,
                json!({"tab": tab.id, "guard": g}),
                &CancellationToken::new(),
            )
            .await;
        match v {
            Ok(v) => {
                gate = v.get("gate").and_then(Value::as_u64);
                answered(inner, call, t, v)?;
            }
            Err(e) => return Err(failure(inner, &t.link, e, None, &call.cancel).await),
        }
    }
    let run = async {
        if call.cancel.is_cancelled() {
            return Err("the call was cancelled before the press".to_owned());
        }
        if inner.desk.locked() {
            return Err("the screen locked, and Kai presses nothing while it is".to_owned());
        }
        // The last look, right before the press (module doc).
        let seen = look(inner, &call.cancel).await;
        let last = locate(inner, t, &seen, aim, query).await.map_err(nothing)?;
        let (access, _) = reach(inner, &last.target)?;
        let found = access
            .found_at(&last.at)
            .await
            .map_err(|e| nothing(e.to_string()))?;
        access.press(&found).await.map_err(|e| e.to_string())
    }
    .await;
    if let Some(w) = &watching {
        w.release();
    }
    let mut lines = Vec::new();
    if let Some(g) = gate {
        let r = inner
            .links
            .op(
                link,
                crate::browser::tools::trusted::GATE_RESULT,
                json!({"gate": g}),
                &CancellationToken::new(),
            )
            .await;
        match r {
            Ok(mut v) => {
                crate::browser::tools::page::withhold(&mut v, &policy.sites);
                inner.renumber(&t.link, &mut v);
                let held: Vec<crate::browser::tools::act::Request> = v
                    .get("requests")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
                    .unwrap_or_default();
                lines.extend(held.iter().map(|r| {
                    crate::browser::tools::act::request_line(r, &policy.sites, answer_ms)
                }));
                if v.get("expired").and_then(Value::as_bool) == Some(true) {
                    lines.push(
                        "the commit guard's watch had ended before Kai read it, so the page's \
                         requests after the press went unasked"
                            .into(),
                    );
                }
            }
            Err(e) => {
                let words = failure(inner, &t.link, e, None, &CancellationToken::new()).await;
                lines.push(format!(
                    "Kai could not read what became of the page's requests after the press: \
                     {words}"
                ));
            }
        }
    } else if serves {
        lines.push(
            "the page's requests after the press were not watched: Kai for Firefox cannot reach \
             this page"
                .into(),
        );
    }
    drop(watching);
    run.map(|_| lines)
}
