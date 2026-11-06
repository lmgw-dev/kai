//! The RPM's native-messaging manifest and where the spec installs it (kai-firefox design §4.5,
//! §10). The manifest comes from `scripts/browser-manifest.sh` and its one template
//! (`packaging/dev.rootshell.kai.json.in`), which `scripts/browser-dev.sh` uses too; these tests run
//! that script as the spec does and read the spec's own text, so a path changed in one place and
//! not the other fails here, without an RPM build.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// Where Fedora's RPM puts the host (`%{_libexecdir}/kai`): what the manifest names.
const HOST: &str = "/usr/libexec/kai/kai-browser-host";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn spec() -> String {
    std::fs::read_to_string(root().join("packaging/kai.spec")).unwrap()
}

/// The manifest the script prints for `host`, as the spec's `%install` runs it.
fn manifest(host: &str, note: Option<&str>) -> Value {
    let mut cmd = Command::new("bash");
    cmd.current_dir(root())
        .arg("scripts/browser-manifest.sh")
        .arg(host);
    cmd.args(note);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// What a line of the spec says, after its macros for the folders: the standard values of Fedora
/// on x86_64.
fn expanded(spec_text: &str) -> String {
    spec_text
        .replace("%{_libexecdir}", "/usr/libexec")
        .replace("%{_libdir}", "/usr/lib64")
}

#[test]
fn the_system_manifest_names_the_host_stdio_and_only_kai_for_firefox() {
    let m = manifest(HOST, None);
    assert_eq!(
        m,
        json!({
            "name": crate::APP_ID,
            "description": "Kai for Firefox's link to Kai",
            "path": HOST,
            "type": "stdio",
            "allowed_extensions": ["kai-firefox@rootshell.dev"],
        })
    );
    // Firefox needs exactly these members of a native manifest, and no other extension may start
    // the host.
    let mut keys: Vec<_> = m.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["allowed_extensions", "description", "name", "path", "type"]
    );
}

#[test]
fn a_dev_manifest_differs_only_in_its_path_and_its_note() {
    let dev = manifest(
        "/home/owner/kai/target/debug/kai-browser-host",
        Some("dev build /home/owner/kai/target/debug/kai-browser-host"),
    );
    let mut system = manifest(HOST, None);
    for key in ["path", "description"] {
        assert_ne!(dev[key], system[key], "{key}");
        system[key] = dev[key].clone();
    }
    assert_eq!(dev, system);
    assert_eq!(
        dev["description"],
        "Kai for Firefox's link to Kai (dev build /home/owner/kai/target/debug/kai-browser-host)"
    );
}

#[test]
fn a_path_is_escaped_as_json_and_a_bad_one_refused() {
    let odd = "/tmp/kai \"quoted\" \\back & @HOST@ @DESCRIPTION@/kai-browser-host";
    assert_eq!(manifest(odd, None)["path"], odd);
    for bad in ["relative/host", "/tmp/new\nline/host"] {
        let out = Command::new("bash")
            .current_dir(root())
            .args(["scripts/browser-manifest.sh", bad])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{bad:?}");
        assert!(out.stdout.is_empty(), "no manifest for {bad:?}");
    }
}

/// The spec builds the host with Kai, installs it to libexec and the manifest to the one system
/// folder a 64-bit Firefox reads (Firefox 159's `NativeManifests.sys.mjs` looks in the user's
/// `~/.mozilla/native-messaging-hosts` and in `XRESysNativeManifests`, `/usr/lib64/mozilla`), made
/// by the same script, and lists both files.
#[test]
fn the_spec_installs_the_host_and_the_manifest_where_firefox_looks() {
    let text = spec();
    let line = |wanted: &str| text.lines().any(|l| l.trim() == wanted);
    let global = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("%global {name} ")))
            .map(|v| expanded(v.trim()))
            .unwrap_or_else(|| panic!("no %global {name}"))
    };
    assert_eq!(global("browser_host"), HOST);
    assert_eq!(
        global("native_hosts"),
        "/usr/lib64/mozilla/native-messaging-hosts"
    );
    // The workspace is built, so the host is in the same build as Kai.
    assert!(
        line("cargo build --release --locked --workspace"),
        "the build does not take the workspace"
    );
    assert!(
        line(
            "install -Dsm755 \"${CARGO_TARGET_DIR:-target}/release/kai-browser-host\" \
             %{buildroot}%{browser_host}"
        ),
        "the host is not installed from the release build"
    );
    assert!(
        line("bash scripts/browser-manifest.sh %{browser_host} > dev.rootshell.kai.json"),
        "the manifest is not made by the script"
    );
    assert!(
        line(
            "install -Dm644 dev.rootshell.kai.json \
             %{buildroot}%{native_hosts}/dev.rootshell.kai.json"
        ),
        "the manifest is not installed in the system folder"
    );
    // %files lists both and the host's folder.
    let files = text.split("\n%files\n").nth(1).expect("a %files section");
    let files = files.split("\n%changelog").next().unwrap();
    for entry in [
        "%dir %{_libexecdir}/kai",
        "%{browser_host}",
        "%{native_hosts}/dev.rootshell.kai.json",
    ] {
        assert!(
            files.lines().any(|l| l.trim() == entry),
            "%files lacks {entry}"
        );
    }
    // The manifest the spec's script line makes names the host the spec installs.
    assert_eq!(manifest(&global("browser_host"), None)["path"], HOST);
    // The folder belongs to Fedora's mozilla-filesystem, which the package requires.
    assert!(
        line("Requires:       mozilla-filesystem"),
        "mozilla-filesystem owns the manifests' folder"
    );
}
