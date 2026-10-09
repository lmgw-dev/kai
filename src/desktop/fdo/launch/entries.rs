//! Desktop entries (the freedesktop Desktop Entry Specification 1.5): where they are, which are
//! installed, which one `app` names (K22's resolution), and an entry's command line for some files
//! or URLs (its `Exec` with the field codes expanded).
//!
//! **Installed** means: a `Type=Application` entry in `$XDG_DATA_HOME/applications` or a
//! `$XDG_DATA_DIRS/*/applications` (the first directory holding an id wins, and a `Hidden=true`
//! there hides the id), not hidden from this desktop by `OnlyShowIn` or `NotShowIn`, with an
//! `Exec` line whose `TryExec` program (when given) is found.
//!
//! **`app` resolves** by the first rule that matches anything (any case): the id (with or without
//! `.desktop`); the id's last dotted part (`dolphin` for `org.kde.dolphin`); the name, as written
//! or in the session's language; the command (`Exec`'s program, `firefox`); a fragment of the name
//! or the generic name (`browser`). `NoDisplay=true` entries (helpers, handlers) match only by
//! their id. More than one match is an error listing them, one line each.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::host::confirm::{quoted, shown_text};

/// Where the entries and the defaults are, and what the session is, from the environment the
/// launched program gets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dirs {
    pub home: PathBuf,
    pub data_home: PathBuf,
    pub data_dirs: Vec<PathBuf>,
    pub config_home: PathBuf,
    pub config_dirs: Vec<PathBuf>,
    /// `XDG_CURRENT_DESKTOP`'s names (`KDE`).
    pub desktops: Vec<String>,
    /// The language's variants, most specific first (`de_DE`, `de`).
    pub langs: Vec<String>,
    /// `PATH`.
    pub path: String,
}

impl Dirs {
    /// From an environment's variables (the XDG defaults where one is unset or relative).
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Self {
        let home = PathBuf::from(get("HOME").unwrap_or_default());
        let one = |var: &str, fallback: &str| match get(var).map(PathBuf::from) {
            Some(p) if p.is_absolute() => p,
            _ => home.join(fallback),
        };
        let list = |var: &str, fallback: &str| {
            let v = get(var).filter(|v| !v.is_empty());
            v.as_deref()
                .unwrap_or(fallback)
                .split(':')
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .collect::<Vec<_>>()
        };
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|v| get(v).filter(|v| !v.is_empty()))
            .unwrap_or_default();
        Self {
            data_home: one("XDG_DATA_HOME", ".local/share"),
            data_dirs: list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            config_home: one("XDG_CONFIG_HOME", ".config"),
            config_dirs: list("XDG_CONFIG_DIRS", "/etc/xdg"),
            desktops: get("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .filter(|d| !d.is_empty())
                .map(str::to_owned)
                .collect(),
            langs: lang_variants(&locale),
            path: get("PATH").unwrap_or_default(),
            home,
        }
    }

    /// The data directories, most important first: `XDG_DATA_HOME`, then `XDG_DATA_DIRS`.
    pub fn data(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.data_home).chain(&self.data_dirs)
    }
}

/// `de_DE.UTF-8@euro` → `de_DE@euro`, `de_DE`, `de@euro`, `de` (the spec's match order).
fn lang_variants(locale: &str) -> Vec<String> {
    if locale.is_empty() || locale == "C" || locale == "POSIX" {
        return Vec::new();
    }
    let (rest, modifier) = match locale.split_once('@') {
        Some((r, m)) => (r, Some(m)),
        None => (locale, None),
    };
    let rest = rest.split('.').next().unwrap_or(rest);
    let (lang, country) = match rest.split_once('_') {
        Some((l, c)) => (l, Some(c)),
        None => (rest, None),
    };
    let mut v = Vec::new();
    if let (Some(c), Some(m)) = (country, modifier) {
        v.push(format!("{lang}_{c}@{m}"));
    }
    if let Some(c) = country {
        v.push(format!("{lang}_{c}"));
    }
    if let Some(m) = modifier {
        v.push(format!("{lang}@{m}"));
    }
    v.push(lang.to_owned());
    v
}

/// One installed application.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entry {
    /// The desktop file id without `.desktop` (`org.kde.dolphin`).
    pub id: String,
    pub file: PathBuf,
    /// `Name` as written.
    pub name: String,
    /// `Name` in the session's language, where the entry has it.
    pub local_name: Option<String>,
    /// `GenericName`, as written and in the session's language.
    pub generic: Vec<String>,
    /// `Exec`, unescaped as a string (its own quoting is [`exec_words`]').
    pub exec: String,
    /// `Path`: the working directory.
    pub workdir: Option<String>,
    pub terminal: bool,
    pub no_display: bool,
    pub icon: Option<String>,
    pub mime_types: Vec<String>,
}

impl Entry {
    /// The name to show: in the session's language where the entry has it.
    pub fn shown_name(&self) -> &str {
        self.local_name.as_deref().unwrap_or(&self.name)
    }

    /// `Exec`'s program, as written (`firefox`, `/usr/bin/kate`).
    pub fn program(&self) -> Option<String> {
        exec_words(&self.exec).ok()?.into_iter().next()
    }

    /// A candidate's line: `<id> "<name>"`.
    pub fn line(&self) -> String {
        format!("{} {}", shown_text(&self.id), quoted(self.shown_name()))
    }
}

/// The `[Desktop Entry]` group's keys of one file, `Key[locale]` kept as written.
fn parse_group(text: &str) -> Vec<(String, String)> {
    let mut in_group = false;
    let mut keys = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_group = line.trim_end() == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            keys.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    keys
}

/// A `string` value's escapes: `\s`, `\n`, `\t`, `\r`, `\\`.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A `;`-separated list (`\;` is a semicolon in an item).
fn list(v: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&';') => {
                cur.push(';');
                chars.next();
            }
            ';' => items.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        items.push(cur);
    }
    items
        .into_iter()
        .map(|i| unescape(&i))
        .filter(|i| !i.is_empty())
        .collect()
}

/// Why a file is not an installed application, or the entry it is.
enum Read {
    App(Box<Entry>),
    /// `Hidden=true`: the id is taken and hidden (a lower directory's entry is not used).
    Hidden,
    /// Not an application of this desktop: the id is free for a lower directory.
    Skip,
}

fn read_entry(id: String, file: &Path, text: &str, dirs: &Dirs) -> Read {
    let keys = parse_group(text);
    let get = |k: &str| keys.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let local = |k: &str| {
        dirs.langs
            .iter()
            .find_map(|l| get(&format!("{k}[{l}]")))
            .map(unescape)
    };
    let yes = |k: &str| get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if yes("Hidden") {
        return Read::Hidden;
    }
    if get("Type") != Some("Application") {
        return Read::Skip;
    }
    let shown_here = |k: &str| {
        get(k).map(|v| {
            list(v)
                .iter()
                .any(|d| dirs.desktops.iter().any(|c| c.eq_ignore_ascii_case(d)))
        })
    };
    if shown_here("OnlyShowIn") == Some(false) || shown_here("NotShowIn") == Some(true) {
        return Read::Skip;
    }
    let Some(exec) = get("Exec").map(unescape).filter(|e| !e.trim().is_empty()) else {
        return Read::Skip;
    };
    if let Some(t) = get("TryExec").map(unescape)
        && find_program(&t, &dirs.path, &dirs.home).is_none()
    {
        return Read::Skip;
    }
    let mut generic: Vec<String> = get("GenericName").map(unescape).into_iter().collect();
    generic.extend(local("GenericName"));
    Read::App(Box::new(Entry {
        id,
        file: file.to_owned(),
        name: get("Name").map(unescape).unwrap_or_default(),
        local_name: local("Name"),
        generic,
        exec,
        workdir: get("Path").map(unescape).filter(|p| !p.is_empty()),
        terminal: yes("Terminal"),
        no_display: yes("NoDisplay"),
        icon: get("Icon").map(unescape).filter(|i| !i.is_empty()),
        mime_types: get("MimeType").map(list).unwrap_or_default(),
    }))
}

/// Every `.desktop` file below `dir`, with its id (`kde/foo.desktop` is `kde-foo`), sorted.
fn files(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Some(rel) = p
                .strip_prefix(base)
                .ok()
                .and_then(Path::to_str)
                .and_then(|r| r.strip_suffix(".desktop"))
            {
                out.push((rel.replace('/', "-"), p));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The installed applications (module doc), ordered by their directory's precedence, then id.
pub fn scan(dirs: &Dirs) -> Vec<Entry> {
    let mut taken = HashSet::new();
    let mut apps = Vec::new();
    for data in dirs.data() {
        for (id, file) in files(&data.join("applications")) {
            if taken.contains(&id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            match read_entry(id.clone(), &file, &text, dirs) {
                Read::App(e) => {
                    let e = *e;
                    taken.insert(id);
                    apps.push(e);
                }
                Read::Hidden => {
                    taken.insert(id);
                }
                Read::Skip => {}
            }
        }
    }
    apps
}

/// The entry `app` names (module doc).
pub fn resolve<'a>(apps: &'a [Entry], app: &str) -> Result<&'a Entry, String> {
    let q = app.trim().to_lowercase();
    if q.is_empty() {
        return Err("name an application (its id, name or command)".into());
    }
    let id_q = q.strip_suffix(".desktop").unwrap_or(&q).to_owned();
    let program = |e: &Entry| {
        e.program()
            .map(|p| p.rsplit('/').next().unwrap_or(&p).to_lowercase())
    };
    let rules: [&dyn Fn(&Entry) -> bool; 5] = [
        &|e| e.id.to_lowercase() == id_q,
        &|e| !e.no_display && e.id.to_lowercase().rsplit('.').next() == Some(id_q.as_str()),
        &|e| {
            !e.no_display
                && (e.name.to_lowercase() == q
                    || e.local_name.as_ref().is_some_and(|n| n.to_lowercase() == q))
        },
        &|e| !e.no_display && program(e).as_deref() == Some(q.as_str()),
        &|e| {
            !e.no_display
                && std::iter::once(&e.name)
                    .chain(&e.local_name)
                    .chain(&e.generic)
                    .any(|n| n.to_lowercase().contains(&q))
        },
    ];
    for rule in rules {
        let found: Vec<&Entry> = apps.iter().filter(|e| rule(e)).collect();
        match found.len() {
            0 => continue,
            1 => return Ok(found[0]),
            n => {
                let mut text = format!("{n} applications match; name one by its id:");
                for e in found {
                    text.push('\n');
                    text.push_str(&e.line());
                }
                return Err(text);
            }
        }
    }
    Err("no installed application has that id, name or command".into())
}

/// `program` as `execvp` would find it: a path with a `/` as it is (`~/` from `home`), else the
/// first executable file of that name on `path`.
pub fn find_program(program: &str, path: &str, home: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let runnable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.is_empty() {
        return None;
    }
    if let Some(rest) = program.strip_prefix("~/") {
        let p = home.join(rest);
        return runnable(&p).then_some(p);
    }
    if program.contains('/') {
        let p = PathBuf::from(program);
        return runnable(&p).then_some(p);
    }
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(program))
        .find(|p| runnable(p))
}

/// `Exec`'s words, split as GLib's `g_shell_parse_argv` does (what KIO and GLib launch with): the
/// spec's double quotes (a quoted word keeps spaces, and `\"`, `` \` ``, `\$` and `\\` are its
/// escapes), single quotes (everything literal up to the next one) and a backslash outside quotes
/// (the next character literal). An unclosed quote is an error.
pub fn exec_words(exec: &str) -> Result<Vec<String>, String> {
    let unclosed = || "its Exec line has an unclosed quote".to_owned();
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err(unclosed()),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '`' | '$' | '\\')) => cur.push(e),
                            Some('\n') => {}
                            Some(o) => {
                                cur.push('\\');
                                cur.push(o);
                            }
                            None => return Err(unclosed()),
                        },
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err(unclosed()),
                        Some('\'') => break,
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(o) => {
                    in_word = true;
                    cur.push(o);
                }
                None => {
                    in_word = true;
                    cur.push('\\');
                }
            },
            ' ' | '\t' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            o => {
                in_word = true;
                cur.push(o);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// `v` as one shell word (GLib's `g_shell_quote`): in single quotes, a `'` as `'\''`.
pub fn shell_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

/// A file or URL to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Url(String),
}

impl Target {
    /// `t` as a caller writes it: a URL with its scheme (`file:` ones are files), else a path,
    /// absolute, from `~/`, or from the home directory. A scheme is one when `t` has `://` or
    /// `known` says so (`mime::Schemes`): `notes:x.txt` is a file.
    pub fn parse(t: &str, home: &Path, known: impl Fn(&str) -> bool) -> Result<Target, String> {
        let t = t.trim();
        if t.is_empty() {
            return Err("an empty file or URL".into());
        }
        if let Some(scheme) = scheme_of(t)
            && (t.contains("://") || known(scheme))
        {
            let url = url::Url::parse(t).map_err(|e| format!("not a URL ({e})"))?;
            if url.scheme() == "file" {
                return url
                    .to_file_path()
                    .map(Target::File)
                    .map_err(|()| "a file: URL that names no local file".into());
            }
            return Ok(Target::Url(url.into()));
        }
        Ok(Target::File(if let Some(rest) = t.strip_prefix("~/") {
            home.join(rest)
        } else if t == "~" {
            home.to_owned()
        } else {
            home.join(t)
        }))
    }

    /// As a URL (`%u`): a file as `file://`.
    fn url(&self) -> Result<String, String> {
        match self {
            Target::Url(u) => Ok(u.clone()),
            Target::File(p) => url::Url::from_file_path(p)
                .map(Into::into)
                .map_err(|()| "a path Kai cannot write as a file: URL".into()),
        }
    }

    /// As a local path (`%f`): a URL other than `file:` is not one.
    fn path(&self) -> Option<String> {
        match self {
            Target::File(p) => Some(p.to_string_lossy().into_owned()),
            Target::Url(_) => None,
        }
    }
}

/// The `scheme` of `scheme:` at the start (RFC 3986), not a lone drive-like letter.
fn scheme_of(t: &str) -> Option<&str> {
    let (scheme, _) = t.split_once(':')?;
    (scheme.len() > 1
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

/// An entry's command lines for `targets`: one, or one per target where it takes a single file or
/// URL (`%f`, `%u`) and was given several; with the notes the result says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub argvs: Vec<Vec<String>>,
    pub notes: Vec<String>,
}

/// A quoting state: outside quotes, inside single or inside double quotes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quoting {
    None,
    Single,
    Double,
}

/// Where a field code stands: in `Exec`'s own quoting (`outer`), and, inside an `Exec` quote, in
/// the quoting of the shell the quoted word is for (`inner`: `sh -c "mpv '%f'"` is outer double,
/// inner single), as that shell reads the word `Exec`'s split makes.
#[derive(Clone, Copy)]
struct Place {
    outer: Quoting,
    inner: Quoting,
    /// The inner shell's backslash: its next character is literal.
    inner_escape: bool,
}

impl Place {
    /// The inner shell reads `c` (a character of the word as `Exec`'s split leaves it).
    fn feed_inner(&mut self, c: char) {
        if self.inner_escape {
            self.inner_escape = false;
            return;
        }
        self.inner = match (self.inner, c) {
            (Quoting::None, '\'') => Quoting::Single,
            (Quoting::None, '"') => Quoting::Double,
            (Quoting::None | Quoting::Double, '\\') => {
                self.inner_escape = true;
                self.inner
            }
            (Quoting::Single, '\'') | (Quoting::Double, '"') => Quoting::None,
            (q, _) => q,
        };
    }
}

/// `v` for a shell at `inner`: read back as `v` itself, one literal piece of its word.
fn for_shell(v: &str, inner: Quoting) -> String {
    match inner {
        Quoting::None => shell_quote(v),
        // Inside its single quotes: a `'` closes them, is escaped and reopens them.
        Quoting::Single => v.replace('\'', r"'\''"),
        Quoting::Double => {
            let mut o = String::with_capacity(v.len());
            for c in v.chars() {
                if matches!(c, '"' | '\\' | '$' | '`') {
                    o.push('\\');
                }
                o.push(c);
            }
            o
        }
    }
}

/// `v` written into `Exec` at `at`, so that it stays one literal piece of its word whatever it
/// holds (a file named `x;curl …|sh;.mp4`). Outside quotes it is quoted as a shell word (GLib's
/// `g_shell_quote`), so the word holds `v` itself. Inside `Exec`'s quotes the word is the argument
/// of a shell the entry runs (`sh -c "mpv %f"`): the word then holds `v` as that shell reads it
/// literally where the code stands in it ([`for_shell`]), and that is written for `Exec`'s own
/// quote (inside double quotes escaped for them, inside single quotes between a closing and a
/// reopening quote).
fn insert(out: &mut String, v: &str, at: Place) {
    match at.outer {
        Quoting::None => out.push_str(&shell_quote(v)),
        Quoting::Double => out.push_str(&for_shell(&for_shell(v, at.inner), Quoting::Double)),
        Quoting::Single => {
            out.push('\'');
            out.push_str(&shell_quote(&for_shell(v, at.inner)));
            out.push('\'');
        }
    }
}

/// The field codes of `exec` in order (`%%` is none).
fn codes(exec: &str) -> Vec<char> {
    let mut found = Vec::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(code) = chars.next()
            && code != '%'
        {
            found.push(code);
        }
    }
    found
}

/// `entry`'s `Exec` with its field codes expanded for `group`, before it is split (as GLib and KIO
/// do, each value inserted quoted: [`insert`]).
fn expand(entry: &Entry, group: &[&Target]) -> Result<String, String> {
    let mut out = String::with_capacity(entry.exec.len());
    let mut at = Place {
        outer: Quoting::None,
        inner: Quoting::None,
        inner_escape: false,
    };
    let mut chars = entry.exec.chars();
    while let Some(c) = chars.next() {
        match (c, at.outer) {
            ('\\', Quoting::None) => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                    at.feed_inner(n);
                }
                continue;
            }
            ('\\', Quoting::Double) => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                    // What the split leaves of the pair is what the inner shell reads.
                    match n {
                        '"' | '\\' | '$' | '`' => at.feed_inner(n),
                        '\n' => {}
                        o => {
                            at.feed_inner('\\');
                            at.feed_inner(o);
                        }
                    }
                }
                continue;
            }
            ('\'', Quoting::None) => at.outer = Quoting::Single,
            ('"', Quoting::None) => at.outer = Quoting::Double,
            ('\'', Quoting::Single) | ('"', Quoting::Double) => at.outer = Quoting::None,
            ('%', _) => {
                match chars.next() {
                    Some('%') => {
                        out.push('%');
                        if at.outer != Quoting::None {
                            at.feed_inner('%');
                        }
                    }
                    Some('f') => {
                        if let Some(p) = group.first().and_then(|t| t.path()) {
                            insert(&mut out, &p, at);
                        }
                    }
                    Some('F') => {
                        let paths: Vec<String> = group.iter().filter_map(|t| t.path()).collect();
                        for (i, p) in paths.iter().enumerate() {
                            if i > 0 {
                                out.push(' ');
                            }
                            insert(&mut out, p, at);
                        }
                    }
                    Some('u') => {
                        if let Some(t) = group.first() {
                            insert(&mut out, &t.url()?, at);
                        }
                    }
                    Some('U') => {
                        for (i, t) in group.iter().enumerate() {
                            if i > 0 {
                                out.push(' ');
                            }
                            insert(&mut out, &t.url()?, at);
                        }
                    }
                    Some('i') => {
                        if let Some(icon) = &entry.icon {
                            out.push_str("--icon ");
                            insert(&mut out, icon, at);
                        }
                    }
                    Some('c') => insert(&mut out, entry.shown_name(), at),
                    Some('k') => insert(&mut out, &entry.file.to_string_lossy(), at),
                    // The deprecated codes and unknown ones expand to nothing.
                    Some(_) | None => {}
                }
                continue;
            }
            (o, Quoting::Single | Quoting::Double) => at.feed_inner(o),
            (o, Quoting::None) => {
                // Unquoted whitespace ends the word, and with it what a shell would read of it.
                if matches!(o, ' ' | '\t' | '\n') {
                    at.inner = Quoting::None;
                    at.inner_escape = false;
                } else {
                    at.feed_inner(o);
                }
            }
        }
        out.push(c);
    }
    Ok(out)
}

/// Expands `entry`'s `Exec` for `targets` (the spec's field codes; the deprecated ones are
/// dropped), each value inserted quoted before the line is split ([`expand`]).
pub fn plan(entry: &Entry, targets: &[Target]) -> Result<Plan, String> {
    exec_words(&entry.exec)?;
    let takes = codes(&entry.exec)
        .into_iter()
        .find(|c| matches!(c, 'f' | 'F' | 'u' | 'U'));
    let mut notes = Vec::new();
    if matches!(takes, Some('f' | 'F')) && targets.iter().any(|t| t.path().is_none()) {
        return Err(format!(
            "{} opens local files only, not URLs",
            shown_text(entry.shown_name())
        ));
    }
    let groups: Vec<Vec<&Target>> = match takes {
        None => {
            if !targets.is_empty() {
                notes.push("it takes no files or URLs, so none were passed".to_owned());
            }
            vec![Vec::new()]
        }
        Some('f' | 'u') if targets.len() > 1 => targets.iter().map(|t| vec![t]).collect(),
        _ => vec![targets.iter().collect()],
    };
    let mut argvs = Vec::new();
    for group in groups {
        let argv = exec_words(&expand(entry, &group)?)?;
        if argv.is_empty() {
            return Err("its Exec line names no program".into());
        }
        argvs.push(argv);
    }
    Ok(Plan { argvs, notes })
}
