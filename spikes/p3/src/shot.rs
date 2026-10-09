//! `org.kde.KWin.ScreenShot2` from this binary, so that KWin's restricted D-Bus check sees this
//! executable (it reads the caller's `/proc/<pid>/exe` and looks for a `.desktop` naming it).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};

use zbus::zvariant::{Fd, OwnedValue, Value};

pub enum Target {
    Window(String),
    Active,
    Workspace,
}

pub fn shot(target: Target, out: Option<&str>, decoration: bool) -> i32 {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let conn = zbus::Connection::session().await.expect("session bus");
        let (reader, writer) = std::io::pipe().expect("pipe");
        let reader = std::thread::spawn(move || {
            let mut r = reader;
            let mut v = Vec::new();
            r.read_to_end(&mut v).map(|_| v)
        });
        let wfd: OwnedFd = writer.into();
        let mut opts: HashMap<&str, Value> = HashMap::new();
        opts.insert("include-decoration", Value::from(decoration));
        opts.insert("include-shadow", Value::from(false));
        opts.insert("native-resolution", Value::from(true));
        let fd = Fd::from(wfd.as_fd());
        let t0 = std::time::Instant::now();
        let iface = Some("org.kde.KWin.ScreenShot2");
        let dest = Some("org.kde.KWin");
        let path = "/org/kde/KWin/ScreenShot2";
        let reply = match &target {
            Target::Window(h) => conn.call_method(dest, path, iface, "CaptureWindow", &(h.as_str(), opts, fd)).await,
            Target::Active => conn.call_method(dest, path, iface, "CaptureActiveWindow", &(opts, fd)).await,
            Target::Workspace => conn.call_method(dest, path, iface, "CaptureWorkspace", &(opts, fd)).await,
        };
        drop(wfd);
        let msg = match reply {
            Ok(m) => m,
            Err(e) => {
                println!("shot: ERROR {e}");
                return 2;
            }
        };
        let meta: HashMap<String, OwnedValue> = msg.body().deserialize().expect("a{sv}");
        let data = reader.join().unwrap().expect("read pipe");
        let ms = t0.elapsed().as_millis();
        let get = |k: &str| meta.get(k).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
        let (w, h, stride, format) = (get("width"), get("height"), get("stride"), get("format"));
        let mut keys: Vec<_> = meta.keys().cloned().collect();
        keys.sort();
        println!("shot: ok in {ms} ms: {w}x{h} stride {stride} QImage format {format}, {} bytes, keys {keys:?}", data.len());
        if w == 0 || h == 0 || data.len() < (stride * h) as usize {
            println!("shot: no usable pixels");
            return 3;
        }
        // QImage ARGB32 / RGB32 (formats 4, 5, 6) on little endian: B, G, R, A in memory.
        let px = |x: u32, y: u32| {
            let o = (y * stride + x * 4) as usize;
            (data[o + 2], data[o + 1], data[o])
        };
        let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
        for y in 0..h {
            for x in 0..w {
                let p = px(x, y);
                r += p.0 as u64;
                g += p.1 as u64;
                b += p.2 as u64;
            }
        }
        let n = (w * h) as u64;
        println!("shot: mean rgb ({},{},{}), pixel at 3/4 height centre {:?}", r / n, g / n, b / n, px(w / 2, h * 3 / 4));
        if let Some(path) = out {
            let mut f = std::fs::File::create(path).expect("out");
            write!(f, "P6\n{w} {h}\n255\n").unwrap();
            let mut row = Vec::with_capacity((w * 3) as usize);
            for y in 0..h {
                row.clear();
                for x in 0..w {
                    let p = px(x, y);
                    row.extend_from_slice(&[p.0, p.1, p.2]);
                }
                f.write_all(&row).unwrap();
            }
            println!("shot: wrote {path}");
        }
        0
    })
}
