//! The browser tools' calls (kai-firefox design §5, §6.4): which operations each tool needs
//! ([`route`]), what is checked before every call ([`prepare`]: the tab or the element the call
//! names, the site rules, the focus rule, the commit guard's question about a navigation), the
//! handlers ([`tabs`], [`context`], the reading tools of [`read`] and [`text`], a PDF's text in
//! [`pdf`], the screenshots in [`see`], the acting tools of [`act`] with the commit guard's
//! questions in [`guard`] and the trusted path in [`trusted`], Firefox's features in
//! [`features`], the source and the DOM in [`source`], the style sheets in [`style`], the scripts
//! in [`script`], their shared words in [`lines`], and what they share with the preparation of a
//! ref in [`page`]) and the words of the extension's errors ([`said`]).
//!
//! **Built and not yet.** A tool is listed only while a linked extension serves every operation it
//! needs (`super::provider`, D3). The tools whose operations the contract has today are built here:
//! the tab tools, `browser_context`, the reading tools of §5.3 with the screenshots ([`see`]), the
//! acting tools of §5.4, the hints and `element_point`, Firefox's own features of §5.5
//! ([`features`]), the source and the DOM ([`source`]), the style sheets ([`style`]) and the
//! scripts ([`script`]), and reading aloud ([`aloud`], §7.3, WP9). A tool the contract names and no
//! work package has built yet would be [`Route::NotYet`]: never listed, so never called.
//!
//! **Before every call** of a tool that reaches a page ([`prepared`]: one that acts, or names a
//! tab, a URL or a ref), Kai lists the tabs and resolves the tab (`super::tabs`), or resolves the
//! ref to the link that gave it out (`super::refs`) and asks that extension what it names now
//! (`element.describe@1`: its tab, role and name, or that the ref is stale); the dialog names the
//! tab (and the element), and the handler reads or acts on exactly that. When the owner was asked,
//! the handler of a tab tool lists the tabs again first and refuses a tab that closed or went to
//! another site while the dialog waited.
//! - **site rules** (`super::sites`): no tool reaches a tab on a `none` site, a ref's tab checked
//!   like a named one and a ref's frame by its own origin; a tool that acts on the page is refused
//!   on a `read` one. `tab_open` and `tab_go` refuse a `none` site's URL too. `tab_list` and
//!   `browser_context` leave a `none` site's tabs out of what they say. A reading tool's answer
//!   whose page went to a `none` site while it ran, or that names no URL, is not said, and of a
//!   frame of a `none` site in it nothing but its place is ([`page::withhold`]). A failure that
//!   concerns a tab on a `none` site says the tab's id and what failed, never the extension's
//!   message ([`failure`]);
//! - **the focus rule** (`super::moves`): a tool that acts on the page ([`Reach::Act`], on a tab
//!   or on a ref's tab) is confirmed on a tab that is not in front of the user as the rule means
//!   it, even where K15's policy would run it unasked (`Prepared::ask`), unless
//!   `browser.act_in_background`. With two profiles each reporting a tab in front, the focused
//!   window decides (`super::tabs::front`); where that cannot be told, or a profile's tabs could not
//!   be listed, Kai asks. A call that names its tab and that a trusted approver approved in lmgw
//!   (`Origin::approval_counted`) needs no second question: such a call must name its tab, so the
//!   approver saw where it acts (as K15 asks of window actions; so must every call of a tool that
//!   does more than read, the scripts among them, kai-firefox §5.8). A call that names a ref only is
//!   asked all the same (its approver saw a number, not the tab: kai-firefox WP6's review), and
//!   so is the trusted press's real key for any call approved in lmgw (`trusted::step`). An
//!   approval that does not count spares no question; the call must still name its tab;
//! - **the commit guard on navigations** (§6.4, [`guard::navigation`]): `tab_open` and `tab_go` to
//!   a URL whose site the user's history in that Firefox has never seen are confirmed, as the
//!   focus rule's question is, since a URL the model wrote can carry data out as it loads; a site
//!   in the history only through Kai's own navigations counts as unseen (the extension's
//!   `seen_by_user`), and so does one the extension does not say it of. A search goes to the
//!   user's own search engine and is not asked about. A counted approval spares the question (the
//!   approver saw the URL). `page_css` is asked the same way about each site other than the page's
//!   that its sheet fetches from, whatever its state and also when approved in lmgw ([`style`]).

pub mod act;
pub mod aloud;
pub mod chrome;
pub mod context;
pub mod features;
pub mod guard;
pub mod lines;
pub mod page;
pub mod pdf;
pub mod read;
pub mod script;
pub mod see;
pub mod source;
pub mod style;
pub mod tabs;
pub mod text;
pub mod trusted;

pub use page::{Answer, answered, ask_page, checked, send};

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use kai_desktop::contract::ToolSpec;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::links::{About, LinkId, LinkInfo, Links, OpError};
use super::moves::{Away, Moves};
use super::refs::Refs;
use super::sites::Rule;
use super::tabs::{Front, LIST, Listing, Seen, Tab};
use crate::browser::announce::words::asked::{self as asked_words, About as Asks, Question};
use crate::host::confirm::{Confirmer, quoted, shown_text};
use crate::host::policy::SharedPolicy;
use crate::host::registry::{Args, Call, Origin, Prepared, Preparer, Reply};
use crate::voice::link::Pending;
use crate::voice::speak::Words;

/// What the handlers share.
pub struct Inner {
    pub links: Links,
    /// The host's policy, live: the browser's part is read at each call.
    pub policy: SharedPolicy,
    /// Kai for Firefox's input spans, and the focus rule's marks (`super::moves`), shared with the
    /// desktop's input tools.
    pub spans: super::spans::Spans,
    /// Kai's numbers for the extensions' refs (`super::refs`).
    pub refs: Mutex<Refs>,
    /// The route table ([`route`]; the tests give their own).
    pub routes: fn(&str) -> Route,
    /// Kai's confirmation (K15's dialog) for the questions a call asks while it runs: the commit
    /// guard's ([`guard`]) and the trusted key's ([`trusted`]).
    pub confirmer: Arc<dyn Confirmer>,
    /// The desktop's part of the trusted path ([`trusted::Desk`]).
    pub desk: Arc<dyn trusted::Desk>,
    /// The operations whose network gate may ask now, by link and operation id ([`guard`]).
    pub gates: Mutex<HashMap<(LinkId, u64), Arc<guard::Watch>>>,
    /// The network gate's questions are followed ([`guard::follow`]).
    pub following: AtomicBool,
    /// The style sheets `page_css` added, by link and the extension's id, with their tab
    /// ([`style`]): which Firefox `page_css_remove` goes to, and the tab its site rules check.
    pub sheets: Mutex<HashMap<(LinkId, i64), i64>>,
    /// Where Kai's speech without a model goes (`crate::voice::speak`: the core's).
    pub mouth: Arc<dyn crate::voice::speak::Mouth>,
    /// The events and their announcements (`super::announce`).
    pub announcer: Arc<super::announce::Announcer>,
    /// The reading aloud now, or the last one, ended ([`aloud`]).
    pub reading: Mutex<Option<Arc<aloud::Reading>>>,
    /// K26's job table, where Kai's voice says whether its continuation for jobs' results runs
    /// ([`owners_move`]).
    pub jobs: crate::jobs::Jobs,
}

impl Inner {
    pub fn browser(&self) -> crate::browser::policy::BrowserPolicy {
        self.policy
            .read()
            .expect("never poisoned")
            .browser()
            .clone()
    }

    pub(super) fn moves(&self) -> std::sync::MutexGuard<'_, Moves> {
        self.spans.moves()
    }

    pub(super) fn refs(&self) -> std::sync::MutexGuard<'_, Refs> {
        self.refs.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn gates(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<(LinkId, u64), Arc<guard::Watch>>> {
        self.gates.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn sheets(&self) -> std::sync::MutexGuard<'_, HashMap<(LinkId, i64), i64>> {
        self.sheets.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn reading(&self) -> std::sync::MutexGuard<'_, Option<Arc<aloud::Reading>>> {
        self.reading.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every ref of `answer` (from `link`) as Kai's number (`super::refs`).
    pub fn renumber(&self, link: &LinkInfo, answer: &mut Value) {
        let live = self.links.list();
        self.refs().renumber(&live, link, answer);
    }
}

/// A tool's handler.
pub type Run = fn(Arc<Inner>, Call) -> Pending<Reply>;

/// How a tool reaches pages, for the site rules and the focus rule (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Reads: refused on a `none` site only.
    Read,
    /// Acts on the page in its tab (a navigation, a reload, closing it): refused on `read` and
    /// `none` sites; the focus rule applies.
    Act,
    /// Changes the tab without changing its page (`tab_switch` brings it to the user, `tab_mute`
    /// silences it): refused on `none` sites only.
    Tab,
    /// Shows the user something on the page without changing the page (kai-firefox WP5b's draft
    /// decision): the hints and `element_point`'s ring, which page CSS and scripts cannot reach,
    /// and `element_show`'s scroll. Allowed on `read` sites, outside the focus rule, refused on
    /// `none` sites.
    View,
    /// Opens a new tab (`tab_open`): its URL is checked.
    Open,
}

/// One tool's route (module doc).
#[derive(Clone, Copy)]
pub enum Route {
    Built {
        /// The operations it needs, with their versions: all served by one linked extension.
        ops: &'static [&'static str],
        reach: Reach,
        run: Run,
    },
    /// Its work package has not built it yet: never listed.
    NotYet,
}

/// The route of each tool of `contract::browser` (module doc).
pub fn route(name: &str) -> Route {
    let built = |ops, reach, run| Route::Built { ops, reach, run };
    match name {
        "browser_context" => built(&[context::GET], Reach::Read, context::run),
        "tab_list" => built(&[LIST], Reach::Read, tabs::list),
        "tab_open" => built(
            &[LIST, "tabs.open@1", guard::HISTORY_SEEN],
            Reach::Open,
            tabs::open,
        ),
        "tab_go" => built(
            &[LIST, "tabs.go@1", guard::HISTORY_SEEN],
            Reach::Act,
            tabs::go,
        ),
        "tab_switch" => built(&[LIST, "tabs.switch@1"], Reach::Tab, tabs::switch),
        "tab_close" => built(&[LIST, "tabs.close@1"], Reach::Act, tabs::close),
        "tab_back" => built(&[LIST, "tabs.back@1"], Reach::Act, tabs::back),
        "tab_forward" => built(&[LIST, "tabs.forward@1"], Reach::Act, tabs::forward),
        "tab_reload" => built(&[LIST, "tabs.reload@1"], Reach::Act, tabs::reload),
        "tab_mute" => built(&[LIST, "tabs.mute@1"], Reach::Tab, tabs::mute),
        "page_outline" => built(&[LIST, read::OUTLINE], Reach::Read, read::outline),
        "page_controls" => built(&[LIST, read::CONTROLS], Reach::Read, read::controls),
        "page_links" => built(&[LIST, read::LINKS], Reach::Read, read::links),
        "page_fields" => built(&[LIST, read::FIELDS], Reach::Read, read::fields),
        "page_text" => built(&[LIST, text::TEXT], Reach::Read, text::page_text),
        "page_text_all" => built(&[LIST, text::TEXT_ALL], Reach::Read, text::all),
        "page_text_visible" => built(&[LIST, text::TEXT_VISIBLE], Reach::Read, text::visible),
        "element_text" => built(&[DESCRIBE, text::ELEMENT], Reach::Read, text::element_text),
        "table_read" => built(&[DESCRIBE, text::TABLE], Reach::Read, text::table),
        "page_selection" => built(&[LIST, read::SELECTION], Reach::Read, read::selection),
        "page_find" => built(&[LIST, read::FIND], Reach::Read, read::find),
        "hints_show" => built(&[LIST, read::HINTS_SHOW], Reach::View, read::hints_show),
        "hints_hide" => built(&[LIST, read::HINTS_HIDE], Reach::View, read::hints_hide),
        "element_point" => built(&[DESCRIBE, read::POINT], Reach::View, read::point),
        // §5.4: what acts on an element lists the tabs for the focus rule on its tab.
        "element_press" => built(
            &[
                DESCRIBE,
                LIST,
                act::PRESS,
                trusted::FOCUS_CHECK,
                trusted::GATE_RESULT,
            ],
            Reach::Act,
            act::press,
        ),
        "element_fill" => built(&[DESCRIBE, LIST, act::FILL], Reach::Act, act::fill),
        "element_choose" => built(&[DESCRIBE, LIST, act::CHOOSE], Reach::Act, act::choose),
        "element_check" => built(&[DESCRIBE, LIST, act::CHECK], Reach::Act, act::check),
        "element_file" => built(&[DESCRIBE, LIST, act::FILE], Reach::Act, act::file),
        "element_focus" => built(&[DESCRIBE, LIST, act::FOCUS], Reach::Act, act::focus),
        "element_hover" => built(&[DESCRIBE, LIST, act::HOVER], Reach::Act, act::hover),
        "element_drag" => built(&[DESCRIBE, LIST, act::DRAG], Reach::Act, act::drag),
        "element_scroll" => built(&[DESCRIBE, LIST, act::SCROLL], Reach::Act, act::scroll),
        // A view tool, like element_point: it scrolls the page to show the user an element (the
        // design decision), so a `read` site allows it and the focus rule does not apply.
        "element_show" => built(&[DESCRIBE, act::SHOW], Reach::View, act::show),
        "page_scroll" => built(&[LIST, act::PAGE_SCROLL], Reach::Act, act::page_scroll),
        "page_key" => built(&[LIST, act::KEY], Reach::Act, act::page_key),
        // §5.3's screenshots: images, so K25's setting of who looks at them applies (the
        // provider is listed through `crate::delegate::screenshots::HostView`).
        "page_see" => built(&[LIST, see::PAGE], Reach::Read, see::page_see),
        "element_see" => built(&[DESCRIBE, see::ELEMENT], Reach::Read, see::element_see),
        // §5.5: the zoom changes how the user sees the page, not the page (a view tool); Reader
        // view loads another page in the tab, as a reload does. Firefox keeps the zoom per site by
        // default, so with page_zoom set to allow, a zoom of any tab (outside the focus rule, as a
        // view tool) also zooms the user's tab in front of them where it shows the same site.
        "page_zoom" => built(&[LIST, features::ZOOM], Reach::View, features::zoom),
        "reader_view" => built(&[LIST, features::READER], Reach::Act, features::reader),
        "bookmarks_find" => built(&[features::BOOKMARKS], Reach::Read, features::bookmarks),
        "history_find" => built(&[features::HISTORY], Reach::Read, features::history),
        "downloads_list" => built(&[features::DOWNLOADS], Reach::Read, features::downloads),
        // §5.6
        "page_source" => built(&[LIST, source::SOURCE], Reach::Read, source::page_source),
        "page_dom" => built(&[LIST, source::DOM], Reach::Read, source::page_dom),
        "element_dom" => built(
            &[DESCRIBE, source::ELEMENT],
            Reach::Read,
            source::element_dom,
        ),
        // §5.7: a sheet changes what the page shows; removing Kai's own takes that back, so it
        // is a view tool. What a sheet fetches is read by the extension's own parser first
        // (`page.css_check@1`), and a fetch from a site the user never visited is asked about (the
        // commit guard).
        "page_css" => built(
            &[LIST, style::CHECK, style::ADD, guard::HISTORY_SEEN],
            Reach::Act,
            style::add,
        ),
        "page_css_remove" => built(&[LIST, style::REMOVE], Reach::View, style::remove),
        // §5.8: confirmed by default (the contract's `Ask::Always`), acting on the page.
        "page_script" => built(&[LIST, script::ISOLATED], Reach::Act, script::isolated),
        "page_script_main" => built(&[LIST, script::MAIN], Reach::Act, script::main),
        // §7.3: the mark scrolls the page to show the user the paragraph, as element_show does,
        // so reading aloud is a view tool (a `read` site allows it, the focus rule does not
        // apply). Pause, resume and skip name no page: the reading's tab was checked when it
        // began, and each paragraph is checked as it is read.
        "read_aloud" => built(
            &[
                LIST,
                aloud::OPEN,
                aloud::PARAGRAPH,
                aloud::MARK,
                aloud::CLOSE,
            ],
            Reach::View,
            aloud::aloud,
        ),
        "read_aloud_from" => built(
            &[
                DESCRIBE,
                aloud::OPEN,
                aloud::PARAGRAPH,
                aloud::MARK,
                aloud::CLOSE,
            ],
            Reach::View,
            aloud::aloud_from,
        ),
        "read_pause" => built(&[aloud::OPEN], Reach::Read, aloud::pause),
        "read_resume" => built(&[aloud::OPEN, aloud::PARAGRAPH], Reach::Read, aloud::resume),
        "read_skip" => built(&[aloud::OPEN, aloud::PARAGRAPH], Reach::Read, aloud::skip),
        // kai-firefox §3, WP12: Firefox's own UI through the desktop's accessibility; listed only
        // where the desktop offers it (`super::provider`). The press opens the network gate on
        // the tab's page without a focus check.
        "firefox_ui" => built(&[LIST], Reach::Read, chrome::ui),
        "firefox_press" => built(
            &[LIST, trusted::GATE_OPEN, trusted::GATE_RESULT],
            Reach::Act,
            chrome::press,
        ),
        _ => Route::NotYet,
    }
}

/// What a ref names now (the contract's `element.describe@1`): every tool that takes a ref asks it
/// in its preparation.
pub const DESCRIBE: &str = "element.describe@1";

/// Whether a tool is prepared before its policy's question (module doc): one that does more than
/// read, or that reaches a page through a tab, a URL or a ref. Only a reading tool that names no
/// page (`tab_list`, `browser_context`, which leave `none` sites out of what they say) is not.
pub fn prepared(spec: &ToolSpec, reach: Reach) -> bool {
    reach != Reach::Read
        || ["tab", "url", "ref"]
            .iter()
            .any(|p| spec.param(p).is_some())
}

/// Where `tab_open` and `tab_go` go: Kai decides whether the text is a URL (design §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Url(String),
    Search(String),
}

impl Destination {
    /// A URL with a scheme Firefox opens, or a host with an optional port and path (`https://` is
    /// added, `http://` for `localhost` and an IP address). A host is a name whose suffix is on the
    /// public suffix list, as Firefox's URL fixup decides it (so `node.js` and `index.html` are
    /// searches), one that ends in a dot, or one under the special-use names no list has
    /// ([`SPECIAL_USE`]). Anything else (text with a space, one word, a question) is a search.
    pub fn of(text: &str) -> Self {
        const SCHEMES: &[&str] = &[
            "http",
            "https",
            "file",
            "about",
            "data",
            "view-source",
            "moz-extension",
            "ftp",
            "blob",
            "mailto",
            "tel",
            "javascript",
        ];
        let t = text.trim();
        if t.is_empty() || t.chars().any(char::is_whitespace) {
            return Self::Search(t.to_owned());
        }
        if let Ok(u) = url::Url::parse(t)
            && SCHEMES.contains(&u.scheme())
        {
            return Self::Url(t.to_owned());
        }
        let host_part = t.split(['/', '?', '#']).next().unwrap_or_default();
        let host = match host_part.rsplit_once(':') {
            Some((h, port)) if !h.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
            _ => host_part,
        };
        // An address written out in full; `1.2`, which the URL parser reads as 1.0.0.2, is not.
        let ip = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok();
        let Ok(parsed) = url::Url::parse(&format!("http://{t}")) else {
            return Self::Search(t.to_owned());
        };
        let name = match parsed.host() {
            Some(url::Host::Domain(d)) => Some(d),
            _ => None,
        };
        let local = ip
            || name.is_some_and(|d| {
                let d = d.strip_suffix('.').unwrap_or(d);
                d == "localhost" || d.ends_with(".localhost")
            });
        let scheme = match (local, name) {
            (true, _) => "http",
            (false, Some(d)) if known_host(d) => "https",
            _ => return Self::Search(t.to_owned()),
        };
        match url::Url::parse(&format!("{scheme}://{t}")) {
            Ok(u) => Self::Url(u.to_string()),
            Err(_) => Self::Search(t.to_owned()),
        }
    }

    /// The operation's argument: `url` or `search`.
    pub fn arg(&self) -> (&'static str, &str) {
        match self {
            Self::Url(u) => ("url", u),
            Self::Search(s) => ("search", s),
        }
    }
}

/// The special-use names (RFC 6761, 6762, and ICANN's `internal`) that are hosts though no
/// public suffix list has them: `shop.example`, `printer.local`.
pub const SPECIAL_USE: &[&str] = &[
    "example",
    "test",
    "invalid",
    "localhost",
    "local",
    "internal",
];

/// `host` (as the URL parser has it: lowercase, punycode) is a name Firefox's fixup takes for a
/// host ([`Destination::of`]).
fn known_host(host: &str) -> bool {
    if host.ends_with('.') {
        return host.len() > 1;
    }
    let Some((_, last)) = host.rsplit_once('.') else {
        return false;
    };
    !host.starts_with('.')
        && (SPECIAL_USE.contains(&last)
            || psl::suffix(host.as_bytes()).is_some_and(|s| s.is_known()))
}

/// What a call was prepared for: the link it goes to and the tab it acts on (`None` for
/// `tab_open`), as the dialog showed them.
#[derive(Debug, Clone)]
pub struct Target {
    pub link: LinkInfo,
    pub tab: Option<Tab>,
    pub destination: Option<Destination>,
    /// `tab_open`: the profile's tab ids when the call was prepared, so a call that ends without
    /// naming its new tab still leaves it marked as Kai's (`super::moves`).
    pub before: HashSet<i64>,
    /// A tool that takes a ref: the element, in `tab`.
    pub element: Option<Aimed>,
    /// The element as the dialog showed it (`52 button "Buy now"`), for the questions the call
    /// asks while it runs.
    pub shown: Option<String>,
    /// `element_drag`: the element it drops on, and how the dialog showed it.
    pub to: Option<(Aimed, String)>,
    /// `element_file`: the file, opened as the dialog named it (`act::pick`).
    pub file: Option<Arc<act::Picked>>,
    /// The call carries an approval of lmgw's (`Origin::approved_in_lmgw`): its approver saw the
    /// arguments, a ref, not the element; the trusted press's key is asked at Kai's desk then
    /// (`trusted::step`).
    pub approved: bool,
    /// `page_css`: the origins its sheet fetches from, as the dialog named them (`style::fetches`),
    /// the only ones the call lets Firefox fetch from.
    pub fetch_origins: Vec<String>,
    /// `firefox_press`: the control of Firefox's own UI as the dialog named it ([`chrome`]).
    pub chrome: Option<Arc<chrome::Aim>>,
}

/// An element a call names: Kai's number for it and the extension's (`super::refs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aimed {
    pub kai: i64,
    pub ext: i64,
}

/// The target a call was prepared for.
pub fn target(call: &Call) -> Result<Arc<Target>, String> {
    call.prepared
        .as_ref()
        .and_then(|p| p.state.clone().downcast::<Target>().ok())
        .ok_or_else(|| {
            // Every browser tool that acts or names a page has a preparer (`prepared`), which the
            // host runs first.
            format!(
                "{} reached its handler without its preparation",
                call.spec.name
            )
        })
}

/// Every linked extension's tabs (each that serves `tabs.list@1`), side by side; the looks feed
/// the focus rule's marks, and a window's focus Kai's own input gave it is taken out of them
/// (`super::moves::Moves::discount`).
pub async fn look(inner: &Inner, cancel: &CancellationToken) -> Vec<Seen> {
    let linked = inner.links.list();
    let mark = inner.refs().mark();
    let calls = linked
        .iter()
        .filter(|l| l.ops.iter().any(|o| o == LIST))
        .map(|l| async move {
            let listing = inner
                .links
                .op(l.id, LIST, serde_json::json!({}), cancel)
                .await
                .and_then(|v| {
                    serde_json::from_value::<Listing>(v).map_err(|e| {
                        OpError::failed(
                            "bad_answer",
                            format!("{LIST} answered something Kai does not read: {e}"),
                        )
                    })
                });
            Seen {
                link: l.clone(),
                listing,
            }
        });
    let mut seen = futures_util::future::join_all(calls).await;
    let mut moves = inner.moves();
    for s in &mut seen {
        if let Ok(l) = &mut s.listing {
            moves.look(&s.link, l);
            // The windows' focus as the focus rule reads it (`super::moves`).
            moves.discount(&s.link.profile, l);
        }
    }
    drop(moves);
    // The refs of a tab that closed are retired (`super::refs`).
    let mut refs = inner.refs();
    for s in &seen {
        if let Ok(l) = &s.listing {
            refs.closed(s.link.id, &l.tabs.iter().map(|t| t.id).collect(), mark);
        }
    }
    drop(refs);
    seen
}

/// The preparation of a tool that names a tab or a URL (module doc).
pub fn preparer(inner: Arc<Inner>, spec: &'static ToolSpec, reach: Reach) -> Preparer {
    Arc::new(move |args: &Args, origin: Origin| {
        let inner = inner.clone();
        let args = args.clone();
        Box::pin(async move { prepare(&inner, spec, reach, &args, origin).await })
    })
}

async fn prepare(
    inner: &Inner,
    spec: &ToolSpec,
    reach: Reach,
    args: &Args,
    origin: Origin,
) -> Result<Prepared, String> {
    let policy = inner.browser();
    let tool = spec.name;
    if spec.param("ref").is_some() {
        return page::prepare_element(inner, spec, reach, args, origin).await;
    }
    if tool == "page_css_remove" {
        return style::prepare_remove(inner, spec, args, origin).await;
    }
    let destination = args.text("url").map(Destination::of);
    if let Some(Destination::Url(u)) = &destination
        && policy.sites.rule(u) == Rule::None
    {
        return Err(format!(
            "Kai does not go to {}: Kai's site rules keep it away from that site",
            shown_text(u)
        ));
    }
    let cancel = CancellationToken::new();
    if reach == Reach::Open {
        let seen = look(inner, &cancel).await;
        let at = open_link(&seen)?;
        // Its tabs before, so the new one is known as Kai's however the call ends.
        let before = match &at.listing {
            Ok(l) => l.tabs.iter().map(|t| t.id).collect(),
            Err(e) => {
                return Err(format!(
                    "Kai could not list the tabs of profile {} ({}), so {tool} did not run",
                    shown_text(&at.link.profile),
                    said(e)
                ));
            }
        };
        let asked = match &destination {
            Some(d) if !origin.approval_counted => {
                guard::navigation(inner, at, d, &policy, &cancel).await
            }
            _ => None,
        };
        let several = seen.len() > 1;
        let host = match &destination {
            Some(Destination::Url(u)) => host_of(u),
            _ => String::new(),
        };
        let spoken = Question {
            tool,
            about: Asks::Goes { host: &host },
            why: asked.iter().map(|_| asked_words::unseen_site()).collect(),
            file: None,
        }
        .spoken(policy.announce_max_chars);
        let mut lines = Vec::new();
        if several {
            lines.push(format!(
                "opens in: profile {}",
                shown_text(&at.link.profile)
            ));
        }
        if args.boolean("sandbox") == Some(true) {
            lines.push("in Kai's own container, without the user's cookies".to_owned());
        }
        return Ok(Prepared {
            lines,
            state: Arc::new(Target {
                link: at.link.clone(),
                tab: None,
                destination,
                before,
                element: None,
                shown: None,
                to: None,
                file: None,
                approved: origin.approved_in_lmgw,
                fetch_origins: Vec::new(),
                chrome: None,
            }),
            notes: asked.iter().map(|a| a.note.clone()).collect(),
            options: Vec::new(),
            ask: asked.map(|a| a.why),
            spoken: Some(spoken),
            confirmed: false,
        });
    }
    let query = args.text("tab");
    if reach != Reach::Read && origin.approved_in_lmgw && super::tabs::implicit(query) {
        return Err(format!(
            "this call of {tool} was approved in lmgw before it ran, and the user's tab now may not \
             be the one it was approved for; name the tab (tab_list lists them)"
        ));
    }
    let seen = look(inner, &cancel).await;
    let (link, tab) = super::tabs::resolve(&seen, query, &policy.sites)?;
    if tab.url.trim().is_empty() {
        return Err(format!(
            "Kai for Firefox listed tab {} without its URL, so Kai cannot tell its site (Kai's \
             site rules); {tool} did not run",
            tab.id
        ));
    }
    match (policy.sites.rule(&tab.url), reach) {
        (Rule::None, _) => {
            return Err(format!(
                "tab {} is on a site Kai does not reach (Kai's site rules); {tool} did not run",
                tab.id
            ));
        }
        (Rule::Read, Reach::Act) => {
            return Err(format!(
                "{} is on a site where Kai may only read (Kai's site rules); {tool} did not run",
                super::tabs::head(tab, &policy.sites)
            ));
        }
        _ => {}
    }
    let away = (reach == Reach::Act && !origin.approval_counted && !policy.act_in_background)
        .then(|| focus(inner, &seen, link, tab).err())
        .flatten();
    let at = seen
        .iter()
        .find(|s| s.link.id == link.id)
        .expect("resolved among them");
    let mut asked: Vec<guard::Asked> = match &destination {
        Some(d) if !origin.approval_counted => guard::navigation(inner, at, d, &policy, &cancel)
            .await
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    let mut lines = vec![format!(
        "{}: {}",
        verb_of(tool, reach),
        super::tabs::line(tab, link, seen.len() > 1, &policy.sites)
    )];
    let mut fetch_origins = Vec::new();
    if tool == "page_css" {
        let css = args.text("css").unwrap_or_default();
        // Asked also for a call a trusted approver approved in lmgw: its approver saw the sheet's
        // text, not which of its sites the user has never been to.
        let fetched = style::fetches(inner, at, tab, css, &policy, &cancel).await?;
        lines.extend(fetched.lines);
        asked.extend(fetched.asked);
        fetch_origins = fetched.origins;
    }
    let mut chrome = None;
    if tool == "firefox_press" {
        // The control is found before the confirmation, so the dialog names it (`chrome`).
        let partial = Target {
            link: link.clone(),
            tab: Some(tab.clone()),
            destination: None,
            before: HashSet::new(),
            element: None,
            shown: None,
            to: None,
            file: None,
            approved: origin.approved_in_lmgw,
            fetch_origins: Vec::new(),
            chrome: None,
        };
        let (aim, more) = chrome::prepare_press(inner, &partial, args).await?;
        lines.extend(more);
        chrome = Some(Arc::new(aim));
    }
    let mut notes: Vec<String> = away.map(|a| a.note().to_owned()).into_iter().collect();
    notes.extend(asked.iter().map(|a| a.note.clone()));
    let ask = away.map(|a| a.why()).or(asked.first().map(|a| a.why));
    // The question in Kai's words, for a call of Kai's own (`crate::approvals::asked`).
    let mut why: Vec<Words> = away.map(asked_words::away).into_iter().collect();
    if !asked.is_empty() {
        why.push(if tool == "page_css" {
            Words::new(
                "Its style sheet fetches from a site you have never visited.",
                "Sein Stylesheet lädt von einer Seite, die du noch nie besucht hast.",
            )
        } else {
            asked_words::unseen_site()
        });
    }
    let host = match &destination {
        Some(Destination::Url(u)) => host_of(u),
        _ => String::new(),
    };
    let about = if host.is_empty() {
        Asks::Tab { title: &tab.title }
    } else {
        Asks::Goes { host: &host }
    };
    let spoken = Question {
        tool,
        about,
        why,
        file: None,
    }
    .spoken(policy.announce_max_chars);
    Ok(Prepared {
        lines,
        state: Arc::new(Target {
            link: link.clone(),
            tab: Some(tab.clone()),
            destination,
            before: HashSet::new(),
            element: None,
            shown: None,
            to: None,
            file: None,
            approved: origin.approved_in_lmgw,
            fetch_origins,
            chrome,
        }),
        notes,
        options: Vec::new(),
        ask,
        spoken: Some(spoken),
        confirmed: false,
    })
}

/// The site a URL goes to, as Kai's question names it: its host (and port), else the URL.
pub(super) fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| lines::site(Some(&u.origin().ascii_serialization())))
        .unwrap_or_else(|| url.to_owned())
}

/// The dialog's words for what `tool` does to its tab: [`verb`]'s, but where the tool says more.
fn verb_of(tool: &str, reach: Reach) -> &'static str {
    match tool {
        "page_zoom" => "zooms",
        "page_css" => "adds a style sheet to",
        "page_script" | "page_script_main" => "runs the script in",
        "firefox_ui" => "reads Firefox's own UI around",
        "firefox_press" => "presses in Firefox's own UI around",
        _ => verb(reach),
    }
}

/// The dialog's word for what a call does to its tab.
fn verb(reach: Reach) -> &'static str {
    match reach {
        Reach::Read => "reads",
        Reach::View => "shows on",
        Reach::Act | Reach::Tab | Reach::Open => "acts on",
    }
}

/// The focus rule's verdict on `tab` of `link` (`super::moves`), and Kai's doubt: with more than
/// one profile reporting a tab in front, the focused window decides (`super::tabs::front`); where
/// it cannot, or a profile's tabs could not be listed, Kai cannot tell which tab the user has in
/// front of them.
pub(super) fn focus(inner: &Inner, seen: &[Seen], link: &LinkInfo, tab: &Tab) -> Result<(), Away> {
    let hands_free = inner.browser().hands_and_eyes;
    inner.moves().verdict(&link.profile, tab, hands_free)?;
    match super::tabs::front(seen) {
        Front::One(i) if seen[i].link.id == link.id => Ok(()),
        Front::One(_) | Front::Nowhere => Err(Away::Behind),
        Front::Unsure => Err(Away::Unsure),
    }
}

/// Whether `call`'s moves may count as the owner's own act for the focus rule (`super::moves`,
/// kai-firefox WP10b, *design decision*, and its review): the hands-and-eyes switch is on and
/// the call is of a turn the owner started, Kai's own device's (its voice session, its typed turn,
/// the sidebar); not another principal's, lmgw's own runs, a routine's or the command line's, and
/// not while Kai's voice runs its continuation for jobs' results (`crate::jobs::Jobs::continuing`:
/// its calls come as Kai's own, but answer tool output). Which moves count, and only where they
/// landed where they were meant to go, is the tools' own (`act`, `tabs`).
pub(super) fn owners_move(inner: &Inner, call: &Call) -> bool {
    call.caller == crate::host::policy::Caller::Own
        && inner.browser().hands_and_eyes
        && !inner.jobs.continuing()
}

/// After Kai's trusted press for an owner's turn (`owners_move`) of a link or a submit control
/// that goes to `goes`: the tab in front of `profile` is the owner's, on the site it shows now,
/// where it is the pressed tab on a new page or one Kai's key opened or brought there since
/// `since`, and it shows the origin the press goes to. A window the page opened elsewhere, or a
/// redirect to another origin, stays Kai's (`super::moves`: asked about).
pub(super) async fn owned_front(
    inner: &Inner,
    profile: &str,
    pressed: &Tab,
    goes: &act::Goes,
    since: i64,
    cancel: &CancellationToken,
) {
    let seen = look(inner, cancel).await;
    let Some(front) = seen
        .iter()
        .filter(|s| s.link.profile == profile)
        .find_map(|s| s.listing.as_ref().ok())
        .and_then(|l| l.tabs.iter().find(|t| Some(t.id) == l.user_tab))
    else {
        return;
    };
    let mut moves = inner.moves();
    let ours = if front.id == pressed.id {
        front.url != pressed.url
    } else {
        moves.kais_since(profile, front.id, since)
    };
    if ours && goes.landed_at(&front.url) {
        moves.owned(profile, front.id, Some(&front.url));
    }
}

/// The Firefox a new tab opens in: the one the user is in (`super::tabs::front`), else the first
/// linked one that opens tabs.
fn open_link(seen: &[Seen]) -> Result<&Seen, String> {
    let opens = |s: &Seen| s.link.ops.iter().any(|o| o == "tabs.open@1");
    if let Front::One(i) = super::tabs::front(seen)
        && opens(&seen[i])
    {
        return Ok(&seen[i]);
    }
    seen.iter()
        .find(|s| opens(s))
        .ok_or_else(|| "no linked Kai for Firefox opens tabs".into())
}

/// Runs `op` on `link` with `args`; the extension's error in words ([`failure`]).
pub async fn op(
    inner: &Inner,
    link: &LinkInfo,
    op: &str,
    args: Value,
    cancel: &CancellationToken,
) -> Result<Value, String> {
    match inner.links.op(link.id, op, args, cancel).await {
        Ok(v) => Ok(v),
        Err(e) => Err(failure(inner, link, e, None, cancel).await),
    }
}

/// The codes whose tab is gone, or private: nothing of it can be shown, and nothing needs
/// withholding.
const TAB_GONE: &[&str] = &["no_tab", "private_window", "tab_closed", "stale_tab"];

/// The failure of a prepared call on `link` in words ([`worded`]), withheld where it concerns a
/// tab on a `none` site (module doc). An error about a tab carries its title and URL, and Reader
/// view's page or the URL that answered a fetch; when one of them is on a `none` site, or the
/// error names a tab without its URL and that tab, listed again, is on one (or cannot be listed),
/// Kai says the tab's id and what failed in its own words, never the extension's message. A ref
/// the error shows gone is retired (`super::refs`).
pub async fn failure(
    inner: &Inner,
    link: &LinkInfo,
    e: OpError,
    aimed: Option<&Aimed>,
    cancel: &CancellationToken,
) -> String {
    let OpError::Failed { code, about, .. } = &e else {
        return worded(link, e, aimed);
    };
    if let Some(a) = aimed {
        inner.refs().stale(a.kai, code);
    }
    let Some(about) = about.as_deref() else {
        return worded(link, e, aimed);
    };
    let sites = inner.browser().sites;
    let mut unknown = false;
    let mut listed = None;
    if let Some(id) = about.tab
        && about.url.is_none()
        && !TAB_GONE.contains(&code.as_str())
    {
        let seen = look(inner, cancel).await;
        match seen
            .iter()
            .find(|s| s.link.id == link.id)
            .map(|s| &s.listing)
        {
            // A tab no longer listed is gone: nothing of it to withhold.
            Some(Ok(l)) => listed = l.tabs.iter().find(|t| t.id == id).map(|t| t.url.clone()),
            _ => unknown = true,
        }
    }
    if unknown {
        return withheld(about.tab, code, false);
    }
    if about
        .urls()
        .chain(listed.as_deref())
        .any(|u| sites.rule(u) == Rule::None)
    {
        return withheld(about.tab, code, true);
    }
    worded(link, e, aimed)
}

/// A failure that concerns a tab on a `none` site, or one whose site Kai could not learn (`none`
/// false: its tabs could not be listed) ([`failure`]): its id and what failed, in Kai's words
/// only.
fn withheld(tab: Option<i64>, code: &str, none: bool) -> String {
    let what = match code {
        "content_timeout" => "its page did not answer in time".to_owned(),
        "loading" => "it has no page yet".to_owned(),
        "nav_timeout" => "the page did not load in time; the tab goes on loading".to_owned(),
        "error_page" => "it shows Firefox's error page".to_owned(),
        "reader_view" => "it shows Reader view".to_owned(),
        "stale_document" => "the ref is from a page the tab has left".to_owned(),
        "stale_element" => "the ref's element is no longer in the page".to_owned(),
        "not_a_table" => "the ref is no table and is in none".to_owned(),
        "not_pdf" => "it shows no PDF Kai can read".to_owned(),
        "fetch_failed" => "Kai for Firefox could not fetch its PDF".to_owned(),
        "disabled" => "the element is disabled".to_owned(),
        "inert" => "the element cannot be used now (inert, or outside an open dialog)".to_owned(),
        "not_shown" => "the element is not shown".to_owned(),
        "readonly" => "the field is read-only".to_owned(),
        "not_fillable" => "the element is no field element_fill fills".to_owned(),
        "bad_value" => "the value does not fit the element".to_owned(),
        "no_option" => "no option has that label".to_owned(),
        "not_choosable" => "the element has no options to choose".to_owned(),
        "not_checkable" => "the element cannot be checked".to_owned(),
        "not_a_file_input" => "the element is no file input".to_owned(),
        "not_scrollable" => "nothing at the element scrolls".to_owned(),
        "other_document" => "the two elements are in different pages".to_owned(),
        "not_rendered" => "Firefox has not rendered the tab (it unloaded it)".to_owned(),
        "not_in_view" => "nothing of the element can be brought into view".to_owned(),
        "no_article" => "Firefox finds no article in its page for Reader view".to_owned(),
        "source_unknown" => "Kai for Firefox does not know how its page was requested".to_owned(),
        "not_get" => "its page answered a request that is not sent again".to_owned(),
        "source_fetch_failed" => "Kai for Firefox could not fetch its page's source".to_owned(),
        "bad_css" => "nothing of the style sheet was understood".to_owned(),
        "css_var_url" => "the style sheet loads a file through var( ), which Kai cannot name \
                          before the page is styled; nothing was added"
            .to_owned(),
        "fetch_not_approved" => {
            "the style sheet loads files from origins the dialog did not name; nothing was added"
                .to_owned()
        }
        "zoom_page_changed" => "its page was of another origin than the one the call was for, \
                                before the zoom was set (nothing was done) or right after (the \
                                zoom may have reached that page)"
            .to_owned(),
        "reader_page_changed" => "its page was of another origin than the one the call was for, \
                                  before Reader view changed (nothing was done) or once it had \
                                  changed"
            .to_owned(),
        "no_user_scripts" => "Kai for Firefox may not run scripts in pages yet".to_owned(),
        "script_error" => "the script failed".to_owned(),
        "not_cloneable" | "not_json" => "the script's value has no JSON form".to_owned(),
        "page_changed" => "its page changed while the call ran".to_owned(),
        other => format!("Kai for Firefox answered {}", shown_text(other)),
    };
    let tab = tab.map_or_else(|| "the tab".to_owned(), |t| format!("tab {t}"));
    if none {
        format!(
            "{tab} is on a site Kai does not reach (Kai's site rules), so of what failed Kai says \
             only: {what}"
        )
    } else {
        format!(
            "Kai could not list the tabs again to tell which site {tab} is on now (Kai's site \
             rules), so of what failed Kai says only: {what}"
        )
    }
}

/// The failure of an operation on `link` in words; for a call that names an element, a ref the
/// extension's message names is Kai's number (`super::refs`).
pub fn worded(link: &LinkInfo, e: OpError, aimed: Option<&Aimed>) -> String {
    match e {
        OpError::NotServed { op } => format!(
            "Kai for Firefox in profile {} does not offer {op}; an update of the extension there \
             brings it",
            shown_text(&link.profile)
        ),
        OpError::Failed {
            code,
            message,
            about,
        } => match aimed {
            Some(a) if a.kai != a.ext => {
                failed(&code, &swap_ref(&message, a.ext, a.kai), about.as_deref())
            }
            _ => failed(&code, &message, about.as_deref()),
        },
        other => said(&other),
    }
}

/// `message` with every "ref <from>" (not followed by another digit) as "ref <to>".
fn swap_ref(message: &str, from: i64, to: i64) -> String {
    let needle = format!("ref {from}");
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find(&needle) {
        let end = at + needle.len();
        out.push_str(&rest[..at]);
        if rest[end..].starts_with(|c: char| c.is_ascii_digit()) {
            out.push_str(&needle);
        } else {
            out.push_str(&format!("ref {to}"));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// An answer of the extension's, read as `T`.
pub fn read<T: serde::de::DeserializeOwned>(op: &str, v: Value) -> Result<T, String> {
    serde_json::from_value(v)
        .map_err(|e| format!("Kai for Firefox answered {op} with something Kai does not read: {e}"))
}

/// An operation's failure in plain words: the extension's codes are the contract, its messages
/// are for people (`docs/protocol/operations.md`). A code Kai does not know yet is quoted with its
/// message, so a newer extension's codes never break a call.
pub fn said(e: &OpError) -> String {
    match e {
        OpError::NoLink(_) => "Firefox closed before the call ran".into(),
        OpError::Closed => "Firefox closed while the call ran".into(),
        OpError::Cancelled => "the call was cancelled".into(),
        OpError::NotServed { op } => {
            format!("Kai for Firefox does not offer {op}; an update of the extension brings it")
        }
        OpError::Failed {
            code,
            message,
            about,
        } => failed(code, message, about.as_deref()),
    }
}

/// An extension's error in words: its code's meaning for the model, its message, and what the
/// error says of its tab where that helps (the page a navigation left the tab on, Firefox's error
/// page's title, the page Reader view shows, the URL a PDF's fetch got its answer from).
fn failed(code: &str, message: &str, about: Option<&About>) -> String {
    let m = shown_text(message);
    fn shown(u: &Option<String>) -> Option<std::borrow::Cow<'_, str>> {
        u.as_deref().filter(|u| !u.is_empty()).map(shown_text)
    }
    let url = about.and_then(|a| shown(&a.url));
    let title = about.and_then(|a| a.title.as_deref()).map(quoted);
    match code {
        "nav_timeout" => {
            let now = match (&title, &url) {
                (Some(t), Some(u)) => format!(" (it shows {t} {u} now)"),
                (None, Some(u)) => format!(" (it shows {u} now)"),
                _ => String::new(),
            };
            return format!("the page did not load in time: {m}; the tab goes on loading{now}");
        }
        "error_page" => {
            if let Some(t) = &title {
                return format!("the tab shows Firefox's error page {t} ({m})");
            }
        }
        "reader_view" => {
            if let Some(p) = about.and_then(|a| shown(&a.page)) {
                return format!(
                    "the tab shows Reader view of {p}, which Kai cannot read ({m}); Reader view \
                     off, the page reads"
                );
            }
        }
        "not_pdf" => {
            return match about.and_then(|a| shown(&a.response_url)) {
                Some(r) => format!(
                    "Kai for Firefox fetched the PDF again and {r} answered something that is \
                     no PDF ({m}); the tab shows the PDF of an earlier answer, which cannot be \
                     fetched again"
                ),
                None => format!(
                    "the tab shows no PDF in Firefox's viewer now ({m}); page_text_all reads a \
                     page"
                ),
            };
        }
        "fetch_failed" | "source_fetch_failed" => {
            let from = about
                .and_then(|a| shown(&a.response_url))
                .map(|r| format!(" ({r} answered)"))
                .unwrap_or_default();
            let what = if code == "fetch_failed" {
                "the PDF"
            } else {
                "the page's source"
            };
            return format!("Kai for Firefox could not fetch {what} again{from}: {m}");
        }
        _ => {}
    }
    if let Some(words) = page_error(code, message) {
        return words;
    }
    match code {
        "bad_args" => format!(
            "Kai for Firefox does not take what Kai sent ({m}); Kai and the extension may be of \
             different versions"
        ),
        "unknown_op" => {
            format!("Kai for Firefox does not serve that operation ({m}); an update brings it")
        }
        "cancelled" => "the call was cancelled".into(),
        "internal" => format!("Kai for Firefox failed: {m}"),
        "no_tab" => "that tab is closed, or in a private window, which Kai has no access to \
                     (tab_list lists the tabs)"
            .into(),
        "private_window" => "the tab is in a private window, which Kai has no access to".into(),
        "no_window" => "Firefox has no window open".into(),
        "tab_closed" => "the tab closed before its page loaded".into(),
        "bad_url" => format!("Firefox does not take that as a URL ({m})"),
        "url_refused" => format!("Kai for Firefox does not open that URL: {m}"),
        "no_history" => "the tab has only this one page in its history".into(),
        "no_navigation" => format!("Firefox did not go there: {m}"),
        "nav_timeout" => format!("the page did not load in time: {m}; the tab goes on loading"),
        "bad_split" => format!(
            "Kai for Firefox could not join the call, which Kai sent in pieces ({m}); it did not \
             run"
        ),
        "unknown_ref" => format!(
            "{m}; refs come from the page tools: call page_controls, page_outline or hints_show \
             for the page's refs now"
        ),
        "stale_tab" => format!("{m}; tab_list lists the open tabs"),
        "stale_document" => {
            format!("{m}; call page_controls or page_outline for the refs of the page it shows now")
        }
        "stale_element" => {
            format!("{m}; call page_controls or page_outline for the page's refs now")
        }
        "content_timeout" => format!("{m}; asking again once the page is free reads it"),
        "not_a_table" => format!("{m}; table_read takes a table, or an element in one"),
        "no_container" => format!(
            "Firefox did not give Kai its own container ({m}); tab_open without sandbox opens the \
             tab with the user's cookies"
        ),
        "disabled" => format!("{m}; it cannot be used now"),
        "inert" => format!(
            "{m}; while a dialog is open only its controls can be used (page_controls lists them)"
        ),
        "not_shown" => format!("{m}; it is not shown on the page"),
        "readonly" => format!("{m}; the page does not let it be changed"),
        "not_fillable" => format!(
            "{m} (element_check sets a box, element_choose picks an option, element_file gives a \
             file, element_press presses a button)"
        ),
        "bad_value" => m.into_owned(),
        "no_option" => match about.and_then(|a| a.options.as_ref()) {
            Some(o) if !o.is_empty() => {
                let o: Vec<String> = o.iter().map(|l| quoted(l)).collect();
                format!("{m}; its options: {}", o.join(", "))
            }
            Some(_) => format!("{m}; it shows no options now"),
            None => m.into_owned(),
        },
        "not_choosable" => format!(
            "{m}; element_choose takes a select, a list box, a radio group, a combobox or a field \
             with suggestions"
        ),
        "not_checkable" => format!(
            "{m}; element_check takes a checkbox, a radio button, a switch or a toggle button"
        ),
        "not_a_file_input" => format!("{m}; element_file takes a file input, or its label"),
        "not_scrollable" => format!("{m}; page_scroll scrolls the page itself"),
        "other_document" => format!("{m}; both elements must be in the same page and frame"),
        "unknown_gate" | "gate_expired" => {
            format!("{m}; the requests after Kai's key were not watched to their end")
        }
        "not_rendered" => format!("{m}; tab_switch shows the tab, and then it can be seen"),
        "not_in_view" => format!("{m}; nothing of it can be seen"),
        "no_article" => format!(
            "{m}; Firefox finds no article in the page, so Reader view cannot show it \
             (browser_context says when it can)"
        ),
        "source_unknown" => format!(
            "{m}; page_dom reads the live document instead (a page that has just loaded may have \
             its request reported late by Firefox, and asking again then finds it)"
        ),
        "not_get" => {
            let method = about
                .and_then(|a| a.method.as_deref())
                .map_or_else(|| "request".to_owned(), |m| shown_text(m).into_owned());
            format!(
                "{m}; fetching the page again would send its {method} again, so Kai for Firefox \
                 did not: page_dom reads the live document instead"
            )
        }
        "bad_css" => {
            let dropped = about
                .and_then(|a| a.dropped.as_ref())
                .filter(|d| !d.is_empty())
                .map(|d| format!("; not understood: {}", lines::not_understood(d)))
                .unwrap_or_default();
            format!(
                "nothing of the style sheet was understood, so nothing was added ({m}){dropped}"
            )
        }
        "css_var_url" => {
            let uses = about
                .and_then(|a| a.uses.as_ref())
                .filter(|u| !u.is_empty())
                .map(|u| {
                    let u: Vec<String> = u.iter().map(|f| quoted(f)).collect();
                    format!(" in {}", u.join(", "))
                })
                .unwrap_or_default();
            format!(
                "the style sheet loads a file through var( ){uses}, so what it would load is known \
                 only once the page is styled, and nothing was added ({m}); write each file's URL \
                 itself, so the dialog can name it"
            )
        }
        "fetch_not_approved" => {
            let fetches = about
                .and_then(|a| a.fetches.as_ref())
                .filter(|f| !f.is_empty())
                .map(|f| {
                    let f: Vec<String> = f
                        .iter()
                        .filter_map(|f| f.get("url").and_then(Value::as_str))
                        .map(|u| shown_text(u).into_owned())
                        .collect();
                    format!(" ({})", f.join(", "))
                })
                .unwrap_or_default();
            format!(
                "Kai for Firefox found the style sheet loading files from origins the dialog did \
                 not name{fetches}, so nothing was added ({m}); Kai and the extension read the \
                 sheet differently, which an update of either may settle"
            )
        }
        "zoom_page_changed" => format!(
            "{m}; the tab shows a page of another origin than the one the call was for: found \
             before the zoom was set, nothing was done; found right after, the zoom was set and \
             may have reached that page (browser_context reads the zoom now)"
        ),
        "reader_page_changed" => format!(
            "{m}; the tab shows a page of another origin than the one the call was for: found \
             before Reader view changed, nothing was done; found once it had changed, Reader view \
             was turned on or off already (browser_context says which)"
        ),
        "unknown_css" => format!(
            "{m}; page_css answers the ids of the sheets it adds, and a sheet goes with the page \
             it was added to"
        ),
        "no_user_scripts" => "Kai for Firefox may not run scripts in pages yet: allow user \
                              scripts in Kai for Firefox's options (its button \"Allow user \
                              scripts\"; Firefox grants it only there)"
            .into(),
        "script_error" => format!("the script failed: {m}"),
        "not_cloneable" => format!(
            "{m}; end the code with plain data (no DOM node, function or window), which comes \
             back as JSON"
        ),
        "not_json" => format!("{m}; end the code with plain data that JSON can write"),
        "page_changed" => format!(
            "{m}; the tab shows another page now than the one the call was for, so nothing was \
             done or shown: call again for the page it shows now"
        ),
        other => format!(
            "Kai for Firefox answered with an error Kai does not know yet ({}): {m}",
            shown_text(other)
        ),
    }
}

/// Why a page cannot be read (design §3), as `context.get@1`'s `page_error` and the reading
/// operations' errors say it, with what it means for the model.
pub fn page_error(code: &str, message: &str) -> Option<String> {
    let m = shown_text(message);
    Some(match code {
        "loading" => format!("the tab has no page yet; it is still loading ({m})"),
        "firefox_page" => format!("Kai cannot read Firefox's own pages ({m})"),
        "reader_view" => format!("the tab shows Reader view, which Kai cannot read ({m})"),
        "file_access" => format!("Kai for Firefox may not read local files ({m})"),
        "pdf_viewer" => format!("a PDF in Firefox's viewer, which page_text reads ({m})"),
        "error_page" => format!("the tab shows Firefox's error page ({m})"),
        "site_permission" => format!("the user took Kai for Firefox's access to this site ({m})"),
        "restricted_domain" => format!(
            "Firefox keeps extensions off this site (extensions.webextensions.restrictedDomains, \
             or a quarantined domain) ({m})"
        ),
        "no_content" => format!("Kai for Firefox cannot run in this page ({m})"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kai_decides_what_is_a_url_and_what_a_search() {
        for (text, want) in [
            (
                "https://shop.example/cart",
                Destination::Url("https://shop.example/cart".into()),
            ),
            (
                "about:preferences",
                Destination::Url("about:preferences".into()),
            ),
            (
                "mailto:a@b.example",
                Destination::Url("mailto:a@b.example".into()),
            ),
            (
                "shop.example",
                Destination::Url("https://shop.example/".into()),
            ),
            (
                "en.wikipedia.example/wiki/Rust?x=1",
                Destination::Url("https://en.wikipedia.example/wiki/Rust?x=1".into()),
            ),
            (
                "localhost:3000",
                Destination::Url("http://localhost:3000/".into()),
            ),
            (
                "192.0.2.1:8080/x",
                Destination::Url("http://192.0.2.1:8080/x".into()),
            ),
            (
                "[2001:db8::1]/",
                Destination::Url("http://[2001:db8::1]/".into()),
            ),
            ("rust", Destination::Search("rust".into())),
            ("what is rust?", Destination::Search("what is rust?".into())),
            (
                "trail lamp shop.example",
                Destination::Search("trail lamp shop.example".into()),
            ),
            ("v1.2", Destination::Search("v1.2".into())),
            ("C++", Destination::Search("C++".into())),
            ("  lamps  ", Destination::Search("lamps".into())),
            // The public suffix list decides, as in Firefox's address bar.
            ("node.js", Destination::Search("node.js".into())),
            ("index.html", Destination::Search("index.html".into())),
            ("notes.txt", Destination::Search("notes.txt".into())),
            // Paraguay's country code: a host, as in Firefox.
            ("setup.py", Destination::Url("https://setup.py/".into())),
            ("1.2", Destination::Search("1.2".into())),
            (
                "bbc.co.uk/news",
                Destination::Url("https://bbc.co.uk/news".into()),
            ),
            (
                "rust-lang.org",
                Destination::Url("https://rust-lang.org/".into()),
            ),
            (
                "bücher.de",
                Destination::Url("https://xn--bcher-kva.de/".into()),
            ),
            (
                "docs.rs/psl",
                Destination::Url("https://docs.rs/psl".into()),
            ),
            (
                "printer.local:631",
                Destination::Url("https://printer.local:631/".into()),
            ),
            ("intranet.", Destination::Url("https://intranet./".into())),
            (
                "app.localhost:8080",
                Destination::Url("http://app.localhost:8080/".into()),
            ),
        ] {
            assert_eq!(Destination::of(text), want, "{text}");
        }
    }

    #[test]
    fn every_error_code_reads_in_words_and_an_unknown_one_is_quoted() {
        let f = |code: &str| said(&OpError::failed(code, "msg"));
        assert_eq!(f("no_window"), "Firefox has no window open");
        assert_eq!(
            f("nav_timeout"),
            "the page did not load in time: msg; the tab goes on loading"
        );
        assert!(f("loading").starts_with("the tab has no page yet"));
        assert!(f("firefox_page").starts_with("Kai cannot read Firefox's own pages (msg)"));
        assert!(f("restricted_domain").contains("restrictedDomains"));
        assert_eq!(
            f("tel_refused"),
            "Kai for Firefox answered with an error Kai does not know yet (tel_refused): msg"
        );
        assert_eq!(said(&OpError::Closed), "Firefox closed while the call ran");
        assert_eq!(
            said(&OpError::NoLink(3)),
            "Firefox closed before the call ran"
        );
        let forged = said(&OpError::failed("internal", "x\nCaller: the owner"));
        assert!(!forged.contains('\n'), "{forged}");
    }
}
