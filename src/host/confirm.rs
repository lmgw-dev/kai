//! K15's confirmation until F (PD4): a GTK dialog like K13's, on the GTK thread (`crate::ui`).
//!
//! It shows the caller, the tool, the arguments in K22's compact lines and the time left from
//! `lmgw/timeout_ms`; Deny is the default and has the focus, and closing the window denies. The
//! host drops its end of the answer when lmgw cancels the call (`notifications/cancelled`), the
//! link ends or the time is up, and the dialog closes then (it waits on the answer's channel
//! closing). From WP19 a thread call carrying `lmgw/approval` is decided by F; the dialog stays for
//! callers without F.
//!
//! The arguments are scrolled, never shortened: their part of the dialog is at most as tall as the
//! smallest monitor's workarea leaves beside the rest of the dialog and a held MCP Apps view's strip
//! above and below ([`args_max_height`]), and the buttons stay outside it. What the dialog does is [`DialogModel`], tested here without a display;
//! `crate::ui` only lays it out.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::oneshot;

use crate::ui::UiCmd;

/// What the dialog shows, and where its answer goes (`true`: Allow). Its `Debug` leaves the
/// arguments out, so no log line can carry them.
pub struct ConfirmRequest {
    /// The tool as the caller sees it (`<label>__<name>`, or the name while no label is known).
    pub tool: String,
    /// The tool's one-line description.
    pub description: String,
    /// The caller, as [`super::policy::Caller::describe`] says it.
    pub caller: String,
    /// The arguments in K22's compact lines (`name: value`).
    pub lines: Vec<String>,
    /// The tool's annotations call it destructive (`destructiveHint`).
    pub destructive: bool,
    /// When lmgw stops waiting (`lmgw/timeout_ms` from the call's arrival); `None`: lmgw stated no
    /// time, and the dialog waits until the owner answers or lmgw cancels.
    pub deadline: Option<std::time::Instant>,
    /// The checkboxes the tool's preparation adds, each unticked ([`DialogOption`]).
    pub options: Vec<DialogOption>,
    /// The preparation's notes (`registry::Prepared::notes`), shown under the description.
    pub notes: Vec<String>,
    pub reply: oneshot::Sender<bool>,
}

/// A checkbox in the dialog, unticked when it opens: what the owner ticks is read by the call's
/// handler after Allow (`routine_enable`'s grant of what a routine asks, K28). It stays unticked
/// when no dialog is shown (the tool set to `allow`), so only the owner's own tick sets it.
#[derive(Debug, Clone)]
pub struct DialogOption {
    pub label: String,
    pub ticked: Arc<AtomicBool>,
}

impl DialogOption {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ticked: Arc::default(),
        }
    }

    pub fn is_ticked(&self) -> bool {
        self.ticked.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for ConfirmRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmRequest")
            .field("tool", &self.tool)
            .field("caller", &self.caller)
            .field("arguments", &self.lines.len())
            .field("deadline", &self.deadline)
            .field(
                "options",
                &self.options.iter().map(|o| &o.label).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// Asks the owner. A request that cannot be shown drops its `reply` (the call is then denied,
/// saying so).
pub trait Confirmer: Send + Sync + 'static {
    fn ask(&self, req: ConfirmRequest);
}

/// The GTK dialog (`UiCmd::ConfirmTool`).
pub struct Dialog(pub async_channel::Sender<UiCmd>);

impl Confirmer for Dialog {
    fn ask(&self, req: ConfirmRequest) {
        if self.0.try_send(UiCmd::ConfirmTool(req)).is_err() {
            tracing::warn!("host: the GTK thread is gone; the confirmation cannot be shown");
        }
    }
}

/// The dialog's text (the GTK side lays it out; tested here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogText {
    pub heading: String,
    pub description: String,
    /// A destructive tool's warning, in the warning colour.
    pub warning: Option<String>,
    /// The preparation's notes, each a line under the description (outside the scrolled part).
    pub notes: Vec<String>,
    /// The argument lines ("No arguments." without any), in the scrolled part.
    pub args: Vec<String>,
    /// The checkboxes' labels, below the arguments, one each ([`DialogOption`]).
    pub options: Vec<String>,
}

pub fn dialog_text(req: &ConfirmRequest) -> DialogText {
    DialogText {
        heading: format!("Let {} run {}?", req.caller, req.tool),
        description: req.description.clone(),
        warning: req
            .destructive
            .then(|| "It cannot simply be undone (the tool is marked destructive).".to_owned()),
        notes: req.notes.clone(),
        args: if req.lines.is_empty() {
            vec!["No arguments.".into()]
        } else {
            req.lines.clone()
        },
        options: req.options.iter().map(|o| o.label.clone()).collect(),
    }
}

/// The dialog's buttons, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Deny,
    Allow,
}

impl Button {
    pub const ALL: [Button; 2] = [Button::Deny, Button::Allow];
    /// The default button, which has the focus (PD4): Enter denies.
    pub const DEFAULT: Button = Button::Deny;

    pub fn label(self) -> &'static str {
        match self {
            Button::Deny => "Deny",
            Button::Allow => "Allow",
        }
    }
}

/// What the dialog's periodic look finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// The host's end is gone (lmgw cancelled the call, the time is up, the link ended) or the
    /// owner answered: the window closes.
    Close,
    /// Still asking: the time-left line (none without a deadline).
    Left(Option<String>),
}

/// What the dialog does, without GTK: the answer goes once, a button or the window's end gives it
/// (the window manager's close and Kai's quit deny), and it closes when the host stops waiting.
pub struct DialogModel {
    reply: Option<oneshot::Sender<bool>>,
    deadline: Option<std::time::Instant>,
    options: Vec<Arc<AtomicBool>>,
}

impl DialogModel {
    /// The text to show and the model behind it.
    pub fn new(req: ConfirmRequest) -> (DialogText, Self) {
        let text = dialog_text(&req);
        let model = Self {
            reply: Some(req.reply),
            deadline: req.deadline,
            options: req.options.into_iter().map(|o| o.ticked).collect(),
        };
        (text, model)
    }

    /// A button: its answer, once. The window closes after it either way.
    pub fn press(&mut self, button: Button) {
        self.answer(button == Button::Allow);
    }

    /// The window is gone (the window manager's close, Kai's quit): denied, unless answered.
    pub fn destroyed(&mut self) {
        self.answer(false);
    }

    /// Checkbox `index`, ticked or not; it counts only until the answer went.
    pub fn set_option(&mut self, index: usize, ticked: bool) {
        if let (Some(o), Some(_)) = (self.options.get(index), &self.reply) {
            o.store(ticked, Ordering::SeqCst);
        }
    }

    fn answer(&mut self, allow: bool) {
        if let Some(tx) = self.reply.take() {
            // A denied call's handler never runs; the box is cleared all the same.
            if !allow {
                for o in &self.options {
                    o.store(false, Ordering::SeqCst);
                }
            }
            let _ = tx.send(allow);
        }
    }

    pub fn tick(&self, now: std::time::Instant) -> Tick {
        match &self.reply {
            Some(tx) if !tx.is_closed() => Tick::Left(time_left(self.deadline, now)),
            _ => Tick::Close,
        }
    }
}

/// The most the arguments' scrolled part may take: what the smallest monitor's workarea leaves
/// beside `rest` (the rest of the dialog's natural height) and `reserve` above and below the
/// dialog, and never less than `one_line` (one argument line's), so it always shows a line to
/// scroll. `reserve` is what an MCP Apps view held for the dialog takes from the output's top or
/// bottom edge (`crate::widgets::held_reserve`): a dialog the compositor centres then leaves at
/// least that much on both sides, so the held strip cannot reach it (§10.1). `None` without a
/// monitor (GTK's natural size then).
pub fn args_max_height(workareas: &[i32], rest: i32, one_line: i32, reserve: i32) -> Option<i32> {
    let smallest = workareas.iter().copied().filter(|h| *h > 0).min()?;
    Some(
        smallest
            .saturating_sub(reserve.saturating_mul(2))
            .saturating_sub(rest)
            .max(one_line),
    )
}

/// Characters that would let a caller's text change how the dialog reads: a line break of any kind
/// (a forged line: the controls, U+2028, U+2029), and every character Unicode leaves unseen: the
/// format characters (General_Category Cf: the bidi controls of Trojan Source, U+200B, U+FEFF, the
/// tags U+E0000–U+E007F) and the Default_Ignorable_Code_Points (U+00AD, U+034F, the variation
/// selectors, the Hangul fillers U+115F, U+1160, U+3164, U+FFA0, and the unassigned ignorables),
/// by the properties, not a list of Kai's own.
fn deceptive(c: char) -> bool {
    use icu_properties::props::{DefaultIgnorableCodePoint, GeneralCategory};
    use icu_properties::{CodePointMapData, CodePointSetData};
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}')
        || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
        || CodePointMapData::<GeneralCategory>::new().get(c) == GeneralCategory::Format
}

/// The black flag a subdivision flag's tag sequence follows (🏴 then tags, UTS #51).
const BLACK_FLAG: char = '\u{1F3F4}';
/// The cancel tag that ends a subdivision flag's tag sequence.
const CANCEL_TAG: char = '\u{E007F}';

/// `s`'s characters, each with whether it is deceptive where it stands: [`deceptive`], except the
/// invisible parts of an emoji that render as that emoji (UTS #51), which change nothing about how
/// the text reads:
/// - a variation selector (U+FE0E, U+FE0F) right after an Extended_Pictographic character (❤️,
///   ⚠️, ✔️), or after a keycap base (0–9, #, *) and before the keycap U+20E3;
/// - a zero-width joiner between two emoji of a ZWJ sequence (👨‍👩‍👧): after an
///   Extended_Pictographic character (or one with its variation selector or skin tone) and before
///   another;
/// - a subdivision flag's tag sequence (🏴󠁧󠁢󠁳󠁣󠁴󠁿): U+1F3F4, then tag digits and small letters, then the
///   cancel tag, all of it.
///
/// Every other one (a lone U+FE0F, a joiner between letters, a stray tag) stays deceptive.
fn marked(s: &str) -> Vec<(char, bool)> {
    use icu_properties::CodePointSetData;
    use icu_properties::props::{EmojiModifier, ExtendedPictographic};
    let pictographic = |c: char| CodePointSetData::new::<ExtendedPictographic>().contains(c);
    let modifier = |c: char| CodePointSetData::new::<EmojiModifier>().contains(c);
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == BLACK_FLAG {
            let tags = chars[i + 1..]
                .iter()
                .take_while(|t| matches!(t, '\u{E0030}'..='\u{E0039}' | '\u{E0061}'..='\u{E007A}'))
                .count();
            if tags > 0 && chars.get(i + 1 + tags) == Some(&CANCEL_TAG) {
                out.extend(chars[i..=i + 1 + tags].iter().map(|c| (*c, false)));
                i += tags + 2;
                continue;
            }
        }
        let prev = |back: usize| i.checked_sub(back).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let emoji_part = match c {
            '\u{FE0E}' | '\u{FE0F}' => {
                prev(1).is_some_and(pictographic)
                    || (prev(1).is_some_and(|p| matches!(p, '0'..='9' | '#' | '*'))
                        && next == Some('\u{20E3}'))
            }
            '\u{200D}' => {
                let after_emoji = prev(1).is_some_and(pictographic)
                    || (prev(1).is_some_and(|p| p == '\u{FE0F}' || modifier(p))
                        && prev(2).is_some_and(pictographic));
                after_emoji && next.is_some_and(pictographic)
            }
            _ => false,
        };
        out.push((c, !emoji_part && deceptive(c)));
        i += 1;
    }
    out
}

/// `s` without its deceptive characters, for a place that has no quoting of its own (a journal
/// stream's identifier, a scope's description).
pub fn plain(s: &str) -> String {
    marked(s)
        .into_iter()
        .filter(|(_, d)| !d)
        .map(|(c, _)| c)
        .collect()
}

/// A caller's text as the dialog shows it: as it is when it holds no deceptive character, else
/// JSON-quoted with every such character escaped (`\n`, `\u202e`), so it cannot forge a line or
/// reorder one. Never shortened.
pub fn shown_text(s: &str) -> std::borrow::Cow<'_, str> {
    if marked(s).iter().any(|(_, d)| *d) {
        quoted(s).into()
    } else {
        s.into()
    }
}

/// `s` as a JSON string, every deceptive character escaped (serde_json leaves the bidi and
/// format characters as they are).
pub fn quoted(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for (c, deceptive) in marked(s) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if deceptive => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{u:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The time-left line: "38 s left (lmgw stops waiting then)", or none without a deadline.
pub fn time_left(deadline: Option<std::time::Instant>, now: std::time::Instant) -> Option<String> {
    let left = deadline?.saturating_duration_since(now);
    Some(format!(
        "{} s left (lmgw stops waiting then)",
        left.as_secs_f64().ceil() as u64
    ))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// The preparation's notes (WP16c's remote-control line) are the dialog's, in order.
    #[test]
    fn the_preparations_notes_are_shown() {
        let (reply, _rx) = oneshot::channel();
        let req = ConfirmRequest {
            tool: "kai__type_text".into(),
            description: "Types text.".into(),
            caller: "the owner".into(),
            lines: vec!["receives it: a".into()],
            destructive: false,
            deadline: None,
            options: Vec::new(),
            notes: vec!["one".into(), "two".into()],
            reply,
        };
        let text = dialog_text(&req);
        assert_eq!(text.notes, ["one", "two"]);
        assert_eq!(text.args, ["receives it: a"]);
    }

    #[test]
    fn the_dialog_says_who_what_and_with_which_arguments() {
        let (reply, _rx) = oneshot::channel();
        let req = ConfirmRequest {
            tool: "kai__clipboard_set".into(),
            description: "Put text on the clipboard.".into(),
            caller: "key 'laptop'".into(),
            lines: vec!["text: hello".into()],
            destructive: false,
            deadline: None,
            options: Vec::new(),
            notes: Vec::new(),
            reply,
        };
        let text = dialog_text(&req);
        assert!(
            !format!("{req:?}").contains("hello"),
            "no argument in its Debug"
        );
        assert_eq!(text.heading, "Let key 'laptop' run kai__clipboard_set?");
        assert_eq!(text.description, "Put text on the clipboard.");
        assert_eq!(
            (text.warning, text.args),
            (None, vec!["text: hello".into()])
        );
        let req = ConfirmRequest {
            destructive: true,
            lines: vec![],
            ..req
        };
        let text = dialog_text(&req);
        assert!(text.warning.unwrap().contains("destructive"));
        assert_eq!(text.args, ["No arguments."]);
        let now = Instant::now();
        assert_eq!(
            time_left(Some(now + Duration::from_millis(37_200)), now).unwrap(),
            "38 s left (lmgw stops waiting then)"
        );
        assert_eq!(
            time_left(Some(now), now + Duration::from_secs(1)).unwrap(),
            "0 s left (lmgw stops waiting then)"
        );
        assert_eq!(time_left(None, now), None);
    }

    /// What a caller's text cannot hide or reorder: by Unicode's properties, so the tags, the
    /// variation selectors, the soft hyphen, the combining grapheme joiner and the Hangul fillers
    /// are escaped as the bidi controls are; ordinary letters, marks and emoji are not.
    #[test]
    fn every_invisible_or_format_character_is_escaped() {
        for c in [
            '\u{0007}',
            '\n',
            '\u{0085}',
            '\u{2028}',
            '\u{2029}',
            '\u{061C}',
            '\u{180E}',
            '\u{200B}',
            '\u{200D}',
            '\u{202E}',
            '\u{2066}',
            '\u{FEFF}',
            '\u{FFF9}',
            '\u{00AD}',
            '\u{034F}',
            '\u{115F}',
            '\u{1160}',
            '\u{3164}',
            '\u{FFA0}',
            '\u{FE0F}',
            '\u{E0001}',
            '\u{E0041}',
            '\u{E007F}',
            '\u{E0100}',
            '\u{0600}',
            '\u{110BD}',
            '\u{1D173}',
        ] {
            assert!(deceptive(c), "U+{:04X}", c as u32);
            assert!(
                matches!(shown_text(&format!("a{c}b")), std::borrow::Cow::Owned(_)),
                "U+{:04X}",
                c as u32
            );
        }
        for c in [
            'a', 'ß', 'ä', '\u{0301}', '€', '😀', ' ', '\u{3042}', '\u{05D0}',
        ] {
            assert!(!deceptive(c), "U+{:04X}", c as u32);
        }
        // A tag sequence is escaped as UTF-16 surrogates, in JSON's way.
        assert_eq!(quoted("a\u{E0041}"), "\"a\\udb40\\udc41\"");
    }

    /// L3: the invisible parts of an emoji are shown as the emoji, not escaped (the dialog's and
    /// the model's text alike): a variation selector after its base, a keycap, a ZWJ sequence, a
    /// subdivision flag. The same characters anywhere else stay escaped.
    #[test]
    fn emoji_sequences_are_shown_as_they_are() {
        for s in [
            "I \u{2764}\u{FE0F} it",
            "\u{26A0}\u{FE0F} careful",
            "done \u{2714}\u{FE0F}",
            "\u{2764}\u{FE0E}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F} Scotland",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
            "\u{1F44B}\u{1F3FD}\u{200D}\u{2642}\u{FE0F}",
            "\u{1F1E9}\u{1F1EA}",
        ] {
            assert!(
                matches!(shown_text(s), std::borrow::Cow::Borrowed(_)),
                "{s:?}"
            );
            assert_eq!(plain(s), s);
            assert_eq!(quoted(s), format!("\"{s}\""));
        }
        for s in [
            "a\u{FE0F}b",
            "\u{FE0F}",
            "\u{FE0F}\u{2764}",
            "1\u{FE0F}",
            "a\u{200D}b",
            "1\u{200D}2",
            "\u{1F468}\u{200D}",
            "\u{200D}\u{1F468}",
            // A tag sequence without its base, without its cancel tag, or with a capital tag.
            "\u{E0067}\u{E0062}\u{E007F}",
            "\u{1F3F4}\u{E0067}\u{E0062}",
            "\u{1F3F4}\u{E0047}\u{E007F}",
            "\u{1F3F4}\u{E007F}",
        ] {
            assert!(matches!(shown_text(s), std::borrow::Cow::Owned(_)), "{s:?}");
        }
        // Only the stray part is escaped: the emoji beside it stays.
        assert_eq!(
            quoted("\u{2764}\u{FE0F}\u{FE0F}"),
            "\"\u{2764}\u{FE0F}\\ufe0f\""
        );
        assert_eq!(plain("a\u{200D}b\u{2764}\u{FE0F}"), "ab\u{2764}\u{FE0F}");
    }

    fn request(deadline: Option<Instant>) -> (ConfirmRequest, oneshot::Receiver<bool>) {
        let (reply, rx) = oneshot::channel();
        let req = ConfirmRequest {
            tool: "kai__poke".into(),
            description: "Change a thing.".into(),
            caller: "the owner".into(),
            lines: vec![],
            destructive: false,
            deadline,
            options: Vec::new(),
            notes: Vec::new(),
            reply,
        };
        (req, rx)
    }

    /// PD4's dialog without a display: Deny is the default; a button answers once; the window
    /// manager's close denies; the host's end closing (a cancel, the timeout) closes the window.
    #[test]
    fn the_dialog_answers_once_and_closes_when_the_host_stops_waiting() {
        assert_eq!(Button::DEFAULT, Button::Deny);
        assert_eq!(Button::ALL[0], Button::DEFAULT, "Deny comes first");
        assert_eq!(
            Button::ALL.map(Button::label),
            ["Deny", "Allow"],
            "the labels"
        );

        let now = Instant::now();
        // Allow, then the window closes: one answer, and it is Allow.
        let (req, mut rx) = request(Some(now + Duration::from_secs(30)));
        let (_, mut m) = DialogModel::new(req);
        assert_eq!(
            m.tick(now),
            Tick::Left(Some("30 s left (lmgw stops waiting then)".into()))
        );
        m.press(Button::Allow);
        m.destroyed();
        assert_eq!(rx.try_recv(), Ok(true));
        assert_eq!(m.tick(now), Tick::Close, "answered: the window goes");

        // The window manager's close: denied.
        let (req, mut rx) = request(None);
        let (_, mut m) = DialogModel::new(req);
        assert_eq!(m.tick(now), Tick::Left(None), "no deadline: no time line");
        m.destroyed();
        assert_eq!(rx.try_recv(), Ok(false));

        // Deny.
        let (req, mut rx) = request(None);
        let (_, mut m) = DialogModel::new(req);
        m.press(Button::DEFAULT);
        assert_eq!(rx.try_recv(), Ok(false));

        // lmgw cancelled the call, or its time is up: the host drops its end, the dialog closes,
        // and a late press goes nowhere.
        let (req, rx) = request(Some(now + Duration::from_secs(30)));
        let (_, mut m) = DialogModel::new(req);
        drop(rx);
        assert_eq!(m.tick(now), Tick::Close);
        m.press(Button::Allow);
        m.destroyed();
    }

    /// The checkbox: unticked when it opens, the owner's tick stands with Allow, a denial clears
    /// it, and nothing sets it after the answer.
    #[test]
    fn the_checkbox_counts_with_allow_only() {
        let with = |label: &str| {
            let (req, rx) = request(None);
            let o = DialogOption::new(label);
            let req = ConfirmRequest {
                options: vec![o.clone()],
                ..req
            };
            (req, rx, o)
        };
        let (req, mut rx, o) = with("Also grant it");
        assert_eq!(dialog_text(&req).options, ["Also grant it"]);
        let (_, mut m) = DialogModel::new(req);
        assert!(!o.is_ticked(), "unticked when it opens");
        m.set_option(0, true);
        m.press(Button::Allow);
        assert_eq!(rx.try_recv(), Ok(true));
        assert!(o.is_ticked());
        m.set_option(0, false);
        assert!(o.is_ticked(), "answered: the box no longer changes");

        let (req, mut rx, o) = with("Also grant it");
        let (_, mut m) = DialogModel::new(req);
        m.set_option(0, true);
        m.destroyed();
        assert_eq!(rx.try_recv(), Ok(false));
        assert!(!o.is_ticked(), "a denial clears it");
        let (req, _rx) = request(None);
        assert!(dialog_text(&req).options.is_empty());
    }

    /// Several boxes are independent: each tick is read on its own, a denial clears all.
    #[test]
    fn each_box_is_its_own() {
        let (req, mut rx) = request(None);
        let (a, b) = (DialogOption::new("a"), DialogOption::new("b"));
        let req = ConfirmRequest {
            options: vec![a.clone(), b.clone()],
            ..req
        };
        assert_eq!(dialog_text(&req).options, ["a", "b"]);
        let (_, mut m) = DialogModel::new(req);
        m.set_option(1, true);
        m.set_option(5, true);
        m.press(Button::Allow);
        assert_eq!(rx.try_recv(), Ok(true));
        assert!(!a.is_ticked() && b.is_ticked());
    }

    #[test]
    fn the_arguments_scroll_within_the_smallest_workarea() {
        // Two monitors: the smaller one's workarea bounds it, less the rest of the dialog.
        assert_eq!(args_max_height(&[1400, 1032], 300, 20, 0), Some(732));
        // A rest taller than the workarea still leaves one line to scroll.
        assert_eq!(args_max_height(&[600], 900, 20, 0), Some(20));
        assert_eq!(args_max_height(&[], 300, 20, 0), None, "no monitor known");
        assert_eq!(args_max_height(&[0], 300, 20, 0), None);
        // A held view's strip (24 px) and its margin (24 px) are kept free above and below.
        assert_eq!(args_max_height(&[1400, 1032], 300, 20, 48), Some(636));
        assert_eq!(args_max_height(&[400], 300, 20, 48), Some(20));
    }

    #[test]
    fn a_centred_dialog_at_its_largest_leaves_the_held_strip_free() {
        // The held strip spans [margin, margin + strip) from the top edge, or the same from the
        // bottom; the dialog is centred. Wherever one argument line still fits beside the reserve,
        // the dialog at its largest misses the strip on both edges.
        for area in [480, 600, 768, 1032, 1080, 1400, 2160] {
            for margin in [0, 8, 24, 100] {
                let strip = crate::widgets::view::STRIP_PX as i32;
                let reserve = strip + margin;
                for rest in [120, 300, 500] {
                    let one_line = 20;
                    let Some(args) = args_max_height(&[area, area + 200], rest, one_line, reserve)
                    else {
                        panic!("a monitor is known");
                    };
                    let dialog = rest + args;
                    if rest + one_line > area - 2 * reserve {
                        // The rest alone does not fit: the dialog is one line, the known limit.
                        assert_eq!(args, one_line);
                        continue;
                    }
                    let top = (area - dialog) / 2;
                    let bottom = top + dialog;
                    assert!(top >= margin + strip, "{area} {margin} {rest}: top {top}");
                    assert!(
                        bottom <= area - margin - strip,
                        "{area} {margin} {rest}: bottom {bottom}"
                    );
                }
            }
        }
    }
}
