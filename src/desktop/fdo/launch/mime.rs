//! `open`'s type and default application (the shared MIME-info database and the MIME Applications
//! Associations specification).
//!
//! - **The type:** a URL other than `file:` is `x-scheme-handler/<scheme>`; a directory is
//!   `inode/directory`; a file is typed by its name against the `mime/globs2` files of the data
//!   directories (the highest weight wins, then the longest pattern; a pattern is case-sensitive
//!   only where it says `cs`). A name no glob matches is a program by its first bytes (an ELF
//!   binary, a `#!` script, a DOS/Windows `MZ` executable), else `text/plain` when its first 4 KiB
//!   are UTF-8 without a NUL, else `application/octet-stream`; the database's magic rules are not
//!   read.
//! - **Programs and launchers** (an executable type, [`executable`], the type or a parent of it)
//!   are refused by `open`, which never runs one: they are `run_command`'s, which the owner
//!   confirms.
//! - **A scheme** counts as one (`notes:x.txt` is a file, not a URL of scheme `notes`) when the
//!   text has `://`, or the scheme is a common one, or an application here handles
//!   `x-scheme-handler/<scheme>` ([`Schemes`]).
//! - **The default application,** for the type and then its parents (`mime/subclasses`, and
//!   `text/plain` for every `text/*`), aliases resolved (`mime/aliases`): the first installed id of
//!   `[Default Applications]` in the `mimeapps.list` files in the spec's order (each directory's
//!   `<desktop>-mimeapps.list` before its `mimeapps.list`; `XDG_CONFIG_HOME`, `XDG_CONFIG_DIRS`,
//!   then the data directories' `applications/`); else the first installed one of the
//!   `[Added Associations]` and then of the entries that list the type, leaving out those a
//!   `[Removed Associations]` names.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::entries::{Dirs, Entry};

/// One `globs2` line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Glob {
    weight: u32,
    mime: String,
    pattern: String,
    case_sensitive: bool,
}

/// The parts of the shared MIME-info database `open` reads.
#[derive(Debug, Clone, Default)]
pub struct MimeDb {
    globs: Vec<Glob>,
    aliases: HashMap<String, String>,
    parents: HashMap<String, Vec<String>>,
}

impl MimeDb {
    pub fn load(dirs: &Dirs) -> Self {
        let mut db = MimeDb::default();
        for data in dirs.data() {
            let mime = data.join("mime");
            if let Ok(t) = std::fs::read_to_string(mime.join("globs2")) {
                for line in t.lines().filter(|l| !l.starts_with('#')) {
                    let mut parts = line.splitn(4, ':');
                    let (Some(w), Some(m), Some(p)) = (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    let flags = parts.next().unwrap_or_default();
                    if m == "__NOGLOBS__" {
                        continue;
                    }
                    db.globs.push(Glob {
                        weight: w.parse().unwrap_or(50),
                        mime: m.to_owned(),
                        pattern: p.to_owned(),
                        case_sensitive: flags.split(',').any(|f| f == "cs"),
                    });
                }
            }
            if let Ok(t) = std::fs::read_to_string(mime.join("aliases")) {
                for (alias, canon) in t.lines().filter_map(|l| l.split_once(' ')) {
                    db.aliases
                        .entry(alias.to_owned())
                        .or_insert_with(|| canon.to_owned());
                }
            }
            if let Ok(t) = std::fs::read_to_string(mime.join("subclasses")) {
                for (child, parent) in t.lines().filter_map(|l| l.split_once(' ')) {
                    let list = db.parents.entry(child.to_owned()).or_default();
                    if !list.iter().any(|p| p == parent) {
                        list.push(parent.to_owned());
                    }
                }
            }
        }
        db
    }

    /// The canonical name of `mime` (an alias resolved).
    pub fn canonical(&self, mime: &str) -> String {
        let m = mime.to_ascii_lowercase();
        self.aliases.get(&m).cloned().unwrap_or(m)
    }

    /// `mime`, then its parents, nearest first.
    pub fn with_parents(&self, mime: &str) -> Vec<String> {
        let mut out = vec![self.canonical(mime)];
        let mut i = 0;
        while i < out.len() {
            let mut next: Vec<String> = self
                .parents
                .get(&out[i])
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|p| self.canonical(p))
                .collect();
            if out[i].starts_with("text/") && out[i] != "text/plain" {
                next.push("text/plain".into());
            }
            for n in next {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            i += 1;
        }
        out
    }

    /// The type of the file named `name` by the globs, if one matches.
    pub fn by_name(&self, name: &str) -> Option<String> {
        let lower = name.to_lowercase();
        self.globs
            .iter()
            .filter(|g| {
                if g.case_sensitive {
                    glob_match(&g.pattern, name)
                } else {
                    glob_match(&g.pattern.to_lowercase(), &lower)
                }
            })
            .max_by_key(|g| (g.weight, g.pattern.len()))
            .map(|g| self.canonical(&g.mime))
    }

    /// The type of the existing `path` (module doc).
    pub fn of_path(&self, path: &Path) -> String {
        if path.is_dir() {
            return "inode/directory".into();
        }
        if let Some(m) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| self.by_name(n))
        {
            return m;
        }
        let mut head = Vec::with_capacity(4096);
        let read = std::fs::File::open(path)
            .and_then(|f| f.take(4096).read_to_end(&mut head))
            .is_ok();
        if read {
            if head.starts_with(b"\x7fELF") {
                return "application/x-executable".into();
            }
            if head.starts_with(b"#!") {
                return "application/x-shellscript".into();
            }
            if head.starts_with(b"MZ") {
                return "application/x-ms-dos-executable".into();
            }
        }
        let text = read
            && !head.contains(&0)
            && match std::str::from_utf8(&head) {
                Ok(_) => true,
                // A character cut by the 4 KiB end is still text.
                Err(e) => e.error_len().is_none(),
            };
        if text {
            "text/plain".into()
        } else {
            "application/octet-stream".into()
        }
    }
}

/// `fnmatch` without flags: `*`, `?` and `[…]` (with `!` or `^` negation and ranges).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    fn class(p: &[char], c: char) -> Option<(bool, usize)> {
        // p starts after '['; returns (matched, length consumed including ']').
        let mut i = 0;
        let negate = matches!(p.first(), Some('!' | '^'));
        if negate {
            i += 1;
        }
        let mut matched = false;
        let mut first = true;
        while i < p.len() {
            if p[i] == ']' && !first {
                return Some((matched != negate, i + 1));
            }
            first = false;
            if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                if p[i] <= c && c <= p[i + 2] {
                    matched = true;
                }
                i += 3;
            } else {
                if p[i] == c {
                    matched = true;
                }
                i += 1;
            }
        }
        None
    }
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|k| go(&p[1..], &n[k..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some('[') => match (
                n.first(),
                class(&p[1..], n.first().copied().unwrap_or('\0')),
            ) {
                (Some(_), Some((true, len))) => go(&p[1 + len..], &n[1..]),
                (Some(c), None) => *c == '[' && go(&p[1..], &n[1..]),
                _ => false,
            },
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    go(&p, &n)
}

/// The three groups of one `mimeapps.list`, types canonical, ids without `.desktop`.
#[derive(Debug, Clone, Default)]
struct Lists {
    default: HashMap<String, Vec<String>>,
    added: HashMap<String, Vec<String>>,
    removed: HashMap<String, Vec<String>>,
}

/// The `mimeapps.list` files in the spec's order (module doc).
pub fn mimeapps_files(dirs: &Dirs) -> Vec<PathBuf> {
    let desktops: Vec<String> = dirs.desktops.iter().map(|d| d.to_lowercase()).collect();
    let mut bases: Vec<PathBuf> = vec![dirs.config_home.clone()];
    bases.extend(dirs.config_dirs.iter().cloned());
    bases.extend(dirs.data().map(|d| d.join("applications")));
    let mut files = Vec::new();
    for b in bases {
        for d in &desktops {
            files.push(b.join(format!("{d}-mimeapps.list")));
        }
        files.push(b.join("mimeapps.list"));
    }
    files
}

fn read_lists(file: &Path, db: &MimeDb) -> Lists {
    let mut lists = Lists::default();
    let Ok(text) = std::fs::read_to_string(file) else {
        return lists;
    };
    let mut group: Option<&mut HashMap<String, Vec<String>>> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            group = match line {
                "[Default Applications]" => Some(&mut lists.default),
                "[Added Associations]" => Some(&mut lists.added),
                "[Removed Associations]" => Some(&mut lists.removed),
                _ => None,
            };
            continue;
        }
        if let (Some(g), Some((k, v))) = (group.as_mut(), line.split_once('=')) {
            let ids = v
                .split(';')
                .map(str::trim)
                .filter(|i| !i.is_empty())
                .map(|i| i.strip_suffix(".desktop").unwrap_or(i).to_owned());
            g.entry(db.canonical(k.trim())).or_default().extend(ids);
        }
    }
    lists
}

/// The default application for `mime` among `apps` (module doc), with the type it was found for
/// (`mime` or a parent).
pub fn default_app<'a>(
    mime: &str,
    db: &MimeDb,
    dirs: &Dirs,
    apps: &'a [Entry],
) -> Option<(&'a Entry, String)> {
    let lists: Vec<Lists> = mimeapps_files(dirs)
        .iter()
        .map(|f| read_lists(f, db))
        .collect();
    let installed = |id: &str| apps.iter().find(|e| e.id == id);
    for m in db.with_parents(mime) {
        for l in &lists {
            if let Some(e) = l
                .default
                .get(&m)
                .into_iter()
                .flatten()
                .find_map(|id| installed(id))
            {
                return Some((e, m));
            }
        }
        let removed: Vec<&String> = lists
            .iter()
            .filter_map(|l| l.removed.get(&m))
            .flatten()
            .collect();
        let kept = |e: &&Entry| !removed.contains(&&e.id);
        let added = lists
            .iter()
            .filter_map(|l| l.added.get(&m))
            .flatten()
            .find_map(|id| installed(id).filter(kept));
        let listed = || {
            apps.iter()
                .filter(kept)
                .find(|e| e.mime_types.iter().any(|t| db.canonical(t) == m))
        };
        if let Some(e) = added.or_else(listed) {
            return Some((e, m));
        }
    }
    None
}

/// The types `open` refuses: programs, scripts and launchers, which their handler would run (Wine,
/// an AppImage launcher, an interpreter). Their parents are not: a script is a `text/plain` too.
const EXECUTABLE: [(&str, &str); 21] = [
    ("application/x-desktop", "a desktop entry (a launcher)"),
    ("application/x-executable", "a program"),
    ("application/x-pie-executable", "a program"),
    ("application/x-sharedlib", "a program or a library"),
    ("application/x-elf", "a program"),
    ("application/x-shellscript", "a shell script"),
    ("application/x-csh", "a shell script"),
    ("text/x-shellscript", "a shell script"),
    ("application/x-ms-dos-executable", "a Windows program"),
    ("application/x-msdownload", "a Windows program"),
    (
        "application/vnd.microsoft.portable-executable",
        "a Windows program",
    ),
    ("application/x-msi", "a Windows installer"),
    ("application/x-ms-shortcut", "a Windows shortcut"),
    ("application/x-appimage", "an AppImage (a program)"),
    ("application/vnd.appimage", "an AppImage (a program)"),
    ("application/x-java-archive", "a Java program"),
    ("application/x-java-jnlp-file", "a Java Web Start launcher"),
    ("text/x-python", "a Python script"),
    ("text/x-python3", "a Python script"),
    ("application/x-perl", "a Perl script"),
    ("application/x-ruby", "a Ruby script"),
];

/// What kind of program `mime` (or a type it is a kind of, its aliases resolved) is, if `open`
/// must refuse it.
pub fn executable(mime: &str, db: &MimeDb) -> Option<&'static str> {
    // Only the type and the types it is a subclass of in the database, not text/plain's fallback.
    let mut chain = vec![db.canonical(mime)];
    let mut i = 0;
    while i < chain.len() {
        for p in db.parents.get(&chain[i]).cloned().unwrap_or_default() {
            let p = db.canonical(&p);
            if !chain.contains(&p) {
                chain.push(p);
            }
        }
        i += 1;
    }
    chain.iter().find_map(|m| {
        EXECUTABLE
            .iter()
            .find(|(t, _)| db.canonical(t) == *m)
            .map(|(_, kind)| *kind)
    })
}

/// Common URL schemes, whether or not an application here handles them (the error then says no
/// application opens it, not that there is no such file).
const COMMON_SCHEMES: [&str; 20] = [
    "http", "https", "ftp", "ftps", "sftp", "file", "mailto", "tel", "sms", "magnet", "irc",
    "ircs", "ssh", "smb", "webcal", "geo", "news", "xmpp", "matrix", "data",
];

/// The URL schemes this session knows (module doc).
#[derive(Debug, Clone, Default)]
pub struct Schemes(std::collections::HashSet<String>);

impl Schemes {
    /// The schemes an application here handles: `x-scheme-handler/<scheme>` in a `mimeapps.list`
    /// default or association, or in an installed entry's `MimeType`.
    pub fn load(dirs: &Dirs, apps: &[Entry]) -> Self {
        let db = MimeDb::default();
        let mut set = std::collections::HashSet::new();
        let mut add = |m: &str| {
            if let Some(s) = m.to_ascii_lowercase().strip_prefix("x-scheme-handler/") {
                set.insert(s.to_owned());
            }
        };
        for f in mimeapps_files(dirs) {
            let l = read_lists(&f, &db);
            l.default.keys().chain(l.added.keys()).for_each(|m| add(m));
        }
        apps.iter().flat_map(|e| &e.mime_types).for_each(|m| add(m));
        Self(set)
    }

    pub fn known(&self, scheme: &str) -> bool {
        let s = scheme.to_ascii_lowercase();
        COMMON_SCHEMES.contains(&s.as_str()) || self.0.contains(&s)
    }
}
