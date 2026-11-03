//! cam — camera viewer for nopeekOS.
//!
//! Reads the kernel's camera feed (`npk_camera_*`, right CAMERA), which
//! the `uvc` driver fills from the active camera while someone reads, and
//! shows each frame in a canvas: MJPEG decoded on the GPU's JPEG engine or
//! the CPU, YUY2 converted in the scene. The footer counts frames and how
//! long a CPU decode takes.
//!
//! 480p is YUY2, 720p and 1080p are MJPEG; the buttons or the keys 1, 2, 3
//! switch the size cam asks the feed for.

#![no_std]

extern crate alloc;

#[cfg(target_arch = "wasm32")]
mod app {
    use alloc::string::String;
    use alloc::vec::Vec;

    use nopeek_widgets::app_meta::IconRef;
    use nopeek_widgets::*;

    #[unsafe(link_section = ".npk.app_meta")]
    #[used]
    static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
        = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

    #[unsafe(link_section = ".npk.caps")]
    #[used]
    static NPK_CAPS: [u8; 2] = [caps::READ | caps::CANVAS | caps::RENDER, caps::ext::CAMERA];

    #[global_allocator]
    static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! { log("[cam] panic!"); core::arch::wasm32::unreachable() }

    const CANVAS_ID: u32 = 1;

    const ACT_480: u32 = 1;
    const ACT_720: u32 = 2;
    const ACT_1080: u32 = 3;

    fn log(msg: &str) { host::log_serial(msg); }

    /// A frame as a scene layer (the compositor scales it on the GPU and
    /// leaves the window alone), or the plain canvas on a kernel without.
    fn show(px: &[u8], w: u32, h: u32) {
        if !host::canvas_frame(CANVAS_ID, px, w, h) {
            host::canvas_commit(CANVAS_ID, px, w, h);
        }
    }

    /// The size a button asks the feed for.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode { P480, P720, P1080 }

    impl Mode {
        fn size(self) -> (u16, u16) {
            match self {
                Mode::P480 => (640, 480),
                Mode::P720 => (1280, 720),
                Mode::P1080 => (1920, 1080),
            }
        }
    }

    struct Cam {
        reader: i32,
        mode: Mode,
        jpeg: jpeg::Decoder,
        frames: u64,
        decode_ms: u64,
        decoded: u64,
        jpeg_errors: u64,
        /// MJPEG goes to the GPU until it says it cannot.
        gpu_jpeg: bool,
        gpu_busy: u64,
        last: Option<npk_sys::CameraFrame>,
    }

    impl Cam {
        /// A reader for `mode`'s size; the old one goes.
        fn open(&mut self, mode: Mode) -> Result<(), String> {
            if self.reader >= 0 { npk_sys::camera_close(self.reader); }
            self.mode = mode;
            let (w, h) = mode.size();
            self.reader = npk_sys::camera_open(w, h);
            self.frames = 0;
            self.decode_ms = 0;
            self.decoded = 0;
            self.jpeg_errors = 0;
            self.last = None;
            if self.reader < 0 { return Err("cam has no CAMERA right.".into()); }
            Ok(())
        }

        fn avg_ms(&self) -> u64 {
            if self.decoded == 0 { 0 } else { self.decode_ms / self.decoded }
        }
    }

    fn mode_button(label: &str, act: u32, on: bool) -> Widget {
        let style = if on { prefab::ButtonStyle::Primary } else { prefab::ButtonStyle::Secondary };
        prefab::button(label, style, ActionId(act))
    }

    fn render(c: Option<&Cam>, line1: &str, line2: &str) -> Widget {
        let mode = c.map(|c| c.mode);
        Widget::Column {
            children: alloc::vec![
                Widget::Row {
                    children: alloc::vec![
                        mode_button("480p", ACT_480, mode == Some(Mode::P480)),
                        mode_button("720p", ACT_720, mode == Some(Mode::P720)),
                        mode_button("1080p", ACT_1080, mode == Some(Mode::P1080)),
                    ],
                    spacing: Spacing::Sm.as_u16(),
                    align: Align::Center,
                    modifiers: alloc::vec![Modifier::PaddingXY { x: 8, y: 6 }],
                },
                Widget::Divider,
                Widget::Canvas {
                    id: CanvasId(CANVAS_ID),
                    width: 640,
                    height: 360,
                    modifiers: alloc::vec![Modifier::Flex(1), Modifier::Background(Token::Page)],
                },
                Widget::Divider,
                Widget::Text {
                    content: line1.into(),
                    style: TextStyle::Body,
                    modifiers: alloc::vec![Modifier::PaddingXY { x: 8, y: 4 }],
                },
                Widget::Text {
                    content: line2.into(),
                    style: TextStyle::Mono,
                    modifiers: alloc::vec![Modifier::PaddingXY { x: 8, y: 4 }, Modifier::Tint(Token::OnSurfaceMuted)],
                },
            ],
            spacing: 0,
            align: Align::Stretch,
            modifiers: alloc::vec![Modifier::Background(Token::Surface)],
        }
    }

    fn commit(c: Option<&Cam>, line1: &str, line2: &str) {
        if let Ok(bytes) = wire::encode(&render(c, line1, line2)) {
            if !host::scene_commit(&bytes) { log("[cam] commit failed"); }
        }
    }

    fn stats_line(c: &Cam, fps: u64) -> String {
        let mut line = alloc::format!("{} fps  frames {}", fps, c.frames);
        if let Some(f) = c.last {
            if f.format == npk_sys::CAMERA_MJPEG {
                if c.gpu_jpeg {
                    line.push_str("  GPU decode");
                    if c.gpu_busy > 0 { line.push_str(&alloc::format!(" (dropped {})", c.gpu_busy)); }
                } else {
                    line.push_str(&alloc::format!("  CPU decode {} ms", c.avg_ms()));
                }
            }
        }
        if c.jpeg_errors > 0 { line.push_str(&alloc::format!("  jpeg errors {}", c.jpeg_errors)); }
        line
    }

    enum Step { Go, Close, Gone }

    /// Events: Esc closes, a button or 1/2/3 switches the mode.
    fn events(c: &mut Cam, buf: &mut [u8]) -> Step {
        loop {
            let next = match nopeek_widgets::events::poll(buf) {
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Escape)) => return Step::Close,
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Char(b'1')))
                | nopeek_widgets::events::Poll::Event(Event::Action(ActionId(ACT_480))) => Some(Mode::P480),
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Char(b'2')))
                | nopeek_widgets::events::Poll::Event(Event::Action(ActionId(ACT_720))) => Some(Mode::P720),
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Char(b'3')))
                | nopeek_widgets::events::Poll::Event(Event::Action(ActionId(ACT_1080))) => Some(Mode::P1080),
                nopeek_widgets::events::Poll::Event(_) => None,
                nopeek_widgets::events::Poll::Empty => return Step::Go,
                nopeek_widgets::events::Poll::Gone => return Step::Gone,
            };
            if let Some(m) = next {
                if m != c.mode {
                    commit(Some(c), "Switching…", "");
                    if let Err(e) = c.open(m) {
                        log(&alloc::format!("[cam] {}", e));
                        commit(Some(c), &e, "");
                    }
                }
            }
        }
    }

    /// `run cam pattern`: colour bars with a moving stripe, 1280x720, through
    /// the same layer path, no camera. A picture to check the path by.
    fn pattern(yuv: bool) {
        let (w, h) = (1280usize, 720usize);
        let bars: [u32; 8] = [0xFFFFFF, 0xFFFF00, 0x00FFFF, 0x00FF00, 0xFF00FF, 0xFF0000, 0x0000FF, 0x000000];
        let mut px = alloc::vec![0u8; w * h * 4];
        let mut ev = alloc::vec![0u8; 4096];
        let mut t = 0usize;
        let mut layer = true;
        // The window first: events and canvases need it.
        commit(None, "Test pattern 1280x720", "");
        loop {
            match nopeek_widgets::events::poll(&mut ev) {
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Escape)) => { host::close_widget(); return; }
                nopeek_widgets::events::Poll::Gone => return,
                _ => {}
            }
            let stripe = (t * 8) % w;
            for y in 0..h {
                for x in 0..w {
                    let c = if x >= stripe && x < stripe + 16 { 0x808080 } else { bars[x * 8 / w] };
                    let c = if y >= h * 3 / 4 { (c >> 1) & 0x7F7F7F } else { c };
                    let o = (y * w + x) * 4;
                    px[o] = c as u8;
                    px[o + 1] = (c >> 8) as u8;
                    px[o + 2] = (c >> 16) as u8;
                    px[o + 3] = 255;
                }
            }
            if yuv {
                // The same bars as YUYV (BT.601 limited), through the GPU's
                // conversion.
                let mut yv = alloc::vec![0u8; w * h * 2];
                for i in 0..w * h / 2 {
                    let p0 = &px[i * 8..i * 8 + 4];
                    let p1 = &px[i * 8 + 4..i * 8 + 8];
                    let luma = |p: &[u8]| ((66 * p[2] as i32 + 129 * p[1] as i32 + 25 * p[0] as i32 + 128) >> 8) + 16;
                    let (r, g, b) = (p0[2] as i32, p0[1] as i32, p0[0] as i32);
                    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    yv[i * 4] = luma(p0) as u8;
                    yv[i * 4 + 1] = u.clamp(0, 255) as u8;
                    yv[i * 4 + 2] = luma(p1) as u8;
                    yv[i * 4 + 3] = v.clamp(0, 255) as u8;
                }
                if !host::canvas_frame_as(CANVAS_ID, &yv, w as u32, h as u32, host::frame::YUYV) {
                    log("[cam] pattern-yuv: refused");
                }
                if t % 30 == 0 { commit(None, "Test pattern 1280x720 YUYV", "scene layer, GPU converts"); }
                t += 1;
                host::sleep_ms(33);
                continue;
            }
            if layer && !host::canvas_frame(CANVAS_ID, &px, w as u32, h as u32) {
                layer = false;
                log("[cam] pattern: no frame layer, plain canvas");
            }
            if !layer { host::canvas_commit(CANVAS_ID, &px, w as u32, h as u32); }
            if t % 30 == 0 {
                commit(None, "Test pattern 1280x720", if layer { "scene layer" } else { "canvas" });
            }
            t += 1;
            host::sleep_ms(33);
        }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn _start() {
        match host::launch_arg().as_deref() {
            Some("pattern") => { pattern(false); return; }
            Some("pattern-yuv") => { pattern(true); return; }
            _ => {}
        }
        commit(None, "Waiting for the camera…", "");
        let mut c = Cam {
            reader: -1, mode: Mode::P720, jpeg: jpeg::Decoder::new(), frames: 0, decode_ms: 0, decoded: 0,
            jpeg_errors: 0, gpu_jpeg: true, gpu_busy: 0, last: None,
        };
        if let Err(e) = c.open(Mode::P720) {
            log(&alloc::format!("[cam] {}", e));
            commit(None, &e, "");
            wait_close();
            return;
        }
        let mut buf: Vec<u8> = alloc::vec![0u8; 512 * 1024];
        let mut bgra: Vec<u8> = Vec::new();
        let mut ev = alloc::vec![0u8; 4096];
        let mut last_tick = host::ticks_ms();
        let mut frames_at_tick = 0u64;
        let mut logged = 0u32;
        loop {
            match events(&mut c, &mut ev) {
                Step::Go => {}
                Step::Close => {
                    npk_sys::camera_close(c.reader);
                    host::close_widget();
                    return;
                }
                Step::Gone => {
                    npk_sys::camera_close(c.reader);
                    return;
                }
            }
            let got = match npk_sys::camera_read(c.reader, &mut buf) {
                Ok(v) => v,
                Err(0) => None,
                Err(need) => {
                    buf.resize(need, 0);
                    None
                }
            };
            if let Some((n, f)) = got {
                c.frames += 1;
                c.last = Some(f);
                let (w, h) = (f.width as usize, f.height as usize);
                let data = &buf[..n];
                match f.format {
                    npk_sys::CAMERA_MJPEG if c.gpu_jpeg => {
                        match host::canvas_jpeg(CANVAS_ID, data, w as u32, h as u32) {
                            host::JpegFrame::Queued => c.decoded += 1,
                            host::JpegFrame::Busy => c.gpu_busy += 1,
                            host::JpegFrame::Unsupported => {
                                c.gpu_jpeg = false;
                                log("[cam] no GPU JPEG decode, decoding on the CPU");
                            }
                        }
                    }
                    npk_sys::CAMERA_MJPEG => {
                        let t0 = host::ticks_ms();
                        // The smallest scale that still fills the canvas: a
                        // preview needs no full frame.
                        let scale = host::canvas_rect(CANVAS_ID).map_or(1, |(_, _, rw, rh)| {
                            [8u32, 4, 2].into_iter().find(|&s| {
                                (w as i32) / s as i32 >= rw.max(1) && (h as i32) / s as i32 >= rh.max(1)
                            }).unwrap_or(1)
                        });
                        match c.jpeg.decode_scaled(data, scale, &mut bgra) {
                            Ok((dw, dh)) => {
                                c.decode_ms += host::ticks_ms() - t0;
                                c.decoded += 1;
                                show(&bgra, dw, dh);
                            }
                            Err(e) => {
                                c.jpeg_errors += 1;
                                if logged < 5 {
                                    logged += 1;
                                    log(&alloc::format!("[cam] jpeg: {:?} ({} B)", e, n));
                                }
                            }
                        }
                    }
                    _ => {
                        // The layer takes YUY2 as it comes; the GPU converts.
                        // A kernel without falls back.
                        let need = w * h * 2;
                        if n < need
                            || !host::canvas_frame_as(CANVAS_ID, &data[..need], w as u32, h as u32, host::frame::YUYV)
                        {
                            bgra.resize(w * h * 4, 0);
                            yuyv_to_bgra(data, w, h, &mut bgra);
                            show(&bgra, w as u32, h as u32);
                        }
                    }
                }
            }
            let now = host::ticks_ms();
            if now - last_tick >= 1000 {
                let fps = c.frames.saturating_sub(frames_at_tick) * 1000 / (now - last_tick);
                frames_at_tick = c.frames;
                let desc = match c.last {
                    Some(f) => alloc::format!("{}x{} {}", f.width, f.height,
                        if f.format == npk_sys::CAMERA_MJPEG { "MJPEG" } else { "YUY2" }),
                    None => String::from("Waiting for the camera (uvc runs it)…"),
                };
                commit(Some(&c), &desc, &stats_line(&c, fps));
                last_tick = now;
            }
            if got.is_none() { host::sleep_ms(5); }
        }
    }

    /// YUY2 (BT.601, limited range) to BGRA.
    fn yuyv_to_bgra(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
        let n = (w * h / 2).min(src.len() / 4).min(dst.len() / 8);
        for i in 0..n {
            let s = &src[i * 4..i * 4 + 4];
            let (y0, u, y1, v) = (s[0] as i32, s[1] as i32 - 128, s[2] as i32, s[3] as i32 - 128);
            for (k, y) in [y0, y1].into_iter().enumerate() {
                let c = 298 * (y - 16);
                let r = (c + 409 * v + 128) >> 8;
                let g = (c - 100 * u - 208 * v + 128) >> 8;
                let b = (c + 516 * u + 128) >> 8;
                let o = (i * 2 + k) * 4;
                dst[o] = b.clamp(0, 255) as u8;
                dst[o + 1] = g.clamp(0, 255) as u8;
                dst[o + 2] = r.clamp(0, 255) as u8;
                dst[o + 3] = 255;
            }
        }
    }

    fn wait_close() {
        let mut events = alloc::vec![0u8; 4096];
        loop {
            match nopeek_widgets::events::poll(&mut events) {
                nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Escape)) => { host::close_widget(); return; }
                nopeek_widgets::events::Poll::Gone => return,
                _ => host::sleep_ms(50),
            }
        }
    }

    #[allow(dead_code)]
    fn _keep_iconref_alive() -> Option<IconRef> { None }
}
