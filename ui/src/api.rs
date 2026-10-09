//! Kai's own routes of the scheme handler (`src/settings/window.rs`), as the page calls them (the
//! profile editor's requests to lmgw are the kit's own, under `LMGW_BASE`).

use gloo_net::http::Request;

use crate::wire::{self, Conversation, Refused, RemoteControl, ToolInfo};

/// A failed call's text for the owner: the refusal's own sentence, else what went wrong.
fn failure(status: u16, body: &str) -> String {
    match serde_json::from_str::<Refused>(body) {
        Ok(r) => r.refused,
        Err(_) if body.trim().is_empty() => format!("Kai answered {status}"),
        Err(_) => format!("Kai answered {status}: {}", body.trim()),
    }
}

/// `POST /api/apply`: one operation. A refusal is the `Err` with the loader's sentence.
pub async fn apply(request: &wire::Request) -> Result<wire::Response, String> {
    let body = serde_json::to_string(request).map_err(|e| e.to_string())?;
    // text/plain: a simple request, so nothing in the page's origin needs a preflight.
    let response = Request::post("/api/apply")
        .header("Content-Type", "text/plain")
        .body(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| format!("Kai did not answer: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status == 200 {
        serde_json::from_str(&text).map_err(|e| format!("an answer the page does not read: {e}"))
    } else {
        Err(failure(status, &text))
    }
}

/// `GET /api/tools`: the tools the contract has.
pub async fn tools() -> Result<Vec<ToolInfo>, String> {
    let response = Request::get("/api/tools")
        .send()
        .await
        .map_err(|e| format!("Kai did not answer: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status == 200 {
        serde_json::from_str(&text).map_err(|e| format!("an answer the page does not read: {e}"))
    } else {
        Err(failure(status, &text))
    }
}

/// `GET /api/conversation`: Kai's conversation as the core knows it.
pub async fn conversation() -> Result<Conversation, String> {
    let response = Request::get("/api/conversation")
        .send()
        .await
        .map_err(|e| format!("Kai did not answer: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status == 200 {
        serde_json::from_str(&text).map_err(|e| format!("an answer the page does not read: {e}"))
    } else {
        Err(failure(status, &text))
    }
}

/// `GET /api/remote-control`, or (`forget`) `POST /api/remote-control/forget`: the RemoteDesktop
/// portal's stored permission, looked at or forgotten.
pub async fn remote_control(forget: bool) -> Result<RemoteControl, String> {
    let request = if forget {
        // text/plain and no body: a simple request, as `apply`'s.
        Request::post("/api/remote-control/forget")
            .header("Content-Type", "text/plain")
            .body("")
            .map_err(|e| e.to_string())?
    } else {
        Request::get("/api/remote-control")
            .build()
            .map_err(|e| e.to_string())?
    };
    let response = request
        .send()
        .await
        .map_err(|e| format!("Kai did not answer: {e}"))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status == 200 {
        serde_json::from_str(&text).map_err(|e| format!("an answer the page does not read: {e}"))
    } else {
        Err(failure(status, &text))
    }
}
