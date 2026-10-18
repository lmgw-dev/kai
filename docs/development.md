# Developing Kai

How the source is laid out, how to build and check it, and how releases are made. For what Kai
does and how to install it, see the [README](../README.md). The full design record, with the
reasoning behind each part, is [design/2026-10-06-kai-design.md](design/2026-10-06-kai-design.md);
the `§` numbers below point into it.

## Layout

- The workspace has two members: the binary `kai` at the root, and `crates/kai-desktop`, the tool
  contract (every tool's name, description, parameters and MCP annotations, with no desktop or
  runtime dependency). `just check` and `just test` cover both.
- `ui/` is the settings window's page, a Leptos app compiled to WebAssembly with trunk into
  `ui/dist`. A debug build reads it from disk, a release build embeds it.
- lmgw's checkout sits next to this repository (`../lmgw`), and Kai takes `lmgw-client`,
  `lmgw-api-types`, `lmgw-update` and `lmgw-ui-kit` from it by path (`Cargo.toml`). A worktree of
  Kai must be a sibling of it too (`../kai-wt-<name>`), never nested inside another repository.
- `spikes/` holds trial builds that answered design questions before the design was written.

The main modules in `src/`:

| Module | What it is |
| --- | --- |
| `lmgw_link.rs`, `lmgw_link/` | The link to lmgw (§3): the device link's TLS (`tls.rs`: the one client configuration every connection comes from, the URLs and their order), the Chat feed (`feed.rs`, on `lmgw-client`'s `FeedReader`), the voice's realtime adapter (`realtime.rs`) |
| `pairing.rs`, `pairing/` | Pairing over lmgw's device link (§3.1): the `lmgw-pair:` link v2 (`lmgw-client`'s), the device's P-256 key (`key.rs`), the enrolment, and what a pairing keeps: the key in the Secret Service (`secret.rs`), the rest in `$XDG_CONFIG_HOME/kai/device-link.json` |
| `voice/` | The voice session (§5). It reaches lmgw only through the `VoiceLink` seam in `voice/link.rs`; `voice/mock.rs` is the scripted lmgw the tests use |
| `audio/` | PipeWire capture and playback, the echo modes, the level taps (§6) |
| `conversation.rs` | The Kai conversation's folder (§4): looked up by `conversation.folder_name`, created when missing, the steps it needs, recovery when it changes. **Open in lmgw** opens `<lmgw's dashboard>/chat?t=<thread>` with `xdg-open`, the address lmgw's feed sends in its `hello` |
| `shortcuts.rs` | Global shortcuts through the XDG GlobalShortcuts portal (§8) |
| `tray/` | The tray item, its icons and menu (§7) |
| `overlay/` | The ring overlay (§9): a layer-shell window with its own WebKit view. Its page is `assets/overlay/`, whose `viz/` holds lmgw's `engine.js` and `ring.js`, vendored byte-identical at the commit in `viz/PIN` |
| `widgets/` | Tool-controlled overlays and the MCP Apps host (§10): text, HTML, image, emote, weather, `overlay_app` |
| `host/` | The MCP server Kai serves to lmgw's `/mcp/host` over the device link (§10.1, §11.1): the registry of providers (`registry.rs`), the policy (`policy.rs`), the confirmation dialog (`confirm.rs`), MCP tasks (`tasks.rs`). lmgw lists the tools as `<label>__<tool>` under the device's hosting grant |
| `desktop/` | The desktop providers behind the tools: KWin, portals, accessibility, search |
| `approvals/` | The approval card (§11.1): a layer-shell surface with keyboard on demand, its page in `assets/approvals/`, the spoken answer (`words.rs`) |
| `jobs/` | Asynchronous jobs: a long tool call runs as an MCP task and its result is spoken at a quiet moment or notified |
| `delegate/` | The `look` and `find` helpers: a second model with a tool loop |
| `routines/` | Routines from `$XDG_DATA_HOME/kai/routines/<name>/routine.toml` (format in `routines/def.rs`) |
| `settings/` | Configuration operations and the settings window |
| `cli/` | `kai tool` and `kai model` over a socket of the running Kai |
| `update.rs`, `update/` | The self-updater (§13.1) |
| `config.rs` | `$XDG_CONFIG_HOME/kai/config.toml`, every key documented in its module comment |

## Everyday commands

```sh
just build          # debug build
just test           # every test of the workspace
just check          # formatting, clippy (warnings are errors), the tests, doc tests, the ring's
                    # vendored files, the dev-grant and publish-script checks, the page's gates and build
just ui             # build the settings window's page (ui/dist)
just install-dev    # the .desktop file and icons a debug run needs (the portal refuses the app id otherwise)
just run            # the debug build in its own app scope; Ctrl+C quits
just rpm            # the RPM, in a Fedora Podman container
just readme-shots   # the README's screenshots (docs/images/), from stand-in content
```

`just --list` shows every recipe. A few more:

- **KWin's restricted interfaces for a debug build.** The RPM's `.desktop` file declares the
  interfaces the tools use (window management, screenshots, virtual input). A debug build gets them
  only by opting in: `just grant-dev` writes a `.desktop` file for this checkout's binary,
  `just revoke-dev` removes it, `just grants-dev` lists all of them and marks those whose binary is
  gone. `install-dev` leaves the interface keys out.
- **The ring's vendored files.** `just viz-check` compares them with lmgw (it skips itself next to
  a public clone of lmgw, which lacks the vendoring history) and checks the adapter and palette;
  `just viz-vendor` copies them again; `just viz-reference` regenerates the level reference from
  lmgw's `engine.js`.
- **Tool list cost.** `just tool-cost <label> [aliases…]` counts the tokens of Kai's tool list as
  lmgw sends it to a model, through lmgw's `/v1/count_tokens` (needs `LMGW_KEY`).
- **Configuration and logging.** `$XDG_CONFIG_HOME/kai/config.toml`, keys in `src/config.rs`; log
  level `KAI_LOG`.

## Live checks

Unit tests run against fakes. The live checks run the real thing, isolated from your session:
nested KWins with their own bus and XDG directories (`scripts/nested-kwin.sh`, nothing appears on
your screen), private PipeWire instances (`scripts/pw-private.sh`, nothing is audible), and a dev
lmgw on `127.0.0.1:8897` with its own data directory and mocked models (no GPU). Those that need a
dev lmgw read it from `../lmgw`, whose headless example must be built first
(`cargo build -p lmgw-core --example headless` there). Reports land in `target/live-*/`.

| Recipe | Checks |
| --- | --- |
| `live-check-link` | Pairing, the GPU hold badge, lmgw stopping and starting, every kind of revocation |
| `live-check-audio <clip>` | Capture, playback and the heard position in a private PipeWire |
| `live-check-voice`, `live-check-voice-lmgw` | The voice session against a scripted lmgw, then a dev lmgw: round trips, barge-in, takeover, restarts |
| `live-check-conversation` | The Kai folder from a first run: steps, recovery, the idle rollover, a new conversation, a second device |
| `live-check-shortcuts` | The shortcuts portal up to the bind, with no dialog and nothing registered |
| `live-check-ring` | The ring over the desktop: states, cost, crashes, outputs going away |
| `live-check-host` | The hosted MCP server against a dev lmgw with a tool-calling mock |
| `live-check-desktop`, `live-check-read`, `live-check-launch`, `live-check-input` | The window, reading, launching and input tools in a nested KWin |
| `live-check-widgets` | The overlays' sandbox, sound, positions, memory, MCP Apps views |
| `live-check-approvals` | The approval card's keys, route and spoken answer, and the screen lock |
| `live-check-jobs` | An asynchronous job end to end |
| `live-check-routines` | A routine's script calling `kai tool` from its own scope |
| `live-check-window` | The settings window driven through its page against real configuration operations |

The clip the audio checks take is any 24 kHz mono PCM16 WAV; it is never committed.
`live-check-ring` shows the ring on your real desktop for about two minutes (its levels still come
from a private PipeWire); the others show nothing on your screen. The `owner-check-*` recipes are
the manual counterparts on real devices and real key presses, and ask before each step.

`just live-check-link` uses the `live-test` cargo feature (debug builds only): `KAI_LIVE_LINK=1`
confirms pairing links without the dialog. Kai pairs over lmgw's device link (enrolment and mutual
TLS), so every check that pairs does what an owner does on the dev lmgw's `/api` with its owner
bearer: the device listener on (`settings_set {"devices": {"listener": true}}`, 127.0.0.1:8898
beside the dev lmgw's 8897), `key_create` with `local_only` for the v2 link, Kai's enrolment of its
own key, then `device_confirm {id, confirm_code}` with the code Kai shows. The scripts share
`scripts/live-device-link.sh` for it: `live-check-link` and `live-check-host` hand the link to the
live-test Kai and read the code from its tooltip; `live-check-window` and `live-check-widgets`
enrol with the test binary's ignored `pairing::tests::live_enrol` and hand the pairing to their
live test; the Rust live tests pair with `voice::lmgw_live_tests::device`.
`scripts/live-check-pairing.sh` needs no lmgw: it hands over a v2 link that is never confirmed.

Unit tests never reach lmgw: `lmgw_link::test_pki` is a mock device listener (TLS 1.3 with a
device CA, server leaves and device certificates rcgen makes), and every test server of Kai's
lmgw-facing code runs behind it.

## The lmgw pin

`LMGW_PIN` holds three lines naming the same lmgw source:

- `lmgw_commit`, the commit in lmgw's development history, which GitLab's CI clones;
- `lmgw_tree`, its tree. The public repository gets one squashed commit per release, with other
  hashes but the same tree, so the tree is what identifies the source;
- `lmgw_tag`, the published lmgw release ([github.com/lmgw-dev/lmgw](https://github.com/lmgw-dev/lmgw))
  with that tree, which GitHub's CI clones, or `unpublished` while no release has it yet.

So Kai follows lmgw's development without waiting for its releases: GitLab's builds take the commit
and never depend on the tag. Only publishing Kai to GitHub needs a published one: `scripts/release.sh`
and `scripts/publish-github.sh` refuse a pin whose tag is `unpublished` or names a release with
another tree (`scripts/lmgw-pin.sh verify-published`, which fetches that tag alone into
`target/lmgw-public.git`; `scripts/publish-github.sh` checks the pin committed in the tree it
publishes).

`scripts/lmgw-pin.sh verify-tree <checkout>` fails unless a checkout holds the pinned tree (GitHub's
CI runs it on its clone). `just check` and the live checks build with whatever `../lmgw` holds and
warn, never fail, when it is not at the pinned commit or tree, has changes, when `lmgw_commit`'s
tree is not `lmgw_tree`, or when the checkout's own tag `lmgw_tag` has another tree. `just rpm`
always stages the four crates and lmgw's workspace root from the pinned tree (`git archive`), so
the RPM is the same whatever the checkout has checked out; that checkout must hold the tree
wherever the RPM is built.

**Bumping it to a new lmgw commit** (any commit of lmgw's `main`; GitLab's next build takes it):

1. Check out the new lmgw commit in `../lmgw` and build its headless example.
2. Run `just check` and the live checks that touch lmgw: `live-check-link`, `live-check-host`,
   `live-check-voice-lmgw`, `live-check-conversation`. Each runs on a private bus or PipeWire, so a
   running Kai may stay up.
3. Write `lmgw_commit` (`git -C ../lmgw rev-parse HEAD`) and `lmgw_tree`
   (`git -C ../lmgw rev-parse 'HEAD^{tree}'`). Set `lmgw_tag` to the published release with that
   tree, if there is one (`scripts/lmgw-pin.sh verify-published` checks it), else to `unpublished`.
4. Commit the three lines with the results and the `Cargo.lock` the build updated (lmgw's crates
   carry its version).

**Naming the release** (before Kai goes to GitHub): once lmgw has published the pinned tree, write
its tag to `lmgw_tag` and run `scripts/lmgw-pin.sh verify-published`. When lmgw's release has other
source than the pin, bump to the release's commit first (above).

## The RPM

`packaging/kai.spec` and `packaging/build-rpm.sh`: the build stages lmgw's four crates and a
minimal workspace root from the pinned tree into the source tarball, then builds in a Fedora Podman
container of the host's release (the image from `packaging/Containerfile`, built once). Only
`target/rpm` is mounted, with `:Z`; the output lands in `target/rpm/out/x86_64/`. The settings
page is built inside with trunk (`packaging/build-rpm.sh --page-only` stops after it).

## Updates

Kai looks for a newer build in its update feed and offers it in a notification (**Install**,
**Later**, then **Restart**); the tray's **Check for Updates…** always answers. There is no built-in
feed: a CI build compiles one in through `KAI_UPDATE_MANIFEST_URL`, and a local `just rpm` has none,
and says so.

- **The public feed.** A GitHub release build (`packaging/gh-build.sh`,
  `.github/workflows/release.yml`) points at this repository's
  `releases/latest/download/latest.json` and is marked public (`KAI_UPDATE_PUBLIC_FEED`), so it
  never sends its feed a token.
- **The GitLab feed.** Development builds come from `.gitlab-ci.yml` on a self-hosted GitLab that
  holds Kai's and lmgw's projects side by side: every push to `main` builds `X.Y.Z+<pipeline>`
  against the lmgw commit `LMGW_PIN` names and publishes it with its `latest.json` to the project's
  generic package registry. The build compiles that feed in with a read-only deploy token (`KAI_UPDATE_DEPLOY_TOKEN`),
  so it follows `main`, never GitHub.
- **Which feed.** An installed Kai follows the feed it was built with; there is no channel setting.
  Installing one RPM of the other kind switches it. `X.Y.Z+<pipeline>` sorts after `X.Y.Z`, so going
  from a GitLab build back to the release of the same version is a `dnf downgrade`.
- **Another feed.** `KAI_UPDATE_ENDPOINT` points a running Kai at another feed, and
  `KAI_REGISTRY_TOKEN` gives it a token, only together with `KAI_UPDATE_ENDPOINT` and never to a
  GitHub host. The compiled-in feed ignores that token, and the log says so.
- **What Kai installs.** A manifest must name the RPM's sha256 as 64 hex digits and an https URL;
  anything else is refused before a download starts. Redirects from https to http are never
  followed. Installing needs `pkexec` and `dnf`.
- **Versions.** A GitHub release's version is the tag's `X.Y.Z`, which must equal `Cargo.toml`'s.
- `[update]` in the configuration sets the background check (on, 20 s after the start, then every
  6 h).

## Releases and publishing

- `scripts/release.sh X.Y.Z [notes]` sets the version in every `Cargo.toml`, the spec's default and
  both lock files, commits "release X.Y.Z" and makes the annotated tag `vX.Y.Z`, whose message
  becomes the release notes. When the tree already carries the version it only tags. It pushes
  nothing.
- This repository receives one squashed commit per publish, an audited and scanned copy of a tree,
  never the development history: `scripts/publish-github.sh --ref vX.Y.Z -F <message> --tag
  vX.Y.Z` (with `scripts/publish_scan.py`). It reads the public parent from the push URL and refuses
  one that descends from the development history's root. `--tag` puts the release tag on the
  commit.
- `--install-guard` disables a plain `git push github` (and refuses a `github` remote that names a
  different URL) and installs `scripts/pre-push-github.sh` as the pre-push hook. The hook refuses
  any push to GitHub, in any letter case, that does not come from the publish script, and refuses
  the development history to every URL but origin's.
- `python3 scripts/publish_github_test.py` (part of `just check`) tests the script against local
  bare repositories. The commit's author and committer must be a GitHub no-reply address.
- `.github/workflows/release.yml` runs on the tag in two jobs: a read-only one builds the RPM and
  `latest.json`, and a separate one with write access checks the RPM's sha256 against
  `latest.json` before it publishes the release, so the token that can write never meets the build.

## CI

`.github/workflows/ci.yml` runs formatting, clippy, the tests, the script checks and the page's
build. It skips `viz-check` (which needs lmgw's development history and says so next to a public
clone) and the live checks, which need a desktop session.

## The README's screenshots

`just readme-shots` (`scripts/readme-shots.sh`) makes `docs/images/` again from stand-in content
only: a nested KWin with one 1080p output at 125 % and a plain backdrop
(`scripts/readme-backdrop.py`) for the overlays and the approval card
(`src/widgets/live_tests/readme.rs`), and a second one for the settings window
(`readme_window_nested` in `src/settings/window/live_tests.rs`). It starts no lmgw and contacts no
host; the weather comes from a loopback stand-in. Look at every image before committing it.
