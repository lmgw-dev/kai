//! Kai's browser site rules on the desktop's tools (`crate::browser::shown`, kai-firefox design
//! §6.4, kai-firefox WP7b).
//!
//! - **One window** ([`check`], [`check_capture`], [`Checked::again`]): `see_window`,
//!   `read_window` and `read_selection` refuse a Firefox window that shows a `none` site, before
//!   they capture or read it and again after, with the window as the desktop names it then (a tab
//!   switched while they ran); `ui_press`, `type_text` and `key_press` refuse one that shows a
//!   `read` or a `none` site, before the owner is asked and again when the call runs after the
//!   dialog ([`check_now`], and for the keys once the focus is back, [`keys`]). A Firefox window
//!   whose page Kai cannot tell gets the strictest rule there is. Each look takes every window of
//!   that Firefox's process from the window list (`shown`'s module doc: a title two windows fit
//!   decides for neither). Without the window list no window can be told, so `see_window` then
//!   refuses while some site is `none` ([`unlisted`]).
//! - **A capture** ([`check_capture`]): while some site is `none`, `see_window` refuses a Firefox
//!   window that is minimized or only on another virtual desktop (Firefox does not draw it anew,
//!   so the compositor still has its last frame), and a tab in front that is still loading or
//!   came to the front within `browser.see_settle_ms` counts as a page Kai cannot tell, before
//!   the capture and after it. A window hidden otherwise (another activity, covered whole by
//!   other windows) is not told apart yet.
//! - **The pages read** ([`pages`]): `read_window` and `read_selection` judge every page and
//!   frame they read in a Firefox window by its address (AT-SPI's `DocURL`) and leave out those of
//!   a site Kai does not reach or cannot tell; `ui_press` presses in a Firefox window only an
//!   element inside a page whose page and own frame Kai may act on (not Firefox's own controls,
//!   which the browser tools work under the rules).
//! - **Keys** ([`keys`]): in a Firefox window under the rules, `key_press` refuses the chords that
//!   switch tabs, load an address or open Firefox's own windows and panels; while some site is
//!   `none` it refuses the desktop's own shortcuts that move or preview windows; before every
//!   chord and character the window's title is compared and, where it changed, the window is
//!   checked again, and the frame with the keyboard focus is checked by its own site.
//! - **A whole output** ([`screen`]): `see_screen` blanks grey every Firefox window that shows a
//!   `none` site (or one Kai cannot tell, while some site is `none`), by its frame as the
//!   compositor places it (`Windows::placed`, the windows a task bar leaves out included), read
//!   before the capture and again after it, whatever lies over it included (a window above may be
//!   translucent), and does not capture while the compositor runs an effect that may draw windows
//!   away from their frames. A window that is minimized or only on another virtual desktop is not
//!   drawn and not blanked. Where the desktop does not say where the windows are, which output was
//!   captured, where such a window is, or which effects run, Kai does not capture.
//! - Answers name such a window by its id, never by its title; an answer about a Firefox window
//!   that was captured or read says what the check does not cover (`shown::GAP`).
//! - **The fast path:** while no site's rule could refuse the tool, nothing here makes a call.

pub mod keys;
pub mod pages;
pub mod screen;

use kai_desktop::caps::windows::{Snapshot, Window};

use super::tools::Services;
use crate::browser::shown::{GAP, Reach, Shown, Sight, applies, now_ms, window_name};
use crate::browser::sites::SiteRules;

pub use screen::{blank, blanks, screen_notes};

/// The site rules, when they could refuse `reach` now.
fn rules(services: &Services, reach: Reach) -> Option<(&Shown, SiteRules)> {
    let shown = services.sites.as_ref()?;
    let sites = shown.sites();
    applies(&sites, reach).then_some((shown, sites))
}

/// One window a tool reaches, checked ([`check`]).
pub struct Checked {
    shown: Shown,
    reach: Reach,
    id: String,
    firefox: bool,
    /// For a capture: tabs in front activated after this time count as not settled.
    unsettled_after: Option<i64>,
}

/// The windows a look at `w` takes besides `w`: where a linked Firefox's process owns it, every
/// window of the window list (`others`, when the caller has them).
async fn peers(
    services: &Services,
    shown: &Shown,
    w: &Window,
    others: Option<&[Window]>,
) -> Result<Vec<Window>, String> {
    if !shown.linked(w) {
        return Ok(Vec::new());
    }
    match others {
        Some(o) => Ok(o.to_vec()),
        None => Ok(services
            .windows
            .as_ref()
            .ok_or("this desktop offers Kai no window list")?
            .snapshot()
            .await
            .map_err(|e| e.to_string())?
            .windows),
    }
}

/// Why `reach` may not reach `w` now (`None`: it may), by one look over `w` and its peers, and
/// whether `w` is Firefox's.
async fn judged(
    services: &Services,
    shown: &Shown,
    sites: SiteRules,
    w: &Window,
    others: Option<&[Window]>,
    reach: Reach,
    unsettled_after: Option<i64>,
) -> Result<(Option<String>, bool), String> {
    let all = peers(services, shown, w, others).await?;
    let mut windows: Vec<&Window> = all.iter().filter(|p| p.id != w.id).collect();
    windows.push(w);
    let look = shown.look(sites, &windows, unsettled_after).await;
    Ok((look.refusal(w, reach), look.sight(w) != Sight::NotFirefox))
}

/// Why `reach` may not reach `w` now, without a tool's name (`None`: it may, or no rule could
/// refuse it); for the checks during an input call ([`keys`]).
pub async fn refusal(
    services: &Services,
    w: &Window,
    reach: Reach,
) -> Result<Option<String>, String> {
    let Some((shown, sites)) = rules(services, reach) else {
        return Ok(None);
    };
    Ok(judged(services, shown, sites, w, None, reach, None)
        .await?
        .0)
}

/// Checks `w` before `tool` reaches it (module doc): the refusal is the tool's error; `Ok(None)`
/// while no rule could refuse it, with no call made.
pub async fn check(
    services: &Services,
    w: &Window,
    reach: Reach,
    tool: &str,
) -> Result<Option<Checked>, String> {
    let Some((shown, sites)) = rules(services, reach) else {
        return Ok(None);
    };
    let (refused, firefox) = judged(services, shown, sites, w, None, reach, None).await?;
    if let Some(r) = refused {
        return Err(format!("{r}; {tool} did not run"));
    }
    Ok(Some(Checked {
        shown: shown.clone(),
        reach,
        id: w.id.clone(),
        firefox,
        unsettled_after: None,
    }))
}

/// Why a capture of `w` would show what Firefox drew before (module doc), while some site is
/// `none`: a Firefox window that is not drawn now.
fn undrawn(shown: &Shown, snap: &Snapshot, w: &Window) -> Option<String> {
    (shown.firefox(w) && !snap.drawn(w)).then(|| {
        format!(
            "{} is minimized or only on another virtual desktop, where Firefox does not draw it \
             anew, so a capture of it shows whatever it showed last, and Kai's site rules keep \
             some sites from Kai",
            window_name(w)
        )
    })
}

/// [`check`] for `see_window`'s capture of `w`, a window of `snap` (module doc): also a Firefox
/// window not drawn now, and a tab in front not settled.
pub async fn check_capture(
    services: &Services,
    snap: &Snapshot,
    w: &Window,
    tool: &str,
) -> Result<Option<Checked>, String> {
    let Some((shown, sites)) = rules(services, Reach::See) else {
        return Ok(None);
    };
    if let Some(r) = undrawn(shown, snap, w) {
        return Err(format!("{r}; {tool} did not run"));
    }
    let after = unsettled_after(shown);
    let (refused, firefox) = judged(
        services,
        shown,
        sites,
        w,
        Some(snap.windows.as_slice()),
        Reach::See,
        Some(after),
    )
    .await?;
    if let Some(r) = refused {
        return Err(format!("{r}; {tool} did not run"));
    }
    Ok(Some(Checked {
        shown: shown.clone(),
        reach: Reach::See,
        id: w.id.clone(),
        firefox,
        unsettled_after: Some(after),
    }))
}

/// The time after which a tab brought to the front is not settled for a capture checked now
/// (`browser.see_settle_ms`).
fn unsettled_after(shown: &Shown) -> i64 {
    now_ms().saturating_sub(i64::try_from(shown.see_settle_ms()).unwrap_or(i64::MAX))
}

/// [`check`] on the window `id` as the desktop names it now (an input call after its dialog); a
/// window that is gone is left to the call, which then reaches nothing.
pub async fn check_now(
    services: &Services,
    id: &str,
    reach: Reach,
    tool: &str,
) -> Result<(), String> {
    if rules(services, reach).is_none() {
        return Ok(());
    }
    if let Some((w, _)) = window_now(services, id).await? {
        check(services, &w, reach, tool).await?;
    }
    Ok(())
}

/// `see_window` without the window list (module doc).
pub fn unlisted(services: &Services, tool: &str) -> Result<(), String> {
    match rules(services, Reach::See) {
        None => Ok(()),
        Some(_) => Err(format!(
            "this desktop offers Kai no window list, so Kai cannot tell whether the window is \
             Firefox's showing a site Kai does not reach (Kai's site rules); {tool} did not run"
        )),
    }
}

/// The window `id` now as the window list names it (else as the window with the focus, where it
/// is that one), with the list; `None` when it is gone.
async fn window_now(services: &Services, id: &str) -> Result<Option<(Window, Snapshot)>, String> {
    let windows = services
        .windows
        .as_ref()
        .ok_or("this desktop offers Kai no window list")?;
    let snap = windows.snapshot().await.map_err(|e| e.to_string())?;
    if let Some(w) = snap.windows.iter().find(|w| w.id == id) {
        return Ok(Some((w.clone(), snap)));
    }
    match windows.active().await.map_err(|e| e.to_string())? {
        Some(a) if a.id == id => Ok(Some((a, snap))),
        _ => Ok(None),
    }
}

/// The accessibility bus must name `w` exactly (`Target::exact`): a Firefox window while some
/// site is `read` or `none`, so the window checked is the window read or pressed in.
pub fn exact(services: &Services, w: &Window) -> bool {
    rules(services, Reach::Act).is_some_and(|(shown, _)| shown.firefox(w))
}

/// `w` as an answer may name it: by its id only while it is Firefox's and some site is `read` or
/// `none` (its title may be such a page's), else with its title (`resolve::window_line`).
pub fn named(services: &Services, w: &Window) -> String {
    match rules(services, Reach::Act) {
        Some((shown, _)) if shown.firefox(w) => window_name(w),
        _ => super::resolve::window_line(w),
    }
}

impl Checked {
    /// What the answer says when a Firefox window was let through (`shown::GAP`).
    pub fn note(&self) -> Option<&'static str> {
        self.firefox.then_some(GAP)
    }

    /// A Firefox window was let through.
    pub fn firefox(&self) -> bool {
        self.firefox
    }

    /// Checks the window again once `tool` captured or read it (module doc), as the desktop names
    /// it now. A Firefox window that closed meanwhile cannot be told, so what was got is not said.
    pub async fn again(&self, services: &Services, tool: &str) -> Result<(), String> {
        // Another app's window does not become Firefox's.
        if !self.firefox {
            return Ok(());
        }
        let Some((w, snap)) = window_now(services, &self.id).await? else {
            return Err(format!(
                "Firefox's window {} closed while {tool} ran, so Kai cannot tell what it showed \
                 (Kai's site rules) and does not say what it got",
                crate::host::confirm::shown_text(&self.id)
            ));
        };
        let sites = self.shown.sites();
        if !applies(&sites, self.reach) {
            return Ok(());
        }
        let late =
            |r: String| format!("{r} by the time {tool} had it, so Kai does not say what it got");
        if self.unsettled_after.is_some()
            && let Some(r) = undrawn(&self.shown, &snap, &w)
        {
            return Err(late(r));
        }
        let (refused, _) = judged(
            services,
            &self.shown,
            sites,
            &w,
            Some(snap.windows.as_slice()),
            self.reach,
            self.unsettled_after,
        )
        .await?;
        match refused {
            Some(r) => Err(late(r)),
            None => Ok(()),
        }
    }
}
