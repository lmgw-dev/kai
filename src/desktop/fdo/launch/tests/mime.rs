//! `open`'s type and default application.

use std::path::Path;

use super::super::entries::{Dirs, Entry};
use super::super::mime::*;
use super::write;
use crate::desktop::tests::bus::temp_dir;

fn dirs(root: &Path) -> Dirs {
    Dirs {
        home: root.join("home"),
        data_home: root.join("data"),
        data_dirs: vec![root.join("sys")],
        config_home: root.join("config"),
        config_dirs: vec![root.join("etc")],
        desktops: vec!["KDE".into()],
        ..Default::default()
    }
}

fn db(root: &Path) -> MimeDb {
    write(
        &root.join("sys/mime/globs2"),
        "# comment\n50:text/x-csrc:*.c\n50:text/x-c++src:*.C:cs\n50:text/x-c++src:*.cpp\n\
         80:text/x-makefile:Makefile\n10:text/x-makefile:makefile\n50:image/png:*.png\n\
         60:application/x-compressed-tar:*.tar.gz\n50:application/gzip:*.gz\n\
         50:text/x-log:*.[lL]og\n50:application/x-oldtype:*.old\n",
    );
    write(
        &root.join("sys/mime/aliases"),
        "application/x-oldtype application/x-newtype\n",
    );
    write(
        &root.join("sys/mime/subclasses"),
        "text/x-c++src text/x-csrc\napplication/x-compressed-tar application/gzip\n",
    );
    MimeDb::load(&dirs(root))
}

#[test]
fn globs_match_like_fnmatch() {
    assert!(glob_match("*.txt", "a.txt"));
    assert!(!glob_match("*.txt", "a.txt.bak"));
    assert!(glob_match("?.c", "a.c"));
    assert!(glob_match("*.[lL]og", "x.Log"));
    assert!(!glob_match("*.[!l]og", "x.log"));
    assert!(glob_match("[a-c]*", "b.x"));
    assert!(glob_match("Makefile", "Makefile"));
    assert!(!glob_match("Makefile", "Makefile.am"));
}

/// The highest weight wins, then the longest pattern; `cs` patterns care about case, the others
/// do not; an alias is resolved.
#[test]
fn a_file_is_typed_by_its_name() {
    let t = temp_dir();
    let db = db(t.path());
    assert_eq!(db.by_name("main.c").as_deref(), Some("text/x-csrc"));
    assert_eq!(db.by_name("MAIN.CPP").as_deref(), Some("text/x-c++src"));
    assert_eq!(db.by_name("main.C").as_deref(), Some("text/x-c++src"));
    assert_eq!(db.by_name("Makefile").as_deref(), Some("text/x-makefile"));
    assert_eq!(
        db.by_name("a.tar.gz").as_deref(),
        Some("application/x-compressed-tar")
    );
    assert_eq!(db.by_name("a.gz").as_deref(), Some("application/gzip"));
    assert_eq!(db.by_name("x.LOG").as_deref(), Some("text/x-log"));
    assert_eq!(
        db.by_name("x.old").as_deref(),
        Some("application/x-newtype")
    );
    assert_eq!(db.by_name("x.unknown"), None);
    assert_eq!(
        db.with_parents("text/x-c++src"),
        ["text/x-c++src", "text/x-csrc", "text/plain"]
    );
    assert_eq!(
        db.with_parents("application/x-compressed-tar"),
        ["application/x-compressed-tar", "application/gzip"]
    );
}

/// No glob: text by its first bytes, else octet-stream; a directory is `inode/directory`.
#[test]
fn an_unnamed_type_is_text_or_bytes() {
    let t = temp_dir();
    let db = db(t.path());
    let dir = t.path().join("files");
    write(&dir.join("notes"), "plain words\nü\n");
    std::fs::write(dir.join("blob"), [0u8, 1, 2, 255]).unwrap();
    let mut cut = "a".repeat(4095).into_bytes();
    cut.extend("ü".as_bytes());
    std::fs::write(dir.join("cut"), cut).unwrap();
    assert_eq!(db.of_path(&dir.join("notes")), "text/plain");
    assert_eq!(db.of_path(&dir.join("blob")), "application/octet-stream");
    assert_eq!(db.of_path(&dir.join("cut")), "text/plain");
    assert_eq!(db.of_path(&dir), "inode/directory");
    std::fs::write(dir.join("x.png"), "not really").unwrap();
    assert_eq!(db.of_path(&dir.join("x.png")), "image/png");
}

fn app(id: &str, mimes: &[&str]) -> Entry {
    Entry {
        id: id.into(),
        name: id.into(),
        exec: id.into(),
        mime_types: mimes.iter().map(|m| m.to_string()).collect(),
        ..Default::default()
    }
}

/// The spec's order: the desktop's own list before the plain one, the config directories before
/// the data directories; a default not installed is passed over; associations after every
/// default, without the removed ones; the entries' own types last; then the parent types.
#[test]
fn the_default_application_follows_the_spec() {
    let t = temp_dir();
    let r = t.path();
    let db = db(r);
    let d = dirs(r);
    assert_eq!(
        mimeapps_files(&d)[..3],
        [
            r.join("config/kde-mimeapps.list"),
            r.join("config/mimeapps.list"),
            r.join("etc/kde-mimeapps.list"),
        ]
    );
    let apps = vec![
        app("org.kde.kate", &["text/plain", "text/x-csrc"]),
        app("gedit", &["text/plain"]),
        app("org.kde.gwenview", &["image/png"]),
        app("ark", &["application/gzip"]),
        app("org.kde.okular", &[]),
    ];
    write(
        &r.join("config/mimeapps.list"),
        "[Default Applications]\ntext/plain=gedit.desktop\nimage/png=not-installed.desktop;\n\
         [Added Associations]\nimage/png=org.kde.okular.desktop;\n\
         [Removed Associations]\ntext/x-csrc=org.kde.kate.desktop\n",
    );
    write(
        &r.join("config/kde-mimeapps.list"),
        "[Default Applications]\ntext/plain=org.kde.kate.desktop\n",
    );
    write(
        &r.join("sys/applications/mimeapps.list"),
        "[Default Applications]\nimage/png=org.kde.gwenview.desktop\n\
         application/x-newtype=ark.desktop\n",
    );
    let default = |m: &str| default_app(m, &db, &d, &apps).map(|(e, found)| (e.id.clone(), found));
    assert_eq!(
        default("text/plain"),
        Some(("org.kde.kate".into(), "text/plain".into()))
    );
    assert_eq!(
        default("image/png"),
        Some(("org.kde.gwenview".into(), "image/png".into())),
        "a default anywhere before any association"
    );
    assert_eq!(
        default("text/x-csrc"),
        Some(("org.kde.kate".into(), "text/plain".into())),
        "kate is removed for C, so the parent's default"
    );
    assert_eq!(
        default("text/x-c++src"),
        Some(("org.kde.kate".into(), "text/plain".into()))
    );
    assert_eq!(
        default("application/x-oldtype"),
        Some(("ark".into(), "application/x-newtype".into())),
        "an alias in the list"
    );
    assert_eq!(
        default("application/x-compressed-tar"),
        Some(("ark".into(), "application/gzip".into())),
        "the parent's listed application"
    );
    assert_eq!(default("video/mp4"), None);
}

/// Programs and launchers by type, their subclasses and aliases too, never by `text/plain`'s
/// fallback; an unnamed file by its first bytes.
#[test]
fn programs_are_known_by_type_and_first_bytes() {
    let t = temp_dir();
    let r = t.path();
    write(
        &r.join("sys/mime/subclasses"),
        "application/x-pie-executable application/x-executable\n\
         application/x-shellscript text/plain\n",
    );
    write(
        &r.join("sys/mime/aliases"),
        "application/x-sh application/x-shellscript\n",
    );
    let db = MimeDb::load(&dirs(r));
    assert_eq!(
        executable("application/x-desktop", &db),
        Some("a desktop entry (a launcher)")
    );
    assert_eq!(executable("application/x-sh", &db), Some("a shell script"));
    assert_eq!(
        executable("application/vnd.appimage", &db),
        Some("an AppImage (a program)")
    );
    for fine in ["text/plain", "text/x-log", "image/png", "inode/directory"] {
        assert_eq!(executable(fine, &db), None, "{fine}");
    }
    let home = r.join("home");
    for (name, bytes, want) in [
        (
            "elf",
            b"\x7fELF\x02\x01".as_slice(),
            "application/x-executable",
        ),
        (
            "script",
            b"#!/bin/sh\necho".as_slice(),
            "application/x-shellscript",
        ),
        (
            "exe",
            b"MZ\x90\x00".as_slice(),
            "application/x-ms-dos-executable",
        ),
        ("note", b"just text\n".as_slice(), "text/plain"),
    ] {
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(name), bytes).unwrap();
        assert_eq!(db.of_path(&home.join(name)), want, "{name}");
    }
}

/// A scheme is one where an application here handles it, or it is a common one.
#[test]
fn schemes_are_the_ones_the_session_handles() {
    let t = temp_dir();
    let r = t.path();
    write(
        &r.join("config/mimeapps.list"),
        "[Default Applications]\nx-scheme-handler/zoommtg=zoom.desktop\n",
    );
    let apps = [app(
        "org.example.chat",
        &["x-scheme-handler/matrix", "x-scheme-handler/tg"],
    )];
    let s = Schemes::load(&dirs(r), &apps);
    for known in ["zoommtg", "tg", "https", "mailto", "HTTPS"] {
        assert!(s.known(known), "{known}");
    }
    assert!(!s.known("notes"));
}
