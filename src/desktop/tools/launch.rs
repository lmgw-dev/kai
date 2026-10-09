//! The tools of `contract::launch` (WP16a) over the Launch service: `open`, `app_launch` and
//! `run_command`. Results are compact lines (K22) that echo no argument:
//!
//! - `open`: `opened with <name> (<id>) as <type>`. The application is resolved before the
//!   owner's confirmation ([`prepare_open`]: the dialog names it) and the call starts that one;
//! - `app_launch`: `started <name> (<id>)`, or `started <n> instances of …, one per file`;
//! - `run_command`: a first line with how it ended, how long it ran and each stream's real size,
//!   then each stream that wrote anything under its own `stdout:` or `stderr:` line. Output that
//!   does not fit shows its start and its end, measured as the JSON the link carries (a byte that
//!   is not UTF-8 is a U+FFFD, a control character an escape): the result's room within the
//!   link's message limit (`Call::room`), or the owner's `launch.output_max_mb` where that is
//!   smaller. The first line says how much was cut and which limit cut it; it is Kai's alone, so
//!   the output cannot forge it. The command stops at the call's deadline less
//!   [`ANSWER_MARGIN`], or at the owner's `launch.command_timeout_s` where that comes first.

use std::sync::Arc;
use std::time::Duration;

use kai_desktop::caps::launch::{Captured, Launch, Opening, Ran, RunBounds, Started, Status};

use super::{Ctx, Services};
use crate::desktop::resolve::shown_text;
use crate::host::limits::escaped_len;
use crate::host::registry::{Args, Origin, Prepared, Preparer};

/// What a command's time limit leaves before lmgw's deadline for the answer: killing its scope,
/// reading its pipes, writing the result and its frames. Measured on loopback: about
/// 14 ms from the limit to the provider's answer (`a_command_stops_at_its_calls_deadline`), and
/// about 240 ms from a handler's 60 MiB result to lmgw's side having read it whole (the fake
/// host, a release build); 500 ms holds both with room for a link slower than loopback.
pub const ANSWER_MARGIN: Duration = Duration::from_millis(500);

/// `name` is one of `contract::launch`'s tools.
pub fn handles(name: &str) -> bool {
    kai_desktop::contract::launch::AREA
        .tools
        .iter()
        .any(|t| t.name == name)
}

fn launch(services: &Services) -> Result<&Arc<dyn Launch>, String> {
    // Not listed without the capability (K23): only a race with a relist gets here.
    services
        .launch
        .as_ref()
        .ok_or_else(|| "this desktop offers Kai no way to start programs".to_owned())
}

/// The confirmation's lines for what `open` will start.
pub fn opening_lines(o: &Opening) -> Vec<String> {
    let mut lines = vec![format!(
        "opens with: {} ({}) as {}",
        shown_text(&o.name),
        shown_text(&o.id),
        shown_text(&o.mime)
    )];
    lines.extend(o.notes.iter().map(|n| shown_text(n).into_owned()));
    lines
}

/// `open`'s resolution before the confirmation (the host runs it after the policy): the dialog
/// shows the application, and a refusal (no such file, a program, no application) asks nobody.
pub fn prepare_open(services: &Services) -> Preparer {
    let services = services.clone();
    Arc::new(move |args: &Args, _: Origin| {
        let services = services.clone();
        let target = args.text("target").unwrap_or_default().to_owned();
        Box::pin(async move {
            let l = launch(&services)?;
            let o = l.open_plan(&target).await.map_err(|e| e.to_string())?;
            Ok(Prepared {
                lines: opening_lines(&o),
                state: Arc::new(o),
                notes: Vec::new(),
                options: Vec::new(),
            })
        })
    })
}

pub async fn run(
    services: &Services,
    name: &str,
    args: &Args,
    ctx: &Ctx,
) -> Result<String, String> {
    let l = launch(services)?;
    match name {
        "open" => {
            let prepared = ctx
                .prepared
                .as_ref()
                .and_then(|p| p.state.clone().downcast::<Opening>().ok());
            let opening = match prepared {
                Some(o) => o,
                None => Arc::new(
                    l.open_plan(args.text("target").unwrap_or_default())
                        .await
                        .map_err(|e| e.to_string())?,
                ),
            };
            let mut s = l.open_start(&opening).await.map_err(|e| e.to_string())?;
            s.mime = Some(opening.mime.clone());
            Ok(started_lines(&s, true))
        }
        "app_launch" => {
            let urls: Vec<String> = args
                .texts("urls")
                .unwrap_or_default()
                .into_iter()
                .map(str::to_owned)
                .collect();
            l.app_launch(args.text("app").unwrap_or_default(), &urls)
                .await
                .map(|s| started_lines(&s, false))
                .map_err(|e| e.to_string())
        }
        "run_command" => {
            let argv: Vec<String> = args
                .texts("argv")
                .unwrap_or_default()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let stop_at = ctx.deadline.map(|d| {
                let at = d.into_std();
                (
                    at.checked_sub(ANSWER_MARGIN).unwrap_or(at),
                    format!(
                        "lmgw/timeout_ms = {}, less {:.1} s for the answer",
                        ctx.timeout_ms.unwrap_or_default(),
                        ANSWER_MARGIN.as_secs_f64()
                    ),
                )
            });
            let bounds = RunBounds {
                keep: ctx.room,
                stop_at,
            };
            l.run(&argv, args.text("cwd"), &bounds)
                .await
                .map(|r| ran_lines(&r, ctx.room, &ctx.room_says))
                .map_err(|e| e.to_string())
        }
        other => Err(format!("Kai's launch module has no tool {other}")),
    }
}

/// `open`'s and `app_launch`'s lines.
pub fn started_lines(s: &Started, opened: bool) -> String {
    let app = format!("{} ({})", shown_text(&s.name), shown_text(&s.id));
    let mut text = if opened {
        let mut t = format!("opened with {app}");
        if let Some(m) = &s.mime {
            t.push_str(&format!(" as {}", shown_text(m)));
        }
        t
    } else if s.instances > 1 {
        format!(
            "started {} instances of {app}, one per file or URL",
            s.instances
        )
    } else {
        format!("started {app}")
    };
    for n in &s.notes {
        text.push('\n');
        text.push_str(n);
    }
    text
}

/// A size as a person reads it, exact below 1 KiB.
pub fn size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if b < K * K {
        format!("{:.1} KiB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MiB", b / (K * K))
    } else {
        format!("{:.1} GiB", b / (K * K * K))
    }
}

fn signal_name(sig: i32) -> Option<&'static str> {
    Some(match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return None,
    })
}

/// One stream's bytes as text: each character with the bytes it came from and whether those were
/// not UTF-8 (an invalid sequence is one U+FFFD, as `String::from_utf8_lossy` writes it).
fn decoded(bytes: &[u8]) -> impl Iterator<Item = (char, u64, bool)> + '_ {
    bytes.utf8_chunks().flat_map(|chunk| {
        let bad = chunk.invalid().len() as u64;
        chunk
            .valid()
            .chars()
            .map(|c| (c, c.len_utf8() as u64, false))
            .chain((bad > 0).then_some(('\u{FFFD}', bad, true)))
    })
}

/// The bytes `c` takes in a JSON string.
fn esc(c: char) -> u64 {
    let mut b = [0u8; 4];
    escaped_len(c.encode_utf8(&mut b))
}

/// The bytes `bytes` take in a JSON string as text.
fn escaped(bytes: &[u8]) -> u64 {
    decoded(bytes).map(|(c, _, _)| esc(c)).sum()
}

/// `bytes` without a character cut off at its end.
fn whole_chars_at_end(bytes: &[u8]) -> &[u8] {
    let n = bytes.len();
    for back in 1..=n.min(4) {
        let b = bytes[n - back];
        if b & 0xC0 == 0x80 {
            continue;
        }
        let len = match b {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        return if len > back {
            &bytes[..n - back]
        } else {
            bytes
        };
    }
    bytes
}

/// `bytes` without a character's rest at its start.
fn whole_chars_at_start(bytes: &[u8]) -> &[u8] {
    let skip = bytes
        .iter()
        .take(3)
        .take_while(|b| (**b & 0xC0) == 0x80)
        .count();
    &bytes[skip..]
}

/// What a stream shows: its start (all of it when nothing is cut) and its end, with the bytes each
/// came from.
#[derive(Debug, Default, PartialEq, Eq)]
struct Shown {
    head: String,
    tail: String,
    head_bytes: u64,
    tail_bytes: u64,
    /// Bytes that are not UTF-8 show as U+FFFD.
    lossy: bool,
}

/// The longest start of `bytes` within `budget` escaped bytes; the escaped bytes it took.
fn take_start(bytes: &[u8], budget: u64, s: &mut Shown) -> u64 {
    let mut used = 0;
    for (c, raw, bad) in decoded(bytes) {
        let e = esc(c);
        if used + e > budget {
            break;
        }
        used += e;
        s.head.push(c);
        s.head_bytes += raw;
        s.lossy |= bad;
    }
    used
}

/// The longest end of `bytes` within `budget` escaped bytes.
fn take_end(bytes: &[u8], budget: u64, s: &mut Shown) {
    let mut rest = escaped(bytes);
    let mut keeping = false;
    for (c, raw, bad) in decoded(bytes) {
        keeping |= rest <= budget;
        if keeping {
            s.tail.push(c);
            s.tail_bytes += raw;
            s.lossy |= bad;
        } else {
            rest -= esc(c);
        }
    }
}

/// What of `c` shows within `budget` escaped bytes (`None`: no bound; only a stream kept whole
/// has none).
fn show(c: &Captured, budget: Option<u64>) -> Shown {
    let mut s = Shown::default();
    if c.cut() == 0 {
        let whole = [c.head.as_slice(), c.tail.as_slice()].concat();
        if budget.is_none_or(|b| escaped(&whole) <= b) {
            s.lossy = std::str::from_utf8(&whole).is_err();
            s.head = String::from_utf8_lossy(&whole).into_owned();
            s.head_bytes = c.total;
            return s;
        }
        // Start and end of the one buffer: they cannot meet, since all of it does not fit.
        let b = budget.unwrap_or(u64::MAX);
        let used = take_start(&whole, b / 2, &mut s);
        take_end(&whole, b - used, &mut s);
        return s;
    }
    let b = budget.unwrap_or(u64::MAX);
    let used = take_start(whole_chars_at_end(&c.head), b / 2, &mut s);
    take_end(whole_chars_at_start(&c.tail), b - used, &mut s);
    s
}

/// What a stream needs to show whole, in escaped bytes: more than any budget when it was not kept
/// whole.
fn need(c: &Captured) -> u64 {
    if c.cut() == 0 {
        escaped(&c.head) + escaped(&c.tail)
    } else {
        u64::MAX
    }
}

/// How `max` escaped bytes are shared between stdout's `out` and stderr's `err`: a stream that
/// needs less than half keeps all of it and the other gets the rest.
pub fn budgets(out: u64, err: u64, max: u64) -> (u64, u64) {
    if out.saturating_add(err) <= max {
        return (out, err);
    }
    let half = max / 2;
    if err <= half {
        (max - err, err)
    } else if out <= half {
        (out, max - out)
    } else {
        (max - half, half)
    }
}

/// A stream's part of the first line: `stdout 6 bytes`, `stderr none`, or its size with what was
/// shown and cut.
fn stream_summary(name: &str, c: &Captured, s: &Shown) -> String {
    if c.total == 0 {
        return format!("{name} none");
    }
    let cut = c.total.saturating_sub(s.head_bytes + s.tail_bytes);
    if cut == 0 {
        return format!("{name} {}", size(c.total));
    }
    format!(
        "{name} {} ({} bytes), its first {} and last {} shown, {} cut between",
        size(c.total),
        c.total,
        size(s.head_bytes),
        size(s.tail_bytes),
        size(cut)
    )
}

fn render(r: &Ran, out: &Shown, err: &Shown, says: &str) -> String {
    let ended = match &r.status {
        Status::Exited(c) => format!("exit {c}"),
        Status::Signalled(s) => match signal_name(*s) {
            Some(n) => format!("killed by signal {s} ({n})"),
            None => format!("killed by signal {s}"),
        },
        Status::TimedOut { setting } => {
            format!("stopped at its time limit ({setting}), its processes killed")
        }
    };
    let mut text = format!(
        "{ended} after {:.1} s; {}; {}",
        r.elapsed.as_secs_f64(),
        stream_summary("stdout", &r.stdout, out),
        stream_summary("stderr", &r.stderr, err)
    );
    let cut = |c: &Captured, s: &Shown| c.total.saturating_sub(s.head_bytes + s.tail_bytes);
    if cut(&r.stdout, out) + cut(&r.stderr, err) > 0 {
        text.push_str(&format!(" (cut to fit {says})"));
    }
    for (name, c, s) in [("stdout", &r.stdout, out), ("stderr", &r.stderr, err)] {
        if c.total == 0 {
            continue;
        }
        text.push('\n');
        text.push_str(name);
        if s.lossy {
            text.push_str(" (not all UTF-8: such bytes show as U+FFFD)");
        }
        text.push_str(":\n");
        text.push_str(&s.head);
        let n = cut(c, s);
        if n > 0 {
            if !s.head.is_empty() && !s.head.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("[{} of {name} cut here]\n", size(n)));
            text.push_str(&s.tail);
        }
        if text.ends_with('\n') {
            text.pop();
        }
    }
    text
}

/// `run_command`'s result (module doc), its text within `room` escaped bytes (`None`: no bound);
/// `room_says` names the link's message limit.
pub fn ran_lines(r: &Ran, room: Option<u64>, room_says: &str) -> String {
    let (out_need, err_need) = (need(&r.stdout), need(&r.stderr));
    // The output's share of the room: the room less the rest of the text, measured on the text
    // itself and taken again when the sizes it states came out longer.
    let mut reserve =
        escaped_len(&render(r, &Shown::default(), &Shown::default(), room_says)) + 256;
    loop {
        let link = room.map(|m| m.saturating_sub(reserve));
        let cap = r.cap.as_ref().map(|(b, _)| *b);
        let (max, says) = match (link, cap) {
            (Some(l), Some(c)) if c < l => (Some(c), r.cap.as_ref().map(|(_, s)| s.clone())),
            (None, Some(c)) => (Some(c), r.cap.as_ref().map(|(_, s)| s.clone())),
            (l, _) => (
                l,
                Some(format!("the tool link's message limit: {room_says}")),
            ),
        };
        let says = says.unwrap_or_default();
        let (out, err) = match max {
            None => (show(&r.stdout, None), show(&r.stderr, None)),
            Some(m) => {
                let (bo, be) = budgets(out_need, err_need, m);
                (show(&r.stdout, Some(bo)), show(&r.stderr, Some(be)))
            }
        };
        let text = render(r, &out, &err, &says);
        match room {
            Some(m) if escaped_len(&text) > m && link.is_some_and(|l| l > 0) => {
                reserve += escaped_len(&text) - m;
            }
            _ => return text,
        }
    }
}

#[cfg(test)]
mod tests;
