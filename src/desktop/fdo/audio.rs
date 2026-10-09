//! Audio output volume and mute through `pactl` (PulseAudio's client, served by PipeWire's pulse
//! server): `pactl -f json list sinks` for the outputs, `set-sink-volume` and `set-sink-mute` for
//! the changes. The program is found at the probe and kept as a path; a sink name reaches it only
//! from the listing, never as the caller wrote it (pactl takes no `--`).

use std::path::PathBuf;
use std::time::Duration;

use kai_desktop::caps::audio::{Audio, Device};
use kai_desktop::caps::{Failure, Fut};
use serde::Deserialize;
use tokio::process::Command;

pub struct Pactl {
    pub program: PathBuf,
    pub timeout: Duration,
}

#[derive(Deserialize)]
struct Sink {
    name: String,
    #[serde(default)]
    description: String,
}

impl Pactl {
    /// The default output's name (the probe's question).
    pub async fn default_output(&self) -> Result<String, Failure> {
        let out = self.run(&["get-default-sink"]).await?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    async fn run(&self, args: &[&str]) -> Result<Vec<u8>, Failure> {
        let out = Command::new(&self.program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(self.timeout, out).await {
            Ok(Ok(o)) if o.status.success() => Ok(o.stdout),
            Ok(Ok(o)) => Err(Failure::Failed(format!(
                "pactl failed ({}): {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ))),
            Ok(Err(e)) => Err(Failure::Failed(format!("cannot run pactl: {e}"))),
            Err(_) => Err(Failure::Failed(format!(
                "pactl did not answer within {} s (dbus.timeout_s)",
                self.timeout.as_secs()
            ))),
        }
    }
}

impl Audio for Pactl {
    fn outputs(&self) -> Fut<'_, Vec<Device>> {
        Box::pin(async move {
            let default = self.default_output().await?;
            let sinks = self.run(&["-f", "json", "list", "sinks"]).await?;
            let sinks: Vec<Sink> = serde_json::from_slice(&sinks)
                .map_err(|_| Failure::Failed("pactl listed the outputs oddly".into()))?;
            Ok(sinks
                .into_iter()
                .map(|s| Device {
                    default: s.name == default,
                    description: if s.description.is_empty() {
                        s.name.clone()
                    } else {
                        s.description
                    },
                    id: s.name,
                })
                .collect())
        })
    }

    fn set_volume<'a>(&'a self, id: &'a str, percent: u32) -> Fut<'a, ()> {
        Box::pin(async move {
            let volume = format!("{percent}%");
            self.run(&["set-sink-volume", id, &volume]).await.map(drop)
        })
    }

    fn set_mute<'a>(&'a self, id: &'a str, on: bool) -> Fut<'a, ()> {
        Box::pin(async move {
            self.run(&["set-sink-mute", id, if on { "1" } else { "0" }])
                .await
                .map(drop)
        })
    }
}
