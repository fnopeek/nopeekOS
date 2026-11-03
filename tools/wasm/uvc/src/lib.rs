//! uvc: the USB video class driver, after Linux `uvcvideo`.
//!
//! The one owner of the camera. It waits until a reader of the kernel's
//! camera feed exists (`npk_camera_wanted`), claims the active camera
//! (`npk_usb_*`, right CAMERA), negotiates the size the largest reader
//! asked for with probe and commit, streams, and hands every good frame
//! to the feed as the camera sent it (MJPEG, or YUY2 for small sizes).
//! When the last reader is gone it stops the stream and lets the camera
//! go, so the camera is on only while someone watches.

#![no_std]

extern crate alloc;

pub mod uvc;

#[cfg(target_arch = "wasm32")]
mod service {
    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::uvc;

    #[unsafe(link_section = ".npk.caps")]
    #[used]
    static NPK_CAPS: [u8; 2] = [0, nopeek_widgets::caps::ext::CAMERA];

    #[global_allocator]
    static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! { log("[uvc] panic"); core::arch::wasm32::unreachable() }

    const CLASS_VIDEO: u8 = 0x0E;
    /// Buffers per stream: the kernel queues them again as they come back.
    /// 1024 isochronous intervals are 128 ms at high speed; 64 bulk TDs of
    /// 16 KiB a megabyte.
    const ISO_TDS: u32 = 1024;
    const BULK_TDS: u32 = 64;
    const READ_BUF: usize = 512 * 1024;
    /// What a reader gets when it asks for no size.
    const DEFAULT_SIZE: (u16, u16) = (1280, 720);
    /// How often an idle driver looks for readers.
    const IDLE_MS: i32 = 200;

    fn log(msg: &str) { npk_sys::print(msg.as_bytes()); npk_sys::print(b"\n"); }

    /// A class request to interface `index` (`__uvc_query_ctrl`).
    fn query(h: i32, req: u8, cs: u16, index: u16, data: &mut [u8]) -> i32 {
        let bm = if req & 0x80 != 0 { 0xA1 } else { 0x21 };
        let len = data.len() as u16;
        let setup = [bm, req, 0, cs as u8, index as u8, (index >> 8) as u8, len as u8, (len >> 8) as u8];
        npk_sys::usb_control(h, &setup, data)
    }

    struct Stream {
        sid: i32,
        bulk_td: usize,
        width: u16,
        height: u16,
        kind: uvc::Kind,
        dec: uvc::Decoder,
    }

    struct Cam {
        handle: i32,
        cam: uvc::Camera,
        vid: u16,
        pid: u16,
        size: (u16, u16),
        stream: Option<Stream>,
    }

    /// Claim the active camera. Err is the line to log.
    fn claim() -> Result<Cam, String> {
        let mut buf = alloc::vec![0u8; 64 * 1024];
        let h = npk_sys::usb_claim(CLASS_VIDEO, &mut buf);
        if h == -2 { return Err("no camera".into()); }
        if h == -3 { return Err("no CAMERA right".into()); }
        if h < 0 { return Err(alloc::format!("claiming the camera failed ({})", h)); }
        let total = u16::from_le_bytes([buf[18 + 2], buf[18 + 3]]) as usize;
        let cfg = &buf[18..(18 + total).min(buf.len())];
        let vid = u16::from_le_bytes([buf[8], buf[9]]);
        let pid = u16::from_le_bytes([buf[10], buf[11]]);
        let Some(cam) = uvc::parse(cfg) else {
            npk_sys::usb_release(h);
            return Err("not a UVC camera".into());
        };
        log(&alloc::format!("[uvc] {:04x}:{:04x} UVC {:x}.{:02x}, VC if {}, VS if {}, ep {:#04x}, {} formats, {}",
            vid, pid, cam.uvc_version >> 8, cam.uvc_version & 0xFF, cam.vc_if, cam.vs_if, cam.ep,
            cam.formats.len(), if cam.is_bulk() { "bulk" } else { "isochronous" }));
        Ok(Cam { handle: h, cam, vid, pid, size: (0, 0), stream: None })
    }

    /// The format and frame for `w` x `h`: YUY2 up to 640x480 and MJPEG
    /// above (a high-speed camera carries raw only for small sizes), else
    /// any kind at that size, else the largest frame not above it.
    fn pick(cam: &uvc::Camera, w: u16, h: u16) -> Option<(&uvc::Format, &uvc::Frame)> {
        let kind = if w as u32 * h as u32 <= 640 * 480 { uvc::Kind::Yuyv } else { uvc::Kind::Mjpeg };
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
        /// `uvc_video_stop_streaming`: setting 0, or for a bulk camera with
        /// one setting CLEAR_FEATURE(ENDPOINT_HALT) on its endpoint.
        fn stop(&mut self) {
            if let Some(st) = self.stream.take() {
                npk_sys::usb_stream_close(self.handle, st.sid);
                let s = &st.dec.stats;
                log(&alloc::format!("[uvc] stopped {}x{}: frames {} (bad {}), packets {} (lost {}, errors {})",
                    st.width, st.height, s.frames, s.bad_frames, s.packets, s.missed, s.errors));
            }
            if self.cam.alts.len() > 1 {
                npk_sys::usb_set_alt(self.handle, self.cam.vs_if, 0);
            } else {
                let setup = [0x02, 0x01, 0, 0, self.cam.ep, 0, 0, 0];
                npk_sys::usb_control(self.handle, &setup, &mut []);
            }
        }

        /// Probe, commit, the setting, the stream (`uvc_video_enable`).
        fn start(&mut self, w: u16, h: u16) -> Result<(), String> {
            self.stop();
            self.size = (w, h);
            let hd = self.handle;
            let cam = &self.cam;
            let (fmt, frame) = pick(cam, w, h).ok_or_else(|| String::from("no YUY2 or MJPEG format"))?;
            // Some cameras misbehave unless the streaming interface sees
            // SET_INTERFACE 0 before any video request (`uvc_video_init`).
            if cam.alts.len() > 1 { npk_sys::usb_set_alt(hd, cam.vs_if, 0); }
            let size = cam.ctrl_size();
            let want = uvc::Ctrl { hint: 1, format: fmt.index, frame: frame.index, interval: frame.default_interval, ..Default::default() };
            let mut probe = want.encode(size);
            let mut got = alloc::vec![0u8; size];
            let mut ctrl = None;
            for _ in 0..2 {
                if query(hd, uvc::SET_CUR, uvc::VS_PROBE_CONTROL, cam.vs_if as u16, &mut probe) < 0
                    || query(hd, uvc::GET_CUR, uvc::VS_PROBE_CONTROL, cam.vs_if as u16, &mut got) < 0
                {
                    return Err("probe failed".into());
                }
                ctrl = uvc::Ctrl::decode(&got, frame.max_buffer);
                probe.copy_from_slice(&got);
                if cam.is_bulk() || ctrl.is_some_and(|c| cam.alt_for(c.max_payload).is_some()) { break; }
            }
            let ctrl = ctrl.ok_or_else(|| String::from("probe answer too short"))?;
            if query(hd, uvc::SET_CUR, uvc::VS_COMMIT_CONTROL, cam.vs_if as u16, &mut got) < 0 {
                return Err("commit failed".into());
            }
            let fps = if ctrl.interval > 0 { 10_000_000 / ctrl.interval } else { 0 };
            let name = if fmt.kind == uvc::Kind::Mjpeg { "MJPEG" } else { "YUY2" };
            let exact = (fmt.kind == uvc::Kind::Yuyv).then_some(frame.width as usize * frame.height as usize * 2);
            let mut dec = uvc::Decoder::new((ctrl.max_frame as usize).max(exact.unwrap_or(0)).max(64 * 1024), exact);
            let (sid, bulk_td, how) = if cam.is_bulk() {
                let maxp = cam.bulk_maxp().max(1);
                let packets = ctrl.max_payload.div_ceil(maxp).clamp(1, 32);
                let td = (maxp * packets) as usize;
                dec.bulk_max_payload = (ctrl.max_payload as usize).max(1);
                (npk_sys::usb_stream_open(hd, cam.ep, td as u32, BULK_TDS), td, alloc::format!("bulk {} B", td))
            } else {
                let alt = cam.alt_for(ctrl.max_payload)
                    .ok_or_else(|| alloc::format!("no setting carries {} B per interval", ctrl.max_payload))?;
                let r = npk_sys::usb_set_alt(hd, cam.vs_if, alt.alt);
                if r < 0 { return Err(alloc::format!("setting alt {} failed ({})", alt.alt, r)); }
                (npk_sys::usb_stream_open(hd, cam.ep, 0, ISO_TDS), 0, alloc::format!("alt {} ({} B)", alt.alt, alt.bpi()))
            };
            if sid < 0 { return Err(alloc::format!("starting the stream failed ({})", sid)); }
            log(&alloc::format!("[uvc] {:04x}:{:04x} {} {}x{} @ {} fps, {}", self.vid, self.pid, name,
                frame.width, frame.height, fps, how));
            self.stream = Some(Stream { sid, bulk_td, width: frame.width, height: frame.height, kind: fmt.kind, dec });
            Ok(())
        }

        fn release(mut self) {
            self.stop();
            npk_sys::usb_release(self.handle);
        }
    }

    /// `stream_read` records into the decoder.
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
        log(concat!("[uvc] ", env!("CARGO_PKG_VERSION"), " - waiting for a camera reader"));
        let mut cam: Option<Cam> = None;
        let mut buf = alloc::vec![0u8; READ_BUF];
        let mut done: Vec<uvc::Done> = Vec::new();
        let mut failed_at: Option<(u16, u16)> = None;
        loop {
            let Some((readers, w, h)) = npk_sys::camera_wanted() else {
                log("[uvc] no CAMERA right - exit");
                return;
            };
            if readers == 0 {
                if let Some(c) = cam.take() {
                    c.release();
                    log("[uvc] no reader left, camera released");
                }
                failed_at = None;
                npk_sys::sleep(IDLE_MS);
                continue;
            }
            let size = if w == 0 || h == 0 { DEFAULT_SIZE } else { (w, h) };
            if cam.is_none() {
                match claim() {
                    Ok(c) => cam = Some(c),
                    Err(e) => {
                        if failed_at != Some(size) { log(&alloc::format!("[uvc] {}", e)); }
                        failed_at = Some(size);
                        npk_sys::sleep(1000);
                        continue;
                    }
                }
            }
            let Some(c) = cam.as_mut() else { continue };
            if c.stream.is_none() || c.size != size {
                if failed_at == Some(size) {
                    npk_sys::sleep(IDLE_MS);
                    continue;
                }
                if let Err(e) = c.start(size.0, size.1) {
                    log(&alloc::format!("[uvc] {}", e));
                    failed_at = Some(size);
                    continue;
                }
                failed_at = None;
            }
            let Some(st) = c.stream.as_mut() else { continue };
            let n = npk_sys::usb_stream_read(c.handle, st.sid, &mut buf);
            if n < 0 {
                log(&alloc::format!("[uvc] stream ended ({}), camera gone", n));
                if let Some(c) = cam.take() { npk_sys::usb_release(c.handle); }
                continue;
            }
            if n > 0 {
                feed(st, &buf[..n as usize], &mut done);
                // Only the newest good frame goes out; readers take the
                // newest anyway.
                if let Some(f) = done.iter().rev().find(|f| !f.error) {
                    let format = if st.kind == uvc::Kind::Mjpeg { npk_sys::CAMERA_MJPEG } else { npk_sys::CAMERA_YUYV };
                    npk_sys::camera_push(&f.data, st.width, st.height, format);
                }
                done.clear();
            } else {
                npk_sys::sleep(2);
            }
        }
    }
}
