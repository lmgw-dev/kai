//! The CLI over a socket under target/: a script's call (a peer in a routine's scope) reaches K15
//! as the routine, a terminal's is the caller `cli`; the socket's mode; arguments by type; a
//! client that goes cancels its call; `kai model` through Kai's pairing.

use std::os::unix::fs::PermissionsExt;

use serde_json::Value;

use super::*;
use crate::cli::server::{Backend, caller_of, proc_cgroup};
use crate::cli::{Answer, Request, Status, client};

const BACKUP_SCOPE: &str = "app-dev.rootshell.kai.routine.backup-4d2a91c.scope";

fn in_routine_scope(_pid: i32) -> Option<String> {
    Some(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
         app-dev.rootshell.kai.routine.backup-4d2a91c.scope\n"
            .into(),
    )
}

pub(super) fn serve(
    rig: &Rig,
    cgroup_of: fn(i32) -> Option<String>,
) -> tokio::task::JoinHandle<()> {
    let backend = Backend {
        local: rig.local.clone(),
        creds: rig.creds.subscribe(),
        cgroup_of,
        scopes: rig.scopes.clone(),
    };
    let socket = rig.home.socket();
    tokio::spawn(crate::cli::server::serve(socket, backend))
}

pub(super) async fn wait_for_socket(rig: &Rig) {
    let end = std::time::Instant::now() + T;
    while !rig.home.socket().exists() {
        assert!(std::time::Instant::now() < end, "no socket");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// `kai`'s own request, made by the client's code on a blocking thread.
async fn ask(rig: &Rig, request: Request) -> Answer {
    let socket = rig.home.socket();
    tokio::task::spawn_blocking(move || client::ask(&socket, &request))
        .await
        .unwrap()
        .unwrap()
}

#[test]
fn the_cgroup_names_the_caller() {
    let scopes = crate::routines::LiveScopes::default();
    // A scope of a routine's name that Kai did not start (systemd-run --user --scope
    // --unit=app-dev.rootshell.kai.routine.backup-…) is not the routine.
    assert_eq!(
        caller_of(&in_routine_scope(1).unwrap(), &scopes).describe(),
        "the command line (kai tool)"
    );
    let mark = scopes.mark(BACKUP_SCOPE);
    assert_eq!(
        caller_of(&in_routine_scope(1).unwrap(), &scopes).describe(),
        "routine 'backup'"
    );
    drop(mark);
    assert_eq!(
        caller_of(&in_routine_scope(1).unwrap(), &scopes).describe(),
        "the command line (kai tool)",
        "the run ended: its scope is Kai's no more"
    );
    let _marks = [
        scopes.mark(BACKUP_SCOPE),
        scopes.mark("app-dev.rootshell.kai.routine.a_b2-ff.scope"),
        scopes.mark("app-dev.rootshell.kai.routine.Bad-1.scope"),
        scopes.mark("app-dev.rootshell.kai.routine.x.scope"),
        scopes.mark("app-dev.rootshell.kai.run-1a.scope"),
    ];
    for not_a_routine in [
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.konsole-1234.scope",
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-dev.rootshell.kai.routine.Bad-1.scope",
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-dev.rootshell.kai.routine.x.scope",
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-dev.rootshell.kai.run-1a.scope",
        "",
    ] {
        assert_eq!(
            caller_of(not_a_routine, &scopes).describe(),
            "the command line (kai tool)",
            "{not_a_routine}"
        );
    }
    // A process the script started in a child cgroup of its scope is the routine's too.
    assert_eq!(
        caller_of(
            "0::/user.slice/user@1000.service/app.slice/app-dev.rootshell.kai.routine.a_b2-ff.scope/sub",
            &scopes
        )
        .describe(),
        "routine 'a_b2'"
    );
}

#[tokio::test]
async fn a_scripts_call_reaches_k15_as_the_routine() {
    let (mut rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.run_with(None);
    let _running = rig.scopes.mark(BACKUP_SCOPE);
    let server = serve(&rig, in_routine_scope);
    wait_for_socket(&rig).await;
    let mode = std::fs::metadata(rig.home.socket())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let pending = {
        let socket = rig.home.socket();
        tokio::task::spawn_blocking(move || {
            client::ask(
                &socket,
                &Request::Tool {
                    name: "echo".into(),
                    args: vec![("text".into(), "hi".into())],
                },
            )
        })
    };
    // echo is read-only; a routine is an untrusted caller unless the owner trusts it.
    let q = rig.ask().await;
    assert_eq!(q.caller, "routine 'backup'");
    assert_eq!(q.lines, ["text: hi"]);
    q.reply.send(true).unwrap();
    let a = pending.await.unwrap().unwrap();
    assert_eq!(a, Answer::done("echo: hi"));
    assert_eq!(rig.ran.all(), ["echo by routine 'backup': hi"]);
    // Trusted: unasked.
    rig.policy
        .write()
        .unwrap()
        .clone_from(&Policy::new(&PolicyConfig {
            trusted_callers: vec!["routine:backup".into()],
            ..Default::default()
        }));
    let a = ask(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "again".into())],
        },
    )
    .await;
    assert_eq!(a, Answer::done("echo: again"));
    server.abort();
}

#[tokio::test]
async fn a_terminals_call_is_the_caller_cli() {
    let (mut rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.run_with(None);
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let pending = {
        let socket = rig.home.socket();
        tokio::task::spawn_blocking(move || {
            client::ask(
                &socket,
                &Request::Tool {
                    name: "poke".into(),
                    args: vec![("text".into(), "x".into())],
                },
            )
        })
    };
    let q = rig.ask().await;
    assert_eq!(q.caller, "the command line (kai tool)");
    q.reply.send(false).unwrap();
    let a = pending.await.unwrap().unwrap();
    assert_eq!(a.status, Status::Failed);
    assert_eq!(a.text, "the owner denied this call of poke");
    assert!(rig.ran.all().is_empty());
    // `cli` in the trust list: read-only tools unasked; a state-changing one is still asked (K15).
    rig.policy
        .write()
        .unwrap()
        .clone_from(&Policy::new(&PolicyConfig {
            trusted_callers: vec!["cli".into()],
            tools: [("poke".to_owned(), crate::host::settings::ToolState::Allow)]
                .into_iter()
                .collect(),
            ..Default::default()
        }));
    let a = ask(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "t".into())],
        },
    )
    .await;
    assert_eq!(a, Answer::done("echo: t"));
    let a = ask(
        &rig,
        Request::Tool {
            name: "poke".into(),
            args: vec![("text".into(), "p".into())],
        },
    )
    .await;
    assert_eq!(a, Answer::done("poke: p"), "allow: unasked (ruling 28)");
    server.abort();
}

#[tokio::test]
async fn the_cli_lists_reads_arguments_by_type_and_refuses_what_kai_lacks() {
    let home = Home::new();
    home.routine(
        "count",
        "description = \"Count.\"\nkind = \"steps\"\nread_only = true\n\
         [[params]]\nname = \"n\"\nkind = \"integer\"\n\
         [[steps]]\ntool = \"echo\"\nargs = { text = \"{n}\" }\n",
        None,
    );
    home.switch_on("count");
    let (rig, _) = rig_with(home, granted(PolicyConfig::default(), &["count"]));
    rig.run_with(None);
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let list = ask(&rig, Request::List { name: None }).await;
    assert_eq!(list.status, Status::Done);
    assert!(
        list.text
            .contains("echo --text <a text>: Answer with the text."),
        "{}",
        list.text
    );
    assert!(
        list.text.contains("count --n <an integer>: Count."),
        "{}",
        list.text
    );
    let one = ask(
        &rig,
        Request::List {
            name: Some("count".into()),
        },
    )
    .await;
    assert_eq!(one.text, "count --n <an integer>: Count.\n  --n an integer");
    let bad = ask(
        &rig,
        Request::Tool {
            name: "count".into(),
            args: vec![("n".into(), "three".into())],
        },
    )
    .await;
    assert_eq!(bad, Answer::usage("count: --n must be an integer"));
    let none = ask(
        &rig,
        Request::Tool {
            name: "nope".into(),
            args: vec![],
        },
    )
    .await;
    assert_eq!(none.status, Status::Usage);
    let extra = ask(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "a".into()), ("secret".into(), "s3".into())],
        },
    )
    .await;
    assert_eq!(extra.status, Status::Failed);
    assert_eq!(extra.text, "echo has no parameter 'secret' (it takes text)");
    // The owner's terminal calling a read-only routine: cli is untrusted, so it is asked.
    server.abort();
}

#[tokio::test]
async fn a_client_that_goes_cancels_its_call() {
    let (mut rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.run_with(None);
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let socket = rig.home.socket();
    let client = std::thread::spawn(move || {
        use std::io::Write;
        let mut s = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        s.write_all(b"{\"op\":\"tool\",\"name\":\"poke\",\"args\":[[\"text\",\"x\"]]}\n")
            .unwrap();
        s
    });
    let q = rig.ask().await;
    drop(client.join().unwrap());
    // The dialog's answer is dropped: the call ended without anyone answering it.
    tokio::time::timeout(T, async {
        while !q.reply.is_closed() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the question stayed open");
    assert!(rig.ran.all().is_empty());
    server.abort();
}

#[tokio::test]
async fn kai_model_asks_lmgw_as_kais_device() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let lmgw = crate::lmgw_link::test_pki::Gateway::bind().await;
    let url = lmgw.url.clone();
    let fake = tokio::spawn(async move {
        let (mut s, _) = lmgw.accept().await.unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if body.len() >= len {
                    break;
                }
            }
        }
        let answer = r#"{"choices":[{"message":{"content":"four"},"finish_reason":"stop"}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nx-lmgw-fallback: cloud/x\r\ncontent-length: {}\r\n\r\n{answer}",
            answer.len()
        );
        s.write_all(resp.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&got).to_string()
    });
    let (mut rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.run_with(None);
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let unlinked = ask(
        &rig,
        Request::Model {
            alias: "gemma".into(),
            prompt: "2+2?".into(),
        },
    )
    .await;
    assert_eq!(
        unlinked,
        Answer::failed("Kai is not linked to lmgw (pair it first)")
    );
    rig.link(&url);
    let a = ask(
        &rig,
        Request::Model {
            alias: "gemma".into(),
            prompt: "2+2?".into(),
        },
    )
    .await;
    assert_eq!(a.status, Status::Done);
    assert_eq!(a.text, "four");
    assert_eq!(a.notes, ["(model: gemma, answered by cloud/x)"]);
    let sent = fake.await.unwrap();
    assert!(sent.starts_with("POST /v1/chat/completions"), "{sent}");
    assert!(
        !sent.to_ascii_lowercase().contains("authorization"),
        "the device's certificate authenticates it: {sent}"
    );
    let body: Value = serde_json::from_str(sent.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], "gemma");
    assert_eq!(body["messages"][0]["content"], "2+2?");
    server.abort();
}

/// The log names the operation, the tool or alias and the caller: never an argument, a prompt, an
/// answer or a credential.
#[test]
fn a_trace_log_carries_nothing_private() {
    let _ = tracing_log::LogTracer::init();
    // A second live dispatcher, as the delegate's log test explains.
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let out = crate::logging::test_support::Capture::default();
    tracing::subscriber::with_default(
        crate::logging::subscriber("trace", out.clone(), false),
        || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let lmgw = crate::delegate::tests::Lmgw::start(
                        crate::delegate::tests::folders(None),
                        vec![crate::delegate::tests::text_turn("ANSWER-31337")],
                    )
                    .await;
                    let (mut rig, _) = rig_with(
                        Home::new(),
                        PolicyConfig {
                            trusted_callers: vec!["cli".into()],
                            ..Default::default()
                        },
                    );
                    rig.link(&lmgw.url);
                    rig.run_with(None);
                    let server = serve(&rig, proc_cgroup);
                    wait_for_socket(&rig).await;
                    let a = ask(
                        &rig,
                        Request::Tool {
                            name: "echo".into(),
                            args: vec![("text".into(), "ARG-4711".into())],
                        },
                    )
                    .await;
                    assert_eq!(a, Answer::done("echo: ARG-4711"));
                    let a = ask(
                        &rig,
                        Request::Model {
                            alias: "gemma".into(),
                            prompt: "PROMPT-0815".into(),
                        },
                    )
                    .await;
                    assert_eq!(a.text, "ANSWER-31337");
                    server.abort();
                });
        },
    );
    let text = out.text();
    for want in ["cli: a tool call", "cli: a model call", "host: answered"] {
        assert!(text.contains(want), "'{want}' missing: {text}");
    }
    for private in ["PRIVATE KEY", "ARG-4711", "PROMPT-0815", "ANSWER-31337"] {
        assert!(!text.contains(private), "{private} is in the log");
    }
}
