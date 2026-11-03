//! UVC after Linux uvcvideo: the descriptors (`uvc_driver.c`), probe and
//! commit (`uvc_video.c` `uvc_probe_video`, `uvc_commit_video`), the
//! alternate setting a stream needs (`uvc_video_start_transfer`) and the
//! payload decoder (`uvc_video_decode_start/_data/_end`, the bulk variant
//! `uvc_video_decode_bulk`). No host calls here, so it runs in host tests.

use alloc::vec::Vec;

const USB_CLASS_VIDEO: u8 = 0x0E;
const SC_VIDEOCONTROL: u8 = 0x01;
const SC_VIDEOSTREAMING: u8 = 0x02;
const DT_INTERFACE: u8 = 0x04;
const DT_ENDPOINT: u8 = 0x05;
const CS_INTERFACE: u8 = 0x24;

const VC_HEADER: u8 = 0x01;
const VS_INPUT_HEADER: u8 = 0x01;
const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
const VS_FRAME_UNCOMPRESSED: u8 = 0x05;
const VS_FORMAT_MJPEG: u8 = 0x06;
const VS_FRAME_MJPEG: u8 = 0x07;

pub const SET_CUR: u8 = 0x01;
pub const GET_CUR: u8 = 0x81;
pub const VS_PROBE_CONTROL: u16 = 0x01;
pub const VS_COMMIT_CONTROL: u16 = 0x02;

const STREAM_FID: u8 = 0x01;
const STREAM_EOF: u8 = 0x02;
const STREAM_ERR: u8 = 0x40;

/// YUY2's GUID (`UVC_GUID_FORMAT_YUY2`).
const GUID_YUY2: [u8; 16] = [
    b'Y', b'U', b'Y', b'2', 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Yuyv,
    Mjpeg,
    /// An uncompressed format other than YUY2.
    Other,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub index: u8,
    pub width: u16,
    pub height: u16,
    pub max_buffer: u32,
    pub default_interval: u32,
}

#[derive(Clone, Debug)]
pub struct Format {
    pub index: u8,
    pub kind: Kind,
    pub frames: Vec<Frame>,
}

#[derive(Clone, Copy, Debug)]
pub struct AltEp {
    pub alt: u8,
    pub addr: u8,
    pub attr: u8,
    pub w_max: u16,
}

impl AltEp {
    /// `uvc_endpoint_max_bpi` at high and full speed: bytes per interval.
    pub fn bpi(&self) -> u32 {
        (self.w_max & 0x7FF) as u32 * (((self.w_max >> 11) & 3) as u32 + 1)
    }
    pub fn is_bulk(&self) -> bool { self.attr & 3 == 2 }
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub uvc_version: u16,
    pub vc_if: u8,
    pub vs_if: u8,
    pub ep: u8,
    pub formats: Vec<Format>,
    /// The streaming interface's settings with the stream endpoint.
    pub alts: Vec<AltEp>,
}

/// Parse the configuration descriptor: the first VideoControl and
/// VideoStreaming interface pair.
pub fn parse(cfg: &[u8]) -> Option<Camera> {
    let mut cam = Camera { uvc_version: 0x0100, vc_if: 0xFF, vs_if: 0xFF, ep: 0, formats: Vec::new(), alts: Vec::new() };
    // (ifnum, alt, subclass) of the interface the descriptors belong to.
    let mut cur: Option<(u8, u8, u8)> = None;
    let mut pos = 0;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        match d[1] {
            DT_INTERFACE if len >= 9 => {
                cur = if d[5] == USB_CLASS_VIDEO { Some((d[2], d[3], d[6])) } else { None };
                if let Some((n, 0, sc)) = cur {
                    if sc == SC_VIDEOCONTROL && cam.vc_if == 0xFF { cam.vc_if = n; }
                    if sc == SC_VIDEOSTREAMING && cam.vs_if == 0xFF { cam.vs_if = n; }
                }
            }
            CS_INTERFACE if len >= 3 => match cur {
                Some((n, 0, SC_VIDEOCONTROL)) if n == cam.vc_if && d[2] == VC_HEADER && len >= 5 => {
                    cam.uvc_version = u16::from_le_bytes([d[3], d[4]]);
                }
                Some((n, 0, SC_VIDEOSTREAMING)) if n == cam.vs_if => match d[2] {
                    VS_INPUT_HEADER if len >= 7 => cam.ep = d[6],
                    VS_FORMAT_UNCOMPRESSED if len >= 21 => {
                        let kind = if d[5..21] == GUID_YUY2 { Kind::Yuyv } else { Kind::Other };
                        cam.formats.push(Format { index: d[3], kind, frames: Vec::new() });
                    }
                    VS_FORMAT_MJPEG if len >= 5 => {
                        cam.formats.push(Format { index: d[3], kind: Kind::Mjpeg, frames: Vec::new() });
                    }
                    VS_FRAME_UNCOMPRESSED | VS_FRAME_MJPEG if len >= 26 => {
                        if let Some(f) = cam.formats.last_mut() {
                            f.frames.push(Frame {
                                index: d[3],
                                width: u16::from_le_bytes([d[5], d[6]]),
                                height: u16::from_le_bytes([d[7], d[8]]),
                                max_buffer: u32::from_le_bytes([d[17], d[18], d[19], d[20]]),
                                default_interval: u32::from_le_bytes([d[21], d[22], d[23], d[24]]),
                            });
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            DT_ENDPOINT if len >= 7 => {
                // The endpoint may come before the input header that names
                // it; filtered below.
                if let Some((n, alt, SC_VIDEOSTREAMING)) = cur {
                    if n == cam.vs_if {
                        cam.alts.push(AltEp { alt, addr: d[2], attr: d[3], w_max: u16::from_le_bytes([d[4], d[5]]) });
                    }
                }
            }
            _ => {}
        }
        pos += len;
    }
    if cam.vc_if == 0xFF || cam.vs_if == 0xFF || cam.ep == 0 { return None; }
    let ep = cam.ep;
    cam.alts.retain(|a| a.addr == ep);
    Some(cam)
}

impl Camera {
    /// Size of the probe and commit controls (`uvc_video_ctrl_size`).
    pub fn ctrl_size(&self) -> usize {
        if self.uvc_version < 0x0110 { 26 } else if self.uvc_version < 0x0150 { 34 } else { 48 }
    }

    /// `uvc_video_start_transfer`: the smallest isochronous setting whose
    /// bytes per interval carry `bandwidth`; None for a bulk endpoint.
    pub fn alt_for(&self, bandwidth: u32) -> Option<AltEp> {
        let need = bandwidth.max(1);
        self.alts.iter().filter(|a| !a.is_bulk() && a.bpi() >= need).min_by_key(|a| a.bpi()).copied()
    }

    pub fn is_bulk(&self) -> bool {
        self.alts.iter().any(|a| a.alt == 0 && a.is_bulk())
    }

    /// The streaming endpoint's wMaxPacketSize in its default setting.
    pub fn bulk_maxp(&self) -> u32 {
        self.alts.iter().find(|a| a.alt == 0).map_or(512, |a| (a.w_max & 0x7FF) as u32)
    }
}

/// The probe/commit control (`struct uvc_streaming_control`), the fields
/// we set or read.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ctrl {
    pub hint: u16,
    pub format: u8,
    pub frame: u8,
    pub interval: u32,
    pub max_frame: u32,
    pub max_payload: u32,
}

impl Ctrl {
    pub fn encode(&self, size: usize) -> Vec<u8> {
        let mut v = alloc::vec![0u8; size];
        v[0..2].copy_from_slice(&self.hint.to_le_bytes());
        v[2] = self.format;
        v[3] = self.frame;
        v[4..8].copy_from_slice(&self.interval.to_le_bytes());
        v
    }

    /// `uvc_get_video_ctrl` + `uvc_fixup_video_ctrl`.
    pub fn decode(b: &[u8], frame_buffer: u32) -> Option<Ctrl> {
        if b.len() < 26 { return None; }
        let mut c = Ctrl {
            hint: u16::from_le_bytes([b[0], b[1]]),
            format: b[2],
            frame: b[3],
            interval: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            max_frame: u32::from_le_bytes([b[18], b[19], b[20], b[21]]),
            max_payload: u32::from_le_bytes([b[22], b[23], b[24], b[25]]),
        };
        if c.max_payload & 0xFFFF_0000 == 0xFFFF_0000 { c.max_payload &= 0xFFFF; }
        if c.max_frame == 0 { c.max_frame = frame_buffer; }
        Some(c)
    }
}

/// Per-stream counters, shown by the viewer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub packets: u64,
    pub empty: u64,
    pub invalid: u64,
    pub missed: u64,
    pub errors: u64,
    pub frames: u64,
    pub bad_frames: u64,
    pub err_bit: u64,
}

/// A finished frame: its bytes and whether it came out whole.
pub struct Done {
    pub data: Vec<u8>,
    pub error: bool,
}

/// The payload decoder: one frame buffer being filled.
pub struct Decoder {
    buf: Vec<u8>,
    cap: usize,
    active: bool,
    error: bool,
    last_fid: Option<u8>,
    /// Uncompressed: the exact size a frame must have.
    exact: Option<usize>,
    bulk_header: Option<u8>,
    bulk_skip: bool,
    bulk_payload: usize,
    pub bulk_max_payload: usize,
    pub stats: Stats,
}

impl Decoder {
    pub fn new(max_frame: usize, exact: Option<usize>) -> Self {
        Decoder {
            buf: Vec::with_capacity(max_frame), cap: max_frame, active: false, error: false, last_fid: None,
            exact, bulk_header: None, bulk_skip: false, bulk_payload: 0, bulk_max_payload: 0,
            stats: Stats::default(),
        }
    }

    fn finish(&mut self, out: &mut Vec<Done>) {
        // `uvc_video_validate_buffer`.
        if let Some(n) = self.exact {
            if self.buf.len() != n { self.error = true; }
        }
        self.stats.frames += 1;
        if self.error { self.stats.bad_frames += 1; }
        let data = core::mem::replace(&mut self.buf, Vec::with_capacity(self.cap));
        out.push(Done { data, error: self.error });
        self.active = false;
        self.error = false;
    }

    /// `uvc_video_decode_start`: the header's length, or None to drop.
    /// A frame finished by a toggled FID goes to `out` first.
    fn start(&mut self, d: &[u8], out: &mut Vec<Done>) -> Option<usize> {
        if d.len() < 2 || d[0] < 2 || d[0] as usize > d.len() {
            self.stats.invalid += 1;
            return None;
        }
        let fid = d[1] & STREAM_FID;
        let err = d[1] & STREAM_ERR != 0;
        if err {
            self.stats.err_bit += 1;
            self.error = true;
        }
        if !self.active {
            // Out of sync until the FID toggles.
            if Some(fid) == self.last_fid { return None; }
            self.active = true;
        } else if Some(fid) != self.last_fid && !self.buf.is_empty() {
            self.finish(out);
            self.active = true;
            self.error = err;
        }
        self.last_fid = Some(fid);
        Some(d[0] as usize)
    }

    /// `uvc_video_decode_data`.
    fn data(&mut self, d: &[u8]) {
        let room = self.cap - self.buf.len().min(self.cap);
        if d.len() > room {
            self.buf.extend_from_slice(&d[..room]);
            self.error = true;
        } else {
            self.buf.extend_from_slice(d);
        }
    }

    /// `uvc_video_decode_end`.
    fn end(&mut self, flags: u8, out: &mut Vec<Done>) {
        if flags & STREAM_EOF != 0 && !self.buf.is_empty() {
            self.finish(out);
        }
    }

    /// One isochronous packet (`uvc_video_decode_isoc`).
    pub fn isoc(&mut self, pkt: &[u8], ok: bool, out: &mut Vec<Done>) {
        self.stats.packets += 1;
        if !ok {
            self.stats.missed += 1;
            self.error = true;
            return;
        }
        if pkt.is_empty() {
            self.stats.empty += 1;
            return;
        }
        let Some(h) = self.start(pkt, out) else { return };
        self.data(&pkt[h..]);
        self.end(pkt[1], out);
    }

    /// One bulk TD of `td_len` requested bytes (`uvc_video_decode_bulk`).
    pub fn bulk(&mut self, td: &[u8], td_len: usize, ok: bool, out: &mut Vec<Done>) {
        self.stats.packets += 1;
        if !ok {
            self.stats.errors += 1;
            self.bulk_skip = true;
        }
        let mut mem = td;
        self.bulk_payload += td.len();
        if self.bulk_header.is_none() && !self.bulk_skip {
            match self.start(mem, out) {
                Some(h) => {
                    self.bulk_header = Some(mem[1]);
                    mem = &mem[h..];
                }
                None => self.bulk_skip = true,
            }
        }
        if !self.bulk_skip && self.bulk_header.is_some() {
            self.data(mem);
        }
        if td.len() < td_len || self.bulk_payload >= self.bulk_max_payload {
            if !self.bulk_skip {
                if let Some(flags) = self.bulk_header { self.end(flags, out); }
            }
            self.bulk_header = None;
            self.bulk_skip = false;
            self.bulk_payload = 0;
        }
    }
}

/// YUY2 (BT.601, limited range) to BGRA.
pub fn yuyv_to_bgra(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
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
