//! WP9's reading aloud through the fake Firefox and a fake speech (kai-firefox design §7.3):
//! paragraphs skipped (nothing to say, a `none` frame, an opaque top origin), marked as they
//! begin, the next made meanwhile; pause, resume, skip; the owner's barge-in; `read_ended` ending
//! it at once with one notice (none for Kai's own action), events on or off; a refused paragraph
//! skipped with a notice, a failure pausing with one, a second failure of the same paragraph
//! skipped; a site that became `none` while it waited; the previous reading kept when
//! `read.open@1` fails.

use serde_json::json;

use super::fake::{Host, Model, Twist, answer, shop};
use super::speak::{announcing, ops};
use crate::browser::settings::ReadAloud;
use crate::host::settings::PolicyConfig;
use crate::voice::speak::{Dropped, Failure, Kind, Said, Spoken, Utterance};

fn page(text: &str, more: serde_json::Value) -> serde_json::Value {
    let mut p = json!({"text": text, "spoken": text, "lang": "en", "ref": 300,
                       "origin": "https://shop.example"});
    for (k, x) in more.as_object().unwrap() {
        p[k] = x.clone();
    }
    p
}

/// A model with `paragraphs`, read from the first.
fn reading(paragraphs: Vec<serde_json::Value>) -> Model {
    let mut model = shop();
    model.paragraphs = paragraphs;
    model.read_start = Some((0, "top"));
    model
}

/// Hands and eyes on (which lists reading aloud; `announcing` has announcements on), `none` for
/// bank.example.
fn hands(h: &Host) {
    let mut config = announcing(&["bank.example"]);
    config.hands_and_eyes = true;
    config.read = ReadAloud {
        voices: vec!["de tts-german anna".into()],
        ..Default::default()
    };
    h.set(PolicyConfig::default(), config);
}

fn en(u: &Utterance) -> &str {
    u.text.en()
}

/// The next utterance that is a notice of Kai's own.
async fn notice(h: &Host) -> Utterance {
    for _ in 0..6 {
        let u = h.mouth.next().await;
        if u.kind == Kind::Notice {
            return u;
        }
    }
    panic!("no notice");
}

/// Reading aloud through the fake speech (§7.3): empty, `none` and opaque-origin paragraphs
/// skipped, the next handed over while one plays, each marked as it begins; pause, resume, skip;
/// the owner's barge-in pauses and what it waits for is in its line.
#[tokio::test]
async fn reading_aloud_paragraph_by_paragraph() {
    let h = Host::start().await;
    let model = reading(vec![
        page("Three nights", json!({"heading": 1, "ref": 214})),
        page("https://x.example", json!({"spoken": ""})),
        page(
            "Card 4111",
            json!({"frame": 7, "origin": "https://bank.example"}),
        ),
        // An opaque top origin gets the strictest rule (none here).
        page("Opaque", json!({"origin": null})),
        page("We left at dawn.", json!({"lang": "de-DE"})),
        page("Then north.", json!({"lang": null})),
    ]);
    let ff = h.firefox("c0ffee", &ops(), model).await;
    hands(&h);
    let (error, said) = h.owner("read_aloud", json!({})).await;
    assert!(!error, "{said}");
    assert_eq!(
        said,
        "\"Checkout\" https://shop.example/cart (tab 12): reading aloud from paragraph 1 of 6 \
         (the top of the page); read_pause, read_resume and read_skip control it"
    );
    let p0 = h.mouth.next().await;
    assert_eq!((p0.kind, en(&p0)), (Kind::Reading, "Three nights"));
    assert!(p0.voice.is_none(), "English: Kai's own voice");
    assert_eq!(
        (p0.gate.as_ref().unwrap())(),
        Ok(true),
        "its site rules hold"
    );
    // Paragraphs 1 (nothing to speak), 2 (a frame of a none site) and 3 (opaque) are skipped; 4
    // is handed over while 0 plays, in the voice for German, with no language sent.
    let p4 = h.mouth.next().await;
    assert_eq!(en(&p4), "We left at dawn.");
    let v = p4.voice.clone().unwrap();
    assert_eq!(
        (v.model.as_str(), v.voice.as_deref(), v.seed),
        ("tts-german", Some("anna"), None)
    );
    let rules = ff.arrived("read.paragraph@1", 5).await;
    assert_eq!(
        rules[0]["rules"],
        json!({"hyphens": true, "markdown": true, "citations": true, "superscripts": false,
               "urls": true, "symbols": true})
    );
    answer(&p0, Said::Started);
    let marks = ff.arrived("read.mark@1", 1).await;
    assert_eq!(marks[0]["index"], 0);
    answer(&p0, Said::Done(Spoken::Heard));
    let p5 = h.mouth.next().await;
    assert_eq!(en(&p5), "Then north.");
    answer(&p4, Said::Started);
    assert_eq!(ff.arrived("read.mark@1", 2).await[1]["index"], 4);
    // What the speech waits for is said.
    answer(&p5, Said::Waiting("do not disturb is on".into()));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (_, ctx) = h.owner("browser_context", json!({})).await;
    assert!(
        ctx.contains("reading aloud (waiting: do not disturb is on): \"Checkout\""),
        "{ctx}"
    );

    // read_pause stops both; read_resume reads paragraph 4 again from its start.
    let (_, paused) = h.owner("read_pause", json!({})).await;
    assert!(
        paused.ends_with(
            "paused (read_pause): \"Checkout\" https://shop.example/cart (tab 12), \
             paragraph 5 of 6"
        ),
        "{paused}"
    );
    assert!(p4.cancel.is_cancelled() && p5.cancel.is_cancelled());
    let (_, resumed) = h.owner("read_resume", json!({})).await;
    assert!(resumed.starts_with("reading aloud: "), "{resumed}");
    let again = h.mouth.next().await;
    assert_eq!(en(&again), "We left at dawn.");
    let next = h.mouth.next().await;
    // A late answer about the paused generation changes nothing.
    answer(&p4, Said::Done(Spoken::Cut("you spoke".into())));

    // read_skip back one: from paragraph 3, which is skipped again, so 4.
    let (error, skipped) = h.owner("read_skip", json!({"by": -1})).await;
    assert!(!error, "{skipped}");
    assert!(again.cancel.is_cancelled() && next.cancel.is_cancelled());
    let from = h.mouth.next().await;
    assert_eq!(en(&from), "We left at dawn.");
    let (error, past) = h.owner("read_skip", json!({"by": 9})).await;
    assert!(error && past.contains("paragraphs 1 to 6"), "{past}");

    // The owner talks over it: paused until read_resume.
    answer(&from, Said::Started);
    answer(&from, Said::Done(Spoken::Cut("you spoke".into())));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (_, ctx) = h.owner("browser_context", json!({})).await;
    assert!(
        ctx.contains("reading aloud, paused (you spoke): \"Checkout\""),
        "{ctx}"
    );

    // The session ended meanwhile (its read_ended lost): resuming finds it, says so once.
    ff.model().read_ended = Some("discarded");
    let (_, _) = h.owner("read_resume", json!({})).await;
    let n = notice(&h).await;
    assert_eq!(en(&n), "Reading stopped: Firefox unloaded the tab.");
    let crate::voice::speak::Text::Kai(w) = &n.text else {
        panic!("Kai's words");
    };
    assert_eq!(w.de, "Vorlesen beendet: Firefox hat den Tab entladen.");
    let (error, ended) = h.owner("read_resume", json!({})).await;
    assert!(
        error && ended.contains("ended (Firefox unloaded the tab)"),
        "{ended}"
    );
    assert!(
        ff.model().ops.iter().all(|(o, _)| o != "read.close@1"),
        "the extension ended it: nothing to close"
    );
}

/// The extension's `read_ended` ends the reading at once: what plays
/// and what waits are taken back, and Kai says why once; when Kai's own action ended it, nothing
/// is said. `read.open@1` with nothing to read says so.
#[tokio::test]
async fn read_ended_stops_it_at_once_and_says_why_once() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &ops(),
            reading(vec![page("One.", json!({})), page("Two.", json!({}))]),
        )
        .await;
    hands(&h);
    h.owner("read_aloud", json!({})).await;
    let one = h.mouth.next().await;
    let two = h.mouth.next().await;
    answer(&one, Said::Started);
    let session = ff.arrived("read.mark@1", 1).await[0]["session"].clone();
    // Another session's end says nothing (the positive control's twin).
    ff.event(
        json!({"kind": "read_ended", "at": 1, "focused": true, "by_kai": false,
                    "session": 1, "tab": 12, "why": "navigated"}),
    );
    h.mouth.silent(100).await;
    assert!(!one.cancel.is_cancelled());
    ff.event(
        json!({"kind": "read_ended", "at": 2, "focused": true, "by_kai": false,
                    "session": session, "tab": 12, "why": "navigated"}),
    );
    let n = h.mouth.next().await;
    assert_eq!(
        (n.kind, en(&n)),
        (Kind::Notice, "Reading stopped: the page changed.")
    );
    assert!(one.cancel.is_cancelled() && two.cancel.is_cancelled());
    h.mouth.silent(100).await;
    let (error, ended) = h.owner("read_resume", json!({})).await;
    assert!(
        error && ended.contains("ended (the page changed)"),
        "{ended}"
    );
    assert!(
        ff.model().ops.iter().all(|(o, _)| o != "read.close@1"),
        "the extension ended it: nothing to close"
    );

    // Kai's own action ended it: stopped, nothing said.
    h.owner("read_aloud", json!({})).await;
    let three = h.mouth.next().await;
    let _ = h.mouth.next().await;
    answer(&three, Said::Started);
    let session = ff.arrived("read.mark@1", 2).await[1]["session"].clone();
    ff.event(
        json!({"kind": "read_ended", "at": 3, "focused": true, "by_kai": true,
                    "session": session, "tab": 12, "why": "tab_closed"}),
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(three.cancel.is_cancelled());
    h.mouth.silent(100).await;

    ff.model().read_start = None;
    let (error, said) = h.owner("read_aloud", json!({})).await;
    assert!(!error, "{said}");
    assert!(said.ends_with(": nothing to read aloud"), "{said}");
}

/// When the speech fails: a paragraph lmgw refuses is skipped with one notice and the reading goes
/// on, a second refusal in a row pauses it; another failure pauses it with one notice; resumed, a
/// second failure of the same paragraph skips it, so a resume never meets the same failure
/// twice. A paragraph whose site became `none` while it waited ends the reading.
#[tokio::test]
async fn failures_skip_or_pause_and_never_loop() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &ops(),
            reading((0..6).map(|i| page(&format!("P{i}."), json!({}))).collect()),
        )
        .await;
    hands(&h);
    let refused = || Failure {
        status: Some(422),
        why: "lmgw did not make the speech: input is empty (empty_input)".into(),
    };
    let down = || Failure {
        status: Some(503),
        why: "lmgw did not make the speech: the GPU is held (gpu_hold)".into(),
    };
    h.owner("read_aloud", json!({})).await;
    let p0 = h.mouth.next().await;
    let p1 = h.mouth.next().await;
    // P0 refused: skipped with a notice; the reading goes on from P1.
    answer(&p0, Said::Done(Spoken::Failed(refused())));
    let n = notice(&h).await;
    assert_eq!(en(&n), "Skipped a paragraph: lmgw would not speak it.");
    assert!(p1.cancel.is_cancelled(), "handed again after the skip");
    let p1 = h.mouth.next().await;
    assert_eq!(en(&p1), "P1.");
    let p2 = h.mouth.next().await;
    // P1 refused too, right after: a second refusal in a row pauses.
    answer(&p1, Said::Done(Spoken::Failed(refused())));
    let n = notice(&h).await;
    assert_eq!(en(&n), "Reading paused: speech failed.");
    assert!(p2.cancel.is_cancelled());
    let (_, resumed) = h.owner("read_resume", json!({})).await;
    assert!(
        resumed.contains("failing again, that paragraph is skipped"),
        "{resumed}"
    );
    // Resumed at P1: heard now; P2 fails (not a refusal): paused with one notice.
    let p1 = h.mouth.next().await;
    let p2 = h.mouth.next().await;
    assert_eq!(en(&p1), "P1.");
    answer(&p1, Said::Started);
    answer(&p1, Said::Done(Spoken::Heard));
    answer(&p2, Said::Done(Spoken::Failed(down())));
    let n = notice(&h).await;
    assert_eq!(en(&n), "Reading paused: speech failed.");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (_, ctx) = h.owner("browser_context", json!({})).await;
    assert!(
        ctx.contains("paused (its speech failed: lmgw did not make the speech: the GPU is held"),
        "{ctx}"
    );
    // Resumed: P2 fails again, skipped, and the reading goes on (no loop).
    h.owner("read_resume", json!({})).await;
    let p2 = h.mouth.next().await;
    assert_eq!(en(&p2), "P2.");
    let _p3 = h.mouth.next().await;
    answer(&p2, Said::Done(Spoken::Failed(down())));
    let n = notice(&h).await;
    assert_eq!(en(&n), "Skipped a paragraph: its speech failed twice.");
    let p3 = h.mouth.next().await;
    assert_eq!(en(&p3), "P3.");
    // A failure of a later paragraph while an earlier one plays waits for its end.
    let p4 = h.mouth.next().await;
    answer(&p3, Said::Started);
    answer(&p4, Said::Done(Spoken::Failed(down())));
    h.mouth.silent(100).await;
    assert!(!p3.cancel.is_cancelled(), "P3 plays to its end");
    answer(&p3, Said::Done(Spoken::Heard));
    let n = notice(&h).await;
    assert_eq!(en(&n), "Reading paused: speech failed.");
    let (_, ctx) = h.owner("browser_context", json!({})).await;
    assert!(ctx.contains("paragraph 5 of 6"), "{ctx}");

    // Its site became none while it waited: the reading ends with one notice.
    h.owner("read_resume", json!({})).await;
    let p4 = h.mouth.next().await;
    let _ = h.mouth.next().await;
    answer(&p4, Said::Done(Spoken::Dropped(Dropped::Site)));
    let n = notice(&h).await;
    assert_eq!(
        en(&n),
        "Reading stopped: its site is one Kai does not reach now."
    );
    let _ = ff;
}

/// A new reading ends the last one only once its `read.open@1` succeeded: a failed open leaves the
/// reading going; a reading's paragraph site gate follows the site rules.
#[tokio::test]
async fn a_failed_open_keeps_the_reading_going() {
    let h = Host::start().await;
    let ff = h
        .firefox(
            "c0ffee",
            &ops(),
            reading(vec![page("One.", json!({})), page("Two.", json!({}))]),
        )
        .await;
    hands(&h);
    h.owner("read_aloud", json!({})).await;
    let one = h.mouth.next().await;
    let _two = h.mouth.next().await;
    ff.model().twists.insert(
        "read.open@1",
        Twist::Fail("stale_element", "the element is gone"),
    );
    let (error, said) = h.owner("read_aloud", json!({})).await;
    assert!(error, "{said}");
    assert!(!one.cancel.is_cancelled(), "the reading goes on");
    let (_, ctx) = h.owner("browser_context", json!({})).await;
    assert!(ctx.contains("reading aloud: \"Checkout\""), "{ctx}");
    // The site rules change: the gate drops the paragraph.
    h.set(PolicyConfig::default(), {
        let mut c = announcing(&["shop.example"]);
        c.hands_and_eyes = true;
        c
    });
    assert_eq!((one.gate.as_ref().unwrap())(), Err(Dropped::Site));
    // The positive control: a new open succeeds and ends the last reading.
    ff.model().twists.remove("read.open@1");
    h.set(PolicyConfig::default(), {
        let mut c = announcing(&[]);
        c.hands_and_eyes = true;
        c
    });
    let (error, said) = h.owner("read_aloud", json!({})).await;
    assert!(!error, "{said}");
    assert!(one.cancel.is_cancelled(), "the last reading ended");
}
