//! `kai tool` and `kai model`, the client's side (module doc of `crate::cli`): no runtime, no bus,
//! one blocking connection to the socket.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;

use super::{Answer, Request, Status};

pub const TOOL_USAGE: &str = "\
usage: kai tool                         list the tools Kai offers now
       kai tool <name> --help           one tool's parameters
       kai tool <name> [--<param> <value>]...   call a tool (--<param>=<value> works too; a
                                        list parameter takes its flag once per item)
";

pub const MODEL_USAGE: &str = "\
usage: kai model <alias> < prompt       the prompt on stdin, the model's answer on stdout
";

/// What the command line asks, or the usage error.
pub fn tool_request(args: &[OsString]) -> Result<Request, String> {
    let mut words = Vec::with_capacity(args.len());
    for a in args {
        words.push(
            a.to_str()
                .ok_or("an argument is not text (UTF-8)")?
                .to_owned(),
        );
    }
    let Some((name, rest)) = words.split_first() else {
        return Ok(Request::List { name: None });
    };
    if name.starts_with('-') {
        return Err(format!("{name}: the tool's name comes first"));
    }
    if rest.len() == 1 && (rest[0] == "--help" || rest[0] == "-h") {
        return Ok(Request::List {
            name: Some(name.clone()),
        });
    }
    let mut pairs = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let Some(flag) = rest[i].strip_prefix("--").filter(|f| !f.is_empty()) else {
            return Err(format!(
                "{name}: parameters are --<param> <value> (argument {} is not)",
                i + 1
            ));
        };
        if let Some((k, v)) = flag.split_once('=') {
            pairs.push((k.to_owned(), v.to_owned()));
            i += 1;
            continue;
        }
        let Some(value) = rest.get(i + 1) else {
            return Err(format!("{name}: --{flag} needs a value"));
        };
        pairs.push((flag.to_owned(), value.clone()));
        i += 2;
    }
    Ok(Request::Tool {
        name: name.clone(),
        args: pairs,
    })
}

/// Sends `request` to the Kai at `socket` and waits for its answer (as long as it takes: a routine
/// may run long; Ctrl+C ends the wait, which cancels the call). A script routine's stdout, streamed
/// before the answer, is put in front of the answer's text (the tests').
#[cfg(test)]
pub fn ask(socket: &Path, request: &Request) -> Result<Answer, String> {
    let mut streamed = Vec::new();
    let mut answer = ask_streaming(socket, request, &mut streamed)?;
    if !streamed.is_empty() {
        answer.text = format!("{}{}", String::from_utf8_lossy(&streamed), answer.text);
    }
    Ok(answer)
}

/// [`ask`], writing a script routine's stdout to `out` as it comes.
pub fn ask_streaming(
    socket: &Path,
    request: &Request,
    out: &mut dyn Write,
) -> Result<Answer, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        format!(
            "no Kai answers at {} ({e}); is Kai running?",
            socket.display()
        )
    })?;
    let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
    line.push('\n');
    // A request Kai refuses before reading it all (over the message limit) ends in a broken pipe
    // here, with Kai's refusal waiting to be read: that answer is the one said.
    let written = stream.write_all(line.as_bytes());
    // The write side stays open: Kai reads its end as the call's cancellation.
    let mut reader = BufReader::new(&stream);
    loop {
        let mut line = String::new();
        if let Err(e) = reader.read_line(&mut line) {
            return Err(match &written {
                Err(w) => format!("cannot write to Kai: {w}"),
                Ok(()) => format!("cannot read Kai's answer: {e}"),
            });
        }
        if line.is_empty() {
            return Err(match &written {
                Err(e) => format!("cannot write to Kai: {e}"),
                Ok(()) => "Kai closed the connection without an answer".into(),
            });
        }
        if let Ok(piece) = serde_json::from_str::<super::Stdout>(&line) {
            out.write_all(&piece.bytes()?)
                .and_then(|()| out.flush())
                .map_err(|e| format!("cannot write the routine's output: {e}"))?;
            continue;
        }
        return serde_json::from_str(&line).map_err(|e| format!("Kai's answer does not read: {e}"));
    }
}

/// Prints `answer` and says how `kai` exits.
fn said(answer: Answer) -> ExitCode {
    for n in &answer.notes {
        eprintln!("kai: {n}");
    }
    match answer.status {
        Status::Done => {
            if !answer.text.is_empty() {
                println!("{}", answer.text);
            }
            ExitCode::SUCCESS
        }
        Status::Failed => {
            eprintln!("{}", answer.text);
            ExitCode::from(1)
        }
        Status::Usage => {
            eprintln!("kai: {}", answer.text);
            ExitCode::from(2)
        }
    }
}

fn socket() -> Result<std::path::PathBuf, String> {
    let paths = crate::paths::Paths::from_env()?;
    Ok(crate::paths::cli_socket(&paths.runtime_dir))
}

/// `kai tool …`.
pub fn tool(args: &[OsString]) -> ExitCode {
    let request = match tool_request(args) {
        Ok(r) => r,
        Err(e) => {
            eprint!("kai: {e}\n{TOOL_USAGE}");
            return ExitCode::from(2);
        }
    };
    run(&request)
}

/// `kai model <alias>`.
pub fn model(args: &[OsString]) -> ExitCode {
    let alias = match args {
        [a] => match a.to_str() {
            Some(s) if !s.trim().is_empty() && !s.starts_with('-') => s.to_owned(),
            _ => {
                eprint!("kai: the alias is one of lmgw's model names\n{MODEL_USAGE}");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprint!("{MODEL_USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut prompt = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut prompt) {
        eprintln!("kai: the prompt on stdin cannot be read ({e}; it must be text)");
        return ExitCode::from(2);
    }
    if prompt.trim().is_empty() {
        eprint!("kai: no prompt on stdin\n{MODEL_USAGE}");
        return ExitCode::from(2);
    }
    run(&Request::Model { alias, prompt })
}

fn run(request: &Request) -> ExitCode {
    let path = match socket() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kai: {e}");
            return ExitCode::from(3);
        }
    };
    match ask_streaming(&path, request, &mut std::io::stdout().lock()) {
        Ok(a) => said(a),
        Err(e) => {
            eprintln!("kai: {e}");
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_command_line_reads_as_a_request() {
        assert_eq!(
            tool_request(&words(&[])).unwrap(),
            Request::List { name: None }
        );
        assert_eq!(
            tool_request(&words(&["notify", "--help"])).unwrap(),
            Request::List {
                name: Some("notify".into())
            }
        );
        assert_eq!(
            tool_request(&words(&[
                "run_command",
                "--argv",
                "ls",
                "--argv=-la",
                "--cwd",
                "--x"
            ]))
            .unwrap(),
            Request::Tool {
                name: "run_command".into(),
                args: vec![
                    ("argv".into(), "ls".into()),
                    ("argv".into(), "-la".into()),
                    ("cwd".into(), "--x".into()),
                ]
            }
        );
        for (bad, says) in [
            (&["--x"][..], "comes first"),
            (&["t", "value"][..], "argument 1"),
            (&["t", "--text"][..], "needs a value"),
        ] {
            let e = tool_request(&words(bad)).unwrap_err();
            assert!(e.contains(says), "{bad:?}: {e}");
        }
    }
}
