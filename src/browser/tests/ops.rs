//! A linked extension's traffic: operations and their results, errors, cancellation, a dropped call
//! and the link's end; the extension's events and the network gate's questions; a sidebar request
//! with an id too long to answer; frames over Firefox's limit in pieces, and the extension's split
//! frames joined.

use base64::Engine as _;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::browser::frame::TO_FIREFOX_MAX;
use crate::browser::links::{Incoming, OpError};

/// The subscriber's next frame.
async fn next(sub: &mut tokio::sync::mpsc::UnboundedReceiver<Incoming>) -> Incoming {
    tokio::time::timeout(WAIT, sub.recv())
        .await
        .unwrap()
        .unwrap()
}

/// The one link's id.
fn only(rig: &Rig) -> u64 {
    rig.links.list()[0].id
}

#[tokio::test]
async fn an_operation_waits_for_its_result() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&["tabs.list@1"]).await;
    rig.links_are(1).await;
    let link = only(&rig);
    let cancel = CancellationToken::new();

    let call = {
        let links = rig.links.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            links
                .op(link, "tabs.list@1", json!({"window": 2}), &cancel)
                .await
        })
    };
    let op = c.recv().await.unwrap();
    assert_eq!(op["t"], "op");
    assert_eq!(op["op"], "tabs.list@1");
    assert_eq!(op["args"], json!({"window": 2}));
    let id = op["id"].clone();
    // A result for no waiting call is dropped; the right one answers.
    c.send(json!({"t": "op_result", "id": 999_999, "ok": "stray"}))
        .await;
    c.send(json!({"t": "op_result", "id": id, "ok": [{"id": 4, "title": "Mail"}]}))
        .await;
    assert_eq!(call.await.unwrap(), Ok(json!([{"id": 4, "title": "Mail"}])));

    // The extension's error, with its code.
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move { links.op(link, "tabs.list@1", json!({}), &cancel).await })
    };
    let op = c.recv().await.unwrap();
    assert_ne!(op["id"], id, "a new id for each call");
    c.send(json!({"t": "op_result", "id": op["id"],
                  "error": {"code": "no_tab", "message": "no tab 9"}}))
        .await;
    assert_eq!(
        call.await.unwrap(),
        Err(OpError::failed("no_tab", "no tab 9"))
    );

    // What the extension does not serve, and a link that is not there, go nowhere.
    let cancel = CancellationToken::new();
    assert_eq!(
        rig.links
            .op(link, "page.outline@1", json!({}), &cancel)
            .await,
        Err(OpError::NotServed {
            op: "page.outline@1".into()
        })
    );
    assert_eq!(
        rig.links
            .op(link + 1, "tabs.list@1", json!({}), &cancel)
            .await,
        Err(OpError::NoLink(link + 1))
    );
}

#[tokio::test]
async fn a_cancelled_operation_tells_the_extension_and_a_closed_link_fails_its_calls() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&["page.text@1"]).await;
    rig.links_are(1).await;
    let link = only(&rig);

    let cancel = CancellationToken::new();
    let call = {
        let (links, cancel) = (rig.links.clone(), cancel.clone());
        tokio::spawn(async move { links.op(link, "page.text@1", json!({}), &cancel).await })
    };
    let op = c.recv().await.unwrap();
    assert_eq!(rig.links.waiting(link), 1);
    cancel.cancel();
    assert_eq!(call.await.unwrap(), Err(OpError::Cancelled));
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "cancel", "id": op["id"]})
    );
    assert_eq!(rig.links.waiting(link), 0);
    // Its late result is dropped; the link goes on.
    c.send(json!({"t": "op_result", "id": op["id"], "ok": "late"}))
        .await;

    let cancel = CancellationToken::new();
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move { links.op(link, "page.text@1", json!({}), &cancel).await })
    };
    assert_eq!(c.recv().await.unwrap()["t"], "op");
    drop(c);
    let r = call.await.unwrap();
    assert_eq!(r, Err(OpError::Closed));
    assert_eq!(
        r.unwrap_err().to_string(),
        "Firefox closed while the call ran"
    );
    rig.links_are(0).await;
}

/// A call whose future is dropped while it waits (its task aborted, lmgw's caller gone) is
/// withdrawn as a cancelled one is: the extension is sent `cancel` and nothing is left waiting.
#[tokio::test]
async fn a_dropped_call_is_withdrawn() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&["page.text@1"]).await;
    rig.links_are(1).await;
    let link = only(&rig);
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            links.op(link, "page.text@1", json!({}), &cancel).await
        })
    };
    let op = c.recv().await.unwrap();
    assert_eq!(op["t"], "op");
    assert_eq!(rig.links.waiting(link), 1);
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "cancel", "id": op["id"]})
    );
    assert_eq!(rig.links.waiting(link), 0);

    // An answered call leaves nothing behind and sends no cancel.
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            links.op(link, "page.text@1", json!({}), &cancel).await
        })
    };
    let op = c.recv().await.unwrap();
    c.send(json!({"t": "op_result", "id": op["id"], "ok": "text"}))
        .await;
    assert_eq!(call.await.unwrap(), Ok(json!("text")));
    assert_eq!(rig.links.waiting(link), 0);
    c.send(json!({"t": "req", "id": "after", "method": "GET", "path": "/"}))
        .await;
    assert_eq!(
        c.recv().await.unwrap()["id"],
        "after",
        "no cancel came first"
    );
}

#[tokio::test]
async fn events_and_questions_reach_the_subscriber() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&[]).await;
    rig.links_are(1).await;
    let link = only(&rig);

    // Nobody subscribes yet: the gate's question is answered no at once.
    c.send(json!({"t": "ask", "id": "q1", "tab": 3, "method": "POST",
                  "url": "https://shop.example/cart", "body": "x=1"}))
        .await;
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "answer", "id": "q1", "allow": false})
    );

    let mut sub = rig.links.subscribe();
    c.send(json!({"t": "event", "kind": "loaded", "title": "Checkout"}))
        .await;
    c.send(json!({"t": "ask", "id": 7, "tab": 3, "method": "POST",
                  "url": "https://shop.example/cart", "body": "x=1"}))
        .await;
    let mut fields = serde_json::Map::new();
    fields.insert("title".into(), "Checkout".into());
    assert_eq!(
        next(&mut sub).await,
        Incoming::Event {
            link,
            kind: "loaded".into(),
            fields
        }
    );
    let Incoming::Ask {
        link: l,
        id,
        fields,
    } = next(&mut sub).await
    else {
        panic!("not an ask");
    };
    assert_eq!((l, &id), (link, &json!(7)));
    assert_eq!(fields["url"], "https://shop.example/cart");
    assert_eq!(fields["method"], "POST");
    rig.links.answer(link, id, true).unwrap();
    assert_eq!(
        c.recv().await.unwrap(),
        json!({"t": "answer", "id": 7, "allow": true})
    );
}

/// A `req` whose id is so long that no piece of its answer fits beside it gets an `error` frame
/// naming the sizes, and the link goes on (such an id once ended the connection's task and left
/// its link behind).
#[tokio::test]
async fn a_req_whose_id_leaves_no_room_gets_an_error() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&[]).await;
    rig.links_are(1).await;
    c.send(json!({"t": "req", "id": "x".repeat(TO_FIREFOX_MAX), "method": "GET", "path": "/"}))
        .await;
    let e = c.recv().await.unwrap();
    assert_eq!(e["t"], "error");
    assert_eq!(e["code"], "id_too_long");
    assert_eq!(e["limit"], TO_FIREFOX_MAX);
    assert!(
        e["size"].as_u64().unwrap() > TO_FIREFOX_MAX as u64,
        "{}",
        e["size"]
    );
    let message = e["message"].as_str().unwrap();
    assert!(message.contains("leaves no room"), "{message}");
    c.send(json!({"t": "req", "id": 2, "method": "GET", "path": "/v1/models"}))
        .await;
    assert_eq!(c.recv().await.unwrap()["id"], 2);
    assert_eq!(rig.links.list().len(), 1);
    drop(c);
    rig.links_are(0).await;
}

/// An `op` whose args are longer than Firefox takes goes as `chunk` frames, each within the limit,
/// which join to the frame.
#[tokio::test]
async fn a_long_operation_goes_in_pieces_within_firefoxs_limit() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&["element.file@1"]).await;
    rig.links_are(1).await;
    let link = only(&rig);
    let file: Vec<u8> = (0..(3 * 1024 * 1024 + 17))
        .map(|i| (i % 251) as u8)
        .collect();
    let data = base64::engine::general_purpose::STANDARD.encode(&file);
    let call = {
        let (links, data) = (rig.links.clone(), data.clone());
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            links
                .op(
                    link,
                    "element.file@1",
                    json!({"ref": 214, "data": data}),
                    &cancel,
                )
                .await
        })
    };
    let mut joined = String::new();
    let mut seq = 0;
    loop {
        let body = c.recv_raw().await.unwrap();
        assert!(
            body.len() <= TO_FIREFOX_MAX,
            "a frame of {} bytes",
            body.len()
        );
        let f: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(f["split"], 1, "the link's first split frame");
        assert_eq!(f["seq"], seq);
        if f["t"] == "end" {
            break;
        }
        assert_eq!(f["t"], "chunk");
        joined.push_str(f["data"].as_str().unwrap());
        seq += 1;
    }
    assert!(seq >= 4, "{seq} pieces");
    let op: Value = serde_json::from_str(&joined).unwrap();
    assert_eq!(op["t"], "op");
    assert_eq!(op["args"]["ref"], 214);
    assert_eq!(op["args"]["data"], data);
    c.send(json!({"t": "op_result", "id": op["id"], "ok": {"files": 1}}))
        .await;
    assert_eq!(call.await.unwrap(), Ok(json!({"files": 1})));
}

/// The extension splits its long frames as Kai does (Firefox writes one large message slowly):
/// a 3 MiB `op_result` in 1 MiB pieces is joined and answers its call; a split that goes wrong is
/// an `error` frame and the link goes on; one the link leaves unfinished goes with it.
#[tokio::test]
async fn the_extensions_split_frames_are_joined() {
    let rig = Rig::start(in_firefox).await;
    let mut c = rig.link(&["page.screenshot@1"]).await;
    rig.links_are(1).await;
    let link = only(&rig);
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            links
                .op(link, "page.screenshot@1", json!({}), &cancel)
                .await
        })
    };
    let op = c.recv().await.unwrap();
    let image = base64::engine::general_purpose::STANDARD.encode(
        (0..(2 * 1024 * 1024 + 5))
            .map(|i| (i % 253) as u8)
            .collect::<Vec<u8>>(),
    );
    let result = json!({"t": "op_result", "id": op["id"], "ok": {"png": image}});
    let pieces = crate::browser::chunk::fit(result.to_string(), || 1, TO_FIREFOX_MAX).unwrap();
    assert!(pieces.len() >= 4, "{} pieces", pieces.len());
    for p in &pieces {
        frame::write(&mut c.wr, p.as_bytes()).await.unwrap();
    }
    assert_eq!(call.await.unwrap(), Ok(json!({"png": image})));

    // A piece out of order: an error naming the split, and the link goes on.
    c.send(json!({"t": "chunk", "split": 2, "seq": 0, "data": "{\"t\":"}))
        .await;
    c.send(json!({"t": "chunk", "split": 2, "seq": 5, "data": "\"event\"}"}))
        .await;
    let e = c.recv().await.unwrap();
    assert_eq!(e["t"], "error");
    assert_eq!(e["code"], "bad_split");
    assert_eq!(e["split"], 2);
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("piece 5 came where piece 1 was due"),
        "{e}"
    );
    c.send(json!({"t": "end", "split": 2, "seq": 2})).await;
    c.send(json!({"t": "req", "id": "next", "method": "GET", "path": "/"}))
        .await;
    assert_eq!(
        c.recv().await.unwrap()["id"],
        "next",
        "the dropped split's end is read past"
    );

    // A split left open when Firefox closes goes with the link.
    c.send(json!({"t": "chunk", "split": 3, "seq": 0, "data": "{"}))
        .await;
    drop(c);
    rig.links_are(0).await;
}

/// An operation Kai sent in pieces that the extension cannot join (its `error` frame
/// `bad_split` names the split and sends no `op_result`) fails at once with the extension's words;
/// another `error` frame is logged and the link goes on. The links' count moves with the links.
#[tokio::test]
async fn a_split_the_extension_cannot_join_fails_its_operation() {
    let rig = Rig::start(in_firefox).await;
    let mut linked = rig.links.linked();
    assert_eq!(*linked.borrow_and_update(), 0);
    let mut c = rig.link(&["page.css@1", "tabs.list@1"]).await;
    rig.links_are(1).await;
    assert_eq!(*linked.borrow_and_update(), 1);
    let link = only(&rig);
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let css = "a{}".repeat(TO_FIREFOX_MAX / 2);
            links
                .op(link, "page.css@1", json!({"css": css}), &cancel)
                .await
        })
    };
    let mut split = None;
    loop {
        let piece = c.recv().await.unwrap();
        assert_eq!(
            piece["t"].as_str().map(|t| t == "chunk" || t == "end"),
            Some(true)
        );
        split = split.or(piece["split"].as_u64());
        if piece["t"] == "end" {
            break;
        }
    }
    // An error of another kind first: logged, nothing fails.
    c.send(json!({"t": "error", "code": "frame_too_large", "message": "x"}))
        .await;
    c.send(
        json!({"t": "error", "code": "bad_split", "split": split.unwrap(),
                  "message": "piece 1 never came"}),
    )
    .await;
    assert_eq!(
        tokio::time::timeout(WAIT, call).await.unwrap().unwrap(),
        Err(OpError::failed("bad_split", "piece 1 never came"))
    );
    assert_eq!(rig.links.waiting(link), 0);
    // The link goes on.
    let call = {
        let links = rig.links.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            links.op(link, "tabs.list@1", json!({}), &cancel).await
        })
    };
    let op = c.recv().await.unwrap();
    c.send(json!({"t": "op_result", "id": op["id"], "ok": {"tabs": []}}))
        .await;
    assert_eq!(call.await.unwrap(), Ok(json!({"tabs": []})));
    drop(c);
    rig.links_are(0).await;
    tokio::time::timeout(WAIT, linked.wait_for(|n| *n == 0))
        .await
        .unwrap()
        .unwrap();
}
