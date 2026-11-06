//! Element refs as the model sees them (kai-firefox design D4; the contract's Element): Kai's
//! numbers for the extensions' refs, and the way back from a number the model names to the link
//! that gave it out.
//!
//! - **One number space.** Each Kai for Firefox numbers its elements from one counter per Firefox
//!   run, so two linked profiles give out the same numbers. Kai shows an extension's own number
//!   (the identity, so the hint labels on the page and Kai's lines agree) unless that number is
//!   taken, held for another element or retired; then the element gets a number of Kai's, above
//!   every number Kai has handed out ([`Refs::number`]). With one profile linked and no number
//!   retired, Kai's numbers are the extension's.
//! - **Every answer is renumbered** ([`Refs::renumber`]): each `ref` in it becomes Kai's number,
//!   which Kai keeps with its link, tab and frame, so a later call resolves it there
//!   ([`Refs::find`]). Where the two differ, the object keeps the extension's number as `ext_ref`:
//!   the number its hint label shows.
//! - **A number is never handed out twice.** A number Kai stops holding is retired, and a retired
//!   number stays taken:
//!   - a new link of a profile (the extension or Firefox restarted: a restarted extension does not
//!     know its earlier refs, a restarted Firefox gives the numbers out again) retires what Kai held
//!     of that profile's earlier links. A link that ends while no newer link of its profile came
//!     keeps its refs, so a call can say that its Firefox is gone;
//!   - an element Kai learns is gone retires its number and those that went with it
//!     ([`Refs::stale`]): a closed tab takes its refs (`stale_tab`, or a tab a listing no longer
//!     has, [`Refs::closed`]); a document that was left takes every ref of its tab and frame up to
//!     its own (an extension's counter only grows, so they are of that document or an earlier
//!     one; the top document's takes its frames' too); an element the page removed, or a ref the
//!     extension does not know, takes itself;
//!   - Kai keeps the highest number it handed out in its state directory ([`Refs::stored`]), and
//!     after a restart every number up to it is retired: the model's context may still hold them.
//! - **A number Kai never handed out** (a hint label drawn after a scroll, which no answer named)
//!   goes to the one linked extension that serves the call, which knows whether it gave it out.
//!   With two profiles linked Kai cannot tell which one, and says so.
//! - **A number that is two things** (kai-firefox WP5b's design decision, until the hints can show
//!   Kai's numbers): a hint label shows the extension's number, so where Kai renumbered an element,
//!   its label's number can be Kai's number for another element. Kai refuses such a number and
//!   says both: the labelled element by its ref, and the other one under a new number of Kai's,
//!   the old number retired, so the two are not mixed up again.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::links::{LinkId, LinkInfo};
use crate::host::confirm::shown_text;

/// The file in Kai's state directory that keeps the highest number Kai handed out.
pub const STORE: &str = "browser-refs";

/// What one of Kai's numbers stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    link: LinkId,
    /// The extension's own number.
    ext: i64,
    /// The tab and frame (0: the top document) the answer that named it gave.
    tab: Option<i64>,
    frame: i64,
    /// When Kai first held it ([`Refs::mark`]).
    seq: u64,
}

/// A link Kai has seen: its profile, kept once, and how many numbers Kai holds of it.
#[derive(Debug)]
struct Linked {
    profile: String,
    held: usize,
}

/// Numbers as sorted, disjoint, non-adjacent ranges (inclusive): a page's refs are mostly
/// consecutive, so retiring them takes a few entries.
#[derive(Debug, Default)]
struct Ranges(Vec<(i64, i64)>);

impl Ranges {
    fn contains(&self, n: i64) -> bool {
        let i = self.0.partition_point(|(lo, _)| *lo <= n);
        i > 0 && self.0[i - 1].1 >= n
    }

    fn insert(&mut self, lo: i64, hi: i64) {
        let (mut lo, mut hi) = (lo, hi);
        let start = self.0.partition_point(|(_, h)| h.saturating_add(1) < lo);
        let mut end = start;
        while end < self.0.len() && self.0[end].0 <= hi.saturating_add(1) {
            lo = lo.min(self.0[end].0);
            hi = hi.max(self.0[end].1);
            end += 1;
        }
        self.0.splice(start..end, [(lo, hi)]);
    }
}

/// Where Kai keeps the highest number it handed out.
#[derive(Debug)]
struct Store {
    path: PathBuf,
    written: i64,
    /// A failed write is logged once.
    warned: bool,
}

/// Kai's numbers (module doc).
#[derive(Debug, Default)]
pub struct Refs {
    held: HashMap<i64, Held>,
    by_ext: HashMap<(LinkId, i64), i64>,
    /// The links Kai has seen that are linked or hold numbers.
    links: HashMap<LinkId, Linked>,
    retired: Ranges,
    /// The highest number Kai has handed out.
    high: i64,
    seq: u64,
    store: Option<Store>,
}

impl Refs {
    /// Kai's numbers kept in `path` (module doc): every number up to the highest one an earlier
    /// Kai handed out is retired.
    pub fn stored(path: PathBuf) -> Self {
        let unread = |why: &dyn std::fmt::Display| {
            tracing::warn!(
                path = %path.display(),
                error = %why,
                "browser: Kai's record of the element refs it handed out does not read; a ref an \
                 earlier Kai handed out may name another element now"
            );
            0
        };
        let high = match std::fs::read_to_string(&path) {
            Ok(text) => text.trim().parse::<i64>().unwrap_or_else(|e| unread(&e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => unread(&e),
        }
        .max(0);
        let mut refs = Self {
            high,
            store: Some(Store {
                path,
                written: high,
                warned: false,
            }),
            ..Self::default()
        };
        if high > 0 {
            refs.retired.insert(1, high);
        }
        refs
    }

    /// Writes the highest number handed out, when it grew.
    fn persist(&mut self) {
        let high = self.high;
        let Some(s) = &mut self.store else {
            return;
        };
        if high <= s.written {
            return;
        }
        match write(&s.path, high) {
            Ok(()) => s.written = high,
            Err(e) if !s.warned => {
                s.warned = true;
                tracing::warn!(
                    path = %s.path.display(),
                    error = %e,
                    "browser: Kai cannot keep the highest element ref it handed out; after a \
                     restart a ref may name another element"
                );
            }
            Err(_) => {}
        }
    }

    /// Notices the links in `live` that are new since the last look: each retires what Kai held of
    /// earlier links of its profile (module doc). A link that is gone and holds nothing is
    /// forgotten (its id never comes again).
    fn update(&mut self, live: &[LinkInfo]) {
        for l in live {
            if self.links.contains_key(&l.id) {
                continue;
            }
            let earlier: HashSet<LinkId> = self
                .links
                .iter()
                .filter(|(_, k)| k.profile == l.profile)
                .map(|(id, _)| *id)
                .collect();
            self.links.insert(
                l.id,
                Linked {
                    profile: l.profile.clone(),
                    held: 0,
                },
            );
            if earlier.is_empty() {
                continue;
            }
            let gone: Vec<i64> = self
                .held
                .iter()
                .filter(|(_, h)| earlier.contains(&h.link))
                .map(|(n, _)| *n)
                .collect();
            for n in gone {
                self.retire(n);
            }
        }
        self.links
            .retain(|id, k| k.held > 0 || live.iter().any(|l| l.id == *id));
    }

    /// `n` is taken for good (module doc).
    fn retire(&mut self, n: i64) {
        if let Some(h) = self.held.remove(&n) {
            self.by_ext.remove(&(h.link, h.ext));
            if let Some(k) = self.links.get_mut(&h.link) {
                k.held = k.held.saturating_sub(1);
            }
        }
        self.retired.insert(n, n);
    }

    fn taken(&self, n: i64) -> bool {
        self.held.contains_key(&n) || self.retired.contains(n)
    }

    /// Kai's number for the extension's `ext` of `link` (in `tab` and `frame`), held from now on:
    /// the extension's own unless that is taken (module doc).
    fn number(&mut self, link: &LinkInfo, ext: i64, tab: Option<i64>, frame: i64) -> i64 {
        if let Some(n) = self.by_ext.get(&(link.id, ext)).copied() {
            if let Some(h) = self.held.get_mut(&n)
                && h.tab.is_none()
            {
                h.tab = tab;
            }
            return n;
        }
        let n = if ext < 1 || self.taken(ext) {
            self.high + 1
        } else {
            ext
        };
        self.high = self.high.max(n);
        self.seq += 1;
        self.held.insert(
            n,
            Held {
                link: link.id,
                ext,
                tab,
                frame,
                seq: self.seq,
            },
        );
        self.by_ext.insert((link.id, ext), n);
        self.links
            .entry(link.id)
            .or_insert_with(|| Linked {
                profile: link.profile.clone(),
                held: 0,
            })
            .held += 1;
        n
    }

    /// Kai's number for the extension's `ext` of `link`, held from now on.
    #[cfg(test)]
    pub fn kai(&mut self, live: &[LinkInfo], link: &LinkInfo, ext: i64) -> i64 {
        self.update(live);
        let n = self.number(link, ext, None, 0);
        self.persist();
        n
    }

    /// Every `ref` of `answer` (an operation's result from `link`) as Kai's number, in the
    /// answer's order, the extension's kept as `ext_ref` where they differ; how many differ.
    pub fn renumber(&mut self, live: &[LinkInfo], link: &LinkInfo, answer: &mut Value) -> usize {
        self.update(live);
        let tab = answer.pointer("/tab/id").and_then(Value::as_i64);
        let n = self.walk(link, tab, 0, answer);
        self.persist();
        n
    }

    /// [`Refs::renumber`]'s walk; serde_json's own depth limit bounds the recursion. An object's
    /// `frame` is the frame of what it holds.
    fn walk(&mut self, link: &LinkInfo, tab: Option<i64>, frame: i64, v: &mut Value) -> usize {
        match v {
            Value::Object(map) => {
                let frame = map.get("frame").and_then(Value::as_i64).unwrap_or(frame);
                let mut changed = 0;
                if let Some(ext) = map.get("ref").and_then(Value::as_i64) {
                    let n = self.number(link, ext, tab, frame);
                    if n != ext {
                        map.insert("ref".into(), n.into());
                        map.insert("ext_ref".into(), ext.into());
                        changed += 1;
                    }
                }
                for (key, child) in map.iter_mut() {
                    if key != "ref" && key != "ext_ref" {
                        changed += self.walk(link, tab, frame, child);
                    }
                }
                changed
            }
            Value::Array(items) => items
                .iter_mut()
                .map(|c| self.walk(link, tab, frame, c))
                .sum(),
            _ => 0,
        }
    }

    /// Where Kai's numbers stand now: [`Refs::closed`] spares the ones held after it.
    pub fn mark(&self) -> u64 {
        self.seq + 1
    }

    /// `link` listed its tabs (`open`), asked after [`Refs::mark`] gave `mark`: the numbers held
    /// before then of a tab the listing no longer has are retired (module doc).
    pub fn closed(&mut self, link: LinkId, open: &HashSet<i64>, mark: u64) {
        let gone: Vec<i64> = self
            .held
            .iter()
            .filter(|(_, h)| {
                h.link == link && h.seq < mark && h.tab.is_some_and(|t| !open.contains(&t))
            })
            .map(|(n, _)| *n)
            .collect();
        for n in gone {
            self.retire(n);
        }
    }

    /// The extension answered `code` for Kai's `n`: the numbers it shows gone are retired (module
    /// doc); any other code changes nothing.
    pub fn stale(&mut self, n: i64, code: &str) {
        let Some(h) = self.held.get(&n).copied() else {
            return;
        };
        let same_tab = |o: &Held| o.link == h.link && h.tab.is_some() && o.tab == h.tab;
        let gone: Vec<i64> = match code {
            "stale_tab" => self
                .held
                .iter()
                .filter(|(_, o)| same_tab(o))
                .map(|(m, _)| *m)
                .collect(),
            "stale_document" => self
                .held
                .iter()
                .filter(|(_, o)| {
                    same_tab(o) && o.ext <= h.ext && (h.frame == 0 || o.frame == h.frame)
                })
                .map(|(m, _)| *m)
                .collect(),
            "stale_element" | "unknown_ref" => Vec::new(),
            _ => return,
        };
        self.retire(n);
        for m in gone {
            self.retire(m);
        }
    }

    /// The profile `link` is of, as a line names it.
    fn profile(&self, link: LinkId) -> String {
        self.links
            .get(&link)
            .map(|k| shown_text(&k.profile).into_owned())
            .unwrap_or_default()
    }

    /// The link and the extension's number that Kai's `n` stands for, among the links `live`;
    /// `serves` is the operation the call starts with (module doc).
    pub fn find(
        &mut self,
        live: &[LinkInfo],
        n: i64,
        serves: &str,
    ) -> Result<(LinkInfo, i64), String> {
        self.update(live);
        let several = live.len() > 1;
        // Hint labels that show `n` while Kai numbers their element otherwise.
        let labels: Vec<(LinkId, i64)> = live
            .iter()
            .filter_map(|l| {
                self.by_ext
                    .get(&(l.id, n))
                    .filter(|k| **k != n)
                    .map(|k| (l.id, *k))
            })
            .collect();
        let labelled = |refs: &Self| {
            labels
                .iter()
                .map(|(l, k)| {
                    let at = if several {
                        format!(" in profile {}", refs.profile(*l))
                    } else {
                        String::new()
                    };
                    format!("the hint labelled {n}{at} is ref {k}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(h) = self.held.get(&n).copied() {
            let Some(l) = live.iter().find(|l| l.id == h.link) else {
                let mut e = format!(
                    "ref {n} is from the Firefox of profile {}, which is not linked to Kai now \
                     (Firefox closed, or Kai for Firefox stopped); its pages need reading again \
                     for new refs",
                    self.profile(h.link)
                );
                if !labels.is_empty() {
                    e.push_str(&format!("; {}", labelled(self)));
                }
                return Err(e);
            };
            if labels.is_empty() {
                return Ok((l.clone(), h.ext));
            }
            // Two things (module doc): the element Kai called `n` gets a new number.
            let fresh = self.high + 1;
            self.high = fresh;
            self.held.remove(&n);
            self.retired.insert(n, n);
            self.held.insert(fresh, h);
            self.by_ext.insert((h.link, h.ext), fresh);
            self.persist();
            let at = if several {
                format!(" in profile {}", self.profile(h.link))
            } else {
                String::new()
            };
            return Err(format!(
                "ref {n} names two elements, since hint labels show Kai for Firefox's own \
                 numbers: {}, and the element Kai's answers called ref {n}{at} is ref {fresh} \
                 from now on; call again with the ref meant",
                labelled(self)
            ));
        }
        if !labels.is_empty() {
            return Err(format!(
                "ref {n} is no number of Kai's now: {} (hint labels show Kai for Firefox's own \
                 numbers); call again with that ref",
                labelled(self)
            ));
        }
        if self.retired.contains(n) {
            return Err(format!(
                "ref {n} names no element Kai can reach now: its page was left or its tab \
                 closed, or Kai for Firefox, Firefox or Kai restarted since; call page_controls \
                 or hints_show for the page's refs now"
            ));
        }
        let serving: Vec<&LinkInfo> = live
            .iter()
            .filter(|l| l.ops.iter().any(|o| o == serves))
            .collect();
        match serving.as_slice() {
            [] => Err("no Firefox with Kai for Firefox is linked".into()),
            [one] => Ok(((*one).clone(), n)),
            several => Err(format!(
                "Kai has not seen ref {n} yet, and Firefox runs in {} linked profiles, so Kai \
                 cannot tell which page it is on; call page_controls or hints_show in its tab \
                 first",
                several.len()
            )),
        }
    }
}

/// Writes `high` to `path` whole: a file beside it, then renamed over it.
fn write(path: &Path, high: i64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{high}\n"))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DESCRIBE: &str = "element.describe@1";

    fn link(id: LinkId, profile: &str) -> LinkInfo {
        LinkInfo {
            id,
            profile: profile.into(),
            ext_version: "0.1.0".into(),
            firefox: "159.0".into(),
            protocol: 1,
            ops: vec![DESCRIBE.into()],
            firefox_pid: None,
        }
    }

    #[test]
    fn one_profile_keeps_the_extensions_numbers() {
        let a = link(1, "c0ffee");
        let live = [a.clone()];
        let mut r = Refs::default();
        let mut v = json!({"tab": {"id": 12}, "items": [{"ref": 3}, {"ref": 5, "level": 1}],
                           "modal": {"ref": 40}, "frames": {"read": 1, "unread": 0}});
        assert_eq!(r.renumber(&live, &a, &mut v), 0);
        assert_eq!(v["items"][1], json!({"ref": 5, "level": 1}));
        assert_eq!(r.find(&live, 5, DESCRIBE), Ok((a.clone(), 5)));
        // A number no answer named (a hint label drawn after a scroll) goes to the one link.
        assert_eq!(r.find(&live, 230, DESCRIBE), Ok((a.clone(), 230)));
    }

    #[test]
    fn two_profiles_share_one_number_space() {
        let a = link(1, "c0ffee");
        let b = link(2, "beef");
        let live = [a.clone(), b.clone()];
        let mut r = Refs::default();
        assert_eq!(r.kai(&live, &a, 5), 5);
        assert_eq!(r.kai(&live, &a, 6), 6);
        // b's 5 is Kai's 7, the next number above all Kai handed out; b's 9 stays 9.
        let mut v = json!({"controls": [{"ref": 5}, {"ref": 9}]});
        assert_eq!(r.renumber(&live, &b, &mut v), 1);
        assert_eq!(
            v,
            json!({"controls": [{"ref": 7, "ext_ref": 5}, {"ref": 9}]})
        );
        // a's own 7 then gets a number of Kai's too.
        assert_eq!(r.kai(&live, &a, 7), 10);
        assert_eq!(r.find(&live, 10, DESCRIBE), Ok((a.clone(), 7)));
        let e = r.find(&live, 300, DESCRIBE).unwrap_err();
        assert!(e.contains("Firefox runs in 2 linked profiles"), "{e}");
        // b's Firefox closes: its refs say so, and are never sent to a.
        let e = r.find(std::slice::from_ref(&a), 9, DESCRIBE).unwrap_err();
        assert!(
            e.starts_with("ref 9 is from the Firefox of profile beef"),
            "{e}"
        );
    }

    /// The design decision: a number that is Kai's for one element and a hint label of another is
    /// refused, and the two are numbered apart.
    #[test]
    fn a_number_that_is_also_a_renumbered_label_is_refused_and_split() {
        let a = link(1, "c0ffee");
        let b = link(2, "beef");
        let live = [a.clone(), b.clone()];
        let mut r = Refs::default();
        assert_eq!(r.kai(&live, &a, 5), 5);
        assert_eq!(r.kai(&live, &b, 5), 6);
        // beef's page shows the label 5, which is Kai's 6; Kai's 5 is c0ffee's.
        let e = r.find(&live, 5, DESCRIBE).unwrap_err();
        assert_eq!(
            e,
            "ref 5 names two elements, since hint labels show Kai for Firefox's own numbers: the \
             hint labelled 5 in profile beef is ref 6, and the element Kai's answers called ref 5 \
             in profile c0ffee is ref 7 from now on; call again with the ref meant"
        );
        assert_eq!(r.find(&live, 7, DESCRIBE), Ok((a.clone(), 5)));
        assert_eq!(r.find(&live, 6, DESCRIBE), Ok((b.clone(), 5)));
        // 5 is retired now: asked again, it is read as the labels it still is.
        let e = r.find(&live, 5, DESCRIBE).unwrap_err();
        assert!(
            e.starts_with(
                "ref 5 is no number of Kai's now: the hint labelled 5 in profile c0ffee is ref 7, \
                 the hint labelled 5 in profile beef is ref 6"
            ),
            "{e}"
        );
        // c0ffee's next answer names its 5 as Kai's 7.
        let mut v = json!({"tab": {"id": 1}, "controls": [{"ref": 5}]});
        r.renumber(&live, &a, &mut v);
        assert_eq!(v["controls"][0], json!({"ref": 7, "ext_ref": 5}));
    }

    #[test]
    fn a_new_link_of_a_profile_retires_its_earlier_refs() {
        let a = link(1, "c0ffee");
        let mut r = Refs::default();
        assert_eq!(r.kai(std::slice::from_ref(&a), &a, 214), 214);
        assert_eq!(r.kai(std::slice::from_ref(&a), &a, 215), 215);
        // The extension (or Firefox) restarted: the same profile links again.
        let a2 = link(2, "c0ffee");
        let live = [a2.clone()];
        let e = r.find(&live, 214, DESCRIBE).unwrap_err();
        assert!(
            e.starts_with("ref 214 names no element Kai can reach now"),
            "{e}"
        );
        // A restarted Firefox gives 214 out again, for another element: it is never Kai's 214.
        assert_eq!(r.kai(&live, &a2, 214), 216);
        assert_eq!(r.find(&live, 216, DESCRIBE), Ok((a2.clone(), 214)));
        assert!(r.find(&live, 215, DESCRIBE).is_err());
        // A number Kai never handed out is the new link's own.
        assert_eq!(r.kai(&live, &a2, 3), 3);
        assert_eq!(r.find(&live, 3, DESCRIBE), Ok((a2.clone(), 3)));
        assert_eq!(r.find(&live, 400, DESCRIBE), Ok((a2.clone(), 400)));
    }

    #[test]
    fn gone_tabs_documents_and_elements_retire_their_refs() {
        let a = link(1, "c0ffee");
        let live = [a.clone()];
        let mut r = Refs::default();
        let mut v = json!({"tab": {"id": 12},
                           "controls": [{"ref": 3}, {"ref": 4}, {"ref": 8, "frame": 9},
                                        {"ref": 10}]});
        r.renumber(&live, &a, &mut v);
        let mut w = json!({"tab": {"id": 14}, "controls": [{"ref": 5}, {"ref": 6}]});
        r.renumber(&live, &a, &mut w);
        // Tab 12's top document was left at ref 4: its refs up to 4 go, its frames' too; what
        // came later may be of a newer document and stays until it is found stale.
        r.stale(4, "stale_document");
        for n in [3, 4] {
            assert!(r.find(&live, n, DESCRIBE).is_err(), "{n}");
        }
        for n in [8, 10, 5] {
            assert!(r.find(&live, n, DESCRIBE).is_ok(), "{n}");
        }
        // A frame's document: that frame's refs up to it only.
        r.stale(8, "stale_document");
        assert!(r.find(&live, 8, DESCRIBE).is_err());
        assert!(r.find(&live, 10, DESCRIBE).is_ok());
        r.stale(10, "stale_element");
        assert!(r.find(&live, 10, DESCRIBE).is_err());
        // Tab 14 closed: a listing without it retires what was held before the listing was asked.
        let mark = r.mark();
        let mut x = json!({"tab": {"id": 14}, "controls": [{"ref": 20}]});
        r.renumber(&live, &a, &mut x);
        r.closed(1, &HashSet::from([12]), mark);
        assert!(r.find(&live, 5, DESCRIBE).is_err());
        assert!(r.find(&live, 6, DESCRIBE).is_err());
        assert!(
            r.find(&live, 20, DESCRIBE).is_ok(),
            "held after the listing was asked"
        );
        r.stale(20, "stale_tab");
        assert!(r.find(&live, 20, DESCRIBE).is_err());
        // Nothing is handed out twice, and the retired numbers take a few ranges.
        assert_eq!(r.kai(&live, &a, 5), 21);
        assert_eq!(r.held.len(), 1);
        assert_eq!(r.retired.0, [(3, 6), (8, 8), (10, 10), (20, 20)]);
    }

    #[test]
    fn ranges_merge() {
        let mut g = Ranges::default();
        for n in [5, 3, 4, 10, 8] {
            g.insert(n, n);
        }
        assert_eq!(g.0, [(3, 5), (8, 8), (10, 10)]);
        g.insert(9, 9);
        assert_eq!(g.0, [(3, 5), (8, 10)]);
        g.insert(1, 20);
        assert_eq!(g.0, [(1, 20)]);
        assert!(g.contains(1) && g.contains(20) && !g.contains(21) && !g.contains(0));
    }

    /// After Kai restarts, every number an earlier Kai handed out stays taken.
    #[test]
    fn the_highest_number_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join(STORE);
        let a = link(1, "c0ffee");
        let live = [a.clone()];
        let mut r = Refs::stored(path.clone());
        assert_eq!(r.kai(&live, &a, 214), 214);
        assert_eq!(r.kai(&live, &a, 7), 7);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "214\n");
        drop(r);
        let mut r = Refs::stored(path.clone());
        let e = r.find(&live, 7, DESCRIBE).unwrap_err();
        assert!(
            e.starts_with("ref 7 names no element Kai can reach now"),
            "{e}"
        );
        assert!(
            r.find(&live, 100, DESCRIBE).is_err(),
            "never handed out, but at or below the highest"
        );
        assert_eq!(r.find(&live, 215, DESCRIBE), Ok((a.clone(), 215)));
        // The same Firefox's elements get new numbers.
        assert_eq!(r.kai(&live, &a, 214), 215);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "215\n");
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(Refs::stored(path).high, 0);
    }
}
