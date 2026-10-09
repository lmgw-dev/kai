# Kai: a Plasma client of lmgw (design §13).
#
# Built by `just rpm` (packaging/build-rpm.sh in a Fedora podman container), by .gitlab-ci.yml and
# by .github/workflows/release.yml.
# Source0 is the tarball packaging/build-rpm.sh makes: kai/ (the tracked files of this repository,
# and ui/dist, the configuration window's page that script built with trunk) next to lmgw/ (the
# lmgw-client, lmgw-api-types, lmgw-update and lmgw-ui-kit crates, see "lmgw-client" below).
#
# Version: the version of Cargo.toml; GitLab CI appends +<pipeline IID> (lmgw's convention, so a
# private build sorts after the release it follows and before the next one); a GitHub release is
# the plain X.Y.Z of its tag:
#   rpmbuild --define 'kai_version 0.1.0+12' ...
# %build hands it to cargo as KAI_VERSION, so the binary reports and compares the same version as
# the package (`kai --version`, the updater); Cargo.toml keeps X.Y.Z, so `--locked` holds.
# The updater's feed (src/update.rs) is compiled in from KAI_UPDATE_MANIFEST_URL,
# KAI_UPDATE_DEPLOY_TOKEN and KAI_UPDATE_PUBLIC_FEED in rpmbuild's environment (CI sets them:
# packaging/ci-build.sh the private feed with its token, packaging/gh-build.sh the public feed,
# marked public so it never gets a token); a build without them has no feed.
%{!?kai_version: %global kai_version 0.1.0}
# The lmgw commit the tarball's lmgw crates come from (LMGW_PIN; packaging/build-rpm.sh defines it),
# and how the description names that source: "commit <sha>" where the build's lmgw checkout holds the
# pinned commit (the private checkout), else "<lmgw_tag> (tree <sha>)" (a clone of the public
# repository, whose commits differ but whose tree at the tag is the pinned tree).
%{!?lmgw_commit: %global lmgw_commit unknown}
%{!?lmgw_source: %global lmgw_source commit %{lmgw_commit}}

# lmgw-client. Kai takes it by path (Cargo.toml). Three ways to make that path resolvable in a build:
#   1. a checkout with lmgw next to this repository (Cargo.toml's path,
#      ../lmgw/crates/lmgw-client). packaging/build-rpm.sh stages exactly the four crates and a
#      minimal workspace root into the tarball, from the tree LMGW_PIN names (`git archive`, not
#      the working tree), sets the paths in the staged Cargo.toml to ../lmgw/crates/<crate>,
#      and so needs no network for lmgw. This is what is used today. lmgw-update (the updater's
#      mechanism) comes the same way.
#   2. `cargo vendor` before the build with the path dependency kept: not needed, a path crate
#      is never vendored; only its registry dependencies are (offline builds, Fedora's guidelines).
#   3. lmgw-client as a git or registry dependency: a later change to Cargo.toml, none to this spec.
# The registry dependencies are downloaded at build time (not a Fedora package).

Name:           kai
Version:        %{kai_version}
Release:        1%{?dist}
Summary:        Voice, tray state and overlays for lmgw on Plasma
License:        FSL-1.1-MIT
URL:            https://github.com/lmgw-dev/kai
Source0:        kai-%{version}.tar.gz

# Rust's own debug info is not packaged (lmgw's RPM has none either); the binary is stripped.
%global debug_package %{nil}

BuildRequires:  rust
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  pkgconf-pkg-config
# bindgen (pipewire-sys, libspa-sys) needs libclang.
BuildRequires:  clang
BuildRequires:  clang-devel
BuildRequires:  gtk3-devel
BuildRequires:  webkit2gtk4.1-devel
BuildRequires:  libsoup3-devel
BuildRequires:  gtk-layer-shell-devel
BuildRequires:  pipewire-devel
BuildRequires:  wayland-devel

# Shared libraries are found by rpm itself (GTK 3, WebKitGTK 4.1, gtk-layer-shell, libpipewire).
# What Kai talks to at run time:
# The global shortcuts portal and the Secret Service; KDE's backend implements the portal.
Requires:       xdg-desktop-portal-kde
# Echo mode `app` (§6.2) starts `pipewire` with module-echo-cancel and needs
# spa-0.2/aec/libspa-aec-webrtc.so, which Fedora ships in pipewire-libs (a dependency of pipewire;
# no package of its own, so no explicit Requires, which rpmlint rejects for libraries).
Requires:       pipewire
# Open in lmgw.
Requires:       xdg-utils
# The Secret Service for the device key (KWallet answers it on Plasma).
Recommends:     kf6-kwallet

%description
Kai is a Plasma desktop client of lmgw, the local model gateway:
voice at any time through global shortcuts, a tray item with the gateway's
state, a ring overlay while a turn runs, and the lmgw-pair: link handler
that pairs it with a gateway.

Built with lmgw-client and lmgw-update of lmgw %{lmgw_source}.

%prep
%autosetup -c -n kai-%{version}

%build
# Fedora's build flags (-O2 -flto, hardening specs) break aws-lc-sys's jitterentropy (it must be
# compiled -O0); cargo's own release profile and cc-rs decide here.
unset CFLAGS CXXFLAGS FFLAGS FCFLAGS LDFLAGS RUSTFLAGS
cd kai
# The page is embedded by the release build (src/settings/window/assets.rs); without it the window
# would open on an apology. packaging/build-rpm.sh builds it into the tarball.
test -f ui/dist/index.html || { echo "ui/dist is missing: build the RPM with packaging/build-rpm.sh" >&2; exit 1; }
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}
export KAI_VERSION=%{kai_version}
cargo build --release --locked

%install
cd kai
install -Dsm755 "${CARGO_TARGET_DIR:-target}/release/kai" %{buildroot}%{_bindir}/kai
sed 's|@EXEC@|%{_bindir}/kai|' packaging/dev.rootshell.kai.desktop.in \
    > dev.rootshell.kai.desktop
install -Dm644 dev.rootshell.kai.desktop \
    %{buildroot}%{_datadir}/applications/dev.rootshell.kai.desktop
install -Dm644 assets/icons/hicolor/scalable/apps/dev.rootshell.kai.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/dev.rootshell.kai.svg
for f in assets/icons/hicolor/scalable/status/kai-tray-*.svg; do
    install -Dm644 "$f" %{buildroot}%{_datadir}/icons/hicolor/scalable/status/$(basename "$f")
done

# No %%post/%%postun: Fedora's file triggers (desktop-file-utils, gtk-update-icon-cache, shared-mime-info)
# refresh the desktop database and the icon cache when these files are installed or removed, which
# is the convention of lmgw's Tauri-built RPM too (it has no scriptlets).
# No autostart file: "Start with Plasma" is per user (~/.config/autostart/dev.rootshell.kai.desktop),
# written by Kai itself after the first pairing.

%files
%license kai/LICENSE.md
%{_bindir}/kai
%{_datadir}/applications/dev.rootshell.kai.desktop
%{_datadir}/icons/hicolor/scalable/apps/dev.rootshell.kai.svg
%{_datadir}/icons/hicolor/scalable/status/kai-tray-*.svg

%changelog
* Wed Oct 07 2026 The owner <owner@example.invalid> - 0.1.0-1
- First package
