# Adversarial review: the Kai spec and the lmgw client API spec (2026-10-06)

> **Note (after v2):** this review cites the v1 numbering (K1–K15, L1–L15) and the API spec's old
> path. v2 renumbered the decisions (K1–K21, L1–L18), and the API spec now lives in the lmgw repo as
> `docs/design/2026-10-06-client-apps-design.md`. Each spec's "Review dispositions" appendix maps
> the findings.

Reviewed:
- `docs/design/2026-10-06-kai-design.md` ("kai §n", K1–K15)
- `docs/design/2026-10-06-lmgw-client-api-design.md` ("api §n", L1–L15)
- `spikes/overlay/RESULTS.md`

Against lmgw main at **2baa764**. The specs cite d113601; the one commit since then is the CORS
change that R1 depends on. Also checked: an earlier private prototype, chat-voice
(2026-10-03), principals (2026-09-22) and server-tools (2026-10-05). lmgw paths are relative to
its repository.

Most file:line claims in the API spec hold. The ones that do not are named in the finding they
affect (R3, R17, R31). Severity follows the brief: breaking one of the owner's standing rules is
at least major.

**What a stolen device key can do, as specified** (R1, R2, R3, R4 take this apart):
- read every non-admin thread, attachment and transcript, and (through search) admin ones too;
- spend on every alias with no budget, because a new device starts with every scope `all`;
- call every tool except `lmgw__*`, the other devices' hosted tools included;
- approve its own gated calls, so Kai skips its confirmation;
- take over any voice session;
- keep all of this after its `expires_at`, because expiry is never checked on `Chat` routes.

Revocation works (api §1.6), and the self-admin plane stays out of reach (`mcp/scope.rs:86`).

---

## Blocker

### R1 (blocker): anonymous callers and unscoped keys reach device-hosted tools

**Where:** api §5.6, §1.5; kai ruling 17, §10.1, §11.1.

**Claim.** "Anything whose tool scope admits them" reaches a device's tools, and the owner limits
reach with tool scopes.

**Evidence.**
- `principal_list` gives `Anonymous` and owners `List::All` (`mcp/scope.rs:245-251`).
- A client key whose tool scope mode is `all` also gets `List::All` (`scope.rs:258`). `all` is
  `KeyPolicy`'s default (`config/keys.rs:111-131`, `ScopeMode` default `All`).
- `Anonymous` holds `Inference` whenever *Require API key* is off (`principal.rs:109`). Off is
  the default (`policy.rs`, the `check_alias` doc).
- `/mcp` refuses foreign browser origins (`mcp/ingress.rs:1000-1044`). `/v1/responses` resolves
  `{type:"mcp"}` blocks under the same scope (`responses.rs:723`), sits behind
  `CorsLayer::permissive()` (`server.rs:132-135`) and checks no `Origin`.

**Consequence.**
- With auth off, any local process can call `kai__see_screen` without a credential. That
  includes a Flatpak app with network access and a container started with `pasta -T`. A web page
  can do the same through `/v1/responses` wherever the browser lets it reach loopback.
- Kai allows read-only tools by default (kai §11.1). KWin restricts `ScreenShot2` to Kai's
  `.desktop` file (ruling 16), and lmgw now hands that capability to every caller.
- `kai__overlay_show {kind:"html", interactive:true}` lets the same callers draw clickable
  content over every window.
- With auth on, every client key minted with the default tool scope reaches the desktop tools the
  moment Kai connects.

**Fix.**
1. A row with `transport = device` is never inside `List::All` for `Anonymous`. Setting a hosting
   grant while auth is off fails with an explicit error naming the setting ("switch on Require
   API key first").
2. A client key reaches a device label only through an explicit allow pattern.
3. The Devices card shows who can reach each label: owner, devices, and which keys.
4. Kai's "read-only defaults to allow" applies only to callers it trusts by `lmgw/caller`
   (owner principals and Kai itself).

The owner's decision that `/mcp` clients reach device tools stands. Claude Code gets an allow
pattern on its key.

---

## Major

### R2 (major): an approval does not say who approved, so a device can approve itself

**Where:** api L12, L13, §5.5, §6.3; kai K15, §11.1.

**Claim.** `_meta["lmgw/approval"] = "approved"` means the owner was asked, so Kai skips its own
confirmation. Any client may decide, and the first decision wins.

**Evidence.**
- A decision carries no identity: `DecidedCall` holds the call, `approved` and `denial` only
  (`agent.rs:136-144`).
- The spec's `_meta` value is `"approved" | "not_required"` (api §5.5).
- Any `Chat` principal may post decisions (api §6.3).
- A device may write a thread's `mcp_tools`, including the new `require_approval`. L5 checks
  labels only.

**Consequence.**
- A device key alone can run state-changing desktop tools with no prompt on the desktop. It
  attaches `kai` to a thread of its own with `require_approval: "always"`, approves its own gated
  `kai__run_program`, and Kai sees `approved` and skips its confirmation. Approvals make this
  worse than `never` would: with `never`, Kai's own confirm would still fire.
- The resumed turn has no defined principal. If the owner approves a device's turn, the
  continuation may run with `ToolScope::gateway()` under `internal:chat`. If a device approves the
  owner's turn, it may run under the device's narrower scope and be refused.

**Fix.**
- `_meta["lmgw/approval"] = {"decision": "approved", "by": {"kind", "name"}}`.
- K15 skips its confirmation only for approvers Kai trusts: by default owner principals and Kai's
  own key, otherwise a list in Kai's configuration.
- A resumed turn runs as the principal that started it. The approver is recorded on the tool row
  and in `approval.decided`.

### R3 (major): "KeyPolicy applies whole" is not what the spec builds

**Where:** api §1.1, §1.2, §1.3.

**Evidence.**
- The gate checks expiry, rpm and concurrency on `Inference` routes only (`server.rs:1129-1134`).
- `check_alias` is scope plus budget (`policy.rs:380-388`, `:441-480`). It neither sees expiry
  nor counts rate windows.
- The helper that does all of it is `policy_checked_call` (`proxy/recording.rs:411-445`). It
  re-reads the key by id, refuses a disabled or deleted key, then runs `count_call` (expiry, rpm,
  tpm; `policy.rs:220-240`).

**Consequence.**
- An expired device key keeps the feed, the host link and every Chat read and write.
- The per-call check §1.3 names would not count the rate windows it says it counts.

**Fix.**
- §1.3 names `policy_checked_call` for every model call of a device turn.
- The gate runs `usable()` (expiry) for `Chat` as well, answering 401 `key_expired` with the
  date.
- The revocation signal (§1.6) also fires at `expires_at`, so the feed, host link and realtime
  session close then.

### R4 (major): L3 hides Admin Chat only partly

**Where:** api L3, §1.3 "Admin Chat", §1.8.

**Evidence.**
- Search covers every thread with no `kind` filter (`store/chat_search.rs:137` on).
- Attachments are addressed by attachment id: `GET /chat/api/attachments/{id}`, `/text`, `/mode`,
  `/delete` and `/transcribe` (`server.rs:379-384`).
- `folders/{id}/delete {threads:"delete"}` deletes every thread in the folder
  (`store/chat_folders.rs:223`), and Admin Chat threads join folders (`web/chat_folders.rs:276-280`).
- Settings, pin, archive, move, delete, speak, persist, voice warm and upload are not in the
  spec's list.

**Consequence.**
- A device reads Admin Chat through search snippets and attachment text. That is the history
  where provider keys get pasted while upstreams are configured.
- It deletes admin threads through a folder delete.
- Reads answer 404, but send and bind answer 409 `chat_thread_admin`, which tells the device the
  thread exists.

**Fix.** One rule, enforced in one place:
- Every route that resolves a thread (by thread id, message id or attachment id) answers a device
  with 404 for an admin thread, and that includes the bind.
- Collection routes (list, search, folder counts, feed) filter admin threads out.
- A device's folder delete skips admin threads and says how many it kept.

### R5 (major): `current` has no model to create a thread with

**Where:** api §3.3; kai §4.

**Evidence.**
- lmgw has no server-side default chat model. The dashboard sends `model_alias` from its
  selector on every create (`lmgw-ui/src/pages/chat.rs:915-920`, `pages/chat_folders.rs:280-283`).
- `create_in_folder` takes `model_alias` from the request, and folder defaults override it only
  when set (`web/chat_folders.rs:281-301`).
- Kai creates the "Kai" folder with `ongoing {idle_minutes:120}` and nothing else.

**Consequence.** The first `current` creates a thread with `model_alias: ""`. The first voice
press then fails at the gate. Phase 1's core flow is broken on first run.

**Fix.**
- `current` answers 409 `folder_no_model` ("set the folder's model in its defaults") when the
  folder names none, and the folder form marks the model as required for an ongoing folder.
- On that error, Kai's first run shows "choose the Kai folder's model in lmgw" with Open in lmgw.
- Do not let each client pass a model to `current`: two clients would then disagree on the
  conversation's model.

### R6 (major): an offline device makes its threads refuse every turn

**Where:** api §5.3 ("an offline device fails at once"); kai §11.1 (the Kai folder attaches
`kai`).

**Evidence.** A label that fails to resolve is reported. When no tool is left, the whole turn is
refused: "none of this thread's MCP servers offered a usable tool" (`web/agentchat.rs:497-530`).

**Consequence.** If `kai` is the Kai folder's only label (phase 2 and 3), every turn of the
conversation is refused while Kai is not connected. That covers phone, dashboard and bound
sessions. Android loses the conversation exactly when the desktop is off.

**Fix.**
- A label that is offline because its device is offline is reported (an error frame and a note),
  and the turn runs without its tools.
- "No usable tool, so refuse" applies only when no failure is "device offline".
- Test: the Kai thread answers from the phone while Kai is down.

### R7 (major): approving by voice declines the call it approves

**Where:** kai §11.1 against api L13, §6.3 and chat-voice §8.1.

**Evidence.**
- Kai sets `create_response: false` and matches the next spoken turn's transcript.
- A turn committed in a bound session becomes conversation that the next response renders, and the
  journal writes it into the thread as a user message (`realtime/lifecycle/pending.rs:1-40`;
  chat-voice §8.6 m9).
- Item deletes are refused `owned_by_thread` (`realtime/thread/owned.rs:120-130`).
- L13 says a new user message declines everything still pending.

**Consequence.** The spoken "ja" is a user message. It declines the call, and it also lands between
the assistant's tool call and its result in the history.

**Fix.** Keep the answer out of the thread.
- While the approval overlay is open, Kai sends the session no audio. Either it streams silence or
  it sends `input_audio_buffer.clear` and never commits.
- Kai records the answer locally and transcribes it through `POST /chat/api/threads/{id}/transcribe`
  (dictation, already in the subset).
- Then it sends `mcp_approval_response` and `response.create`.

### R8 (major): multi-device races, where Kai's automatic actions override another device

**Where:** kai K5, §5.2, §5.6; api §1.3 ("across devices the last writer wins").

**Evidence.**
- `bind_voice` takes over unconditionally (`web/chat_live/voice.rs:53-76`).
- `LiveTurns::begin` cancels the thread's live turn, whoever started it (`web/chat_live.rs:189-215`).

**Consequence.**
- **(a) Kai takes over the phone's session.** Kai rebinds by itself when `folder.current` names
  another thread. If the phone caused the rollover and bound the new thread, Kai's quiet-point
  rebind takes over the phone's live session. That is an automatic reconnect, against chat-voice
  §8.6 ("nothing reconnects by itself") and kai §5.6.
- **(b) The room cancels the phone.** With listening on in automatic mode, any speech in an empty
  room (the TV, a visitor) starts a voice turn. That turn cancels the owner's running text turn
  from the phone in the same conversation. "Last writer wins" becomes "the room wins".

**Fix.**
- Kai never binds automatically onto a thread that another client holds (feed `voice.bound` whose
  `by` is not Kai). It closes its session and waits for a press.
- While another principal's turn runs in Kai's thread (`turn.started` by someone else), automatic
  listening sends silence until `turn.done`.
- Specify both in kai §5.2 and test them in kai WP6.

### R9 (major): listening on all day costs energy and hides the hold

**Where:** kai §5.1, §9.3, §9.4; rule 6, and rule 1's visibility.

**Evidence.**
- The engine drops to 15 fps only when the state is `idle` and both taps are quiet
  (`crates/lmgw-ui/assets/voice/viz/engine.js:426-430`).
- The ring shows in every non-idle state, `listening` included (kai §9.4).
- The spike's 60 fps figure is a measurable share of a core and some GPU power, before the feed
  (RESULTS Q3).
- The tray icon order puts `listening` above `held` (kai §5.1).

**Consequence.**
- With listening on in automatic mode, the ring renders at the display's rate and Kai feeds it at
  30 Hz for as long as listening stays on.
- The amber hold never reaches the icon. That is exactly when the ASR stage may be answered by a
  cloud fallback, and the only notification Kai specifies is for a stage that has *no* fallback.

**Fix.**
- Quiet listening (no `speech_started`, mic below the engine's quiet level) gets the calm path.
  Either Kai shows the ring only from `speech_started` to the turn's end, or `engine.js` treats
  quiet listening as calm (a generic change, with a viz-harness check).
- Show "held" as a badge on every icon state, not as a rung of the order.
- Notify once per hold period when a stage is first answered by a fallback (`answered_by`). That
  is visibility only: no routing exception.

### R10 (major): MCP Apps widgets get the whole device scope with no gate

**Where:** kai §10.1 ("MCP Apps").

**Evidence.**
- Kai proxies a widget's `tools/call` to `/mcp` under its own device key.
- `/mcp` has no approval round trip: it handles `initialize`, `ping`, `tools/list` and
  `tools/call` (`mcp/ingress.rs:766-823`), and F covers only the Chat and bound realtime.

**Consequence.**
- HTML from any registered server's `ui://` resource is third-party content, and it can call every
  tool in Kai's scope with no request shown to the owner. That includes state-changing tools of
  other servers.
- `kind:"html"` is model output, so a prompt injection can reach it. It lives in the same widget
  host page, and a postMessage bridge there answers any frame that posts to it.

**Fix.**
- The bridge answers only `ui_resource` frames, identified by `MessageEvent.source`.
- A widget may call only tools of the server that served its resource (the same `server_label`).
- A state-changing call goes through Kai's confirmation (K15).
- `kind:"html"` gets no bridge.

### R11 (major): hidden limits (rule 2)

**Where:** api §5.1, §2.1; kai §6.2.

**Evidence and consequence.**
- **The host link's sizes.** `/mcp/host` would run on tungstenite's built-in limits (64 MiB per
  message, 16 MiB per frame). For `/v1/realtime`, realtime §10.4 made those explicit settings;
  api §5.1 says nothing. A base64 PNG of a large ultrawide desktop from `see_screen` comes near the frame
  limit.
- **SSE keep-alive** "comes from axum's default" (api §2.1). That is a 15 s constant, and every
  client's dead-link timeout depends on it.
- **The AEC wait.** kai §6.2 takes `aec.rs` "almost as is", including its 4 s wait for the
  module's nodes (the prototype's `src/aec.rs:102`), whose error names no limit.

**Fix.**
- Add host-link message and frame settings, or reuse realtime's. An overrun closes the link with a
  reason naming the setting.
- Put the keep-alive interval in `hello` and make it a setting.
- Make the AEC wait a Kai setting, named in its error.

### R12 (major): Kai fetches images of any size from any address

**Where:** kai §10.1 ("Images arrive through Kai"; "No size cap"); rule 6.

**Consequence.**
- Any caller of `kai__overlay_show` can make Kai read an arbitrary URL into RAM: the model,
  another device, a client key, or anonymous callers (R1). An unbounded fetch can exhaust RAM
  and get the desktop session OOM-killed.
- The fetch runs from the desktop's network position: LAN admin pages, or lmgw's own loopback
  port. A failure is "the tool's error", so the body can leak back to the caller.

**Fix.**
- Stream the body against a visible `overlay.image_max_mb`, named in the tool's error. The real
  constraint is RAM, and a visible limit is what rule 2 allows.
- Check the decode before handing the image to WebKit.
- Refuse loopback, link-local and private targets unless a setting allows them, and log the
  refusal.
- Error text names status and size, never the body.

### R13 (major): G pins an MCP protocol revision without a source

**Where:** api §7.2 ("`/mcp` also negotiates MCP 2026-07-28, the version that defines MCP Apps
(SEP-1865)"); kai ruling 15.

**Evidence.**
- Neither spec cites the text. lmgw has no reference to 2026-07-28 or SEP-1865.
- MCP Apps, as published up to this reviewer's knowledge (June 2026), is an extension negotiated
  through capabilities (`io.modelcontextprotocol/ui`), not a protocol revision.
- `resources/list`, `resources/templates/list` and `resources/read` exist in every revision `/mcp`
  already speaks (`mcp/ingress.rs:60`).
- Rewriting URIs inside tool results also departs from server-tools decision 7 ("results pass
  verbatim") without saying so.

**Consequence.**
- Advertising a revision commits `/mcp` to all of its changes, and G lists none of them.
- If that revision does not define MCP Apps, both specs rest on a wrong anchor.

**Fix.**
- Advertise `resources` and the MCP Apps extension capability on the revisions `/mcp` already
  speaks. A new revision is its own change, with its own list of differences.
- Cite the SEP text for the MIME type, the `_meta.ui.*` keys and the host's obligations.
- State the departure from decision 7.

### R14 (major): Kai's own confirmation outlives the call's timeout

**Where:** kai K15, §11.1; api §5.2 (`timeout_ms` 60 000), §5.4.

**Evidence.**
- The spec sends the device no cancellation.
- A timed-out call reaches the model as failed.
- A barge-in abandons a running call (server-tools decision 5).

**Consequence.** The owner confirms after 60 s, or after the turn was cancelled. lmgw has already
told the model the call failed, and Kai runs the program anyway. The action happens unreported and
the model may retry, so it can happen twice.

**Fix.**
- lmgw sends `notifications/cancelled` for every forwarded call it stops waiting on: a timeout, a
  turn cancel, a link close.
- Kai closes the confirmation and never runs a cancelled call.
- The overlay shows the call's remaining time.
- Use F for calls from a thread, and keep K15 for callers without F (`/mcp`).

### R15 (major): feed rows store rendered JSON, so B's storage is built the hard way

**Where:** api L6, §2.2, §2.3.

**Evidence.**
- An event's `data` is "the thread as the list returns it". The web layer builds that from the
  live snapshot:
  - `thread_row_json` computes `purge_at` from `chat_purge_days`, and `temporary`
    (`web/chat.rs:199-209`);
  - `thread_json` is async and resolves `voice_resolved`;
  - folders carry their thread counts.
- Feed rows are to be written "in the change's own transaction" in `store/*`. Many store writes are
  single statements on the pool: the sweep (`store/chat.rs:381-414`), the thread settings write.
- `turn.*` and `voice.*` are persisted too.

**Consequence.**
- Store code would call web-layer renderers inside transactions, and every store write becomes a
  transaction.
- Stored JSON goes stale (`purge_at`, `voice_resolved`), and catch-up replays the stale copy.
- Every reply is stored twice, once as the message (reasoning included) and once in the feed.
- After a crash, a persisted `turn.started` or `voice.bound` with no end replays as "running" or
  "bound" forever.
- §2.2 lists only the purge for the sweep, so its automatic archive emits nothing.

**Fix.**
- A feed row records what changed, not its rendering:
  `{seq, at, type, thread_id, folder_id, message_ids, by}`. It is written by one store helper
  inside the transaction.
- Delivery renders the current state, or a tombstone, at read time.
- `hello` carries the live state: bound threads with their binder, running turns, the hold.
- `turn.*` and `voice.*` are live-only.
- The sweep writes `thread.updated` for what it archived (`RETURNING id`).

---

## Minor

### R16 (minor): two negative matches would hand out the wrong rights

**Where:** api §1.2 (the `holds` table: "as today").

**Evidence and consequence.**
- The agent arm is `!matches!(cap, Cap::Admin)` (`principal.rs:126`). Adding `Cap::Chat` therefore
  gives every agent token the Chat history unless that arm changes. Here "as today" in code means
  "everything but Admin".
- `describe()` ends in a catch-all (`:159`), so a device would be refused as "a client API key".

**Fix.** Write both arms positively. Keep the planned capability-matrix row for agent × Chat.

### R17 (minor): the feed's `hold` event misses fallback changes

**Where:** api "What exists today" ("written only by `ops::hold_set`"), §2.2.

**Evidence.** That holds for `active` only. `hold.fallback_alias` is also written by
`ops/settings_patch.rs:251-259`.

**Fix.** Emit `hold` whenever the published snapshot's `hold` differs from the previous one.

### R18 (minor): revocation has no recognisable signal

**Where:** kai §3.3 against api §1.6 and §1.8.

**Evidence.**
- After Rotate or Delete, the old bearer matches no row and resolves to `Anonymous`. A Chat route
  then answers 401 `session_required` with the dashboard's login text (`principal.rs:181-188`,
  `server.rs:1117-1122`).
- Neither spec says what an open SSE or WebSocket receives when the key is revoked.

**Fix.**
- Before closing, the feed sends `event: revoked {reason}`, and the WebSocket links close with 4003
  and a reason.
- Kai treats any 401 on the feed as "pair again".

### R19 (minor): edges in finding the folder and the current thread

**Where:** kai §4, §3.2; api §3.2, §3.3, L8.

**Evidence and consequence.**
- Folder names are not unique (`migrations/0048_chat_folders.sql`), so a lookup by name can find
  two.
- `POST /chat/api/threads {folder_id, kind:"admin"}` in an ongoing folder would become its current
  thread (L8), which a device may not read.
- A deleted current thread is cleared by `ON DELETE SET NULL`, which writes no `folder.current`.
  Kai's event table (kai §3.2) has no row for `thread.deleted` or for its thread being archived.

**Fix.**
- The lookup picks the ongoing folder of that name with the lowest id, and logs duplicates.
- Only `kind:"chat"` threads become current.
- Kai asks `current` again when its thread is deleted or archived.

### R20 (minor): the L5 label check passes prefix-less servers and built-in labels

**Where:** api L5, §1.3 "Writes".

**Evidence.**
- `may_reach` answers true for any server with an empty prefix (`mcp/scope.rs:147-148`).
- It has no answer for the built-in labels `kb` and `docs`, which have no server row.

**Consequence.**
- A device with a narrowed scope can still attach a prefix-less server to a thread. The owner's
  later turns in that thread then run its tools with `ToolScope::gateway()`.
- L5 bounds labels only. A device can also rewrite the thread's or folder's system prompt that the
  owner's turns run with.

**Fix.**
- The write check requires `admits` for every tool the label exposes now: a bare server's live
  list, a built-in's namespace.
- State plainly that a device's scope bounds the device's own turns, not the content the owner's
  turns later read.

### R21 (minor): a dropped link reports a call as failed when it may have run

**Where:** api §5.4.

**Evidence.** "Device disconnected during the call" reads as a failure, but the device may have
run the call before the link went.

**Fix.** Use the Chat's abandoned wording ("may or may not have run", server-tools decision 5) so
the model does not retry blindly.

### R22 (minor): aec.rs does not restore defaults after a crash

**Where:** kai §6.2 ("a pair left behind by a crashed run is reused and its defaults restored").

**Evidence.**
- On the reuse path, `prev_source` and `prev_sink` are `None` whenever the current default is
  already `aec_*` (the prototype's `src/aec.rs:66-75`).
- `Drop` restores only `Some` values (`:132-145`).
- The module host is a plain `Command::spawn` child (`:85-92`), not tied to Kai's life.

**Consequence.** After a crash outside the autostart unit (a `systemd-run --scope` dev run, a
terminal), an orphaned `pipewire -c` keeps `aec_*` as the defaults, and the next run's reuse path
restores nothing.

**Fix.**
- Before flipping the defaults, write the real ones to `$XDG_STATE_HOME/kai/aec.json`, and restore
  from that file on reuse.
- Tie the child to Kai: `PR_SET_PDEATHSIG`, or Kai's own scope.

### R23 (minor): the dev recipe's app id is likely wrong, and the pairing launch has no app id

**Where:** kai K12, §8.3, §3.1.

**Evidence.**
- xdg-desktop-portal parses `app[-<launcher>]-<ApplicationID>-<RANDOM>.scope`.
  `app-dev.rootshell.kai-dev-<n>.scope` most likely parses as `dev.rootshell.kai-dev`, because the
  random part is the last dash-separated segment.
- A first instance launched by the `lmgw-pair:` handler from a browser may run inside the
  browser's cgroup.

**Consequence.** Shortcuts get bound to the wrong app id, and a pairing-launched Kai can die with
the browser.

**Fix.**
- Name the dev unit `app-dev.rootshell.kai-dev<n>.scope`, with no dash in the random part.
- Extend WP0 (g) to the scheme-handler path. A handler instance that finds no running Kai stores
  the link, starts Kai through its desktop file (KIO or `gtk-launch`), and exits.

### R24 (minor): assumptions missing from WP0

**Where:** kai §15 WP0, K1, K7, §8.2, §11.1.

Not covered by any WP0 point:
- **Key repeat.** A repeated `Activated` while the key is held would toggle. Debounce, and check it
  in WP0 (f).
- **A locked KWallet at autostart.** It prompts at login, and Kai should say "wallet locked", not
  "not paired".
- **The pairing click.** A `lmgw-pair:` click from lmgw's own window (WebKitGTK navigation
  policy) and from a browser.
- **Fractional scale and HDR content** (RESULTS "Open").
- **The network process after destroy.** WebKitNetworkProcess's PSS after the last view is gone:
  K1's idle "73 MB" is the UI process measured *with* a view.
- **`pw_stream_get_time_n`.** Whether pipewire-rs exposes it (K7), or `pipewire-sys` is needed.
- **Phase 3.** Layer-shell `on_demand` keyboard for the approval overlay; list it now.

### R25 (minor): phase 1 carries Android-sized work

**Where:** api §10 WP2, WP4, WP6; kai §1.

**Evidence.**
- Kai 1 reads `hello`, `hold`, `folder.current`, `thread.updated`, `voice.*` and `resync`
  (kai §3.2). It sends no text, uploads nothing, searches nothing.
- Yet the following sit on Kai 1's path:
  - the whole persisted feed, with `message.*` in every store transaction;
  - typed DTOs and DocRoutes for the whole Chat surface, send-stream frames included;
  - the QR code.

**Fix.**
- **Kai 1:** the feed route, `hello`, the events Kai reads, `resync`, the `lmgw-client` crate, and
  the DTOs it uses.
- **The Android phase:** `message.*` events, catch-up retention, the full DTO set and its docs,
  and the QR code.

### R26 (minor): sampling from a device's server bypasses the device's policy

**Where:** api §5.2, §5.4 (`allow_sampling`).

**Evidence.** Sampling runs under `internal:mcp-sampling` (`mcp/handler.rs` `create_message`;
`telemetry.rs:403`). That is outside the device key's alias scope and budget.

**Fix.** Record and check sampling against the device key, or refuse `allow_sampling` on device
rows.

### R27 (minor): Kai hardcodes its label

**Where:** kai §10.1, §11.1; api §1.5, §2.1.

**Evidence.** Kai's tools are named `kai__…`, and its folder defaults attach `kai`. The label is
chosen at pairing, and `hello` does not carry it.

**Fix.** `hello` carries `hosts_label`, and Kai uses it.

### R28 (minor): public-docs hygiene (rule 7)

**Where:** the API spec, which moves to lmgw's public `docs/design/`; the kai repository.

**Evidence and consequence.**
- The API spec links the private Kai spec and cites "kai §n" about ten times.
- `README.md:3` and `spikes/overlay/RESULTS.md:39` and `:51` name the owner.

**Fix.**
- In lmgw's copy, cite "the first client's spec" without a link.
- Write "the owner" in both files.

### R29 (minor): the talk key, an unreachable lmgw, and listening after a drop

**Where:** kai K6, §8.1, §5.3, §5.6.

**Evidence and consequence.**
- **The tap.** A press while Kai speaks stops the reply, and the short release then toggles
  listening, so a tap meant to interrupt also turns listening off.
- **lmgw down.** With the feed down, a press still buffers for up to 15 s before anything is said.
- **A drop.** Nothing says what "listening on" does after a drop that is not a takeover, such as an
  lmgw restart.

**Fix.**
- A press that stopped speech consumes the gesture.
- A press says "lmgw is not running" at once while the feed is down.
- Listening on resumes when the feed returns, with a log line, or stays off with a notification.
  Pick one and write it down.

### R30 (minor): resync detection and catch-up memory

**Where:** api §2.4, §2.3.

**Evidence and consequence.**
- Only a cursor newer than the newest row counts as foreign, so a restored backup with higher
  `seq` values passes silently.
- Catch-up reads every row past the cursor at once.

**Fix.**
- Add `hello.epoch` (random per database) and use a cursor of the form `"<epoch>:<seq>"`.
- Stream catch-up rows in pages.

---

## Nit

### R31 (nit): claim accuracy

- `TurnOpts` also carries `heard`, `began`, `spoken` and `user_row` (`web/chat_turn/out.rs:105-140`).
- `CAPABILITY_TABLE` runs from `server.rs:189` to `:420`.
- chat-voice has no §8.7 heading. The events table ends §8.6.
- `key_rotate` refuses non-owner rows today (`web/api_settings.rs:1552-1558`). Rotating a
  hash-only device key is a new code path.
- "`McpConn` … reused unchanged" is too strong. The manager's connect, lazy-list and reap paths
  need a `Device` arm that waits for an inbound link.
- The `/mcp/host` WebSocket with reversed roles is an lmgw transport binding, not an MCP transport.
  Say so, and note that an Android MCP SDK needs a custom transport for it.

### R32 (nit): F's wording

- Open question 6 needs no extra opt-in: a bound session is already the opt-in for lmgw events
  (chat-voice §8.6, "The binding is the opt-in").
- §6.5 says unbound approvals are built by server-tools' rules, but "Later" defers them. Pick one.
- State that in a bound session the approved call and the answer come in one response, unlike
  server-tools decision 1.
- `arguments` is OpenAI's JSON string, and `name` follows server-tools decision 6.

### R33 (nit): owner-only exports protect nothing

L2 keeps the exports owner-only, but a device reads every non-admin thread through list and get
anyway. Give the exports to `Chat` as well, or drop the rationale.

### R34 (nit): docs that A must update

- the `/v1/realtime` DocRoute's "`chat_thread` (dashboard only)";
- the `bind.rs` module-doc table;
- chat-voice §8.1's refusal table;
- chat-voice §8.8 NIT 12: bind refusals write no row because "they are the dashboard's own"
  requests, and a device's are not.

### R35 (nit): the pairing key travels on argv

The scheme handler receives the key as `%u`, which shows in `/proc/<pid>/cmdline`. Never log argv,
and hand the link to the running instance over D-Bus only.

### R36 (nit): the Devices card's destructive actions

Disable, Rotate and Delete warn and confirm what they end ("Kai must be paired again"), then do it
(rule 5).

---

## What the specs got right (keep it in the revision)

- **Generic mechanisms only.** A key kind, a capability, an ongoing folder, a feed and a hosted
  server. Core never learns "Kai" (rule 4).
- **The self-admin plane stays out of reach.** A device's turns run under its own key and scope
  (L4), and its label writes are checked (L5). `ToolScope::of_request` derives self-admin from
  `Cap::Admin` (`mcp/scope.rs:86`).
- **Spec corrections.** It corrects chat-voice §8.8's "pass trivially", and restores realtime
  §10.2's policy check before the 101 for a device bind.
- **The binder is named.** A takeover says who took it.
- **Revocation and keys.** Revocation ends open connections. Device keys are stored as hashes only
  (L1).
- **A persisted, ordered feed.** Explicit `resync` with a reason, a visible retention setting, and
  no "lagged; drop".
- **`current`.** A per-folder lock, idleness measured from the newest message, reuse of an empty
  thread, and a sweep that skips the current thread.
- **Offline devices fail at once.** No 10 s lazy-list wait.
- **The contradiction G fixes.** `_meta.ui.resourceUri` passes through `/mcp`, but the resource it
  names cannot be read.
- **WP0.** Every untested platform point gets a verdict, and a failed point changes the spec first.
- **The NVIDIA rule** exactly as the owner set it, with the forbidden variables named.
- **Voice.** Echo modes mirror the Chat, with `device` as the default. Capture is buffered from the
  key press. The truncate comes first, with the heard position from PipeWire's delay. One state
  machine is shared with the dashboard (`lmgw-client`).
- **K4.** Voice is off at login, so autostart never evicts models.
- **K1's RAM choice** rests on measured PSS.
- **The overlay sandbox.** No same-origin, a strict CSP, `decide-policy` as a second wall, and
  nothing left showing unseen.
- **Honest fallbacks.** `answered_by` in the tooltip, privacy as the choice of alias, and
  screenshots that follow fallbacks per rule 1.
- **The test rules.** A dev instance only, crops only, no injected input, no committed speech, and
  publishing left to the owner.
