//! Kai's end of the CLI socket (module doc of `crate::cli`).
//!
//! The log names the operation, the tool or alias and the caller, never an argument, a prompt or
//! an answer.

use std::path::PathBuf;

use kai_desktop::contract::Kind;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{Answer, Request, Stdout};
use crate::delegate::model::{chat_completions, read_turn, send_full};
use crate::host::local::Local;
use crate::host::local::LocalBounds;
use crate::host::policy::Caller;
use crate::host::registry::{Block, Entry};
use crate::host::settings::Named;
use crate::lmgw_link::Credentials;
use crate::routines::LiveScopes;

/// How many stdout pieces (each at most a pipe read, 64 KiB) wait for a slow client before the
/// script's pipe waits too: a pace, not a bound on what is said.
const OUT_PIECES: usize = 4;

/// What the socket's calls reach.
#[derive(Clone)]
pub struct Backend {
    pub local: Local,
    pub creds: watch::Receiver<Option<Credentials>>,
    /// The peer's cgroup file (`/proc/<pid>/cgroup`; the tests give theirs).
    pub cgroup_of: fn(i32) -> Option<String>,
    /// The routine scopes Kai started that still run (`crate::routines::script`).
    pub scopes: LiveScopes,
}

/// The caller a process in the cgroup `text` is (module doc of `crate::cli`): `routine:<name>` only
/// in a routine scope Kai started and whose run has not ended (`scopes`), any other `cli`.
pub fn caller_of(text: &str, scopes: &LiveScopes) -> Caller {
    let prefix = crate::routines::unit_prefix();
    for path in crate::local_socket::cgroup_paths(text) {
        for part in path.split('/') {
            if let Some((name, random)) = part
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_suffix(".scope"))
                .and_then(|r| r.rsplit_once('-'))
                && crate::routines::def::plain_name(name)
                && !random.is_empty()
            {
                if scopes.holds(part) {
                    return Caller::Named(Named::routine(name));
                }
                tracing::warn!(
                    scope = part,
                    "cli: a peer in a routine scope Kai does not run now; it is the caller cli"
                );
            }
        }
    }
    Caller::Named(Named::cli())
}

/// Serves the CLI at `path` until Kai's runtime ends.
pub async fn serve(path: PathBuf, backend: Backend) {
    let listener = match crate::local_socket::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                "cli: cannot serve {}: {e}; kai tool and kai model cannot reach this Kai",
                path.display()
            );
            return;
        }
    };
    let _socket = crate::local_socket::Unlink(path.clone());
    tracing::info!(socket = %path.display(), "cli: serving kai tool and kai model");
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let backend = backend.clone();
                tokio::spawn(async move { connection(stream, backend).await });
            }
            Err(e) => {
                tracing::warn!("cli: accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn write(stream: &mut (impl AsyncWriteExt + Unpin), answer: &Answer) {
    write_line(stream, answer).await;
}

/// One JSON line; whether the client took it.
async fn write_line(
    stream: &mut (impl AsyncWriteExt + Unpin),
    value: &impl serde::Serialize,
) -> bool {
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    line.push('\n');
    stream.write_all(line.as_bytes()).await.is_ok() && stream.flush().await.is_ok()
}

/// One connection: the peer checked, one request, its answer.
pub async fn connection(stream: UnixStream, backend: Backend) {
    let cred = match stream.peer_cred() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("cli: the peer's credentials cannot be read ({e}); refused");
            return;
        }
    };
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    let caller = crate::local_socket::peer_read(&stream, cred.pid(), backend.cgroup_of)
        .map(|c| caller_of(&c, &backend.scopes))
        .unwrap_or_else(|| Caller::Named(Named::cli()));
    let (rd, mut wr) = stream.into_split();
    if cred.uid() != me {
        tracing::warn!(
            peer_uid = cred.uid(),
            "cli: a connection of another user refused"
        );
        write(
            &mut wr,
            &Answer::usage("Kai answers only its own user's programs"),
        )
        .await;
        return;
    }
    // The request line within the host link's message limit: a tool's arguments are what one
    // message of lmgw's carries; `None` (the owner's 0) is no bound.
    let limits = backend.local.limits();
    let bound = limits.and_then(|l| l.message);
    let mut rd = BufReader::new(rd);
    let mut line = Vec::new();
    let read = match bound {
        Some(b) => {
            (&mut rd)
                .take(b.saturating_add(1))
                .read_until(b'\n', &mut line)
                .await
        }
        None => rd.read_until(b'\n', &mut line).await,
    };
    match read {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let body = line.strip_suffix(b"\n").unwrap_or(&line);
    if let Some(b) = bound
        && body.len() as u64 > b
    {
        let says = limits.map(|l| l.message_says()).unwrap_or_default();
        tracing::info!(caller = %caller.describe(), "cli: a request over the message limit refused");
        write(
            &mut wr,
            &Answer::usage(format!(
                "the request is larger than the host link's message limit ({says}); its \
                 arguments must fit one message"
            )),
        )
        .await;
        return;
    }
    let request: Request = match serde_json::from_slice(&line) {
        Ok(r) => r,
        Err(e) => {
            write(
                &mut wr,
                &Answer::usage(format!("the request does not read: {e}")),
            )
            .await;
            return;
        }
    };
    let cancel = CancellationToken::new();
    // The client keeps its side open while it waits; its end is the call's cancellation.
    let gone = {
        let cancel = cancel.clone();
        async move {
            let mut buf = [0u8; 256];
            loop {
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            cancel.cancel();
        }
    };
    let answer = tokio::select! {
        a = answer(&backend, request, &caller, cancel.clone(), &mut wr) => a,
        () = gone => {
            tracing::info!(caller = %caller.describe(), "cli: the client went; its call was cancelled");
            return;
        }
    };
    write(&mut wr, &answer).await;
}

/// The answer to one request by `caller`; a script routine's stdout goes to `wr` as it comes
/// ([`Stdout`] lines), before the answer.
pub async fn answer(
    backend: &Backend,
    request: Request,
    caller: &Caller,
    cancel: CancellationToken,
    wr: &mut (impl AsyncWriteExt + Unpin),
) -> Answer {
    match request {
        Request::List { name } => list(&backend.local, name.as_deref()),
        Request::Tool { name, args } => {
            let Some(entry) = backend
                .local
                .entries()
                .into_iter()
                .find(|e| e.spec.name == name)
            else {
                return Answer::usage(format!(
                    "Kai offers no tool {name} now (kai tool lists those it offers)"
                ));
            };
            let raw = match arguments(&entry, &args) {
                Ok(m) => m,
                Err(why) => return Answer::usage(why),
            };
            tracing::info!(tool = %name, caller = %caller.describe(), "cli: a tool call");
            let (tx, mut rx) = tokio::sync::mpsc::channel(OUT_PIECES);
            let call = backend.local.call_with(
                &entry,
                Some(raw),
                caller,
                LocalBounds {
                    cancel,
                    room: None,
                    out: Some(tx),
                },
            );
            // The pieces until the call drops its sender; a client that cannot take them is
            // drained, so the script never waits on it (its going cancels the call).
            let forward = async {
                let mut open = true;
                while let Some(piece) = rx.recv().await {
                    if open {
                        open = write_line(wr, &Stdout::of(&piece)).await;
                    }
                }
            };
            let (reply, ()) = tokio::join!(call, forward);
            let mut texts = Vec::new();
            let mut notes = Vec::new();
            for b in reply.blocks {
                match b {
                    Block::Text(t) => texts.push(t),
                    Block::Image { mime, base64 } => notes.push(format!(
                        "an image ({mime}, {} bytes as base64) is part of the answer; a terminal \
                         does not print it",
                        base64.len()
                    )),
                }
            }
            Answer {
                status: if reply.error {
                    super::Status::Failed
                } else {
                    super::Status::Done
                },
                text: texts.join("\n"),
                notes,
            }
        }
        Request::Model { alias, prompt } => {
            tracing::info!(%alias, caller = %caller.describe(), "cli: a model call");
            model(backend, &alias, &prompt, cancel).await
        }
    }
}

/// The tools as `kai tool` lists them, or one tool's parameters.
fn list(local: &Local, name: Option<&str>) -> Answer {
    let entries = local.entries();
    let line = |e: &Entry| {
        let params: Vec<String> = e
            .spec
            .params
            .iter()
            .map(|p| {
                let v = format!("--{} <{}>", p.name, p.kind.noun());
                if p.required { v } else { format!("[{v}]") }
            })
            .collect();
        if params.is_empty() {
            format!("{}: {}", e.spec.name, e.spec.description)
        } else {
            format!(
                "{} {}: {}",
                e.spec.name,
                params.join(" "),
                e.spec.description
            )
        }
    };
    match name {
        None => Answer::done(entries.iter().map(line).collect::<Vec<_>>().join("\n")),
        Some(n) => match entries.iter().find(|e| e.spec.name == n) {
            None => Answer::usage(format!("Kai offers no tool {n} now")),
            Some(e) => {
                let mut lines = vec![line(e)];
                for p in e.spec.params {
                    let mut l = format!("  --{} {}", p.name, p.kind.noun());
                    if !p.required {
                        l.push_str(", optional");
                    }
                    if let Some(d) = p.description {
                        l.push_str(": ");
                        l.push_str(d);
                    }
                    lines.push(l);
                }
                Answer::done(lines.join("\n"))
            }
        },
    }
}

/// The `--<param> <value>` pairs as the tool's arguments, each read by its parameter's type. A
/// name the tool lacks is passed on as text for the host's own refusal, which lists what it takes.
fn arguments(entry: &Entry, args: &[(String, String)]) -> Result<Map<String, Value>, String> {
    let tool = entry.spec.name;
    let mut out = Map::new();
    for (name, text) in args {
        let Some(p) = entry.spec.param(name) else {
            out.insert(name.clone(), Value::String(text.clone()));
            continue;
        };
        let bad = || format!("{tool}: --{name} must be {}", p.kind.noun());
        let value = match p.kind {
            Kind::Text => Value::String(text.clone()),
            Kind::Integer => json!(text.trim().parse::<i64>().map_err(|_| bad())?),
            Kind::Number => {
                let n = text.trim().parse::<f64>().map_err(|_| bad())?;
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .ok_or_else(bad)?
            }
            Kind::Boolean => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Value::Bool(true),
                "false" | "no" | "off" | "0" => Value::Bool(false),
                _ => return Err(bad()),
            },
            Kind::TextList => {
                if let Some(Value::Array(items)) = out.get_mut(name.as_str()) {
                    items.push(Value::String(text.clone()));
                    continue;
                }
                Value::Array(vec![Value::String(text.clone())])
            }
        };
        if out.contains_key(name.as_str()) {
            return Err(format!("{tool}: --{name} is given twice"));
        }
        out.insert(name.clone(), value);
    }
    Ok(out)
}

/// `kai model <alias>`: one user message, the answer's text.
async fn model(backend: &Backend, alias: &str, prompt: &str, cancel: CancellationToken) -> Answer {
    let Some(creds) = backend.creds.borrow().clone() else {
        return Answer::failed("Kai is not linked to lmgw (pair it first)");
    };
    let body = json!({
        "model": alias,
        "stream": false,
        "messages": [{"role": "user", "content": prompt}],
    })
    .to_string();
    let req = chat_completions(body);
    let (http, url) = (creds.http(), creds.url());
    let sent = tokio::select! {
        s = send_full(&http, &url, req) => s,
        () = cancel.cancelled() => return Answer::failed("cancelled"),
    };
    let sent = match sent {
        Ok(s) => s,
        Err(e) => return Answer::failed(format!("lmgw did not answer: {e}")),
    };
    match read_turn(sent.status, &sent.body) {
        Ok(turn) => {
            let mut a = Answer::done(turn.text.unwrap_or_default());
            if let Some(f) = sent.fallback {
                tracing::info!(%alias, answered_by = %f, "cli: a fallback answered in place of the alias");
                a.notes.push(format!("(model: {alias}, answered by {f})"));
            }
            a
        }
        Err(e) => Answer::failed(e),
    }
}
