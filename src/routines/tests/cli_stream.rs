//! The CLI's bounds: a script routine's stdout streams to the client as it comes (Kai keeps none
//! of it), and a request larger than the host link's message limit is refused naming the limit.

use super::cli::{serve, wait_for_socket};
use super::*;
use crate::cli::server::proc_cgroup;
use crate::cli::{Answer, Request, Status, client};
use crate::desktop::fdo::launch::settings::LaunchConfig;

async fn ask_streaming(rig: &Rig, request: Request) -> (Vec<u8>, Answer) {
    let socket = rig.home.socket();
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let a = client::ask_streaming(&socket, &request, &mut out).unwrap();
        (out, a)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_scripts_stdout_streams_to_the_client() {
    let Some(launch) = crate::desktop::fdo::launch::tests::rig(LaunchConfig::default()).await
    else {
        return;
    };
    let home = Home::new();
    home.routine(
        "chatty",
        "description = \"Say a lot.\"\nkind = \"script\"\nread_only = true\n",
        // More than a pipe's buffer, then a last line without its line break.
        Some("head -c 300000 /dev/zero | tr '\\0' x; echo; printf 'end'"),
    );
    home.routine(
        "fails",
        "description = \"Fail.\"\nkind = \"script\"\nread_only = true\n",
        Some("echo partial; exit 3"),
    );
    home.switch_on("chatty");
    home.switch_on("fails");
    let (rig, _) = rig_with(
        home,
        granted(
            PolicyConfig {
                trusted_callers: vec!["cli".into()],
                ..Default::default()
            },
            &["chatty", "fails"],
        ),
    );
    rig.run_with(Some(launch.launch.manager.clone()));
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let tool = |name: &str| Request::Tool {
        name: name.into(),
        args: vec![],
    };
    let (out, a) = ask_streaming(&rig, tool("chatty")).await;
    assert_eq!(out.len(), 300_000 + 1 + 3);
    assert!(out.ends_with(b"x\nend"));
    assert_eq!(a, Answer::done(""), "the answer holds none of it");
    let (out, a) = ask_streaming(&rig, tool("fails")).await;
    assert_eq!(out, b"partial\n");
    assert_eq!(a.status, Status::Failed);
    assert_eq!(a.text, "failed: fails exited with 3");
    server.abort();
}

#[tokio::test]
async fn a_request_over_the_message_limit_is_refused_naming_it() {
    let (rig, _) = rig_with(Home::new(), PolicyConfig::default());
    rig.run_with(None);
    rig.local.set_limits(crate::host::limits::LinkLimits {
        message: Some(200),
        frame: None,
        announced: true,
    });
    let server = serve(&rig, proc_cgroup);
    wait_for_socket(&rig).await;
    let (_, a) = ask_streaming(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "y".repeat(400))],
        },
    )
    .await;
    assert_eq!(a.status, Status::Usage);
    assert!(
        a.text
            .contains("larger than the host link's message limit (200 bytes"),
        "{}",
        a.text
    );
    assert!(rig.ran.all().is_empty());
    // One far larger than the socket's buffer: the client's write breaks, Kai's refusal is read.
    let (_, a) = ask_streaming(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "y".repeat(8 << 20))],
        },
    )
    .await;
    assert_eq!(a.status, Status::Usage, "{}", a.text);
    assert!(a.text.contains("(200 bytes"), "{}", a.text);
    // Within it: answered (echo is read-only; cli is asked, so trust it for this call).
    rig.policy
        .write()
        .unwrap()
        .clone_from(&Policy::new(&PolicyConfig {
            trusted_callers: vec!["cli".into()],
            ..Default::default()
        }));
    let (_, a) = ask_streaming(
        &rig,
        Request::Tool {
            name: "echo".into(),
            args: vec![("text".into(), "short".into())],
        },
    )
    .await;
    assert_eq!(a, Answer::done("echo: short"));
    server.abort();
}
