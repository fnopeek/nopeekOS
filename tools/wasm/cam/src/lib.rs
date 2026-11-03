//! cam — camera viewer for nopeekOS.
//!
//! Claims the first USB camera (`npk_usb_*`, right CAMERA), negotiates a
//! format with probe and commit, picks the streaming setting the format
//! needs and shows the frames in a canvas. The footer counts what the
//! stream did: frames, bad frames, packets lost, payloads with the UVC
//! error bit, and how long a frame takes to decode. That makes cam the
//! test of the USB path before any other user of the camera exists.
//!
//! 480p is YUY2, 720p and 1080p are MJPEG (the SDK's `jpeg`); the
//! buttons or the keys 1, 2, 3 switch.

#![no_std]

extern crate alloc;

pub mod uvc;

#[cfg(target_arch = "wasm32")]
mod app {
    use alloc::string::String;
    use alloc::vec::Vec;

    use nopeek_widgets::app_meta::IconRef;
    use nopeek_widgets::*;

    use crate::uvc;

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
    const CLASS_VIDEO: u8 = 0x0E;
    /// Buffers per stream: the kernel queues them again as they come back,
    /// so this is how long cam may be busy (decoding, drawing) before the
    /// camera overruns. 1024 isochronous intervals are 128 ms at high
    /// speed; 64 bulk TDs of 16 KiB a megabyte.
    const ISO_TDS: u32 = 1024;
    const BULK_TDS: u32 = 64;
    const READ_BUF: usize = 512 * 1024;

    const ACT_480: u32 = 1;
    const ACT_720: u32 = 2;
    const ACT_1080: u32 = 3;

    fn log(msg: &str) { host::log_serial(msg); }

    /// A class request to interface `index` (`__uvc_query_ctrl`).
    fn query(h: i32, req: u8, cs: u16, index: u16, data: &mut [u8]) -> i32 {
        let bm = if req & 0x80 != 0 { 0xA1 } else { 0x21 };
        let len = data.len() as u16;
        let setup = [bm, req, 0, cs as u8, index as u8, (index >> 8) as u8, len as u8, (len >> 8) as u8];
        npk_sys::usb_control(h, &setup, data)
    }

    /// The size a button asks for, and the format it prefers.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode { P480, P720, P1080 }

    impl Mode {
        fn want(self) -> (u16, u16, uvc::Kind) {
            match self {
                Mode::P480 => (640, 480, uvc::Kind::Yuyv),
                Mode::P720 => (1280, 720, uvc::Kind::Mjpeg),
                Mode::P1080 => (1920, 1080, uvc::Kind::Mjpeg),
            }
        }
    }

    struct Stream {
        sid: i32,
        bulk_td: usize,
        width: usize,
        height: usize,
        kind: uvc::Kind,
        dec: uvc::Decoder,
        desc: String,
        started: u64,
    }

    struct Cam {
        handle: i32,
        cam: uvc::Camera,
        vid: u16,
        pid: u16,
        mode: Mode,
        stream: Option<Stream>,
        jpeg: jpeg::Decoder,
        decode_ms: u64,
        decoded: u64,
        jpeg_errors: u64,
    }

    /// Claim the first camera. Err is the line the window shows.
    fn claim() -> Result<Cam, String> {
        let mut buf = alloc::vec![0u8; 64 * 1024];
        let h = npk_sys::usb_claim(CLASS_VIDEO, &mut buf);
        if h == -2 { return Err("No camera found.".into()); }
        if h == -3 { return Err("cam has no CAMERA right.".into()); }
        if h < 0 { return Err(alloc::format!("Claiming the camera failed ({}).", h)); }
        let total = u16::from_le_bytes([buf[18 + 2], buf[18 + 3]]) as usize;
        let cfg = &buf[18..(18 + total).min(buf.len())];
        let vid = u16::from_le_bytes([buf[8], buf[9]]);
        let pid = u16::from_le_bytes([buf[10], buf[11]]);
        let Some(cam) = uvc::parse(cfg) else {
            npk_sys::usb_release(h);
            return Err("Not a UVC camera.".into());
        };
        log(&alloc::format!("[cam] {:04x}:{:04x} UVC {:x}.{:02x}, VC if {}, VS if {}, ep {:#04x}, {} formats, {}",
            vid, pid, cam.uvc_version >> 8, cam.uvc_version & 0xFF, cam.vc_if, cam.vs_if, cam.ep,
            cam.formats.len(), if cam.is_bulk() { "bulk" } else { "isochronous" }));
        Ok(Cam { handle: h, cam, vid, pid, mode: Mode::P720, stream: None, jpeg: jpeg::Decoder::new(), decode_ms: 0, decoded: 0, jpeg_errors: 0 })
    }

    /// The format and frame for `mode`: its preferred kind at that size,
    /// else any kind at that size, else the largest frame not above it.
    fn pick(cam: &uvc::Camera, mode: Mode) -> Option<(&uvc::Format, &uvc::Frame)> {
        let (w, h, kind) = mode.want();
        let usable = |f: &&uvc::Format| matches!(f.kind, uvc::Kind::Yuyv | uvc::Kind::Mjpeg);
        let exact = |k: Option<uvc::Kind>| cam.formats.iter().filter(usable)
            .filter(|f| k.is_none_or(|k| f.kind == k))
            .find_map(|f| f.frames.iter().find(|fr| fr.width == w && fr.height == h).map(|fr| (f, fr)));
        exact(Some(kind)).or_else(|| exact(None)).or_else(|| {
            cam.formats.iter().filter(usable)
                .flat_map(|f| f.frames.iter().map(move |fr| (f, fr)))
                .filter(|(_, fr)| fr.width <= w && fr.height <= h)
                .max_by_key(|(f, fr)| (fr.width as u32 * fr.height as u32, f.kind == uvc::Kind::Mjpeg))
        })
    }

    impl Cam {
        fn stop(&mut self) {
            if let Some(st) = self.stream.take() {
                npk_sys::usb_stream_close(self.handle, st.sid);
                let ms = (host::ticks_ms() - st.started).max(1);
                let fps = st.dec.stats.frames * 1000 / ms;
                log(&alloc::format!("[cam] stopped {}: {}", st.desc, stats_line(&st.dec.stats, fps, self.avg_ms())));
            }
            // `uvc_video_stop_streaming`: setting 0, or for a bulk camera
            // with one setting CLEAR_FEATURE(ENDPOINT_HALT) on its
            // endpoint, as Windows does; the camera stops and takes the
            // next commit.
            if self.cam.alts.len() > 1 {
                npk_sys::usb_set_alt(self.handle, self.cam.vs_if, 0);
            } else {
                let setup = [0x02, 0x01, 0, 0, self.cam.ep, 0, 0, 0];
                npk_sys::usb_control(self.handle, &setup, &mut []);
            }
        }

        fn avg_ms(&self) -> u64 {
            if self.decoded == 0 { 0 } else { self.decode_ms / self.decoded }
        }

        /// Probe, commit, the setting, the stream.
        fn start(&mut self) -> Result<(), String> {
            self.stop();
            self.decode_ms = 0;
            self.decoded = 0;
            self.jpeg_errors = 0;
            let h = self.handle;
            let cam = &self.cam;
            let (fmt, frame) = pick(cam, self.mode).ok_or_else(|| String::from("No YUY2 or MJPEG format."))?;
            // Some cameras misbehave unless the streaming interface sees
            // SET_INTERFACE 0 before any video request (`uvc_video_init`).
            if cam.alts.len() > 1 { npk_sys::usb_set_alt(h, cam.vs_if, 0); }
            let size = cam.ctrl_size();
            let want = uvc::Ctrl { hint: 1, format: fmt.index, frame: frame.index, interval: frame.default_interval, ..Default::default() };
            let mut probe = want.encode(size);
            let mut got = alloc::vec![0u8; size];
            let mut ctrl = None;
            for _ in 0..2 {
                if query(h, uvc::SET_CUR, uvc::VS_PROBE_CONTROL, cam.vs_if as u16, &mut probe) < 0
                    || query(h, uvc::GET_CUR, uvc::VS_PROBE_CONTROL, cam.vs_if as u16, &mut got) < 0
                {
                    return Err("Probe failed.".into());
                }
                ctrl = uvc::Ctrl::decode(&got, frame.max_buffer);
                probe.copy_from_slice(&got);
                if cam.is_bulk() || ctrl.is_some_and(|c| cam.alt_for(c.max_payload).is_some()) { break; }
            }
            let ctrl = ctrl.ok_or_else(|| String::from("Probe answer too short."))?;
            if query(h, uvc::SET_CUR, uvc::VS_COMMIT_CONTROL, cam.vs_if as u16, &mut got) < 0 {
                return Err("Commit failed.".into());
            }
            let fps = if ctrl.interval > 0 { 10_000_000 / ctrl.interval } else { 0 };
            let name = if fmt.kind == uvc::Kind::Mjpeg { "MJPEG" } else { "YUY2" };
            log(&alloc::format!("[cam] {} {}x{} @ {} fps, frame {} B, payload {} B",
                name, frame.width, frame.height, fps, ctrl.max_frame, ctrl.max_payload));

            let exact = (fmt.kind == uvc::Kind::Yuyv).then_some(frame.width as usize * frame.height as usize * 2);
            let mut dec = uvc::Decoder::new((ctrl.max_frame as usize).max(exact.unwrap_or(0)).max(64 * 1024), exact);
            let (sid, bulk_td, how) = if cam.is_bulk() {
                let maxp = cam.bulk_maxp().max(1);
                let packets = ctrl.max_payload.div_ceil(maxp).clamp(1, 32);
                let td = (maxp * packets) as usize;
                dec.bulk_max_payload = (ctrl.max_payload as usize).max(1);
                (npk_sys::usb_stream_open(h, cam.ep, td as u32, BULK_TDS), td, alloc::format!("bulk {} B", td))
            } else {
                let alt = cam.alt_for(ctrl.max_payload)
                    .ok_or_else(|| alloc::format!("No setting carries {} B per interval.", ctrl.max_payload))?;
                let r = npk_sys::usb_set_alt(h, cam.vs_if, alt.alt);
                if r < 0 { return Err(alloc::format!("Setting alt {} failed ({}).", alt.alt, r)); }
                (npk_sys::usb_stream_open(h, cam.ep, 0, ISO_TDS), 0, alloc::format!("alt {} ({} B)", alt.alt, alt.bpi()))
            };
            if sid < 0 { return Err(alloc::format!("Starting the stream failed ({}).", sid)); }
            let desc = alloc::format!("{:04x}:{:04x}  {} {}x{} @ {} fps  {}", self.vid, self.pid, name,
                frame.width, frame.height, fps, how);
            self.stream = Some(Stream {
                sid, bulk_td, width: frame.width as usize, height: frame.height as usize,
                kind: fmt.kind, dec, desc, started: host::ticks_ms(),
            });
            Ok(())
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

    fn stats_line(s: &uvc::Stats, fps: u64, decode_ms: u64) -> String {
        alloc::format!("{} fps  decode {} ms  frames {} (bad {})  packets {} (empty {}, lost {}, errors {})  err bit {}  invalid {}",
            fps, decode_ms, s.frames, s.bad_frames, s.packets, s.empty, s.missed, s.errors, s.err_bit, s.invalid)
    }

    /// Parse `stream_read` records into the decoder.
    fn feed(st: &mut Stream, data: &[u8], out: &mut Vec<uvc::Done>) {
        let mut p = 0;
        while p + 4 <= data.len() {
            let len = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
            let status = data[p + 2];
            p += 4;
            if p + len > data.len() { break; }
            let pkt = &data[p..p + len];
            if st.bulk_td > 0 {
                st.dec.bulk(pkt, st.bulk_td, status == 0, out);
            } else {
                st.dec.isoc(pkt, status == 0, out);
            }
            p += len;
        }
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
                if m != c.mode || c.stream.is_none() {
                    c.mode = m;
                    commit(Some(c), "Switching…", "");
                    if let Err(e) = c.start() {
                        log(&alloc::format!("[cam] {}", e));
                        commit(Some(c), &e, "");
                    }
                }
            }
        }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn _start() {
        commit(None, "Opening the camera…", "");
        let mut c = match claim() {
            Ok(c) => c,
            Err(e) => {
                log(&alloc::format!("[cam] {}", e));
                commit(None, &e, "");
                wait_close();
                return;
            }
        };
        if let Err(e) = c.start() {
            log(&alloc::format!("[cam] {}", e));
            commit(Some(&c), &e, "");
        }
        let mut buf = alloc::vec![0u8; READ_BUF];
        let mut bgra: Vec<u8> = Vec::new();
        let mut ev = alloc::vec![0u8; 4096];
        let mut done: Vec<uvc::Done> = Vec::new();
        let mut last_tick = host::ticks_ms();
        let mut frames_at_tick = 0u64;
        let mut logged = 0u32;
        loop {
            match events(&mut c, &mut ev) {
                Step::Go => {}
                Step::Close => {
                    c.stop();
                    npk_sys::usb_release(c.handle);
                    host::close_widget();
                    return;
                }
                Step::Gone => {
                    c.stop();
                    npk_sys::usb_release(c.handle);
                    return;
                }
            }
            let Some(st) = c.stream.as_mut() else {
                host::sleep_ms(20);
                continue;
            };
            let n = npk_sys::usb_stream_read(c.handle, st.sid, &mut buf);
            if n < 0 {
                log(&alloc::format!("[cam] stream ended ({})", n));
                let desc = st.desc.clone();
                c.stream = None;
                commit(Some(&c), &desc, "The camera is gone.");
                wait_close();
                return;
            }
            if n > 0 {
                feed(st, &buf[..n as usize], &mut done);
                // Only the newest good frame is drawn; older ones would
                // only make the picture lag.
                if let Some(f) = done.iter().rev().find(|f| !f.error) {
                    let (w, h) = (st.width, st.height);
                    match st.kind {
                        uvc::Kind::Mjpeg => {
                            let t0 = host::ticks_ms();
                            // The smallest scale that still fills the
                            // canvas: a preview needs no full frame.
                            let scale = host::canvas_rect(CANVAS_ID).map_or(1, |(_, _, rw, rh)| {
                                [8u32, 4, 2].into_iter().find(|&s| {
                                    (w as i32) / s as i32 >= rw.max(1) && (h as i32) / s as i32 >= rh.max(1)
                                }).unwrap_or(1)
                            });
                            match c.jpeg.decode_scaled(&f.data, scale, &mut bgra) {
                                Ok((dw, dh)) => {
                                    c.decode_ms += host::ticks_ms() - t0;
                                    c.decoded += 1;
                                    host::canvas_commit(CANVAS_ID, &bgra, dw, dh);
                                }
                                Err(e) => {
                                    c.jpeg_errors += 1;
                                    if logged < 5 {
                                        logged += 1;
                                        log(&alloc::format!("[cam] jpeg: {:?} ({} B)", e, f.data.len()));
                                    }
                                }
                            }
                        }
                        _ => {
                            bgra.resize(w * h * 4, 0);
                            uvc::yuyv_to_bgra(&f.data, w, h, &mut bgra);
                            host::canvas_commit(CANVAS_ID, &bgra, w as u32, h as u32);
                        }
                    }
                }
                for f in done.iter().filter(|f| f.error) {
                    if logged < 5 {
                        logged += 1;
                        log(&alloc::format!("[cam] bad frame: {} B", f.data.len()));
                    }
                }
                done.clear();
            }
            let now = host::ticks_ms();
            if now - last_tick >= 1000 {
                if let Some(st) = c.stream.as_ref() {
                    let fps = (st.dec.stats.frames.saturating_sub(frames_at_tick)) * 1000 / (now - last_tick);
                    frames_at_tick = st.dec.stats.frames;
                    let mut line = stats_line(&st.dec.stats, fps, c.avg_ms());
                    if c.jpeg_errors > 0 { line.push_str(&alloc::format!("  jpeg errors {}", c.jpeg_errors)); }
                    let desc = st.desc.clone();
                    commit(Some(&c), &desc, &line);
                }
                last_tick = now;
            }
            if n == 0 { host::sleep_ms(2); }
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
