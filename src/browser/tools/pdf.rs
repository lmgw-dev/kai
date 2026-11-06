//! `page_text` on a PDF in Firefox's viewer (kai-firefox design §3, §5.3): the extension cannot
//! read the viewer, so it fetches the PDF's bytes (`pdf.bytes@1`) and Kai extracts the text with
//! poppler's `pdftotext` (Kai's RPM requires `poppler-utils`), the approach of lmgw's
//! `extract/pdf.rs` (`run_piped_within`):
//! - the PDF goes to `pdftotext -enc UTF-8 - -` on stdin and the text comes back on stdout, so no
//!   file is written and no path is handed to poppler. The default mode reads in reading order;
//!   `-layout` would interleave columns;
//! - **the binary** is `/usr/bin/pdftotext`, the RPM's; only where that is missing one on `PATH`,
//!   which every error then names ([`Tool::find`]). It runs with a cleared environment but `PATH`
//!   and `LANG`, and in a sandbox where `bwrap` is installed: `bwrap --unshare-all
//!   --die-with-parent --new-session`, with `/usr` (and a binary outside it) bound read-only and
//!   nothing else of the system. Without `bwrap` it runs unsandboxed, said once in the log;
//! - **every run is bounded by settings**, and a run past one is stopped and the call fails, naming
//!   it ([`Limits`]): its time (`browser.pdf_timeout_s`), the text it writes, read as it comes
//!   (`browser.pdf_text_max_mb`), and its address space (`RLIMIT_AS`, set before it runs:
//!   `browser.pdf_memory_max_mb`), so a hostile PDF fails alone. The PDF is written from a task of
//!   its own while the output is read (a PDF larger than the pipe would otherwise deadlock
//!   against the output); stderr is drained, its first lines kept for the error and the rest
//!   counted. The run is killed when the call ends (`kill_on_drop`);
//! - pages are split on form feeds (a trailing one closes the last page);
//! - **text that does not read as text** (a font without a usable encoding gives private-use,
//!   control and replacement characters, or symbols for letters) is left out line by line, each
//!   run of such lines said where it was with how much and why ([`lines`]); a page without text
//!   says it has no text layer, a scan most likely. Dot leaders, `____` blanks, rules and a
//!   symbol font's bullet at a line's start do not count against a line ([`count`]).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::lines::page_line;

/// The extractor Kai's RPM installs.
pub const PDFTOTEXT: &str = "/usr/bin/pdftotext";

/// The sandbox.
pub const BWRAP: &str = "/usr/bin/bwrap";

const FORM_FEED: char = '\u{c}';

/// How many of a failed run's stderr lines its error shows; the rest are counted. Display only:
/// stderr is always drained, and the run's outcome never depends on it.
const STDERR_LINES_SHOWN: usize = 20;

/// How much of one shown stderr line is kept. Display only, as [`STDERR_LINES_SHOWN`].
const STDERR_LINE_BYTES: usize = 4096;

/// The bounds of one run, each a setting the refusal names (module doc).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// `browser.pdf_timeout_s`.
    pub time: Duration,
    /// `browser.pdf_text_max_mb`, in bytes.
    pub text_bytes: u64,
    /// `browser.pdf_memory_max_mb`, in bytes.
    pub memory_bytes: u64,
}

/// The `pdftotext` a run uses, and the sandbox it runs in (module doc).
#[derive(Debug, Clone)]
pub struct Tool {
    /// `None`: not installed.
    pub bin: Option<PathBuf>,
    /// Found on `PATH` because [`PDFTOTEXT`] is missing.
    pub on_path: bool,
    /// `bwrap`; `None`: unsandboxed.
    pub bwrap: Option<PathBuf>,
}

/// The log has said that `bwrap` is missing.
static UNSANDBOXED: AtomicBool = AtomicBool::new(false);

impl Tool {
    /// The RPM's `pdftotext`, else one on `PATH`, and `bwrap` where it is installed.
    pub fn find() -> Self {
        let usr = Path::new(PDFTOTEXT);
        let (bin, on_path) = if usr.is_file() {
            (Some(usr.to_path_buf()), false)
        } else {
            (on_path("pdftotext"), true)
        };
        Self::sandboxed(bin, on_path)
    }

    /// `bin` (a test's), in the sandbox where `bwrap` is installed.
    #[cfg(test)]
    pub fn at(bin: &Path) -> Self {
        Self::sandboxed(Some(bin.to_path_buf()), false)
    }

    fn sandboxed(bin: Option<PathBuf>, on_path: bool) -> Self {
        let bwrap = Path::new(BWRAP).is_file().then(|| PathBuf::from(BWRAP));
        if bwrap.is_none() && !UNSANDBOXED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "browser: {BWRAP} is not installed (the package bubblewrap), so pdftotext reads \
                 PDFs for page_text without a sandbox"
            );
        }
        Self {
            bin,
            on_path,
            bwrap,
        }
    }

    /// The binary as the errors name it.
    fn name(&self) -> String {
        match &self.bin {
            Some(b) if self.on_path => format!(
                "pdftotext (found on PATH at {}, as {PDFTOTEXT} is not there)",
                b.display()
            ),
            _ => "pdftotext".to_owned(),
        }
    }

    /// The command for `bin`: in the sandbox where there is one, with a cleared environment.
    fn command(&self, bin: &Path) -> tokio::process::Command {
        let mut cmd = match &self.bwrap {
            Some(bwrap) => {
                let mut cmd = tokio::process::Command::new(bwrap);
                cmd.args([
                    "--unshare-all",
                    "--die-with-parent",
                    "--new-session",
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--symlink",
                    "usr/lib64",
                    "/lib64",
                    "--symlink",
                    "usr/lib",
                    "/lib",
                    "--symlink",
                    "usr/bin",
                    "/bin",
                    "--symlink",
                    "usr/sbin",
                    "/sbin",
                    "--dev",
                    "/dev",
                ]);
                if !bin.starts_with("/usr/") {
                    cmd.arg("--ro-bind").arg(bin).arg(bin);
                }
                cmd.arg("--").arg(bin);
                cmd
            }
            None => tokio::process::Command::new(bin),
        };
        cmd.args(["-enc", "UTF-8", "-", "-"]);
        cmd.env_clear();
        for key in ["PATH", "LANG"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd
    }
}

/// An executable file `name` in a directory of `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// What a run's stderr kept: its first lines and how many it had.
#[derive(Debug, Default)]
struct Stderr {
    kept: Vec<String>,
    lines: usize,
    /// It spoke of memory running out (poppler's "Out of memory", a C++ `bad_alloc`, the loader
    /// failing to map a library).
    memory: bool,
}

impl Stderr {
    /// The kept lines, joined, with how many more there were.
    fn said(&self) -> String {
        let mut words: Vec<String> = self
            .kept
            .iter()
            .map(|l| page_line(l.trim()))
            .filter(|l| !l.is_empty())
            .collect();
        let more = self.lines.saturating_sub(self.kept.len());
        if more > 0 {
            words.push(format!("(… {more} more)"));
        }
        words.join(" / ")
    }
}

/// Drains `stderr` to its end, keeping its first [`STDERR_LINES_SHOWN`] lines.
async fn drain(mut stderr: tokio::process::ChildStderr) -> Stderr {
    use tokio::io::AsyncReadExt;
    let mut out = Stderr::default();
    let mut line: Vec<u8> = Vec::new();
    let mut open = false;
    // The end of the previous chunk, so a word cut by the chunk's edge is still seen.
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(n) = stderr.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let mut seen = std::mem::take(&mut tail);
        seen.extend_from_slice(&chunk[..n]);
        let low = String::from_utf8_lossy(&seen).to_ascii_lowercase();
        out.memory |= [
            "out of memory",
            "bad_alloc",
            "memory alloc",
            "cannot allocate memory",
            "failed to map segment",
        ]
        .iter()
        .any(|w| low.contains(w));
        tail = seen[seen.len().saturating_sub(32)..].to_vec();
        for &b in &chunk[..n] {
            if b == b'\n' {
                out.lines += 1;
                open = false;
                if out.kept.len() < STDERR_LINES_SHOWN {
                    out.kept
                        .push(String::from_utf8_lossy(&std::mem::take(&mut line)).into_owned());
                }
                line.clear();
            } else {
                open = true;
                if out.kept.len() < STDERR_LINES_SHOWN && line.len() < STDERR_LINE_BYTES {
                    line.push(b);
                }
            }
        }
    }
    if open {
        out.lines += 1;
        if out.kept.len() < STDERR_LINES_SHOWN {
            out.kept.push(String::from_utf8_lossy(&line).into_owned());
        }
    }
    out
}

/// How a run ended without its text.
enum Failed {
    Spawn(std::io::Error),
    Read(std::io::Error),
    Time,
    Text,
    Memory,
    Status(std::process::ExitStatus, Stderr),
}

/// `pdftotext`'s text of the PDF `bytes`, whole, within `limits` (module doc).
pub async fn extract(tool: &Tool, bytes: Vec<u8>, limits: &Limits) -> Result<String, String> {
    let Some(bin) = tool.bin.as_deref() else {
        return Err(format!(
            "Kai cannot read PDFs here: poppler's pdftotext is not installed ({PDFTOTEXT} is \
             missing, and no pdftotext is on PATH; the package poppler-utils: sudo dnf install \
             poppler-utils)"
        ));
    };
    let name = tool.name();
    let out = run(tool, bin, bytes, limits).await.map_err(|f| match f {
        Failed::Spawn(e) if e.kind() == std::io::ErrorKind::NotFound => format!(
            "Kai cannot read PDFs here: {} could not be started, a file it needs is missing \
             ({e}; poppler's pdftotext comes with the package poppler-utils: sudo dnf install \
             poppler-utils)",
            tool.bwrap
                .as_ref()
                .map_or_else(|| name.clone(), |b| format!("{name} in {}", b.display()))
        ),
        Failed::Spawn(e) if e.raw_os_error() == Some(libc::ENOMEM) => memory(&name, limits),
        Failed::Spawn(e) => format!("Kai could not run {name}: {e}"),
        Failed::Read(e) => format!("Kai could not read {name}'s output: {e}"),
        Failed::Time => format!(
            "{name} did not finish within {} s and was stopped (Kai's setting \
             browser.pdf_timeout_s, \"PDF reading time limit\"); a PDF that takes this long is \
             very large or malformed",
            limits.time.as_secs()
        ),
        Failed::Text => format!(
            "{name} wrote more than {} MB of text and was stopped (Kai's setting \
             browser.pdf_text_max_mb, \"PDF text limit\"); a PDF with this much text is very \
             large or malformed",
            limits.text_bytes >> 20
        ),
        Failed::Memory => memory(&name, limits),
        Failed::Status(status, err) => {
            let why = match status.code() {
                Some(3) => "the PDF's permissions do not allow copying its text",
                _ => "it could not read the PDF",
            };
            let said = err.said();
            if said.is_empty() {
                format!("{name} failed ({status}): {why}")
            } else {
                format!("{name} failed ({status}): {why}: {said}")
            }
        }
    })?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// A run stopped by `browser.pdf_memory_max_mb`, in words.
fn memory(name: &str, limits: &Limits) -> String {
    format!(
        "{name} needed more than {} MB of memory and was stopped (Kai's setting \
         browser.pdf_memory_max_mb, \"PDF reading memory limit\"); the PDF is very large or \
         malformed, or the setting too low",
        limits.memory_bytes >> 20
    )
}

/// One run of `bin` on `bytes` (module doc).
async fn run(tool: &Tool, bin: &Path, bytes: Vec<u8>, limits: &Limits) -> Result<Vec<u8>, Failed> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut cmd = tool.command(bin);
    let memory = limits.memory_bytes;
    // SAFETY: the hook runs in the forked child before exec and calls only `setrlimit`, which is
    // async-signal-safe; it allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit {
                rlim_cur: memory as libc::rlim_t,
                rlim_max: memory as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &lim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(Failed::Spawn)?;
    let piped = |what: &str| Failed::Read(std::io::Error::other(format!("{what} was not piped")));
    let mut stdin = child.stdin.take().ok_or_else(|| piped("stdin"))?;
    let mut stdout = child.stdout.take().ok_or_else(|| piped("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| piped("stderr"))?;
    // A write that fails because pdftotext gave up early is not the error worth saying: its exit
    // status and stderr are.
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
    });
    let errors = tokio::spawn(drain(stderr));
    // Read as it comes, so a run past the text's bound is stopped when it crosses it.
    let work = async {
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 64 << 10];
        loop {
            let n = stdout.read(&mut chunk).await.map_err(Failed::Read)?;
            if n == 0 {
                break;
            }
            if (out.len() + n) as u64 > limits.text_bytes {
                return Err(Failed::Text);
            }
            out.extend_from_slice(&chunk[..n]);
        }
        let status = child.wait().await.map_err(Failed::Read)?;
        Ok((out, status))
    };
    let ended = tokio::time::timeout(limits.time, work).await;
    let (out, status) = match ended {
        Ok(Ok(r)) => r,
        Ok(Err(f)) => {
            writer.abort();
            errors.abort();
            return Err(f);
        }
        Err(_) => {
            writer.abort();
            errors.abort();
            return Err(Failed::Time);
        }
    };
    let _ = writer.await;
    let err = errors.await.unwrap_or_default();
    if out_of_memory(&status, &err) {
        return Err(Failed::Memory);
    }
    if !status.success() {
        return Err(Failed::Status(status, err));
    }
    if !err.kept.is_empty() {
        tracing::debug!(stderr = %err.said(), "browser: pdftotext said something and read the PDF");
    }
    Ok(out)
}

/// The run died of the address-space bound: it failed, and its stderr spoke of memory (poppler's
/// own message, a C++ `bad_alloc` and the abort after it, the loader that could not map a library).
fn out_of_memory(status: &std::process::ExitStatus, err: &Stderr) -> bool {
    err.memory && !status.success()
}

/// The text's pages: split on form feeds, a trailing one closing the last page.
pub fn pages(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix(FORM_FEED)
        .unwrap_or(text)
        .split(FORM_FEED)
        .collect()
}

/// One line's characters as Kai counts them to judge it ([`count`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub chars: usize,
    /// Private-use, control and replacement characters.
    pub odd: usize,
    /// Letters and digits.
    pub words: usize,
}

impl std::ops::AddAssign for Count {
    fn add_assign(&mut self, o: Self) {
        self.chars += o.chars;
        self.odd += o.odd;
        self.words += o.words;
    }
}

fn private_use(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

/// What a font without a usable encoding gives: private-use, control and replacement characters.
fn odd(c: char) -> bool {
    c.is_control() || c == '\u{FFFD}' || private_use(c)
}

/// `line`'s characters as Kai judges them: white space is not counted, nor a private-use bullet at
/// the line's start (a Symbol or Wingdings font's, U+F0B7, with white space or nothing after it),
/// nor a run of one repeated character that is neither a letter, a digit nor odd (dot leaders,
/// `____` blanks, rules; white space between them does not break the run).
pub fn count(line: &str) -> Count {
    let mut rest = line.trim_start().chars();
    let bullet =
        rest.next().is_some_and(private_use) && rest.next().is_none_or(char::is_whitespace);
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    let mut c = Count::default();
    let mut i = usize::from(bullet);
    while i < chars.len() {
        let ch = chars[i];
        let mut j = i + 1;
        while j < chars.len() && chars[j] == ch {
            j += 1;
        }
        let run = j - i;
        if !(run >= 2 && !ch.is_alphanumeric() && !odd(ch)) {
            c.chars += run;
            if odd(ch) {
                c.odd += run;
            } else if ch.is_alphanumeric() {
                c.words += run;
            }
        }
        i = j;
    }
    c
}

/// A line of at least this many counted characters is judged by its letters and digits too; a
/// shorter one says too little ("(1)", a page number). A judgement's floor, not a cap.
const LETTERS_FROM: usize = 8;

/// Whether a line's count says it does not read as text: two or more odd characters that make a
/// tenth of it, or (from [`LETTERS_FROM`] characters) fewer than two in five letters or digits (a
/// font whose codes map to symbols).
pub fn unreadable(c: &Count) -> bool {
    (c.odd >= 2 && c.odd * 10 >= c.chars) || (c.chars >= LETTERS_FROM && c.words * 5 < c.chars * 2)
}

/// Why lines were left out, after "of its" or "of their".
fn why(c: &Count) -> String {
    format!(
        "{} characters, {} are private-use, control or replacement characters and {} letters or \
         digits",
        c.chars, c.odd, c.words
    )
}

/// The lines of the PDF's text (after the tab's line): how many pages, then each page under its
/// number, or what it lacks; a run of lines that do not read as text is one line saying so.
pub fn lines(text: &str) -> Vec<String> {
    let pages = pages(text);
    let mut out = vec![match pages.len() {
        0 => "a PDF, and pdftotext found no page in it".to_owned(),
        1 => "a PDF of 1 page, its text as poppler's pdftotext reads it:".to_owned(),
        n => format!("a PDF of {n} pages, its text as poppler's pdftotext reads it:"),
    }];
    for (i, page) in pages.iter().enumerate() {
        page_lines(i + 1, page, &mut out);
    }
    out
}

/// Page `n`'s lines into `out`.
fn page_lines(n: usize, page: &str, out: &mut Vec<String>) {
    if page.trim().is_empty() {
        out.push(format!("page {n} has no text layer (a scan?)"));
        return;
    }
    let lines: Vec<(&str, Count)> = page
        .trim_end()
        .trim_start_matches('\n')
        .split('\n')
        .map(|l| (l.trim_end(), count(l)))
        .collect();
    let readable = |(l, c): &&(&str, Count)| !l.trim().is_empty() && !unreadable(c);
    if !lines.iter().any(|l| readable(&l)) {
        let mut all = Count::default();
        for (_, c) in &lines {
            all += *c;
        }
        out.push(format!(
            "page {n}'s text is not readable, so Kai leaves it out: of its {}",
            why(&all)
        ));
        return;
    }
    out.push(format!("page {n}:"));
    // The run of unreadable lines being left out: how many, and their count.
    let mut run: Option<(usize, Count)> = None;
    let mut blank = false;
    let flush = |run: &mut Option<(usize, Count)>, out: &mut Vec<String>| {
        if let Some((k, c)) = run.take() {
            out.push(if k == 1 {
                format!(
                    "(Kai leaves out 1 line here that does not read as text: of its {})",
                    why(&c)
                )
            } else {
                format!(
                    "(Kai leaves out {k} lines here that do not read as text: of their {})",
                    why(&c)
                )
            });
        }
    };
    for (line, c) in &lines {
        if line.trim().is_empty() {
            // One empty line between paragraphs, however many pdftotext writes; inside a run of
            // left-out lines it waits for what comes next.
            blank = true;
            continue;
        }
        if unreadable(c) {
            match &mut run {
                Some((k, all)) => {
                    *k += 1;
                    *all += *c;
                }
                None => {
                    if blank {
                        out.push(String::new());
                    }
                    run = Some((1, *c));
                }
            }
            blank = false;
            continue;
        }
        flush(&mut run, out);
        if blank && out.last().is_some_and(|l| !l.is_empty()) {
            out.push(String::new());
        }
        blank = false;
        out.push(page_line(line));
    }
    flush(&mut run, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_split_on_form_feeds() {
        assert_eq!(pages(""), Vec::<&str>::new());
        assert_eq!(pages("a\u{c}b\u{c}"), ["a", "b"]);
        assert_eq!(pages("a\u{c}\u{c}c"), ["a", "", "c"]);
    }

    #[test]
    fn a_line_reads_as_text_or_not() {
        let bad = |l: &str| unreadable(&count(l));
        assert!(!bad("3"), "too little to judge");
        assert!(!bad("Trail Lamp, 12 €. A lamp for the trail; 3 modes."));
        assert!(bad(&"\u{E001}\u{E002}ab".repeat(10)));
        assert!(bad("!\"#$%&'()*+,-./:;<=>?@[]^_ab"));
        // A symbol font for letters (ZapfDingbats, as pdftotext reads it).
        assert!(bad("❁❂❃❄❅❆❇❈❉❊❋●❍■❏❐❑❒▲▼◆❖◗❘❙❚ ✡✢✣✤✥✦✧"));
        // A table of figures, a formula, a line of code, a page number between dashes.
        for l in [
            "Pro   12.00 €",
            "f(x) = (x + 1)² / (x − 1)",
            "if (a[i] != b[j]) { return -1; }",
            "-- 5 --",
            "Next \u{F0E0}",
        ] {
            assert!(!bad(l), "{l}");
        }
    }

    /// Dot leaders, blanks to fill in and a symbol font's bullets are not garbage.
    #[test]
    fn leaders_blanks_and_bullets_do_not_count() {
        let toc = "Contents\n1 Introduction ........................................ 3\n\
                   2 Methods . . . . . . . . . . . . . . . . . . . . . . . . 7\n\
                   2.1 Sampling ____________________________ 9\n3 Results ---------------- 12";
        let form = "Name: ______________________________  Date: ____________\n\
                    Signature: ________________________________________\n\
                    IBAN: __ __ __ __ / ____ ____ ____ ____";
        let slide = "Why lamps\n\u{F0B7} Fast\n\u{F0B7} Cheap\n\u{F0B7} Good\n\u{F0B7} Bright\n\
                     \u{F0B7} Light\n\u{F0B7} Warm\n\u{F0B7} Small\n\u{F0B7} Dry";
        for page in [toc, form, slide] {
            let got = lines(page);
            let want: Vec<String> = std::iter::once("page 1:".to_owned())
                .chain(page.split('\n').map(page_line))
                .collect();
            assert_eq!(got[1..], want, "{page}");
        }
    }

    #[test]
    fn the_lines_name_each_page_and_what_was_left_out() {
        let text = "Prices\n\n\n\nTrail Lamp\t12 €  \n\u{c}\u{c}".to_owned()
            + &"\u{E001}".repeat(30)
            + "\u{c}Heading\n\n"
            + &"\u{E001}\u{E002} \u{E003}\u{E004}x\n".repeat(2)
            + "\nThe end.\n\u{c}";
        assert_eq!(
            lines(&text),
            [
                "a PDF of 4 pages, its text as poppler's pdftotext reads it:",
                "page 1:",
                "Prices",
                "",
                "Trail Lamp\t12 €",
                "page 2 has no text layer (a scan?)",
                "page 3's text is not readable, so Kai leaves it out: of its 30 characters, 30 \
                 are private-use, control or replacement characters and 0 letters or digits",
                "page 4:",
                "Heading",
                "",
                "(Kai leaves out 2 lines here that do not read as text: of their 10 characters, 8 \
                 are private-use, control or replacement characters and 2 letters or digits)",
                "",
                "The end.",
            ]
        );
    }
}
