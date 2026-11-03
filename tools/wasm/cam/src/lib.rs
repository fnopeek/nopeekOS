//! cam — camera viewer for nopeekOS.
//!
//! Claims the first USB camera (`npk_usb_*`, right CAMERA), negotiates a
//! format with probe and commit, picks the streaming setting the format
//! needs and shows the frames in a canvas. The footer counts what the
//! stream did: frames, bad frames, packets lost, payloads with the UVC
//! error bit. That makes cam the test of the USB path before any other
//! user of the camera exists.
//!
//! YUY2 is shown as is; MJPEG needs the SDK's JPEG decoder (not yet).

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

    fn log(msg: &str) { host::log_serial(msg); }

    /// A class request to interface `index` (`__uvc_query_ctrl`).
    fn query(h: i32, req: u8, cs: u16, index: u16, data: &mut [u8]) -> i32 {
        let bm = if req & 0x80 != 0 { 0xA1 } else { 0x21 };
        let len = data.len() as u16;
        let setup = [bm, req, 0, cs as u8, index as u8, (index >> 8) as u8, len as u8, (len >> 8) as u8];
        npk_sys::usb_control(h, &setup, data)
    }

    struct Stream {
        handle: i32,
        sid: i32,
        bulk_td: usize,
        width: usize,
        height: usize,
        kind: uvc::Kind,
        dec: uvc::Decoder,
        desc: String,
    }

    /// Claim, probe, commit, start. Err is the line the window shows.
    fn open() -> Result<Stream, String> {
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

        let Some(fmt) = cam.formats.iter().find(|f| f.kind == uvc::Kind::Yuyv) else {
            npk_sys::usb_release(h);
            return Err("The camera has no YUY2 format; MJPEG is not decoded yet.".into());
        };
        let Some(frame) = fmt.frames.iter().filter(|f| f.width <= 640).max_by_key(|f| f.width as u32 * f.height as u32)
            .or_else(|| fmt.frames.first()) else {
            npk_sys::usb_release(h);
            return Err("YUY2 without frame sizes.".into());
        };
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
                npk_sys::usb_release(h);
                return Err("Probe failed.".into());
            }
            ctrl = uvc::Ctrl::decode(&got, frame.max_buffer);
            probe.copy_from_slice(&got);
            if cam.is_bulk() || ctrl.is_some_and(|c| cam.alt_for(c.max_payload).is_some()) { break; }
        }
        let Some(ctrl) = ctrl else {
            npk_sys::usb_release(h);
            return Err("Probe answer too short.".into());
        };
        if query(h, uvc::SET_CUR, uvc::VS_COMMIT_CONTROL, cam.vs_if as u16, &mut got) < 0 {
            npk_sys::usb_release(h);
            return Err("Commit failed.".into());
        }
        let fps = if ctrl.interval > 0 { 10_000_000 / ctrl.interval } else { 0 };
        log(&alloc::format!("[cam] YUY2 {}x{} @ {} fps, frame {} B, payload {} B",
            frame.width, frame.height, fps, ctrl.max_frame, ctrl.max_payload));

        let exact = frame.width as usize * frame.height as usize * 2;
        let mut dec = uvc::Decoder::new((ctrl.max_frame as usize).max(exact), Some(exact));
        let (sid, bulk_td, how) = if cam.is_bulk() {
            let maxp = cam.bulk_maxp().max(1);
            let packets = ctrl.max_payload.div_ceil(maxp).clamp(1, 32);
            let td = (maxp * packets) as usize;
            dec.bulk_max_payload = (ctrl.max_payload as usize).max(1);
            (npk_sys::usb_stream_open(h, cam.ep, td as u32, BULK_TDS), td, alloc::format!("bulk {} B", td))
        } else {
            let Some(alt) = cam.alt_for(ctrl.max_payload) else {
                npk_sys::usb_release(h);
                return Err(alloc::format!("No setting carries {} B per interval.", ctrl.max_payload));
            };
            let r = npk_sys::usb_set_alt(h, cam.vs_if, alt.alt);
            if r < 0 {
                npk_sys::usb_release(h);
                return Err(alloc::format!("Setting alt {} failed ({}).", alt.alt, r));
            }
            (npk_sys::usb_stream_open(h, cam.ep, 0, ISO_TDS), 0, alloc::format!("alt {} ({} B)", alt.alt, alt.bpi()))
        };
        if sid < 0 {
            npk_sys::usb_release(h);
            return Err(alloc::format!("Starting the stream failed ({}).", sid));
        }
        let desc = alloc::format!("{:04x}:{:04x}  YUY2 {}x{} @ {} fps  {}", vid, pid, frame.width, frame.height, fps, how);
        Ok(Stream {
            handle: h, sid, bulk_td, width: frame.width as usize, height: frame.height as usize,
            kind: uvc::Kind::Yuyv, dec, desc,
        })
    }

    fn render(line1: &str, line2: &str) -> Widget {
        Widget::Column {
            children: alloc::vec![
                Widget::Canvas {
                    id: CanvasId(CANVAS_ID),
                    width: 640,
                    height: 480,
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

    fn commit(line1: &str, line2: &str) {
        if let Ok(bytes) = wire::encode(&render(line1, line2)) {
            if !host::scene_commit(&bytes) { log("[cam] commit failed"); }
        }
    }

    fn stats_line(s: &uvc::Stats, fps: u64) -> String {
        alloc::format!("{} fps  frames {} (bad {})  packets {} (empty {}, lost {}, errors {})  err bit {}  invalid {}",
            fps, s.frames, s.bad_frames, s.packets, s.empty, s.missed, s.errors, s.err_bit, s.invalid)
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

    #[unsafe(no_mangle)]
    pub extern "C" fn _start() {
        commit("Opening the camera…", "");
        let mut st = match open() {
            Ok(s) => s,
            Err(e) => {
                log(&alloc::format!("[cam] {}", e));
                commit(&e, "");
                wait_close();
                return;
            }
        };
        commit(&st.desc, "waiting for the first frame");
        let mut buf = alloc::vec![0u8; READ_BUF];
        let mut bgra = alloc::vec![0u8; st.width * st.height * 4];
        let mut events = alloc::vec![0u8; 4096];
        let mut done: Vec<uvc::Done> = Vec::new();
        let mut last_tick = host::ticks_ms();
        let mut frames_at_tick = 0u64;
        let mut logged = 0u32;
        loop {
            loop {
                match nopeek_widgets::events::poll(&mut events) {
                    nopeek_widgets::events::Poll::Event(Event::Key(KeyCode::Escape)) => {
                        stop(&st);
                        host::close_widget();
                        return;
                    }
                    nopeek_widgets::events::Poll::Event(_) => {}
                    nopeek_widgets::events::Poll::Empty => break,
                    nopeek_widgets::events::Poll::Gone => { stop(&st); return; }
                }
            }
            let n = npk_sys::usb_stream_read(st.handle, st.sid, &mut buf);
            if n < 0 {
                log(&alloc::format!("[cam] stream ended ({})", n));
                commit(&st.desc, "The camera is gone.");
                wait_close();
                return;
            }
            if n > 0 {
                let data = &buf[..n as usize];
                feed_all(&mut st, data, &mut done);
                if let Some(f) = done.iter().rev().find(|f| !f.error) {
                    if st.kind == uvc::Kind::Yuyv {
                        uvc::yuyv_to_bgra(&f.data, st.width, st.height, &mut bgra);
                        host::canvas_commit(CANVAS_ID, &bgra, st.width as u32, st.height as u32);
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
                let fps = (st.dec.stats.frames - frames_at_tick) * 1000 / (now - last_tick);
                frames_at_tick = st.dec.stats.frames;
                last_tick = now;
                commit(&st.desc, &stats_line(&st.dec.stats, fps));
            }
            if n == 0 { host::sleep_ms(2); }
        }
    }

    fn feed_all(st: &mut Stream, data: &[u8], out: &mut Vec<uvc::Done>) { feed(st, data, out); }

    fn stop(st: &Stream) {
        npk_sys::usb_stream_close(st.handle, st.sid);
        npk_sys::usb_release(st.handle);
        log(&alloc::format!("[cam] stopped: {}", stats_line(&st.dec.stats, 0)));
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
