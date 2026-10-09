//! The terminal for a `Terminal=true` application with fake `kdeglobals` files and a fake `PATH`
//! (`super::super::terminal`): an explicit `launch.terminal` always wins; unset, Plasma's configured
//! terminal read through KConfig's cascade, then `xdg-terminal-exec`, then Konsole on Plasma; the
//! error names `launch.terminal` only when none is installed.

use std::path::{Path, PathBuf};

use super::super::entries::Dirs;
use super::super::terminal::{NAMES_NONE, Source, Terminal, expand, plasma_terminal, resolve};
use super::{script, write};
use crate::desktop::tests::bus::temp_dir;

struct Fake {
    dir: tempfile::TempDir,
    desktop: &'static str,
}

impl Fake {
    fn new(desktop: &'static str) -> Self {
        let dir = temp_dir();
        for d in ["home", "bin", "config", "sys1", "sys2"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        Fake { dir, desktop }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    fn env(&self, k: &str) -> Option<String> {
        let p = |s: &str| self.path(s).display().to_string();
        match k {
            "HOME" => Some(p("home")),
            "XDG_CONFIG_HOME" => Some(p("config")),
            // sys1 is the more important one.
            "XDG_CONFIG_DIRS" => Some(format!("{}:{}", p("sys1"), p("sys2"))),
            "PATH" => Some(p("bin")),
            "XDG_CURRENT_DESKTOP" => Some(self.desktop.into()),
            "MYTERM" => Some("kitty".into()),
            _ => None,
        }
    }

    fn dirs(&self) -> Dirs {
        Dirs::from_env(|k| self.env(k))
    }

    fn install(&self, program: &str) -> PathBuf {
        let p = self.path(&format!("bin/{program}"));
        script(&p, "exit 0");
        p
    }

    fn globals(&self, dir: &str, text: &str) {
        write(&self.path(&format!("{dir}/kdeglobals")), text);
    }

    fn resolve(&self, configured: Option<&[String]>) -> Result<Terminal, String> {
        resolve(configured, &self.dirs(), &|k| self.env(k))
    }

    fn plasma(&self) -> Option<(String, PathBuf)> {
        plasma_terminal(&self.dirs(), &|k| self.env(k))
    }
}

fn argv(t: &Result<Terminal, String>) -> Vec<String> {
    t.as_ref().map(|t| t.argv.clone()).unwrap_or_default()
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

#[test]
fn an_explicit_setting_always_wins() {
    let f = Fake::new("KDE");
    f.install("kitty");
    f.install("xdg-terminal-exec");
    f.globals("config", "[General]\nTerminalApplication=kitty\n");
    let set = words(&["foot", "-e"]);
    let t = f.resolve(Some(&set)).unwrap();
    assert_eq!((t.argv, t.source), (set, Source::Setting));
    assert_eq!(f.resolve(Some(&[])).unwrap_err(), NAMES_NONE);
}

#[test]
fn plasmas_configured_terminal_comes_first_with_e() {
    let f = Fake::new("KDE");
    f.install("kitty");
    f.install("xdg-terminal-exec");
    f.install("konsole");
    f.globals(
        "config",
        "# KDE's own file\n[KDE]\nTerminalApplication=wrong-group\n\n[General]\nColorScheme=BreezeDark\n\
         TerminalApplication[de]=localised\nTerminalApplication = kitty --single-instance\n",
    );
    let t = f.resolve(None).unwrap();
    assert_eq!(t.argv, words(&["kitty", "--single-instance", "-e"]));
    assert_eq!(t.source, Source::Plasma(f.path("config/kdeglobals")));
}

#[test]
fn the_cascade_reads_as_kconfig_does() {
    let f = Fake::new("KDE");
    f.install("kitty");
    f.install("alacritty");
    f.install("foot");
    // The least important system file, the more important one, the user's own: the last wins.
    f.globals("sys2", "[General]\nTerminalApplication=foot\n");
    assert_eq!(f.plasma().unwrap().0, "foot");
    f.globals("sys1", "[General]\nTerminalApplication=alacritty\n");
    assert_eq!(f.plasma().unwrap().0, "alacritty");
    f.globals("config", "[General]\nTerminalApplication=kitty\n");
    assert_eq!(
        f.plasma(),
        Some(("kitty".into(), f.path("config/kdeglobals")))
    );
    // An immutable key in a system file stands against the user's.
    f.globals("sys1", "[General]\nTerminalApplication[$i]=alacritty\n");
    assert_eq!(f.plasma().unwrap().0, "alacritty");
    // An immutable group, and an immutable file, likewise.
    f.globals("sys1", "[General][$i]\nTerminalApplication=alacritty\n");
    assert_eq!(f.plasma().unwrap().0, "alacritty");
    f.globals("sys1", "[$i]\n[General]\nTerminalApplication=alacritty\n");
    assert_eq!(f.plasma().unwrap().0, "alacritty");
    // An immutable file without the key keeps what the files before it said.
    f.globals("sys1", "[$i]\n[General]\nColorScheme=X\n");
    assert_eq!(f.plasma().unwrap().0, "foot");
    // `[$d]` deletes an earlier value: nothing configured, so the next in the order.
    f.globals("sys1", "[General]\nTerminalApplication=alacritty\n");
    f.globals(
        "config",
        "[General]\nTerminalApplication[$d]\nTerminalApplication[$d]=\n",
    );
    assert_eq!(f.plasma(), None);
    // KConfig's escapes are undone, and the session's variables expanded (`[$e]` or not:
    // KIO reads the key with readPathEntry, which always expands).
    f.globals(
        "config",
        "[General]\nTerminalApplication[$e]=$HOME/bin/${MYTERM}\\s--hold\n",
    );
    let home = f.path("home").display().to_string();
    assert_eq!(f.plasma().unwrap().0, format!("{home}/bin/kitty --hold"));
    f.globals("config", "[General]\nTerminalApplication=$MYTERM --hold\n");
    assert_eq!(f.plasma().unwrap().0, "kitty --hold");
    // A value that expands to nothing is none; an empty value is none.
    f.globals("config", "[General]\nTerminalApplication=$UNSET\n");
    assert_eq!(f.plasma(), None);
    f.globals("config", "[General]\nTerminalApplication=\n");
    assert_eq!(f.plasma(), None, "the user's empty value configures none");
}

#[test]
fn a_configured_terminal_that_is_not_installed_falls_to_the_next() {
    let f = Fake::new("KDE");
    f.globals("config", "[General]\nTerminalApplication=ghostty\n");
    let exec = f.install("xdg-terminal-exec");
    let t = f.resolve(None).unwrap();
    assert_eq!(
        (t.argv, t.source),
        (words(&["xdg-terminal-exec"]), Source::XdgTerminalExec)
    );
    // An absolute path works where a name on PATH would.
    f.globals(
        "config",
        &format!("[General]\nTerminalApplication={}\n", exec.display()),
    );
    assert_eq!(
        argv(&f.resolve(None)),
        words(&[&exec.display().to_string(), "-e"])
    );
}

#[test]
fn xdg_terminal_exec_then_konsole_on_plasma_then_the_error() {
    let f = Fake::new("KDE");
    f.install("konsole");
    f.install("xdg-terminal-exec");
    assert_eq!(argv(&f.resolve(None)), words(&["xdg-terminal-exec"]));
    std::fs::remove_file(f.path("bin/xdg-terminal-exec")).unwrap();
    let t = f.resolve(None).unwrap();
    assert_eq!(
        (t.argv, t.source),
        (words(&["konsole", "-e"]), Source::Konsole)
    );
    std::fs::remove_file(f.path("bin/konsole")).unwrap();
    let e = f.resolve(None).unwrap_err();
    assert!(
        e.starts_with("no terminal was found")
            && e.contains("Plasma names no terminal")
            && e.contains("xdg-terminal-exec is not installed")
            && e.contains("Konsole is not installed")
            && e.contains("set launch.terminal"),
        "{e}"
    );
    // Konsole is Plasma's fallback, not another desktop's.
    let gnome = Fake::new("GNOME");
    gnome.install("konsole");
    let e = gnome.resolve(None).unwrap_err();
    assert!(
        e.contains("not a Plasma session") && e.contains("launch.terminal"),
        "{e}"
    );
    let mixed = Fake::new("ubuntu:KDE");
    mixed.install("konsole");
    assert_eq!(argv(&mixed.resolve(None)), words(&["konsole", "-e"]));
}

#[test]
fn nothing_outside_the_fake_is_read() {
    // The fake's directories only: the session's own kdeglobals is never the answer.
    let f = Fake::new("KDE");
    assert_eq!(f.plasma(), None);
    assert!(Path::new(&f.dirs().path).ends_with("bin"));
}

#[test]
fn values_expand_as_kconfigs_expand_string() {
    let env = |k: &str| match k {
        "HOME" => Some("/home/u".to_owned()),
        "A" => Some("x".to_owned()),
        "EMPTY" => Some(String::new()),
        "XDG_CONFIG_HOME" => Some("relative".to_owned()),
        _ => None,
    };
    let e = |v: &str| expand(v, &env);
    assert_eq!(
        e("$A/${A}/$A_b/$A-b"),
        "x/x//x-b",
        "a name runs over letters, digits and _"
    );
    assert_eq!(e("$$A and $$"), "$A and $", "$$ is one $");
    assert_eq!(e("$UNSET.$EMPTY."), "..", "unset and empty are empty");
    assert_eq!(e("a$/b $ c"), "a/b  c", "a $ before no name is dropped");
    assert_eq!(e("end$"), "end$", "a $ at the end is kept");
    assert_eq!(
        e("${}x ${A"),
        "x x",
        "an empty ${{}} goes, an unclosed one runs to the end"
    );
    assert_eq!(e("~/bin/term"), "~/bin/term", "no tilde expansion");
    // Nothing is run: `$(` is a dropped `$` before `(`.
    assert_eq!(e("$(touch /tmp/kai-never)"), "(touch /tmp/kai-never)");
    assert_eq!(e("`id`"), "`id`");
    // Qt's own fallbacks where the variables are unset (a relative XDG path is not taken).
    assert_eq!(
        e("$QT_DATA_HOME $QT_CONFIG_HOME ${QT_CACHE_HOME}"),
        "/home/u/.local/share /home/u/.config /home/u/.cache"
    );
    // A value is expanded once: what a variable holds stays as it is.
    let env2 = |k: &str| (k == "B").then(|| "$A".to_owned());
    assert_eq!(expand("$B", &env2), "$A");
}

#[test]
fn an_unclosed_quote_in_plasmas_terminal_says_so() {
    let f = Fake::new("GNOME");
    f.install("kitty");
    f.globals("config", "[General]\nTerminalApplication=kitty \"--title\n");
    let e = f.resolve(None).unwrap_err();
    assert!(
        e.contains(r#"Plasma's terminal ("kitty \"--title" in "#)
            && e.contains("has an unclosed quote")
            && !e.contains("is not installed)"),
        "{e}"
    );
}

/// The launch's "<app> runs in a terminal, and <this> is not installed" names where the terminal
/// came from, not always the setting.
#[test]
fn each_source_is_named_as_the_owner_would_look_for_it() {
    assert_eq!(
        Source::Setting.describe(),
        "the terminal launch.terminal names"
    );
    assert_eq!(
        Source::Plasma(PathBuf::from("/home/u/.config/kdeglobals")).describe(),
        "Plasma's terminal (TerminalApplication in /home/u/.config/kdeglobals)"
    );
    assert_eq!(Source::XdgTerminalExec.describe(), "xdg-terminal-exec");
    assert_eq!(
        Source::Konsole.describe(),
        "Konsole, Plasma's fallback terminal"
    );
}
