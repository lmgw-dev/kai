# Developing Kai

How the source is laid out, how to build and check it, and how releases are made. For what Kai
does and how to install it, see the [README](../README.md). The full design record, with the
reasoning behind each part, is [design/2026-10-06-kai-design.md](design/2026-10-06-kai-design.md);
the `§` numbers below point into it.

## Layout

- The workspace's members: the binary `kai` at the root; `crates/kai-desktop`, the tool contract
  (every tool's name, description, parameters and MCP annotations, with no desktop or runtime
  dependency); `crates/kai-delegate`, the delegates' loop (K25); and `crates/kai-browser-host`, the
  small binary Firefox starts for Kai for Firefox (native messaging), which relays frames between
  Firefox's stdio and Kai's `browser.sock` and holds nothing. `just check` and `just test` cover
  them all.
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
| `pairing.rs`, `pairing/` | Pairing over lmgw's device link (§3.1): the `lmgw-pair:` link v2 (`lmgw-client`'s), the device's P-256 key (`key.rs`), the enrolment, and what a pairing keeps: the key in the Secret Service (`secret.rs`), the rest in `$XDG_CONFIG_HOME/kai/device-link.json`. The enrolment declares the computer's own name (`pairing::device_name`: the pretty hostname, else the hostname; lmgw's `device_name`), the client text and Kai's app id (`pairing::APP_ID`, `dev.rootshell.kai`, the desktop file's), which the Devices card shows (the name and the client text as the device's own claim) and which transfer offers need; a pairing made before keeps none until it is paired again |
| `voice/` | The voice session (§5). It reaches lmgw only through the `VoiceLink` seam in `voice/link.rs`; `voice/mock.rs` is the scripted lmgw the tests use. `voice/speak.rs` is Kai's speech without a model (kai-firefox §7.2, §7.3: announcements and reading aloud, handed over as `Command::Speak`): `POST /v1/audio/speech` as WAV, one request per sentence (`audio/wav.rs` makes it 24 kHz mono through a windowed-sinc low-pass), in Kai's own voice (its conversation thread's `voice_resolved`, read once per run of speech, Kai's words in its reply language for German, else English) or a language's (`browser.read.voices`); made and played only at the quiet moment the jobs wait for (`voice/jobs.rs`' `not_quiet`) with do not disturb off, PipeWire up and `browser.speech_quiet_ms` after it, what waits told why; the sender's gate checked when made and when played; cut by the owner's press or speech, a barge-in over it spent with `input_audio_buffer.clear` and the session silent for `browser.speech_echo_guard_ms` after it stops, so its echo is never the owner's turn; a reply taking the playback back and the paragraph heard again after it; a notice that cannot be spoken notified instead; Kai's own questions (`Kind::Question`, `approvals/asked.rs`) first in line while their call waits, one at a time, once Kai's reply has stopped and no talk key is held; a spoken answer (a question's or the approval card's) starts at the press with no pre-roll, the press stops Kai's own speech, and what the microphone hears while Kai speaks and `browser.speech_echo_guard_ms` after is silence in it |
| `audio/` | PipeWire capture and playback, the echo modes, the level taps (§6) |
| `conversation.rs` | The Kai conversation's folder (§4): looked up by `conversation.folder_name`, created when missing, the steps it needs, recovery when it changes. **Open in lmgw** opens `<lmgw's dashboard>/chat?t=<thread>` with `xdg-open`, the address lmgw's feed sends in its `hello` |
| `shortcuts.rs` | Global shortcuts through the XDG GlobalShortcuts portal (§8) |
| `tray/` | The tray item, its icons and menu (§7) |
| `overlay/` | The ring overlay (§9): a layer-shell window with its own WebKit view. Its page is `assets/overlay/`, whose `viz/` holds lmgw's `engine.js` and `ring.js`, vendored byte-identical at the commit in `viz/PIN` |
| `widgets/` | Tool-controlled overlays and the MCP Apps host (§10): text, HTML, image, emote, weather, `overlay_app` |
| `host/` | The MCP server Kai serves to lmgw's `/mcp/host` over the device link (§10.1, §11.1): the registry of providers (`registry.rs`), the policy (`policy.rs`), the confirmation dialog (`confirm.rs`), MCP tasks (`tasks.rs`). lmgw lists the tools as `<label>__<tool>` under the device's hosting grant |
| `desktop/` | The desktop providers behind the tools: KWin, portals, accessibility, search. `sites.rs` holds Kai for Firefox's site rules over the tools that capture, read or act in a window (`browser/shown.rs` tells what a Firefox window shows by its tab in front, over all of that Firefox's windows in one look, and which frame has the keyboard focus): `see_window`, `read_window` and `read_selection` refuse a window on a `none` site, `ui_press`, `type_text` and `key_press` a `read` one too; `sites/pages.rs` judges the pages and frames read or pressed in by their AT-SPI `DocURL`, `sites/keys.rs` the chords and each key's window and focused frame, `sites/screen.rs` blanks such windows grey by their frames (`Windows::placed`) and waits for KWin's moving effects to end. `fdo/atspi.rs` is the AT-SPI provider (its walk in `fdo/atspi/walk.rs`): it walks, reads and presses only the pages a window shows (a document directly below a window's UI only while `SHOWING`, since Firefox marks a background tab's page visible too), passes Firefox's empty `DocURL` (`data:`, `blob:`) on as empty, and reads an element's actions again after `accessibility.action_settle_ms` where the first read said none; `fdo/atspi/whole.rs` gives Kai's browser module an app's whole tree with each element's id, classes, `SHOWING`, place and action, an element's subtree, an element by its place (refused once it is not shown), the elements a page marked in one document (the Collection interface: one, none or several) and a dialog field's text |
| `approvals/` | The approval card (§11.1): a layer-shell surface with keyboard on demand, its page in `assets/approvals/`, the spoken answer (`words.rs`); Kai's own questions answered by voice the same way (`asked.rs`, kai-firefox WP10b: `VoiceConfirmer` beside the dialog, the questions spoken one at a time as `Kind::Question`, the talk key once one was spoken, for the one that became answerable last) |
| `jobs/` | Asynchronous jobs: a long tool call runs as an MCP task and its result is spoken at a quiet moment or notified |
| `delegate/` | The `look` and `find` helpers: a second model with a tool loop |
| `routines/` | Routines from `$XDG_DATA_HOME/kai/routines/<name>/routine.toml` (format in `routines/def.rs`) |
| `settings/` | Configuration operations and the settings window; `settings/catalog/` gives every key its label, help line, section, unit and control |
| `cli/` | `kai tool` and `kai model` over a socket of the running Kai |
| `browser/` | Kai for Firefox's link (kai-firefox's design, `docs/design/2026-11-03-kai-firefox-design.md` in that repository, §4): `browser.sock` with its peer check (a process in Firefox's systemd unit, `browser.peer_scopes`), the `hello`/`welcome` handshake, the links (one per Firefox profile), operations with their results and cancellation, the extensions' events and questions (`links.rs`, the API the browser tools build on), and frames over Firefox's 1 MiB limit (1 048 576 bytes, inclusive) as `chunk` frames both ways (`chunk.rs`: Kai splits what it sends and joins what the extension split). The browser tools' provider (`provider.rs`, the contract in `crates/kai-desktop/src/contract/browser.rs`): a tool is listed while one linked extension serves every operation it needs, the hands-and-eyes-only ones while `browser.hands_and_eyes` is on; `tools.rs` maps each tool to its operations (the tools of later work packages are `Route::NotYet`) and checks the site rules (`sites.rs`) and the focus rule (`moves.rs`: the extension's tab facts first, Kai's own marks where it saw nothing or raised a window, every acting operation marked as Kai's navigation before it is sent, and with the hands-and-eyes switch on the owner's own moves in a turn of Kai's own device (a pressed link or submit control, an Enter that submits a form, the tab tools; a navigation only where it landed on the origin kai-firefox's `goes` names, never while Kai's continuation for jobs' results runs) counted as the owner's act while the switch is on, kept across Kai's restarts in `$XDG_STATE_HOME/kai/browser-moves`; with two profiles the focused window decides, `tabs.rs`, a focus Kai's own input gave a window not counted) before each call (and again after the owner was asked), and asks before `tab_open` or `tab_go` goes to a site the user has never visited (`history.seen@1`'s `seen_by_user`, `tools/guard.rs`); every input Kai sends through the desktop (the trusted press, `type_text`, `key_press`, `ui_press`) is bracketed by the extension's input spans in each linked profile (`spans.rs`, `input.begin@1`/`input.end@1`, `browser.input_settle_ms`), so Firefox never counts it as the user's act; the reading tools, the hints and `element_point` are `tools/read.rs` and `tools/text.rs` with their shared lines in `tools/lines.rs`, and `page_text` on a PDF runs poppler's `pdftotext` on the bytes the extension fetched (`tools/pdf.rs`; the RPM requires `poppler-utils` and recommends `bubblewrap`: `/usr/bin/pdftotext` runs in a `bwrap` sandbox with a cleared environment, bounded by `browser.pdf_timeout_s`, `browser.pdf_text_max_mb` and `browser.pdf_memory_max_mb`, and lines that do not read as text are left out and counted); `refs.rs` keeps Kai's numbers for the extensions' element refs (one number space over the linked profiles, the identity with one; a number is never handed out twice: dropped ones are retired, and the highest is kept in `$XDG_STATE_HOME/kai/browser-refs` across restarts) and resolves a ref to the link that gave it out, and a tool that takes a ref asks the extension what it names now (`element.describe@1`) before the site rules check its tab and its frame's own origin; what comes from a frame of a `none` site is taken out of every answer (`tools/page.rs`), and a failure on a `none` site says only what failed (`tools.rs`); the acting tools (`tools/act.rs`) carry the commit guard (`browser.commit_guard`, `browser.guard_words`, `browser.guard_window_ms`) on every operation, ask the owner once about the page side's list of verdicts when it stops a press or a key (also at its second look, after the events went) and send it again with them confirmed, answer the network gate's `ask` frames with Kai's dialog one at a time per operation (`tools/guard.rs`, the operation found by the id the question names; after a decline the operation's other requests are declined), and `element_file` opens its file at the preparation and writes its bytes into the frame as it reads them (at most what one JavaScript string of Firefox's holds); a press that needs a real key (or every press with `browser.trusted_press = "always"`) takes the trusted path (`tools/trusted.rs`: key_press's policy, or element_press's with the switch on, and asked for any call approved in lmgw; the approval card let go; the input turn; the spans; the window of the link's own Firefox process to the front through the desktop module, `desk.rs`; `focus.check@1` with a gate, its questions held until the key went; Enter or Space through the Input capability; `gate.result@1` with `key_seen`; however it ends, the gate is read and the spans end), and before the key, where the desktop offers accessibility and the extension the mark, through accessibility (`tools/trusted/accessible.rs`: the tab's own window found, the element marked with `element.mark@1`, found in that window's page by its `kai-mark`, unmarked, checked against the extension's role, name and frames and the site rules, `gate.open@1`, the window looked at again, AT-SPI's `doAction`, `gate.result@1` with `click_seen`; an element not found there takes the key, a mark on two elements is refused as tampering); Firefox's own UI, a page's dialogs and Firefox's own pages (`tools/chrome.rs`, kai-firefox WP12: `firefox_ui` and `firefox_press`, listed only with the desktop's accessibility, and `page_text`'s reading of Firefox's own pages, all around the tab in front of its own window only (`tools/chrome/window.rs`), by `page_rule`, a dialog by the origin its title names, a prompt for a site by the sites its words name; `tools/chrome/parts.rs` is the allow-list of what is read in Firefox's window, `tools/chrome/never.rs` what Kai never reads or presses (the password manager, other extensions' pages, saved cards and addresses; `about:config`, `about:preferences` and the other pages that change how Firefox works), `tools/chrome/press.rs` `firefox_press` with the owner's yes to a prompt's words and the last look before `doAction`, `tools/chrome/own.rs` `page_text` on Firefox's own pages); the screenshots (`tools/see.rs`: an MCP image block, listed through the delegate's view of who looks at screenshots, `delegate/screenshots.rs`, like `see_window`; a frame of a `none` site blanked grey in the PNG before it leaves Kai, by the frames' boxes the extension names, and no image where it names none while a site is `none`), Firefox's own features (`tools/features.rs`: the zoom, Reader view, and bookmarks, the history and the downloads from every linked profile with each entry that has a URL on a `none` site left out and counted), the source and the DOM (`tools/source.rs`, whole, Kai's comment lines where a frame begins and ends), the style sheets (`tools/style.rs`: the sheet's `url()`s read from its text for the dialog, a site the user never visited asked about whatever the tool's state, the sheets Kai added kept per link and tab for `page_css_remove`) and the scripts (`tools/script.rs`: confirmed by default, the value and what was thrown said as page text) send the origin the dialog showed where the extension takes one; the events and announcements (`announce.rs`: `events.config@1` after each `welcome` and when `browser.announce` changes it, one event per frame in its link's `seq` order, an event of a `none` site dropped (a download by its URLs, referrer and source), one sentence per event in English and German with Kai's words first and the page's text last (`announce/words.rs`; a live region with its place, and Kai's own questions in words for its voice in `announce/words/asked.rs`), a newer focus or atomic region's change replacing the one that waits while a log's changes queue, a field's error and its live region spoken once in either order, handed to the voice's direct speech with a gate it runs again (switched off, a `none` site, `browser.announce_max_age_s`, Firefox's focus), and the list `browser_context` reads, bounded by `browser.announce_keep_s` with what was dropped counted) and reading aloud (`tools/aloud.rs`: Kai holds the index, `read.paragraph@1` with `[browser.read]`'s rules, the next paragraph handed over while one plays, `read.mark@1` as each begins, pause, resume, skip, the owner's barge-in pausing it, the extension's `read_ended` ending it at once with one spoken notice, a refused paragraph skipped and a failure pausing with a notice, never looping on one failure); `policy.rs` is `[browser]`'s part of K15's policy; `warning.rs` is the design's §8.3 warning (Kai's lmgw admin level while a Firefox is linked; an open settings window reads its line again at each change); `install.rs` is the settings window's "Install Kai for Firefox" (the build's link to the signed XPI, `KAI_FIREFOX_INSTALL_URL`, and the number of linked extensions; see "Kai for Firefox in the RPM") |
| `local_socket.rs` | What `cli.sock` and `browser.sock` share: mode 0600 set between `bind` and `listen` in Kai's runtime folder (made 0700 at Kai's start, `paths::private_dir`), removal at the end, the peer process read through its pidfd (`SO_PEERCRED`'s pid only on a kernel without `SO_PEERPIDFD`) |
| `update.rs`, `update/` | The self-updater (§13.1) |
| `config.rs` | `$XDG_CONFIG_HOME/kai/config.toml`, every key documented in its module comment |

## Everyday commands

```sh
just build          # debug build
just test           # every test of the workspace
just check          # formatting, clippy (warnings are errors), the tests, doc tests, the ring's
                    # vendored files, the dev-grant, browser-dev and publish-script checks, the page's gates and build
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
- **Kai for Firefox in a dev build.** `just browser-dev` builds `kai-browser-host` and writes the
  per-user native manifest `~/.mozilla/native-messaging-hosts/dev.rootshell.kai.json` naming this
  checkout's debug host (type stdio, only `kai-firefox@rootshell.dev` may start it); Firefox reads
  it before the RPM's system manifest, so it shadows that one for every profile (install says so
  when the system manifest is there). One per user: it replaces another checkout's and says
  whose. `just browser-dev-remove` removes it while its `path` names this checkout's host. It writes only
  into an existing `~/.mozilla` (a Firefox that keeps its profiles in the XDG folders is not
  covered yet). The manifest is made from the template the RPM's comes from
  (`packaging/dev.rootshell.kai.json.in` through `scripts/browser-manifest.sh`), and while it
  exists it shadows the RPM's system manifest, so Firefox starts this checkout's debug host and not
  the RPM's; `just browser-dev-remove` is the way back to the RPM's host (it says which manifest
  Firefox reads next, or that there is none). Kai takes the link only from a Firefox in its own
  systemd unit, as Plasma starts
  it (`browser.peer_scopes`, matched against the innermost `.service` or `.scope` of the peer's
  cgroup); a Firefox started from a terminal is refused, and Kai's log names its unit once (the
  host's retries are logged at debug). `KAI_BROWSER_SOCKET` points a test Kai and a host at another socket.
- **Tool list cost.** `just tool-cost <label> [aliases…]` counts the tokens of Kai's tool list as
  lmgw sends it to a model, through lmgw's `/v1/count_tokens` (needs `LMGW_KEY`).
- **Configuration and logging.** `$XDG_CONFIG_HOME/kai/config.toml`, keys in `src/config.rs`; log
  level `KAI_LOG`.
- **A new setting needs its words (2026-10-19).** The owner's rule: no setting ships as a raw
  text field labelled with its config key. Every key the configuration has
  needs an entry in `src/settings/catalog/table.rs`: a label in sentence case, one plain help line,
  its section, its unit, its control (switch, choice with a name per word, number with the
  loader's bounds only, text, list, read-only) and whether it is advanced tuning. A map's entries
  take one pattern entry (`policy.tools.*`). `every_key_has_a_catalog_entry` fails for a key
  without one, and the catalog's other tests check that a choice's words are its type's and a
  number's bounds the loader's. The settings window shows the catalog's words; the key is only a
  row's tooltip, and a refusal names settings by their labels. The owner tools keep the keys.
- **The Conversation choice (2026-10-19).** By default every Kai client (this desktop and every
  Kai phone) shares one conversation, the Chat folder named "Kai". The Settings page's
  Conversation offers "Shared with your other Kai apps" or "Only on this computer" with a name of
  its own (prefilled "Kai <hostname>"; the "Use this conversation" button writes it). Saving
  switches at once (`Conversation::rename`): the old folder is left quietly (listening resumes on
  the new one once it is ready), `conversation.folder_id` removed, and the new name looked up
  (created plain when missing, with the owner's steps for its model, as at a first start). Shared
  again removes `conversation.folder_name` from the file.
- **Pickers.** A setting that names a microphone, speaker or screen is a `Control::Pick`; the page
  reads `GET /api/devices` once. A switch's help says what it does ("When off, …"), a test fails
  for "On, …"/"Off, …".

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
| `atspi-firefox` | Kai's AT-SPI code against a headless Firefox on a private accessibility bus (`scripts/atspi-firefox.sh`): the pages shown, `DocURL`, the actions' settle, the press by a mark, a duplicated mark, a page's `confirm()` and `prompt()`, a site's permission prompt and its words, Firefox's own UI, an error page's window, another extension's page (a temporary add-on through WebDriver BiDi); the session's accessibility bus is never touched |
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

## Kai for Firefox in the RPM

- **What it installs.** The spec builds the workspace (`cargo build --release --locked --workspace`),
  so `kai-browser-host` comes from the same build as Kai, and installs it as
  `/usr/libexec/kai/kai-browser-host` and the system manifest as
  `/usr/lib64/mozilla/native-messaging-hosts/dev.rootshell.kai.json` (`%{_libdir}`, the folder
  Fedora's `mozilla-filesystem` owns; the package requires it). Firefox 159 reads a native manifest
  from `~/.mozilla/native-messaging-hosts` and then from one system folder, which on a 64-bit
  Fedora is `/usr/lib64/mozilla/native-messaging-hosts` (`/usr/lib/mozilla/native-messaging-hosts`
  exists and is never read by it). The manifest names `dev.rootshell.kai`, type `stdio`, the
  absolute host path and `kai-firefox@rootshell.dev` as the only extension that may start it. It is
  made by `scripts/browser-manifest.sh` from `packaging/dev.rootshell.kai.json.in`, the template
  `just browser-dev`'s per-user manifest comes from too. The RPM does not install the extension:
  Firefox takes signed XPIs only.
- **A dev build shadows it.** The per-user manifest `just browser-dev` writes is read first, so
  while it exists every Firefox profile of that user starts the checkout's debug host, whatever
  the RPM installed. `just browser-dev-remove` removes it and says which host Firefox reads next.
- **Tests without an RPM build.** `src/browser/tests/manifest.rs` runs the manifest script as the
  spec does and reads the spec's text (the workspace build, both `install` lines, `%files`);
  `scripts/browser-dev-check.sh` (in `just check`) tests the script and `browser-dev.sh`. Kai has
  no RPM allowlist yet like lmgw's `ci/rpm-allowlist.toml`, so the two paths are listed only in the
  spec's `%files`. The RPM itself needs a Fedora container (`just rpm`); without one the spec's own
  `rpmbuild -bb --nodeps` runs on a staged tarball, and `rpm -qlp` lists the package.
- **Install Kai for Firefox.** The Firefox section of the settings window shows whether an
  extension is linked to Kai now (the browser server's link count, read again when it changes) and
  a button that has Kai open the signed XPI in the browser (`xdg-open`, so the desktop starts the
  browser in its own app scope; Firefox has to be the default browser to offer the installation).
  The link is compiled in from `KAI_FIREFOX_INSTALL_URL` (`option_env!`, `src/browser/install.rs`),
  which Kai's CI sets per channel: a CI variable for the private builds channel (the URL of
  the signed XPI in kai-firefox's package registry, carrying a deploy token while that channel is
  private; `.gitlab-ci.yml`), and the repository variable of the same name for the GitHub releases
  channel (`.github/workflows/release.yml`; `packaging/gh-build.sh` refuses a link with credentials
  in it). `packaging/build-rpm.sh` hands it to a local build when it is in the environment. Without
  it the section says how to install by hand (about:addons, Install Add-on From File). The page never
  receives the link, and the log and every error name the host at most.

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
