//! The Desktop page's remote-control permission (WP16c, decided by the owner 2026-10-09): Kai keeps
//! the RemoteDesktop portal's restore token (`crate::desktop::fdo::input::TokenFile`), and the
//! page shows whether one is stored and forgets it on the owner's confirmed click, as
//! `kai --forget-remote-control` does. Revoking Kai in System Settings does not end the permission
//! (portal-kde restores from the token all the same), so forgetting it here is how it ends.
//!
//! `GET /api/remote-control` answers [`RemoteControl`] with `forgot: null`; `POST
//! /api/remote-control/forget` removes the file and answers with `forgot` saying whether one was
//! stored. The core runs both (`Command::WindowRemoteControl`), on its own state directory.

use serde::Serialize;

use crate::desktop::fdo::input::TokenFile;

/// What the page shows: whether a token is stored now, and after a forget whether one was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteControl {
    pub stored: bool,
    /// `None` for a look; after a forget, whether a token was stored.
    pub forgot: Option<bool>,
}

/// Looks at the token file, or forgets it (`forget`). A file that cannot be removed is the error,
/// in words naming it.
pub fn answer(file: &TokenFile, forget: bool) -> Result<RemoteControl, String> {
    let forgot = if forget {
        let was = file.forget().map_err(|e| {
            format!(
                "Kai cannot remove the remote-control permission ({}): {e}",
                file.0.display()
            )
        })?;
        tracing::info!(
            stored = was,
            "the owner forgot the remote-control permission from the settings window"
        );
        Some(was)
    } else {
        None
    };
    Ok(RemoteControl {
        stored: file.load().is_some(),
        forgot,
    })
}
