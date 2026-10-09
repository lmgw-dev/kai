// WP0 (p): does pipewire-rs 0.10.1 expose pw_stream_get_time_n? Stream::time() with feature v0_3_50.
use pipewire as pw;
fn main() {
    pw::init();
    let ml = pw::main_loop::MainLoopRc::new(None).unwrap();
    let ctx = pw::context::ContextRc::new(&ml, None).unwrap();
    let core = ctx.connect_rc(None).unwrap();
    let st = pw::stream::StreamBox::new(&core, "wp0-probe", pw::properties::properties! { *pw::keys::MEDIA_CLASS => "Audio/Sink" }).unwrap();
    match st.time() {
        Ok(t) => println!("time(): now={} rate={:?} ticks={} delay={} queued={} buffered={} queued_buffers={}", t.now(), t.rate(), t.ticks(), t.delay(), t.queued(), t.buffered(), t.queued_buffers()),
        Err(e) => println!("time() on an unconnected stream: Err({e:?}) -- the symbol links and the call is safe"),
    }
}
