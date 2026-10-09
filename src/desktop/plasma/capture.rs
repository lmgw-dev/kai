//! Capture on Plasma (§11.1): KWin's `org.kde.KWin.ScreenShot2`, a restricted interface KWin grants
//! per `.desktop` file (ruling 16; `packaging/`'s `X-KDE-DBUS-Restricted-Interfaces`).
//!
//! - `see_window` passes the window's id, the window management uuid, to `CaptureWindow` as it is
//!   (WP13 (e): no mapping), or calls `CaptureActiveWindow`; the content only, without the title
//!   bar or the shadow. A covered or minimized window is captured too (minimized: its last frame).
//! - `see_screen` calls `CaptureScreen` with the output's connector name, or `CaptureActiveScreen`,
//!   with the pointer drawn in.
//! - Always at the native resolution (an output's physical pixels).
//! - KWin answers with the image's facts and writes the raw QImage into a pipe Kai hands it; the
//!   whole capture, the call and the read, is bounded by `dbus.timeout_s`.
//! - No grant: `NoAuthorized`, said as §11.1's error.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd};
use std::time::{Duration, Instant};

use kai_desktop::caps::capture::{Capture, Pixels};
use kai_desktop::caps::{Failure, Fut};
use zbus::zvariant::{Fd, OwnedValue, Value};

use crate::host::registry::SessionBus;

pub const KWIN: &str = "org.kde.KWin";
pub const PATH: &str = "/org/kde/KWin/ScreenShot2";
pub const SCREENSHOT: &str = "org.kde.KWin.ScreenShot2";

pub struct KwinCapture {
    pub bus: SessionBus,
    pub timeout: Duration,
}

/// What to capture.
enum Shot<'a> {
    Window(Option<&'a str>),
    Screen(Option<&'a str>),
}

impl Capture for KwinCapture {
    fn window<'a>(&'a self, id: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(self.capture(Shot::Window(id)))
    }

    fn screen<'a>(&'a self, output: Option<&'a str>) -> Fut<'a, Pixels> {
        Box::pin(self.capture(Shot::Screen(output)))
    }
}

/// The error a build without the grant answers (§11.1).
pub fn not_granted() -> Failure {
    Failure::NotGranted {
        what: format!("KWin grants {SCREENSHOT} per .desktop file"),
    }
}

impl KwinCapture {
    async fn capture(&self, shot: Shot<'_>) -> Result<Pixels, Failure> {
        let deadline = Instant::now() + self.timeout;
        let secs = self.timeout.as_secs();
        let bus = self.bus.get().await.map_err(Failure::Failed)?;
        let (reader, writer) = std::io::pipe()
            .map_err(|e| Failure::Failed(format!("no pipe for the capture: {e}")))?;
        // KWin may write before it answers: the read runs from the start.
        let read = tokio::task::spawn_blocking(move || read_until(reader, deadline));
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("native-resolution", Value::from(true));
        opts.insert("include-shadow", Value::from(false));
        let (method, name) = match shot {
            Shot::Window(id) => {
                opts.insert("include-decoration", Value::from(false));
                opts.insert("include-cursor", Value::from(false));
                match id {
                    Some(id) => ("CaptureWindow", Some(id)),
                    None => ("CaptureActiveWindow", None),
                }
            }
            Shot::Screen(output) => {
                opts.insert("include-cursor", Value::from(true));
                match output {
                    Some(o) => ("CaptureScreen", Some(o)),
                    None => ("CaptureActiveScreen", None),
                }
            }
        };
        let reply = {
            let fd = Fd::from(writer.as_fd());
            let call = async {
                match name {
                    Some(n) => {
                        bus.call_method(Some(KWIN), PATH, Some(SCREENSHOT), method, &(n, opts, fd))
                            .await
                    }
                    None => {
                        bus.call_method(Some(KWIN), PATH, Some(SCREENSHOT), method, &(opts, fd))
                            .await
                    }
                }
            };
            tokio::time::timeout(self.timeout, call).await
        };
        // Kai's end of the pipe goes now, so the read ends when KWin closes its own.
        drop(writer);
        let msg = match reply {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => return Err(refusal(e)),
            Err(_) => {
                return Err(Failure::Failed(format!(
                    "KWin did not capture within {secs} s (dbus.timeout_s)"
                )));
            }
        };
        let facts: HashMap<String, OwnedValue> = msg
            .body()
            .deserialize()
            .map_err(|e| Failure::Failed(format!("KWin answered the capture oddly: {e}")))?;
        let data = read
            .await
            .map_err(|e| Failure::Failed(format!("the capture's read failed: {e}")))?
            .map_err(|e| match e {
                ReadError::Late => Failure::Failed(format!(
                    "KWin did not send the capture within {secs} s (dbus.timeout_s)"
                )),
                ReadError::Io(e) => Failure::Failed(format!("the capture's read failed: {e}")),
            })?;
        // The conversion of a 4K frame is tens of milliseconds of CPU: off the async runtime.
        tokio::task::spawn_blocking(move || pixels(&facts, &data))
            .await
            .map_err(|e| Failure::Failed(format!("the capture's conversion failed: {e}")))?
    }
}

/// A refusal of KWin's, in the model's words.
fn refusal(e: zbus::Error) -> Failure {
    match e {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.kde.KWin.ScreenShot2.Error.NoAuthorized" => not_granted(),
            "org.kde.KWin.ScreenShot2.Error.InvalidWindow" => {
                Failure::Refused("that window is gone or cannot be captured".into())
            }
            "org.kde.KWin.ScreenShot2.Error.InvalidScreen" => {
                Failure::Refused("no such output".into())
            }
            "org.kde.KWin.ScreenShot2.Error.Cancelled" => {
                Failure::Failed("KWin cancelled the capture".into())
            }
            "org.freedesktop.DBus.Error.ServiceUnknown" => {
                Failure::Failed(format!("KWin is not running ({KWIN})"))
            }
            other => Failure::Failed(format!("KWin refused the capture ({other})")),
        },
        other => Failure::Failed(format!("KWin refused the capture: {other}")),
    }
}

enum ReadError {
    Late,
    Io(std::io::Error),
}

/// Everything until the writer closes, or the deadline.
fn read_until(mut reader: std::io::PipeReader, deadline: Instant) -> Result<Vec<u8>, ReadError> {
    let fd = reader.as_raw_fd();
    let mut data = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ReadError::Late);
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ReadError::Io(e));
        }
        if n == 0 {
            continue;
        }
        match reader.read(&mut buf) {
            Ok(0) => return Ok(data),
            Ok(k) => data.extend_from_slice(&buf[..k]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
}

/// QImage's formats Kai reads (little-endian memory order), by their `QImage::Format` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// `B G R A` (RGB32 4, ARGB32 5, ARGB32_Premultiplied 6), alpha used or not, premultiplied or
    /// not.
    Bgra { alpha: bool, premultiplied: bool },
    /// `R G B A` (RGBX8888 16, RGBA8888 17, RGBA8888_Premultiplied 18).
    Rgba { alpha: bool, premultiplied: bool },
    /// RGB888 13.
    Rgb,
    /// BGR888 29.
    Bgr,
    /// RGBX64 25, RGBA64 26, RGBA64_Premultiplied 27: four 16-bit channels in native (little)
    /// endian, reduced to 8 bits.
    Rgba64 { alpha: bool, premultiplied: bool },
}

fn layout(format: u32) -> Option<Layout> {
    Some(match format {
        4 => Layout::Bgra {
            alpha: false,
            premultiplied: false,
        },
        5 => Layout::Bgra {
            alpha: true,
            premultiplied: false,
        },
        6 => Layout::Bgra {
            alpha: true,
            premultiplied: true,
        },
        16 => Layout::Rgba {
            alpha: false,
            premultiplied: false,
        },
        17 => Layout::Rgba {
            alpha: true,
            premultiplied: false,
        },
        18 => Layout::Rgba {
            alpha: true,
            premultiplied: true,
        },
        13 => Layout::Rgb,
        29 => Layout::Bgr,
        25 => Layout::Rgba64 {
            alpha: false,
            premultiplied: false,
        },
        26 => Layout::Rgba64 {
            alpha: true,
            premultiplied: false,
        },
        27 => Layout::Rgba64 {
            alpha: true,
            premultiplied: true,
        },
        _ => return None,
    })
}

/// `QImage::Format`'s name for the formats Kai refuses, so the error says which one it was.
fn format_name(format: u32) -> &'static str {
    match format {
        1 => "Mono",
        2 => "MonoLSB",
        3 => "Indexed8",
        7 => "RGB16",
        19 => "BGR30",
        20 => "A2BGR30_Premultiplied",
        21 => "RGB30",
        22 => "A2RGB30_Premultiplied",
        23 => "Alpha8",
        24 => "Grayscale8",
        28 => "Grayscale16",
        30 => "RGBX16FPx4",
        31 => "RGBA16FPx4",
        32 => "RGBA16FPx4_Premultiplied",
        33 => "RGBX32FPx4",
        34 => "RGBA32FPx4",
        35 => "RGBA32FPx4_Premultiplied",
        36 => "CMYK8888",
        _ => "unknown",
    }
}

fn number(facts: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    let v = facts.get(key)?;
    u32::try_from(v)
        .ok()
        .or_else(|| i32::try_from(v).ok().and_then(|n| u32::try_from(n).ok()))
}

/// The raw QImage KWin wrote, as straight RGBA.
pub fn pixels(facts: &HashMap<String, OwnedValue>, data: &[u8]) -> Result<Pixels, Failure> {
    if let Some(kind) = facts.get("type").and_then(|v| <&str>::try_from(v).ok())
        && kind != "raw"
    {
        return Err(Failure::Failed(format!(
            "KWin sent the capture as {kind:?}, not raw pixels"
        )));
    }
    let (Some(width), Some(height), Some(stride), Some(format)) = (
        number(facts, "width"),
        number(facts, "height"),
        number(facts, "stride"),
        number(facts, "format"),
    ) else {
        return Err(Failure::Failed(
            "KWin's capture lacks its width, height, stride or format".into(),
        ));
    };
    let layout = layout(format).ok_or_else(|| {
        Failure::Failed(format!(
            "KWin sent the capture in QImage format {format} ({}), which Kai does not read (it \
             reads the 8-bit RGB and RGBA formats and the 16-bit RGBA64 ones; 30-bit and \
             floating-point formats, as an HDR output may send, are not read)",
            format_name(format)
        ))
    })?;
    let bpp = match layout {
        Layout::Bgra { .. } | Layout::Rgba { .. } => 4,
        Layout::Rgb | Layout::Bgr => 3,
        Layout::Rgba64 { .. } => 8,
    };
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    if w == 0 || h == 0 || stride < w * bpp || data.len() < stride * (h - 1) + w * bpp {
        return Err(Failure::Failed(format!(
            "KWin's capture is {} bytes, short of {width}x{height} pixels at stride {stride}",
            data.len()
        )));
    }
    // One allocation of the result's size, filled row by row.
    let mut rgba = vec![0u8; w * h * 4];
    for (y, out_row) in rgba.chunks_exact_mut(w * 4).enumerate() {
        let row = &data[y * stride..y * stride + w * bpp];
        for (p, o) in row
            .chunks_exact(bpp)
            .zip(out_row.as_chunks_mut::<4>().0.iter_mut())
        {
            let (r, g, b, a, pre) = match layout {
                Layout::Bgra {
                    alpha,
                    premultiplied,
                } => (
                    p[2],
                    p[1],
                    p[0],
                    if alpha { p[3] } else { 255 },
                    premultiplied,
                ),
                Layout::Rgba {
                    alpha,
                    premultiplied,
                } => (
                    p[0],
                    p[1],
                    p[2],
                    if alpha { p[3] } else { 255 },
                    premultiplied,
                ),
                Layout::Rgb => (p[0], p[1], p[2], 255, false),
                Layout::Bgr => (p[2], p[1], p[0], 255, false),
                Layout::Rgba64 {
                    alpha,
                    premultiplied,
                } => {
                    let c = |i: usize| u32::from(u16::from_le_bytes([p[2 * i], p[2 * i + 1]]));
                    let a16 = if alpha { c(3) } else { 65535 };
                    let un16 = |v: u32| {
                        if !premultiplied || a16 == 65535 {
                            v
                        } else {
                            (v * 65535 + a16 / 2)
                                .checked_div(a16)
                                .unwrap_or(0)
                                .min(65535)
                        }
                    };
                    let to8 = |v: u32| ((v * 255 + 32767) / 65535) as u8;
                    o.copy_from_slice(&[
                        to8(un16(c(0))),
                        to8(un16(c(1))),
                        to8(un16(c(2))),
                        to8(a16),
                    ]);
                    continue;
                }
            };
            let un = |c: u8| {
                if !pre || a == 255 {
                    c
                } else if a == 0 {
                    0
                } else {
                    ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            };
            o.copy_from_slice(&[un(r), un(g), un(b), a]);
        }
    }
    Ok(Pixels {
        width,
        height,
        rgba,
    })
}

/// The probe's look at ScreenShot2: KWin serves the interface (introspection needs no grant; the
/// grant itself shows only at the first capture).
pub async fn serves(bus: &zbus::Connection, timeout: Duration) -> Result<bool, String> {
    let xml: String = crate::desktop::call(
        bus,
        KWIN,
        PATH,
        "org.freedesktop.DBus.Introspectable",
        "Introspect",
        &(),
        timeout,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(xml.contains(&format!("\"{SCREENSHOT}\"")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(w: u32, h: u32, stride: u32, format: u32) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("width".to_owned(), OwnedValue::from(w)),
            ("height".to_owned(), OwnedValue::from(h)),
            ("stride".to_owned(), OwnedValue::from(stride)),
            ("format".to_owned(), OwnedValue::from(format)),
            (
                "type".to_owned(),
                OwnedValue::try_from(Value::from("raw")).unwrap(),
            ),
        ])
    }

    /// QImage's layouts on little endian, the stride's padding skipped, premultiplied alpha undone.
    #[test]
    fn kwins_raw_formats_read_as_straight_rgba() {
        // 2x1 at stride 12 (4 bytes of padding): blue then half-transparent red, premultiplied.
        let data = [255, 0, 0, 255, 0, 0, 128, 128, 9, 9, 9, 9];
        let p = pixels(&facts(2, 1, 12, 6), &data).unwrap();
        assert_eq!((p.width, p.height), (2, 1));
        assert_eq!(p.rgba, [0, 0, 255, 255, 255, 0, 0, 128]);
        // RGB32 ignores the X byte.
        let p = pixels(&facts(1, 1, 4, 4), &[1, 2, 3, 0]).unwrap();
        assert_eq!(p.rgba, [3, 2, 1, 255]);
        let p = pixels(&facts(1, 1, 4, 17), &[1, 2, 3, 4]).unwrap();
        assert_eq!(p.rgba, [1, 2, 3, 4]);
        let p = pixels(&facts(2, 1, 6, 13), &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(p.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        let p = pixels(&facts(1, 1, 3, 29), &[1, 2, 3]).unwrap();
        assert_eq!(p.rgba, [3, 2, 1, 255]);
        // RGBA64 (HDR-capable outputs): 16 bits per channel reduced to 8, premultiplied undone.
        let px = [0xff, 0xff, 0, 0, 0, 0, 0xff, 0xff];
        let p = pixels(&facts(1, 1, 8, 26), &px).unwrap();
        assert_eq!(p.rgba, [255, 0, 0, 255]);
        let half = [0x80, 0x80, 0, 0, 0, 0, 0x80, 0x80];
        let p = pixels(&facts(1, 1, 8, 27), &half).unwrap();
        assert_eq!(p.rgba, [255, 0, 0, 128]);
        let e = pixels(&facts(1, 1, 8, 20), &[0; 8]).unwrap_err();
        assert!(
            e.to_string()
                .starts_with("KWin sent the capture in QImage format 20 (A2BGR30_Premultiplied)"),
            "{e}"
        );
        let e = pixels(&facts(4, 4, 16, 5), &[0; 20]).unwrap_err();
        assert!(e.to_string().contains("short of 4x4"), "{e}");
    }

    #[test]
    fn kwins_refusals_read_as_the_model_should_hear_them() {
        let err = |name: &str| {
            refusal(zbus::Error::MethodError(
                zbus::names::OwnedErrorName::try_from(name).unwrap(),
                Some("details".into()),
                zbus::message::Message::method_call("/", "x")
                    .unwrap()
                    .build(&())
                    .unwrap(),
            ))
        };
        assert_eq!(
            err("org.kde.KWin.ScreenShot2.Error.NoAuthorized").to_string(),
            "needs the installed Kai: KWin grants org.kde.KWin.ScreenShot2 per .desktop file"
        );
        assert!(matches!(
            err("org.kde.KWin.ScreenShot2.Error.InvalidWindow"),
            Failure::Refused(_)
        ));
    }
}
