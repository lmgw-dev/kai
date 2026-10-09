//! Running routines: a script in its scope (the launch tests' fake user manager), its arguments,
//! environment, result and exit code; steps as the routine's own calls under K15; no routine
//! starts a routine.

use serde_json::{Map, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::desktop::fdo::launch::settings::LaunchConfig;
use crate::host::policy::Caller;
use crate::host::registry::{Args, Call};
use crate::host::settings::Named;

fn call_of(spec: &'static ToolSpec, args: serde_json::Value, caller: Caller) -> Call {
    Call::new(
        spec,
        Args::read(spec, args.as_object().cloned()).unwrap(),
        caller,
        CancellationToken::new(),
    )
}

async fn run(rig: &Rig, name: &str, args: serde_json::Value, caller: Caller) -> Reply {
    let entry = rig
        .local
        .entries()
        .into_iter()
        .find(|e| e.spec.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    (entry.handler)(call_of(entry.spec, args, caller)).await
}

const TWO: &str = "description = \"Greet someone.\"\nkind = \"script\"\n\
                   [[params]]\nname = \"who\"\nkind = \"text\"\n\
                   [[params]]\nname = \"times\"\nkind = \"integer\"\noptional = true\n";

#[tokio::test]
async fn a_script_runs_in_its_scope_and_its_exit_code_decides() {
    let Some(launch) = crate::desktop::fdo::launch::tests::rig(LaunchConfig::default()).await
    else {
        return;
    };
    let home = Home::new();
    home.routine("greet", TWO, Some("echo \"hello $1|$2\"; echo note >&2"));
    home.routine(
        "fails",
        "description = \"Fail.\"\nkind = \"script\"\n",
        Some("echo partial; exit 3"),
    );
    home.routine(
        "quiet",
        "description = \"Nothing to say.\"\nkind = \"script\"\n",
        Some("true"),
    );
    for n in ["greet", "fails", "quiet"] {
        home.switch_on(n);
    }
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(Some(launch.launch.manager.clone()));
    let r = run(&rig, "greet", json!({"who": "world"}), Caller::Owner).await;
    assert_eq!(
        r,
        Reply::text("hello world|"),
        "an omitted parameter is empty"
    );
    let r = run(
        &rig,
        "greet",
        json!({"who": "you", "times": 2}),
        Caller::Owner,
    )
    .await;
    assert_eq!(r, Reply::text("hello you|2"));
    let r = run(&rig, "fails", json!({}), Caller::Owner).await;
    assert!(r.error);
    assert_eq!(text(&r), "partial\nfailed: fails exited with 3");
    assert_eq!(
        run(&rig, "quiet", json!({}), Caller::Owner).await,
        Reply::text("done")
    );
    let scopes = launch.scopes();
    assert_eq!(scopes.len(), 4);
    for s in &scopes {
        assert!(
            s.unit.starts_with("app-dev.rootshell.kai.routine.") && s.unit.ends_with(".scope"),
            "{}",
            s.unit
        );
        // Kai's scope while its run went on; after, anyone's (`cli`).
        let text = format!(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/{}",
            s.unit
        );
        assert_eq!(
            crate::cli::server::caller_of(&text, &rig.scopes).describe(),
            "the command line (kai tool)",
            "the run ended: the scope is Kai's no more"
        );
        let live = crate::routines::LiveScopes::default();
        let _mark = live.mark(&s.unit);
        let caller = crate::cli::server::caller_of(&text, &live);
        assert!(
            ["routine 'greet'", "routine 'fails'", "routine 'quiet'"]
                .contains(&caller.describe().as_str()),
            "{}",
            s.unit
        );
        let d: String = s.props["Description"]
            .try_clone()
            .unwrap()
            .try_into()
            .unwrap();
        assert!(d.starts_with("Kai routine "), "{d}");
    }
}

#[tokio::test]
async fn the_scripts_environment_holds_no_secret() {
    let Some(launch) = crate::desktop::fdo::launch::tests::rig(LaunchConfig::default()).await
    else {
        return;
    };
    let home = Home::new();
    home.routine(
        "env_dump",
        "description = \"Dump the environment.\"\nkind = \"script\"\n",
        Some("env > env.txt; pwd >> env.txt"),
    );
    home.switch_on("env_dump");
    let (mut rig, _) = rig_with(home, PolicyConfig::default());
    rig.link("http://127.0.0.1:9");
    rig.run_with(Some(launch.launch.manager.clone()));
    let r = run(&rig, "env_dump", json!({}), Caller::Owner).await;
    assert!(!r.error, "{}", text(&r));
    let dumped = std::fs::read_to_string(rig.home.routines().join("env_dump/env.txt")).unwrap();
    let key = DeviceKey::parse(KEY).unwrap();
    assert!(
        !dumped.contains(key.expose()),
        "the device key never reaches a script"
    );
    assert!(
        !dumped.contains("KAI_LOG="),
        "Kai's own variables stay Kai's"
    );
    assert!(dumped.contains("RIG_MARK=1"), "the session's environment");
    assert!(dumped.contains("KAI_ROUTINE=env_dump"));
    assert!(dumped.contains(&format!("KAI_CLI_SOCKET={}", rig.home.socket().display())));
    assert!(
        dumped
            .trim_end()
            .ends_with(&rig.home.routines().join("env_dump").display().to_string()),
        "it runs in its folder"
    );
}

#[test]
fn the_path_finds_this_kai_first() {
    use super::super::script::environment;
    let tmp = crate::desktop::tests::bus::temp_dir();
    let mine = tmp.path().join("mine");
    let other = tmp.path().join("other");
    for d in [&mine, &other] {
        crate::desktop::fdo::launch::tests::script(&d.join("kai"), "true");
    }
    let socket = tmp.path().join("cli.sock");
    let own = vec![("PATH".to_owned(), format!("{}:/usr/bin", other.display()))];
    let env = environment(None, &own, "r", &socket, Some(&mine));
    let path = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
    assert_eq!(
        path,
        &format!("{}:{}:/usr/bin", mine.display(), other.display())
    );
    let own = vec![("PATH".to_owned(), format!("{}:/usr/bin", mine.display()))];
    let env = environment(None, &own, "r", &socket, Some(&mine));
    let path = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
    assert_eq!(
        path,
        &format!("{}:/usr/bin", mine.display()),
        "already first"
    );
}

#[tokio::test]
async fn steps_are_the_routines_own_calls_under_k15() {
    let home = Home::new();
    home.routine(
        "tidy",
        "description = \"Tidy up.\"\nkind = \"steps\"\nread_only = true\n\
         [[params]]\nname = \"who\"\nkind = \"text\"\n\
         [[steps]]\ntool = \"echo\"\nargs = { text = \"hello {who}\" }\n\
         [[steps]]\ntool = \"poke\"\nargs = { text = \"{who}\" }\n",
        None,
    );
    home.switch_on("tidy");
    let (mut rig, _) = rig_with(
        home,
        granted(
            PolicyConfig {
                trusted_callers: vec!["routine:tidy".into()],
                ..Default::default()
            },
            &["tidy"],
        ),
    );
    rig.run_with(None);
    let entry = rig
        .local
        .entries()
        .into_iter()
        .find(|e| e.spec.name == "tidy")
        .unwrap();
    let mut args = Map::new();
    args.insert("who".into(), json!("Notes"));
    let local = rig.local.clone();
    // The owner runs it: a read-only routine runs unasked.
    let pending = tokio::spawn(async move {
        local
            .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
            .await
    });
    // echo is read-only and the routine trusted: unasked. poke changes state: asked, as the
    // routine's call.
    let ask = rig.ask().await;
    assert_eq!(ask.caller, "routine 'tidy'");
    assert_eq!(ask.tool, "poke");
    assert_eq!(ask.lines, ["text: Notes"]);
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert!(!r.error, "{}", text(&r));
    assert_eq!(text(&r), "echo: echo: hello Notes\npoke: poke: Notes");
    assert_eq!(
        rig.ran.all(),
        [
            "echo by routine 'tidy': hello Notes",
            "poke by routine 'tidy': Notes"
        ]
    );
    // Untrusted, the read-only step is asked about too; a failing step ends the run. (The grant
    // stays: the owner runs tidy unasked.)
    rig.policy
        .write()
        .unwrap()
        .clone_from(&Policy::new(&granted(PolicyConfig::default(), &["tidy"])));
    let entry = rig.local.entry_any("tidy").unwrap();
    let local = rig.local.clone();
    let pending = tokio::spawn(async move {
        let mut args = Map::new();
        args.insert("who".into(), json!("fail"));
        local
            .call(&entry, Some(args), &Caller::Owner, CancellationToken::new())
            .await
    });
    let ask = rig.ask().await;
    assert_eq!(
        (ask.tool.as_str(), ask.caller.as_str()),
        ("echo", "routine 'tidy'")
    );
    ask.reply.send(true).unwrap();
    let ask = rig.ask().await;
    assert_eq!(ask.tool, "poke");
    ask.reply.send(true).unwrap();
    let r = pending.await.unwrap();
    assert!(r.error);
    assert!(
        text(&r).ends_with("poke: it failed\nstep 2 (poke) failed; the rest did not run"),
        "{}",
        text(&r)
    );
}

#[tokio::test]
async fn a_routine_does_not_start_routines() {
    let home = Home::new();
    home.routine(
        "inner_one",
        "description = \"Echo.\"\nkind = \"steps\"\n[[steps]]\ntool = \"echo\"\nargs = { text = \"x\" }\n",
        None,
    );
    home.switch_on("inner_one");
    let (rig, _) = rig_with(home, PolicyConfig::default());
    rig.run_with(None);
    let r = run(
        &rig,
        "inner_one",
        json!({}),
        Caller::Named(Named::routine("outer")),
    )
    .await;
    assert!(r.error);
    assert_eq!(
        text(&r),
        "a routine does not start routines (inner_one was called by the routine outer)"
    );
    assert!(rig.ran.all().is_empty());
}
