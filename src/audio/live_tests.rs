//! Live checks against a private PipeWire instance (`scripts/live-check-audio.sh` sets it up and
//! runs these). Ignored by default; each refuses to run unless `KAI_LIVE_PRIVATE=1` and
//! `PIPEWIRE_RUNTIME_DIR` points away from the session's runtime directory, so they never touch
//! the owner's audio.
//!
//! Env: `KAI_LIVE_CLIP` a 24 kHz mono PCM16 WAV; `KAI_LIVE_OUT` the directory for the reports.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use tokio::sync::mpsc;

use super::playback::monotonic_ns;
use super::*;

pub(crate) const SINK: &str = "kai-test-sink";
const SINK2: &str = "kai-test-sink-2";
const FOLLOWER: &str = "kai-test-sink-follower";
pub(crate) const MIC: &str = "kai-test-mic";
/// The loopback's sink: what plays into it comes out of `MIC`.
pub(crate) const MIC_IN: &str = "kai-test-mic-in";

pub(crate) fn private_or_skip() -> bool {
    let private = std::env::var("KAI_LIVE_PRIVATE").as_deref() == Ok("1");
    let dir = std::env::var("PIPEWIRE_RUNTIME_DIR").unwrap_or_default();
    let ok = private && !dir.is_empty() && !dir.starts_with("/run/user/");
    if !ok {
        eprintln!("not in a private PipeWire instance: skipped (scripts/live-check-audio.sh)");
    }
    ok
}

pub(crate) fn out_dir() -> PathBuf {
    let d =
        PathBuf::from(std::env::var("KAI_LIVE_OUT").unwrap_or_else(|_| "target/live-audio".into()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 24 kHz mono PCM16 WAV's samples.
pub(crate) fn read_wav(path: &str) -> Vec<i16> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(&b[8..12], b"WAVE");
    let mut i = 12;
    let mut fmt_ok = false;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = &b[i + 8..(i + 8 + len).min(b.len())];
        if id == b"fmt " {
            let format = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            assert_eq!((format, channels, rate, bits), (1, 1, RATE, 16), "{path}");
            fmt_ok = true;
        } else if id == b"data" {
            assert!(fmt_ok);
            return body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c))
                .collect();
        }
        i += 8 + len + (len & 1);
    }
    panic!("{path}: no data chunk");
}

pub(crate) fn settings(dir: &std::path::Path) -> AudioSettings {
    AudioSettings {
        echo_mode: EchoMode::Device,
        input: None,
        output: None,
        capture_ring_ms: 2000,
        playback_ring_ms: 2000,
        retry: Duration::from_secs(2),
        aec_ready_timeout: Duration::from_secs(4),
        aec_restore_timeout: Duration::from_secs(4),
        aec_file: dir.join("state").join("aec.json"),
        runtime_dir: dir.join("runtime"),
    }
}

/// Waits for an event that satisfies `f`, at most `secs`.
async fn wait_for(
    rx: &mut mpsc::UnboundedReceiver<AudioEvent>,
    secs: u64,
    what: &str,
    mut f: impl FnMut(&AudioEvent) -> bool,
) -> AudioEvent {
    let t0 = Instant::now();
    loop {
        let left = Duration::from_secs(secs).saturating_sub(t0.elapsed());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(e)) => {
                eprintln!("  event: {e:?}");
                if f(&e) {
                    return e;
                }
            }
            _ => panic!("no event '{what}' within {secs} s"),
        }
    }
}

/// The sink's monitor at 48 kHz, F32 mono, each buffer with its cycle's time: the ground truth
/// of when the sink consumed which samples.
pub(crate) struct Monitor {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Vec<(i64, Vec<f32>)>>,
}

impl Monitor {
    pub(crate) fn start(sink: &str) -> Monitor {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let sink = sink.to_owned();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            pw::init();
            let ml = pw::main_loop::MainLoopRc::new(None).unwrap();
            let ctx = pw::context::ContextRc::new(&ml, None).unwrap();
            let core = ctx.connect_rc(None).unwrap();
            let mut props = pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::NODE_NAME => "kai-live-monitor",
                *pw::keys::TARGET_OBJECT => sink.as_str(),
                "stream.capture.sink" => "true",
            };
            if let Ok(v) = std::env::var("KAI_LIVE_MONITOR_ASYNC") {
                props.insert("node.async", v);
            }
            let stream = pw::stream::StreamRc::new(core, "kai-live-monitor", props).unwrap();
            // The callback runs in the data thread: lock-free hand-off of (cycle time, frames).
            let (mut meta_tx, mut meta_rx) = rtrb::RingBuffer::<(i64, usize)>::new(4096);
            let (mut pcm_tx, mut pcm_rx) = rtrb::RingBuffer::<f32>::new(48_000 * 30);
            let _l = stream
                .add_local_listener_with_user_data(())
                .process(move |s, _| {
                    let now = s.time().map(|t| t.now()).unwrap_or(0);
                    if let Some(mut b) = s.dequeue_buffer() {
                        let d = &mut b.datas_mut()[0];
                        let (off, size) = (d.chunk().offset() as usize, d.chunk().size() as usize);
                        if let Some(bytes) = d.data() {
                            let bytes = &bytes[off..off + size];
                            let n = bytes.len() / 4;
                            let mut pushed = 0;
                            for c in bytes.as_chunks::<4>().0 {
                                if pcm_tx.push(f32::from_le_bytes(*c)).is_ok() {
                                    pushed += 1;
                                }
                            }
                            assert_eq!(pushed, n, "the monitor ring is full");
                            let _ = meta_tx.push((now, n));
                        }
                    }
                })
                .register()
                .unwrap();
            let mut info = spa::param::audio::AudioInfoRaw::new();
            info.set_format(spa::param::audio::AudioFormat::F32LE);
            info.set_rate(48_000);
            info.set_channels(1);
            let mut pos = [0; spa::param::audio::MAX_CHANNELS];
            pos[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
            info.set_position(pos);
            let bytes = spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &spa::pod::Value::Object(spa::pod::Object {
                    type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                    id: spa::param::ParamType::EnumFormat.as_raw(),
                    properties: info.into(),
                }),
            )
            .unwrap()
            .0
            .into_inner();
            let mut params = [spa::pod::Pod::from_bytes(&bytes).unwrap()];
            stream
                .connect(
                    spa::utils::Direction::Input,
                    None,
                    pw::stream::StreamFlags::AUTOCONNECT
                        | pw::stream::StreamFlags::MAP_BUFFERS
                        | pw::stream::StreamFlags::RT_PROCESS,
                    &mut params,
                )
                .unwrap();
            let mut out = Vec::new();
            let mut ready = Some(ready_tx);
            while !stop2.load(Ordering::Relaxed) {
                ml.loop_()
                    .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(5)));
                while let Ok((now, n)) = meta_rx.pop() {
                    let mut v = Vec::with_capacity(n);
                    for _ in 0..n {
                        v.push(pcm_rx.pop().unwrap());
                    }
                    out.push((now, v));
                    if let Some(r) = ready.take() {
                        let _ = r.send(());
                    }
                }
            }
            let _ = stream.disconnect();
            out
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the monitor stream runs");
        Monitor { stop, thread }
    }

    pub(crate) fn finish(self) -> Vec<(i64, Vec<f32>)> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap()
    }
}

/// One probe of the player during playback.
#[derive(Debug, Clone, Copy)]
struct Probe {
    t: i64,
    heard: u64,
    played: u64,
    latency: u64,
    quantum: u64,
    delay: u64,
    buffered: u64,
    queued: u64,
}

/// Where in the monitor (48 kHz index) the clip's first sample lies: the energy onset, refined by
/// correlating the monitor, decimated to 24 kHz, against the clip.
pub(crate) fn align(clip: &[i16], mon: &[f32]) -> i64 {
    let peak = clip.iter().map(|&s| (s as i32).abs()).max().unwrap() as f32 / 32768.0;
    let thr = peak * 0.1;
    let k_on = clip
        .iter()
        .position(|&s| (s as f32 / 32768.0).abs() > thr)
        .unwrap() as i64;
    let m_on = mon
        .iter()
        .position(|&s| s.abs() > thr)
        .expect("the clip reached the sink") as i64;
    let dec: Vec<f32> = mon.iter().step_by(2).copied().collect();
    let x: Vec<f32> = clip.iter().map(|&s| s as f32 / 32768.0).collect();
    let win = (k_on as usize..(k_on as usize + 12_000).min(x.len())).collect::<Vec<_>>();
    let l0 = m_on / 2 - k_on;
    let mut best = (f32::MIN, l0);
    for l in l0 - 400..=l0 + 400 {
        let mut s = 0.0;
        for &k in &win {
            let j = k as i64 + l;
            if j >= 0 && (j as usize) < dec.len() {
                s += x[k] * dec[j as usize];
            }
        }
        if s > best.0 {
            best = (s, l);
        }
    }
    best.1 * 2
}

/// The clip sample the sink presented at `t`, from the monitor's cycle times.
fn truth_at(t: i64, cycles: &[(i64, usize)], m0: i64) -> Option<f64> {
    let i = cycles.partition_point(|&(now, _)| now <= t);
    if i == 0 {
        return None;
    }
    let (now, start) = cycles[i - 1];
    let m = start as f64 + (t - now) as f64 * 48_000.0 / 1e9;
    Some((m - m0 as f64) / 2.0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the private PipeWire instance of scripts/live-check-audio.sh"]
async fn live_heard_position_against_the_sink() {
    if !private_or_skip() {
        return;
    }
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    // KAI_LIVE_SINK: the sink to measure against. kai-test-sink-follower runs in the Dummy
    // driver's cycle with the stream and the monitor: the monitor is the sink's own timeline.
    // kai-test-sink drives the graph: it consumes at the end of its cycle, so its monitor reaches
    // a stream one cycle later, and the truth read from it lags by exactly one period.
    let sink = std::env::var("KAI_LIVE_SINK").unwrap_or_else(|_| FOLLOWER.into());
    let monitor_lag = if sink == FOLLOWER { 0.0 } else { 1.0 };
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(dir.path());
    s.output = Some(sink.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut audio = Audio::start(s, tx).unwrap();
    wait_for(
        &mut rx,
        10,
        "devices",
        |e| matches!(e, AudioEvent::Devices(d) if d.outputs.iter().any(|o| o.name == sink)),
    )
    .await;
    let monitor = Monitor::start(&sink);
    let mut player = audio.open_playback();
    let mut out_levels = audio.levels(Tap::Output);
    tokio::time::sleep(Duration::from_millis(300)).await; // the stream links
    let t_push = monotonic_ns();
    assert!(player.push("clip", &clip));
    player.end("clip");
    let mut probes = Vec::new();
    let mut loudest = 0.0f32;
    let limit =
        Instant::now() + Duration::from_secs_f64(clip.len() as f64 / RATE as f64 * 3.0 + 2.0);
    while Instant::now() < limit {
        if let Some(w) = player.pump() {
            panic!("{w}");
        }
        let t = monotonic_ns();
        let pos = player.position();
        let c = player.count("clip").unwrap();
        probes.push(Probe {
            t,
            heard: c.heard,
            played: c.played,
            latency: pos.latency,
            quantum: pos.quantum,
            delay: pos.delay,
            buffered: pos.buffered,
            queued: pos.queued,
        });
        if probes.len() == 100 {
            // The two streams' node properties while they run.
            let dump = std::process::Command::new("pw-dump").output().unwrap();
            std::fs::write(out_dir().join("heard-pw-dump.json"), &dump.stdout).unwrap();
        }
        loudest = loudest.max(out_levels.read().level);
        if player.idle() && c.heard as usize == clip.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rec = monitor.finish();
    drop(out_levels);
    drop(player);
    audio.shutdown(Duration::from_secs(4)).await;

    // The monitor timeline: (cycle time, first 48 kHz index of its buffer).
    let mut cycles = Vec::new();
    let mut mon = Vec::new();
    for (now, v) in &rec {
        cycles.push((*now, mon.len()));
        mon.extend_from_slice(v);
    }
    let m0 = align(&clip, &mon);
    let len = clip.len() as f64;
    let mut worst_truth = 0.0f64;
    let mut sum_truth = 0.0f64;
    let mut n_truth = 0;
    let mut worst_report = 0i64;
    let mut quantum = 0;
    let mut latency = (u64::MAX, 0u64);
    let mut csv = String::from("t_ms,heard,played,latency,delay,buffered,queued,quantum,truth\n");
    let mut parts = (0u64, 0u64, 0u64);
    for p in &probes {
        quantum = quantum.max(p.quantum);
        latency = (latency.0.min(p.latency), latency.1.max(p.latency));
        let truth = truth_at(p.t, &cycles, m0);
        parts = (
            parts.0.max(p.delay),
            parts.1.max(p.buffered),
            parts.2.max(p.queued),
        );
        csv += &format!(
            "{:.3},{},{},{},{},{},{},{},{}\n",
            (p.t - t_push) as f64 / 1e6,
            p.heard,
            p.played,
            p.latency,
            p.delay,
            p.buffered,
            p.queued,
            p.quantum,
            truth.map(|v| format!("{v:.1}")).unwrap_or_default()
        );
        // (a) against PipeWire's reported delay: played - delay.
        if p.played > 0 && p.played < clip.len() as u64 {
            let reported = p.played.saturating_sub(p.latency) as i64;
            worst_report = worst_report.max((p.heard as i64 - reported).abs());
        }
        // (b) against the sink's own timeline, inside the clip.
        if let Some(tr) = truth
            && tr > 0.0
            && tr < len - 1.0
        {
            let e = p.heard as f64 - tr - monitor_lag * p.quantum as f64;
            worst_truth = worst_truth.max(e.abs());
            sum_truth += e;
            n_truth += 1;
        }
    }
    let ms = |s: f64| s * 1000.0 / RATE as f64;
    let report = format!(
        "sink {sink}; clip {} samples ({:.2} s); {} probes, {} inside the clip; monitor {} cycles\n\
         period (frames per cycle at 24 kHz): {quantum} = {:.1} ms\n\
         reported latency (queued + buffered + delay): {}..{} frames = {:.1}..{:.1} ms \
         (largest delay {}, buffered {}, queued {})\n\
         (a) heard vs played - reported delay: worst {} frames = {:.1} ms\n\
         (b) heard vs the sink's monitor (less {monitor_lag} period of monitor lag): \
         worst {:.1} frames = {:.1} ms, mean {:+.1} frames = {:+.2} ms\n\
         output tap: loudest level {loudest:.3}\n",
        clip.len(),
        len / RATE as f64,
        probes.len(),
        n_truth,
        cycles.len(),
        ms(quantum as f64),
        latency.0,
        latency.1,
        ms(latency.0 as f64),
        ms(latency.1 as f64),
        parts.0,
        parts.1,
        parts.2,
        worst_report,
        ms(worst_report as f64),
        worst_truth,
        ms(worst_truth),
        sum_truth / n_truth.max(1) as f64,
        ms(sum_truth / n_truth.max(1) as f64),
    );
    let out = out_dir();
    std::fs::write(out.join(format!("heard-probes-{sink}.csv")), csv).unwrap();
    std::fs::write(out.join(format!("heard-report-{sink}.txt")), &report).unwrap();
    eprintln!("{report}");
    assert!(n_truth > 50, "too few probes inside the clip");
    assert!(
        worst_report <= quantum as i64,
        "heard is more than one period off played - delay"
    );
    assert!(
        worst_truth <= quantum as f64,
        "heard is more than one period off the sink"
    );
    assert!(
        (sum_truth / n_truth as f64).abs() <= 2.0,
        "heard is off the sink on average"
    );
    assert!(loudest > 0.3, "the output tap saw the clip");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the private PipeWire instance of scripts/live-check-audio.sh"]
async fn live_barge_in_flush_reports_what_was_heard() {
    if !private_or_skip() {
        return;
    }
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(dir.path());
    s.output = Some(SINK.into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut audio = Audio::start(s, tx).unwrap();
    wait_for(
        &mut rx,
        10,
        "devices",
        |e| matches!(e, AudioEvent::Devices(d) if !d.outputs.is_empty()),
    )
    .await;
    let mut player = audio.open_playback();
    tokio::time::sleep(Duration::from_millis(300)).await;
    player.push("a", &clip);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(1500) {
        player.pump();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = player.count("a").unwrap();
    let (counts, note) = player.flushed(Duration::from_millis(1000)).await;
    assert_eq!(note, None, "the stream took the flush");
    let a = counts.iter().find(|c| c.item == "a").unwrap().clone();
    eprintln!(
        "barge-in at 1.5 s: heard {} ms, played {} ms (pushed {} ms); just before: heard {} ms",
        a.heard_ms(),
        a.played_ms(),
        a.pushed * 1000 / RATE as u64,
        before.heard_ms()
    );
    assert!(a.played < a.pushed && a.heard <= a.played);
    assert!(a.heard >= before.heard);
    assert!((1300..1700).contains(&a.heard_ms()), "{a:?}");
    // The flushed item takes no more; a new item plays from the flush point.
    assert!(!player.push("a", &clip[..100]));
    assert!(player.push("b", &clip[..4800]));
    let t1 = Instant::now();
    while !player.idle() && t1.elapsed() < Duration::from_secs(3) {
        player.pump();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let b = player.count("b").unwrap();
    assert_eq!((b.played, b.heard), (4800, 4800));
    let a_after = player.count("a").unwrap();
    assert_eq!(
        a_after.played, a.played,
        "nothing of a played after the flush"
    );
    std::fs::write(
        out_dir().join("barge-in-report.txt"),
        format!("{a:?}\nbefore the flush: {before:?}\nb after: {b:?}\n"),
    )
    .unwrap();
    // A second open replaces the first; the first one's late close leaves the second alone.
    let mut second = audio.open_playback();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(player.closed(), "a second open replaces the first");
    drop(player);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !second.closed(),
        "the first player's close did not close the second"
    );
    second.push("c", &clip[..2400]);
    let t2 = Instant::now();
    while !second.idle() && t2.elapsed() < Duration::from_secs(3) {
        second.pump();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(second.count("c").unwrap().heard, 2400);
    drop(second);
    audio.shutdown(Duration::from_secs(4)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the private PipeWire instance of scripts/live-check-audio.sh"]
async fn live_devices_capture_levels_and_the_device_warning() {
    if !private_or_skip() {
        return;
    }
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(dir.path());
    s.input = Some(MIC.into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut a = Audio::start(s, tx).unwrap();
    let e = wait_for(&mut rx, 10, "devices", |e| {
        matches!(e, AudioEvent::Devices(d)
            if d.outputs.len() >= 2 && d.inputs.iter().any(|i| i.name == MIC) && d.default_output.is_some())
    })
    .await;
    let AudioEvent::Devices(d) = e else {
        unreachable!()
    };
    let names = |v: &[Device]| v.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
    eprintln!(
        "outputs {:?}, inputs {:?}, defaults {:?} / {:?}",
        names(&d.outputs),
        names(&d.inputs),
        d.default_output,
        d.default_input
    );
    assert_eq!(d.default_output.as_deref(), Some(SINK));

    // `device` mode on a speaker that is not the default: the warning offers half duplex.
    a.set_output(Some(SINK2.into()));
    wait_for(&mut rx, 5, "the device warning", |e| {
        matches!(e, AudioEvent::Echo(s) if s.offer_half_duplex && s.note.as_deref().is_some_and(|n| n.contains("'Kai test sink 2'")))
    })
    .await;
    a.set_output(None);
    wait_for(
        &mut rx,
        5,
        "no warning",
        |e| matches!(e, AudioEvent::Echo(s) if !s.offer_half_duplex),
    )
    .await;

    // Capture from the virtual microphone while a second Kai audio plays the clip into it.
    let mut cap = a.open_capture();
    let mut in_levels = a.levels(Tap::Input);
    let (tx_b, _rx_b) = mpsc::unbounded_channel();
    let mut sb = settings(dir.path());
    sb.output = Some(MIC_IN.into());
    let mut b = Audio::start(sb, tx_b).unwrap();
    let mut feeder = b.open_playback();
    tokio::time::sleep(Duration::from_millis(300)).await;
    feeder.push("clip", &clip[..(RATE as usize * 2).min(clip.len())]);
    let mut got = Vec::new();
    let mut loudest = 0.0f32;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(2600) {
        feeder.pump();
        cap.read(&mut got);
        loudest = loudest.max(in_levels.read().level);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(cap.overflow(), None);
    let energy = got.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / got.len().max(1) as f64;
    let report = format!(
        "captured {} samples in 2.6 s ({:.2} s of audio), rms {:.0}; loudest input level {loudest:.3}\n",
        got.len(),
        got.len() as f64 / RATE as f64,
        energy.sqrt()
    );
    eprintln!("{report}");
    std::fs::write(out_dir().join("capture-report.txt"), &report).unwrap();
    assert!(got.len() as f64 > RATE as f64 * 2.0, "the capture ran");
    assert!(energy.sqrt() > 300.0, "the clip reached the capture");
    assert!(loudest > 0.3, "the input tap saw the clip");
    drop(feeder);
    b.shutdown(Duration::from_secs(4)).await;
    assert!(!cap.closed());
    drop(in_levels);
    drop(cap);
    a.shutdown(Duration::from_secs(4)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the private PipeWire instance of scripts/live-check-audio.sh"]
async fn live_app_aec_flips_the_defaults_and_a_switch_away_restores_them() {
    if !private_or_skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(dir.path());
    s.echo_mode = EchoMode::App;
    let aec_file = s.aec_file.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut audio = Audio::start(s, tx).unwrap();
    let t0 = Instant::now();
    wait_for(&mut rx, 10, "app AEC active", |e| {
        matches!(e, AudioEvent::Echo(s) if s.effective == EchoMode::App && s.note.as_deref().is_some_and(|n| n.contains("Kai cancels echo")))
    })
    .await;
    let up = t0.elapsed();
    wait_for(&mut rx, 5, "the default output is Kai's canceller", |e| {
        matches!(e, AudioEvent::Devices(d) if d.default_output.as_deref() == Some(aec::SINK) && d.default_input.as_deref() == Some(aec::SOURCE))
    })
    .await;
    assert!(aec_file.exists(), "aec.json is written before the flip");
    let saved = aec::load(&aec_file).unwrap().unwrap();
    eprintln!("app AEC up in {up:?}; saved {saved:?}");
    assert_eq!(saved.default_sink, SINK);
    // Kai's own playback now goes through the canceller: open one and look at its link.
    let player = audio.open_playback();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let links = std::process::Command::new("pw-link")
        .arg("-l")
        .output()
        .unwrap();
    let links = String::from_utf8_lossy(&links.stdout).to_string();
    std::fs::write(out_dir().join("aec-links.txt"), &links).unwrap();
    assert!(
        links.contains("kai.playback") && links.contains(aec::SINK),
        "{links}"
    );
    drop(player);
    audio.set_echo_mode(EchoMode::Device);
    wait_for(
        &mut rx,
        10,
        "the defaults are back",
        |e| matches!(e, AudioEvent::Devices(d) if d.default_output.as_deref() == Some(SINK)),
    )
    .await;
    let t1 = Instant::now();
    while aec_file.exists() && t1.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !aec_file.exists(),
        "aec.json goes once PipeWire confirmed the restore"
    );
    audio.shutdown(Duration::from_secs(4)).await;
}

/// The owner's check (`scripts/owner-check-audio.sh`): audible, on the session's default output.
/// Plays the clip; Enter interrupts it like a barge-in. Then it replays the clip up to what Kai
/// counted as heard, so the owner can hear whether it ends where their listening stopped.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "the owner's check: plays audibly on the default output (scripts/owner-check-audio.sh)"]
async fn owner_heard_on_the_default_output() {
    if std::env::var("KAI_OWNER_CHECK").as_deref() != Ok("1") {
        eprintln!("the owner's check (audible): run scripts/owner-check-audio.sh");
        return;
    }
    let clip = read_wav(&std::env::var("KAI_LIVE_CLIP").expect("KAI_LIVE_CLIP"));
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut audio = Audio::start(settings(dir.path()), tx).unwrap();
    let AudioEvent::Devices(d) = wait_for(
        &mut rx,
        10,
        "devices",
        |e| matches!(e, AudioEvent::Devices(d) if d.default_output.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    let out = d.default_output.clone().unwrap();
    let label = DeviceList::label_of(&d.outputs, &out)
        .unwrap_or(&out)
        .to_owned();
    println!("\nPlaying the clip on '{label}'. Press Enter while it speaks to interrupt it.");
    let (enter_tx, mut enter_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok() {
            let _ = enter_tx.send(());
        }
    });
    let mut player = audio.open_playback();
    tokio::time::sleep(Duration::from_millis(300)).await;
    player.push("clip", &clip);
    player.end("clip");
    let mut worst = 0i64;
    let mut last = PlayPos::default();
    let mut interrupted = None;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(30) {
        player.pump();
        let pos = player.position();
        let c = player.count("clip").unwrap();
        if c.played > 0 && c.played < clip.len() as u64 {
            let reported = c.played.saturating_sub(pos.latency) as i64;
            worst = worst.max((c.heard as i64 - reported).abs());
            last = pos;
        }
        if enter_rx.try_recv().is_ok() {
            let (counts, note) = player.flushed(Duration::from_millis(1000)).await;
            interrupted = Some((counts.into_iter().find(|c| c.item == "clip").unwrap(), note));
            break;
        }
        if player.idle() && c.heard as usize == clip.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let ms = |f: u64| f as f64 * 1000.0 / RATE as f64;
    let mut report = format!(
        "output '{label}' ({out})\n\
         period {} frames = {:.1} ms; reported latency {} frames = {:.1} ms \
         (delay {:.1} ms, resampler {:.1} ms, queued {:.1} ms)\n\
         heard vs played - reported delay: worst {worst} frames = {:.1} ms\n",
        last.quantum,
        ms(last.quantum),
        last.latency,
        ms(last.latency),
        ms(last.delay),
        ms(last.buffered),
        ms(last.queued),
        ms(worst as u64),
    );
    if let Some((c, note)) = &interrupted {
        report += &format!(
            "interrupted: heard {} ms, handed to PipeWire {} ms{}\n",
            c.heard_ms(),
            c.played_ms(),
            note.as_deref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default()
        );
    }
    println!("\n{report}");
    std::fs::write(out_dir().join("owner-heard-report.txt"), &report).unwrap();
    if let Some((c, _)) = interrupted {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        println!(
            "Replaying the clip up to the heard position ({} ms): does it end where you stopped \
             hearing it?",
            c.heard_ms()
        );
        player.push("replay", &clip[..c.heard as usize]);
        player.end("replay");
        let t1 = Instant::now();
        while !player.idle() && t1.elapsed() < Duration::from_secs(30) {
            player.pump();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    drop(player);
    audio.shutdown(Duration::from_secs(4)).await;
}
