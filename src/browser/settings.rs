//! `[browser]` (`crate::config` holds the field): Kai for Firefox's link (kai-firefox design §4.4).
//!
//! ```toml
//! [browser]
//! # browser.sock takes a connection only from a process in Firefox's own systemd unit: one of
//! # these patterns must match the unit the peer runs in, the innermost .service or .scope of its
//! # cgroup path (* any text, ? one character, everything else literal; no /). A pattern that
//! # would also match user@<uid>.service or a slice above every app (app.slice, user.slice …) is
//! # refused. Read again at every connection, so a change applies to the next one. The defaults
//! # are what Plasma names a Firefox it starts (module doc of crate::browser::peer); a Firefox
//! # started from a terminal runs in the terminal's unit and is refused, logged with its
//! # executable and cgroup once. An empty list lets no Firefox in.
//! peer_scopes = [
//!   "app-org.mozilla.firefox@*.service",
//!   "app-org.mozilla.firefox-*.scope",
//!   "app-firefox@*.service",
//!   "app-firefox-*.scope",
//!   'app-userapp\x2dFirefox\x2d*',
//! ]
//! # The hands-and-eyes switch (design §6.4, "Kai may act in Firefox without asking"): the
//! # state-changing tools of tabs, acting and reading aloud (all but attaching a file) run unasked
//! # for trusted callers, as policy.tools = "allow_trusted" would make them, where policy.tools
//! # sets no state of its own for them; the tools that exist only for hands and eyes are listed.
//! # Takes effect at once (the host lists its tools again).
//! hands_and_eyes = false
//! # The focus rule (§6.4): a tool that acts on a page follows its state only on the user's tab
//! # while the user brought it to the front and on the site it had then; anywhere else it is
//! # confirmed. true: it follows its state on every tab.
//! act_in_background = false
//! # element_point's ring (§5.4) shows this long, fading over its last third; at most 2147483647
//! # (a page's timers take no longer). Takes effect at once.
//! point_ms = 3000
//! # page_text on a PDF (§5.3) runs poppler's pdftotext on the PDF's bytes; a run still going after
//! # this long is stopped and the call fails, naming this setting. Takes effect at once.
//! pdf_timeout_s = 60
//! # The most text one pdftotext run may write, in MB (MiB): a run that writes more (a malformed or
//! # hostile PDF's text can be far larger than the PDF) is stopped and the call fails, naming this
//! # setting. Takes effect at once.
//! pdf_text_max_mb = 1024
//! # The address space one pdftotext run may use, in MB (MiB; RLIMIT_AS): a PDF that asks for more
//! # fails alone, and the call names this setting, instead of the kernel's OOM killer picking a
//! # victim. Takes effect at once.
//! pdf_memory_max_mb = 4096
//! # The commit guard (§6.4): before a press or a submit whose element's name, form button or form
//! # action holds one of guard_words, before a form sent to another site, and before tab_go or
//! # tab_open goes to a site the user's history has never seen, Kai asks; for guard_window_ms after
//! # each action the page's POST, PUT, PATCH and DELETE requests wait for the owner's answer (method,
//! # URL and body shown), cancelled when nobody answers before the call's time is up. false: none
//! # of it. Takes effect at once.
//! commit_guard = true
//! # The words are stems Kai sends the extension, which matches them in what the element pressed
//! # says (its name, the text it shows, a button's value or title), in its form's submit button and
//! # in the form's action, case ignored and NFKC-normalised: a word of four or more letters matches
//! # anywhere, also inside a word (bestell: Bestellung, zahlungspflichtig bestellen), a shorter one
//! # where a word starts (buy in QuickBuy, not in Ebuy). [] lets no word ask; a form sent to another
//! # site, an Enter in an editor and the requests still do. Takes effect at once.
//! guard_words = ["buy", "order", "pay", "checkout", "purchase", "subscribe", "delete", "remove",
//!                "send", "transfer", "confirm", "kauf", "bestell", "bezahl", "zahlung", "zahlen",
//!                "kasse", "abo", "lösch", "entfern", "überweis", "bestätig"]
//! # How long after an action the page's writes wait for the owner, in ms; 0: only what the action
//! # sends while it runs. At most 2147483647. Takes effect at once.
//! guard_window_ms = 2000
//! # After Kai's own keys (type_text, key_press, ui_press) Firefox still counts input as Kai's for this
//! # long, for keys on their way to it (the extension's input spans, §6.3); 0: until Kai's last key
//! # went. At most 2147483647. Takes effect at once.
//! input_settle_ms = 1000
//! # While some site is none: a tab brought to the front of a Firefox window this long before
//! # see_window or see_screen looks at the window, or a tab still loading, counts as a page Kai
//! # cannot tell yet (the strictest rule), since the window's pixels can still show the page
//! # before; in ms. At most 2147483647. Takes effect at once.
//! see_settle_ms = 500
//! # element_press on an element that needs a real key (a link that opens a new window, a media or
//! # full-screen control): Kai brings Firefox's window to the front, checks that the element has
//! # the keyboard focus and presses Enter or Space through the desktop (§6.3). "always" sends every
//! # press that way, for sites that ignore synthetic clicks. Takes effect at once.
//! trusted_press = "when_needed"
//! # Announcements (§7.2): Kai says what changed in your tab (loaded, a dialog, an error at a
//! # field, what a live region says, a finished download, where the focus went) in its own voice
//! # through lmgw's text-to-speech, with no model turn and nothing written into a thread. Page
//! # text is always said as the page's. They wait for Kai's own speech and never interrupt it, and
//! # are spoken only while the screen is unlocked and Firefox has the focused window or
//! # conversation mode is on. The hands-and-eyes switch turns them on too. Takes effect at once.
//! announce = false
//! # A live region or a field's error is announced once it has been quiet this long, its latest
//! # text only (a ticker, a countdown); at most 2147483647. Takes effect at once.
//! announce_quiet_ms = 1000
//! # browser_context lists the announcements since its previous call; one older than this is
//! # dropped from that list, and the list says how many were. Takes effect at once.
//! announce_keep_s = 600
//! # The most characters of page text one announcement speaks (a whole chat log can change at
//! # once); the rest is not spoken, and the announcement says how much was left. 0: all of it.
//! # browser_context always has the whole text. Takes effect at once.
//! announce_max_chars = 600
//! # An announcement that could not be spoken within this long (Kai was speaking, do not disturb
//! # was on, PipeWire was down) is dropped unspoken when its turn comes; browser_context counts
//! # them. Takes effect at once.
//! announce_max_age_s = 60
//! # How long lmgw may take to make the speech of one sentence of an announcement or a paragraph
//! # (a text-to-speech model that is not loaded loads first); past it that speech fails: an
//! # announcement is not spoken (browser_context says why), a reading pauses and says so. Takes
//! # effect at Kai's next start.
//! speech_timeout_s = 60
//! # Kai speaks by itself (announcements, reading aloud) only after nothing else has gone on this
//! # long (its reply, your speech, a tool round of its turn, an approval), so a short pause between
//! # a turn's tool rounds does not start one that the next reply cuts again. Takes effect at Kai's
//! # next start.
//! speech_quiet_ms = 1500
//! # In continuous listening, after that speech stops the microphone sends lmgw silence this long,
//! # so its echo cannot start a turn of yours; what you say while it plays or within this long
//! # stops it and is spent on the stop: say what you want after it. Takes effect at Kai's next
//! # start.
//! speech_echo_guard_ms = 500
//! # Firefox's sidebar (§8.2): how much of one answer from lmgw Kai sends the sidebar ahead of what
//! # it took in, in MB (MiB). Firefox's native messaging has no backpressure, so beyond this Kai
//! # waits for the sidebar and reads no more of the answer from lmgw meanwhile: a large download
//! # or a fast stream never piles up in Firefox's memory. One piece (at most 1 MiB, Firefox's limit
//! # for a message) always goes. 0: no bound. Takes effect at the next request.
//! sidebar_ahead_mb = 4
//!
//! [browser.read]
//! # Reading aloud (§7.3): the speech clean-up rules each paragraph goes through before it is
//! # spoken (Kai for Firefox's, with their defaults); none drops a sentence.
//! hyphens = true      # line breaks inside hyphenated words, soft hyphens, zero-width characters
//! markdown = true     # **bold**, _italic_, `code`, # headings, bullets, table pipes, [text](url)
//! citations = true    # reference marks: [23], [12–15], [^1], [citation needed], [edit]
//! superscripts = false  # ¹²³: off, since they carry meaning in units (m², cm³)
//! urls = true         # URLs and e-mail addresses, which text-to-speech spells out
//! symbols = true      # emoji, ★ ▪ • bullets, arrows, box drawing
//! # The voice for a page's language, one entry each: the language (a code such as de or pt-BR;
//! # pt matches pt-BR too), lmgw's text-to-speech alias, and optionally a voice of it. A paragraph
//! # in a language without an entry is read in Kai's own voice (its conversation's).
//! voices = []         # e.g. ["de tts-german", "fr kokoro ff_siwis"]
//!
//! [browser.sites]
//! # Site rules (§6.4), each entry a site: a host (mail.example), *.domain for the domain and
//! # every subdomain, * for every host, or with a scheme and a port (https://shop.example:8443);
//! # no path. The most specific entry that matches a tab's URL decides (a host over *.domain, a
//! # longer domain over a shorter one, a scheme or a port over none); among equally specific ones
//! # the strictest. A site in no list is act's.
//! act = []            # Kai may read and act there (exceptions to a wider read or none)
//! read = []           # Kai may read, every tool that acts on the page is refused
//! none = []           # no tool reaches it: Kai does not read, act or name its tabs
//! ```

use serde::{Deserialize, Serialize};

/// The longest ring `element.point@1` draws, in ms: the page's timers take at most 2^31 - 1 ms.
pub const POINT_MS_MAX: u64 = (1 << 31) - 1;

/// The most the contract takes for a time in ms: the commit guard's `window_ms` and `answer_ms`,
/// `input.end@1`'s `settle_ms` (2^31 - 1, a page's timer's).
pub const CONTRACT_MS_MAX: u64 = (1 << 31) - 1;

/// The key of the patterns, read again at every connection (`crate::settings` says it takes effect
/// at once).
pub const PEER_SCOPES_KEY: &str = "browser.peer_scopes";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BrowserConfig {
    /// The systemd units (patterns) whose processes may link (module doc).
    pub peer_scopes: Vec<String>,
    /// The hands-and-eyes switch (module doc).
    pub hands_and_eyes: bool,
    /// The focus rule's exception (module doc).
    pub act_in_background: bool,
    /// How long `element_point`'s ring shows, in ms (module doc).
    pub point_ms: u64,
    /// How long `pdftotext` may run on a PDF, in seconds (module doc).
    pub pdf_timeout_s: u64,
    /// The most text one `pdftotext` run may write, in MiB (module doc).
    pub pdf_text_max_mb: u64,
    /// The address space one `pdftotext` run may use, in MiB (module doc).
    pub pdf_memory_max_mb: u64,
    /// The commit guard (module doc).
    pub commit_guard: bool,
    /// The commit guard's words (module doc).
    pub guard_words: Vec<String>,
    /// How long after an action the network gate holds the page's writes, in ms (module doc).
    pub guard_window_ms: u64,
    /// How long Firefox still counts input as Kai's after Kai's own keys, in ms (module doc).
    pub input_settle_ms: u64,
    /// How long a tab brought to the front counts as not settled for the desktop's captures, in
    /// ms (module doc, `super::shown`).
    pub see_settle_ms: u64,
    /// When `element_press` takes the trusted path (module doc).
    pub trusted_press: TrustedPress,
    /// Announcements (module doc).
    pub announce: bool,
    /// The quiet period of live regions and fields' errors, in ms (module doc).
    pub announce_quiet_ms: u64,
    /// How long `browser_context` keeps an announcement, in seconds (module doc).
    pub announce_keep_s: u64,
    /// The most characters of page text one announcement speaks; 0: all (module doc).
    pub announce_max_chars: u64,
    /// An announcement older than this when its turn comes is dropped, in seconds (module doc).
    pub announce_max_age_s: u64,
    /// How long lmgw may take to make one sentence's speech, in seconds (module doc).
    pub speech_timeout_s: u64,
    /// The quiet time before Kai speaks by itself, in ms (module doc).
    pub speech_quiet_ms: u64,
    /// The microphone's silence after Kai's speech stops, in ms (module doc).
    pub speech_echo_guard_ms: u64,
    /// How much of one answer Kai sends Firefox's sidebar ahead of its acknowledgements, in MiB;
    /// 0: no bound (module doc, `super::sidebar::reply`).
    pub sidebar_ahead_mb: u64,
    /// Reading aloud (module doc).
    pub read: ReadAloud,
    /// The site rules (module doc, `super::sites`).
    pub sites: Sites,
}

/// `[browser.read]`: reading aloud's speech clean-up rules (`read.paragraph@1`'s `rules`, with
/// the contract's defaults) and the voices for a page's languages (module doc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReadAloud {
    pub hyphens: bool,
    pub markdown: bool,
    pub citations: bool,
    pub superscripts: bool,
    pub urls: bool,
    pub symbols: bool,
    /// `<language> <tts alias> [<voice>]` each (module doc).
    pub voices: Vec<String>,
}

impl Default for ReadAloud {
    fn default() -> Self {
        Self {
            hyphens: true,
            markdown: true,
            citations: true,
            superscripts: false,
            urls: true,
            symbols: true,
            voices: Vec::new(),
        }
    }
}

impl ReadAloud {
    /// The rules as `read.paragraph@1` takes them, every one named.
    pub fn rules(&self) -> serde_json::Value {
        serde_json::json!({
            "hyphens": self.hyphens,
            "markdown": self.markdown,
            "citations": self.citations,
            "superscripts": self.superscripts,
            "urls": self.urls,
            "symbols": self.symbols,
        })
    }
}

/// One entry of `browser.read.voices`: the voice for a language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangVoice {
    /// The language tag, lowercase (`de`, `pt-br`).
    pub lang: String,
    /// lmgw's text-to-speech alias.
    pub model: String,
    pub voice: Option<String>,
}

/// The entries of `browser.read.voices`, or the first that does not read and why.
pub fn lang_voices(entries: &[String]) -> Result<Vec<LangVoice>, (String, String)> {
    entries
        .iter()
        .map(|e| {
            let mut words = e.split_whitespace();
            let (Some(lang), Some(model)) = (words.next(), words.next()) else {
                return Err((
                    e.clone(),
                    "each entry is a language, lmgw's text-to-speech alias and optionally a \
                     voice, separated by spaces (de tts-german)"
                        .into(),
                ));
            };
            let voice = words.next().map(str::to_owned);
            if words.next().is_some() {
                return Err((
                    e.clone(),
                    "an entry has at most three words: the language, the alias and the voice"
                        .into(),
                ));
            }
            let tag_ok = lang.split('-').enumerate().all(|(i, part)| {
                let n = part.len();
                if i == 0 {
                    (2..=3).contains(&n) && part.chars().all(|c| c.is_ascii_alphabetic())
                } else {
                    (1..=8).contains(&n) && part.chars().all(|c| c.is_ascii_alphanumeric())
                }
            });
            if !tag_ok {
                return Err((
                    e.clone(),
                    format!(
                        "{lang:?} is no language tag: a code of two or three letters (de), \
                         optionally with a region (pt-BR)"
                    ),
                ));
            }
            Ok(LangVoice {
                lang: lang.to_ascii_lowercase(),
                model: model.to_owned(),
                voice,
            })
        })
        .collect()
}

/// The voice of `voices` for a paragraph in `lang` (an HTML `lang`: `en-US`, `de`): the entry of
/// the whole tag, else of a shorter one (`pt` for `pt-BR`); `None`: Kai's own voice.
pub fn voice_for<'a>(voices: &'a [LangVoice], lang: Option<&str>) -> Option<&'a LangVoice> {
    let mut tag = lang?.trim().to_ascii_lowercase().replace('_', "-");
    loop {
        if tag.is_empty() {
            return None;
        }
        if let Some(v) = voices.iter().find(|v| v.lang == tag) {
            return Some(v);
        }
        let i = tag.rfind('-')?;
        tag.truncate(i);
    }
}

/// `browser.trusted_press` (module doc).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedPress {
    /// Only where the extension says the element needs a real key (`needs_trusted`).
    #[default]
    WhenNeeded,
    /// Every press.
    Always,
}

/// The commit guard's words by default (design §6.4: buy, order, pay, checkout, purchase,
/// subscribe, delete, remove, send, transfer, confirm, in English and German), the list the
/// contract ships (`docs/protocol/operations.md`, Guard): the extension matches a word of four or
/// more letters anywhere in a text, also inside a word, and a shorter one where a word starts, so
/// the German entries are stems that cover a word's forms and compounds (`bestell`: bestellen,
/// Bestellung; `kauf`: kaufen, Einkaufswagen; `abo`: Abo, Abonnement).
pub const GUARD_WORDS: [&str; 22] = [
    "buy",
    "order",
    "pay",
    "checkout",
    "purchase",
    "subscribe",
    "delete",
    "remove",
    "send",
    "transfer",
    "confirm",
    "kauf",
    "bestell",
    "bezahl",
    "zahlung",
    "zahlen",
    "kasse",
    "abo",
    "lösch",
    "entfern",
    "überweis",
    "bestätig",
];

/// `[browser.sites]`: the sites of each rule (module doc).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sites {
    pub act: Vec<String>,
    pub read: Vec<String>,
    pub none: Vec<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            peer_scopes: super::peer::DEFAULT_SCOPES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            hands_and_eyes: false,
            act_in_background: false,
            point_ms: 3000,
            pdf_timeout_s: 60,
            pdf_text_max_mb: 1024,
            pdf_memory_max_mb: 4096,
            commit_guard: true,
            guard_words: GUARD_WORDS.iter().map(|w| (*w).to_owned()).collect(),
            guard_window_ms: 2000,
            input_settle_ms: 1000,
            see_settle_ms: 500,
            trusted_press: TrustedPress::WhenNeeded,
            announce: false,
            announce_quiet_ms: 1000,
            announce_keep_s: 600,
            announce_max_chars: 600,
            announce_max_age_s: 60,
            speech_timeout_s: 60,
            speech_quiet_ms: 1500,
            speech_echo_guard_ms: 500,
            sidebar_ahead_mb: 4,
            read: ReadAloud::default(),
            sites: Sites::default(),
        }
    }
}

/// The keys that take effect at once: the peer scopes at the next connection, the rest through the
/// host's policy (`crate::host::policy::Policy::of`), which the core builds again.
pub fn takes_effect_now(key: &str) -> bool {
    key == PEER_SCOPES_KEY
        || key == "browser.hands_and_eyes"
        || key == "browser.act_in_background"
        || key == "browser.point_ms"
        || key == "browser.pdf_timeout_s"
        || key == "browser.pdf_text_max_mb"
        || key == "browser.pdf_memory_max_mb"
        || key == "browser.commit_guard"
        || key == "browser.guard_words"
        || key == "browser.guard_window_ms"
        || key == "browser.input_settle_ms"
        || key == "browser.see_settle_ms"
        || key == "browser.trusted_press"
        || key == "browser.announce"
        || key == "browser.announce_quiet_ms"
        || key == "browser.announce_keep_s"
        || key == "browser.announce_max_chars"
        || key == "browser.announce_max_age_s"
        || key == "browser.sidebar_ahead_mb"
        || key.starts_with("browser.read.")
        || key.starts_with("browser.sites.")
}

impl BrowserConfig {
    /// `crate::config`'s check: a key, its value and the rule it breaks.
    pub fn validate(&self) -> Result<(), (&'static str, String, String)> {
        if let Some(bad) = self
            .peer_scopes
            .iter()
            .find(|p| p.trim().is_empty() || p.contains('/') || p.trim() != p.as_str())
        {
            return Err((
                PEER_SCOPES_KEY,
                format!("{bad:?}"),
                "each entry is a systemd unit's name or a pattern of one (* any text, ? one \
                 character), without spaces around it and without /"
                    .into(),
            ));
        }
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        for p in &self.peer_scopes {
            if let Some(rule) = super::peer::too_wide(p, uid) {
                return Err((PEER_SCOPES_KEY, format!("{p:?}"), rule));
            }
        }
        for (key, value) in [
            ("browser.guard_window_ms", self.guard_window_ms),
            ("browser.input_settle_ms", self.input_settle_ms),
            ("browser.see_settle_ms", self.see_settle_ms),
            ("browser.announce_quiet_ms", self.announce_quiet_ms),
            ("browser.speech_quiet_ms", self.speech_quiet_ms),
            ("browser.speech_echo_guard_ms", self.speech_echo_guard_ms),
        ] {
            if value > CONTRACT_MS_MAX {
                return Err((
                    key,
                    value.to_string(),
                    format!("must be from 0 to {CONTRACT_MS_MAX} (ms)"),
                ));
            }
        }
        if !(1..=POINT_MS_MAX).contains(&self.point_ms) {
            return Err((
                "browser.point_ms",
                self.point_ms.to_string(),
                format!("must be from 1 to {POINT_MS_MAX} (ms)"),
            ));
        }
        for (key, value, unit) in [
            ("browser.pdf_timeout_s", self.pdf_timeout_s, "s"),
            ("browser.pdf_text_max_mb", self.pdf_text_max_mb, "MB"),
            ("browser.pdf_memory_max_mb", self.pdf_memory_max_mb, "MB"),
            ("browser.announce_keep_s", self.announce_keep_s, "s"),
            ("browser.announce_max_age_s", self.announce_max_age_s, "s"),
            ("browser.speech_timeout_s", self.speech_timeout_s, "s"),
        ] {
            if value == 0 {
                return Err((key, "0".into(), format!("must be at least 1 ({unit})")));
            }
        }
        lang_voices(&self.read.voices)
            .map_err(|(entry, rule)| ("browser.read.voices", format!("{entry:?}"), rule))?;
        super::sites::SiteRules::read(&self.sites).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use std::path::Path;

    #[test]
    fn the_defaults_load_and_a_bad_pattern_is_named() {
        let c = Config::parse("", Path::new("kai.toml")).unwrap();
        assert_eq!(
            c.browser.peer_scopes.len(),
            super::super::peer::DEFAULT_SCOPES.len()
        );
        let c = Config::parse(
            "[browser]\npeer_scopes = ['app-userapp\\x2dFirefox\\x2d*']\n",
            Path::new("kai.toml"),
        )
        .unwrap();
        assert_eq!(c.browser.peer_scopes, ["app-userapp\\x2dFirefox\\x2d*"]);
        let c = Config::parse("[browser]\npeer_scopes = []\n", Path::new("kai.toml")).unwrap();
        assert!(c.browser.peer_scopes.is_empty());
        for bad in [
            "\"\"",
            "\"app.slice/app-firefox-*.scope\"",
            "\" app-firefox-*.scope\"",
        ] {
            let e = Config::parse(
                &format!("[browser]\npeer_scopes = [{bad}]\n"),
                Path::new("kai.toml"),
            )
            .unwrap_err()
            .to_string();
            assert!(e.contains("browser.peer_scopes"), "{e}");
            assert!(e.contains("systemd unit"), "{e}");
        }
    }

    /// A pattern that every program of the session would pass, through the user manager's unit
    /// or a slice above every app, is refused and says which it matches.
    #[test]
    fn a_pattern_that_matches_the_whole_session_is_refused() {
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        for (bad, says) in [
            ("*.service", format!("user@{uid}.service")),
            ("user@*", format!("user@{uid}.service")),
            ("*", format!("user@{uid}.service")),
            ("app*", "app.slice".into()),
            ("*slice", "user.slice".into()),
            ("session.*", "session.slice".into()),
            ("app-firefox.slice", "names a slice".into()),
            ("*.slice", "names a slice".into()),
        ] {
            let e = Config::parse(
                &format!("[browser]\npeer_scopes = [\"app-firefox-*.scope\", \"{bad}\"]\n"),
                Path::new("kai.toml"),
            )
            .unwrap_err()
            .to_string();
            assert!(e.contains("browser.peer_scopes"), "{bad}: {e}");
            assert!(e.contains(&says), "{bad}: {e}");
            assert!(e.contains("the unit Firefox runs in"), "{bad}: {e}");
        }
        // The defaults and patterns of one application pass.
        for good in ["app-org.kde.konsole-*.scope", "app-*@*.service"] {
            Config::parse(
                &format!("[browser]\npeer_scopes = [\"{good}\"]\n"),
                Path::new("kai.toml"),
            )
            .unwrap();
        }
    }
}
