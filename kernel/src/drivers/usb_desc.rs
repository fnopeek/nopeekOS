//! USB devices and their descriptors in words, for the `usb` intent
//! (usbutils `lsusb`, `lsusb -v`).
//!
//! Reads what enumeration kept (`xhci::devices`); sends nothing to a
//! device. Class-specific parts follow UVC 1.5 (video) and UAC 1/2 (audio),
//! the ones a camera or a microphone brings.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::kprintln;
use crate::xhci::{self, UsbListed};

const DT_DEVICE: u8 = 0x01;
const DT_CONFIG: u8 = 0x02;
const DT_INTERFACE: u8 = 0x04;
const DT_ENDPOINT: u8 = 0x05;
const DT_IAD: u8 = 0x0B;
const DT_HID: u8 = 0x21;
const DT_CS_INTERFACE: u8 = 0x24;
const DT_CS_ENDPOINT: u8 = 0x25;
const DT_SS_EP_COMP: u8 = 0x30;

const CLASS_AUDIO: u8 = 0x01;
const CLASS_VIDEO: u8 = 0x0E;

/// `usb`: one line per device.
pub fn list() {
    let devs = xhci::devices();
    kprintln!();
    if devs.is_empty() {
        kprintln!("  No USB device found.");
        kprintln!();
        return;
    }
    for (i, d) in devs.iter().enumerate() {
        kprintln!("  [{}] {}", i + 1, headline(d));
        let names = names(d);
        let driver = if d.driver.is_empty() { "no driver" } else { d.driver };
        if d.dev.failed.is_none() {
            kprintln!("      {}{}({})", names, if names.is_empty() { "" } else { "  " }, driver);
        }
    }
    kprintln!();
    kprintln!("  usb <n>: descriptors of device n");
    kprintln!();
}

/// `usb <n>`: the whole descriptor tree of the `n`th device of `list`.
pub fn describe(n: usize) {
    let devs = xhci::devices();
    let Some(d) = n.checked_sub(1).and_then(|i| devs.get(i)) else {
        kprintln!("  usb: no device {} ({} listed)", n, devs.len());
        return;
    };
    kprintln!();
    kprintln!("  [{}] {}", n, headline(d));
    let names = names(d);
    if !names.is_empty() { kprintln!("      {}", names); }
    if d.dev.failed.is_some() {
        kprintln!();
        return;
    }
    device(&d.dev.device);
    config(&d.dev.cfg, d.dev.speed);
    kprintln!();
}

/// Where, which, how fast, and what it is (or how enumeration failed).
fn headline(d: &UsbListed) -> String {
    let at = format!("{:02x}:{:02x}.{} port {}", d.ctrl.bus, d.ctrl.device, d.ctrl.function, d.dev.port + 1);
    let speed = speed_name(d.dev.speed);
    if let Some((step, cc)) = d.dev.failed {
        return format!("{}  {} speed  enumeration failed at {}: {}", at, speed, step, cc_name(cc));
    }
    let dv = &d.dev.device;
    let vid = u16::from_le_bytes([dv[8], dv[9]]);
    let pid = u16::from_le_bytes([dv[10], dv[11]]);
    format!("{}  {:04x}:{:04x}  {} speed  {}", at, vid, pid, speed, kind(dv, &d.dev.cfg))
}

fn names(d: &UsbListed) -> String {
    match (d.dev.manufacturer.is_empty(), d.dev.product.is_empty()) {
        (false, false) => format!("{} {}", d.dev.manufacturer, d.dev.product),
        (false, true) => d.dev.manufacturer.clone(),
        (true, false) => d.dev.product.clone(),
        (true, true) => String::new(),
    }
}

/// What the device is: its class, or for a device that defers to its
/// interfaces, the classes of their first settings.
fn kind(dv: &[u8], cfg: &[u8]) -> String {
    let (class, sub, proto) = (dv[4], dv[5], dv[6]);
    if !matches!(class, 0x00 | 0xEF | 0xFF) {
        return class_name(class, sub, proto).into();
    }
    let mut kinds: Vec<&str> = Vec::new();
    for d in descriptors(cfg) {
        if d[1] != DT_INTERFACE || d.len() < 9 || d[3] != 0 { continue; }
        let k = match d[5] {
            CLASS_AUDIO => "Audio",
            CLASS_VIDEO => "Video",
            c => class_name(c, d[6], d[7]),
        };
        if !kinds.contains(&k) { kinds.push(k); }
    }
    if kinds.is_empty() { class_name(class, sub, proto).into() } else { kinds.join(" + ") }
}

/// The descriptors in `cfg`, each as its own slice; stops at the first one
/// whose length does not fit.
fn descriptors(cfg: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        out.push(&cfg[pos..pos + len]);
        pos += len;
    }
    out
}

fn device(dv: &[u8]) {
    if dv.len() < 18 || dv[1] != DT_DEVICE { return; }
    kprintln!("  Device  USB {}  class {:02x}/{:02x}/{:02x} {}  ep0 {} bytes  release {}  {} configuration{}",
        bcd(u16::from_le_bytes([dv[2], dv[3]])), dv[4], dv[5], dv[6], class_name(dv[4], dv[5], dv[6]),
        dv[7], bcd(u16::from_le_bytes([dv[12], dv[13]])), dv[17], if dv[17] == 1 { "" } else { "s" });
}

/// The interface the class-specific descriptors that follow belong to.
#[derive(Clone, Copy, Default)]
struct Intf { class: u8, sub: u8, proto: u8 }

fn config(cfg: &[u8], speed: u32) {
    let mut intf = Intf::default();
    for d in descriptors(cfg) {
        match d[1] {
            DT_CONFIG if d.len() >= 9 => {
                let ma = d[8] as u32 * if speed >= 4 { 8 } else { 2 };
                let mut attrs = String::from(if d[7] & 0x40 != 0 { "self powered" } else { "bus powered" });
                if d[7] & 0x20 != 0 { attrs.push_str(", remote wakeup"); }
                kprintln!("  Configuration {}  {} interface{}  {}  {} mA  ({} bytes)",
                    d[5], d[4], if d[4] == 1 { "" } else { "s" }, attrs, ma,
                    u16::from_le_bytes([d[2], d[3]]));
            }
            DT_IAD if d.len() >= 8 => {
                let last = d[2] as u16 + d[3].max(1) as u16 - 1;
                kprintln!("    Function  interfaces {}-{}  {}", d[2], last, class_name(d[4], d[5], d[6]));
            }
            DT_INTERFACE if d.len() >= 9 => {
                intf = Intf { class: d[5], sub: d[6], proto: d[7] };
                kprintln!("    Interface {} alt {}  {}  {} endpoint{}",
                    d[2], d[3], class_name(d[5], d[6], d[7]), d[4], if d[4] == 1 { "" } else { "s" });
            }
            DT_ENDPOINT if d.len() >= 7 => endpoint(d, speed),
            DT_SS_EP_COMP if d.len() >= 6 => {
                kprintln!("        SuperSpeed  burst {}  attributes {:#04x}  {} bytes per interval",
                    d[2] as u32 + 1, d[3], u16::from_le_bytes([d[4], d[5]]));
            }
            DT_HID if d.len() >= 9 => {
                kprintln!("      HID {}  report descriptor {} bytes",
                    bcd(u16::from_le_bytes([d[2], d[3]])), u16::from_le_bytes([d[7], d[8]]));
            }
            DT_CS_INTERFACE if d.len() >= 3 => match (intf.class, intf.sub) {
                (CLASS_VIDEO, 0x01) => video_control(d),
                (CLASS_VIDEO, 0x02) => video_streaming(d),
                (CLASS_AUDIO, 0x01) => audio_control(d, intf.proto == 0x20),
                (CLASS_AUDIO, 0x02) => audio_streaming(d, intf.proto == 0x20),
                _ => raw(d),
            },
            DT_CS_ENDPOINT => {}
            _ => raw(d),
        }
    }
}

fn endpoint(d: &[u8], speed: u32) {
    let addr = d[2];
    let attrs = d[3];
    let raw_mps = u16::from_le_bytes([d[4], d[5]]);
    let size = (raw_mps & 0x7FF) as u32;
    let kind = match attrs & 0x03 { 0 => "control", 1 => "isochronous", 2 => "bulk", _ => "interrupt" };
    let mut line = format!("      Endpoint {:#04x} {}  {}", addr, if addr & 0x80 != 0 { "IN " } else { "OUT" }, kind);
    if attrs & 0x03 == 1 {
        let sync = match (attrs >> 2) & 3 { 0 => "", 1 => " async", 2 => " adaptive", _ => " sync" };
        let usage = match (attrs >> 4) & 3 { 1 => " feedback", 2 => " implicit feedback", _ => "" };
        line.push_str(sync);
        line.push_str(usage);
    }
    // High speed: bits 12:11 are extra transactions per microframe.
    let per = if speed == 3 && attrs & 0x01 != 0 { ((raw_mps >> 11) & 3) as u32 + 1 } else { 1 };
    if per > 1 {
        line.push_str(&format!("  {}x {} bytes", per, size));
    } else {
        line.push_str(&format!("  {} bytes", size));
    }
    if attrs & 0x01 != 0 && d[6] != 0 {
        line.push_str(&format!("  every {}", period(speed, attrs & 0x03, d[6])));
    }
    kprintln!("{}", line);
}

/// An interrupt or isochronous endpoint's service interval (USB 2.0
/// §9.6.6, USB 3.2 §9.6.6).
fn period(speed: u32, kind: u8, b_interval: u8) -> String {
    let micro = speed == 3 || speed >= 4;
    let us: u64 = if micro {
        125 << (b_interval.clamp(1, 16) - 1)
    } else if kind == 1 {
        1000 << (b_interval.clamp(1, 16) - 1)
    } else {
        1000 * b_interval as u64
    };
    if us >= 1000 && us % 1000 == 0 { format!("{} ms", us / 1000) } else { format!("{} us", us) }
}

fn video_control(d: &[u8]) {
    match d[2] {
        0x01 if d.len() >= 12 => kprintln!("      VC header  UVC {}  clock {} kHz",
            bcd(u16::from_le_bytes([d[3], d[4]])), u32_at(d, 7) / 1000),
        0x02 if d.len() >= 8 => kprintln!("      Input terminal {}  {}", d[3],
            video_terminal(u16::from_le_bytes([d[4], d[5]]))),
        0x03 if d.len() >= 9 => kprintln!("      Output terminal {}  {}  from {}", d[3],
            video_terminal(u16::from_le_bytes([d[4], d[5]])), d[7]),
        0x04 if d.len() >= 5 => kprintln!("      Selector unit {}", d[3]),
        0x05 if d.len() >= 5 => kprintln!("      Processing unit {}  from {}", d[3], d[4]),
        0x06 if d.len() >= 20 => kprintln!("      Extension unit {}  {}", d[3], guid(&d[4..20])),
        0x07 => kprintln!("      Encoding unit {}", d.get(3).copied().unwrap_or(0)),
        _ => raw(d),
    }
}

fn video_terminal(t: u16) -> String {
    match t {
        0x0100 => "vendor".into(),
        0x0101 => "USB streaming".into(),
        0x0201 => "camera".into(),
        0x0202 => "media transport".into(),
        0x0301 => "display".into(),
        _ => format!("type {:#06x}", t),
    }
}

fn video_streaming(d: &[u8]) {
    match d[2] {
        0x01 if d.len() >= 7 => kprintln!("      VS input header  {} format{}  endpoint {:#04x}",
            d[3], if d[3] == 1 { "" } else { "s" }, d[6]),
        0x02 if d.len() >= 6 => kprintln!("      VS output header  {} formats", d[3]),
        0x03 => kprintln!("      Still image frames"),
        0x04 | 0x10 if d.len() >= 22 => kprintln!("      Format {}  {}  {} bits per pixel  {} frame sizes",
            d[3], fourcc(&d[5..21]), d[21], d[4]),
        0x06 if d.len() >= 6 => kprintln!("      Format {}  MJPEG  {} frame sizes", d[3], d[4]),
        0x05 | 0x07 if d.len() >= 26 => {
            let n = d[25];
            let fps = intervals(d, 26, n, u32_at(d, 21));
            kprintln!("        Frame {}  {}x{}  {}", d[3],
                u16::from_le_bytes([d[5], d[6]]), u16::from_le_bytes([d[7], d[8]]), fps);
        }
        0x11 if d.len() >= 26 => {
            let n = d[21];
            let fps = intervals(d, 26, n, u32_at(d, 17));
            kprintln!("        Frame {}  {}x{}  {}", d[3],
                u16::from_le_bytes([d[5], d[6]]), u16::from_le_bytes([d[7], d[8]]), fps);
        }
        0x0D if d.len() >= 6 => {}
        _ => raw(d),
    }
}

/// A frame's intervals (100 ns units) as frame rates: a list, or for
/// `n == 0` a continuous range.
fn intervals(d: &[u8], at: usize, n: u8, default: u32) -> String {
    let mut out = String::new();
    if n == 0 {
        if d.len() >= at + 12 {
            out = format!("{} to {} fps", fps(u32_at(d, at + 4)), fps(u32_at(d, at)));
        }
    } else {
        let rates: Vec<String> = (0..n as usize)
            .filter(|i| d.len() >= at + 4 * i + 4)
            .map(|i| fps(u32_at(d, at + 4 * i)))
            .collect();
        out = format!("{} fps", rates.join(", "));
    }
    if default != 0 { out.push_str(&format!("  (default {})", fps(default))); }
    out
}

fn fps(interval: u32) -> String {
    if interval == 0 { return "?".into(); }
    let tenths = 100_000_000u64 / interval as u64;
    if tenths % 10 == 0 { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) }
}

fn audio_control(d: &[u8], uac2: bool) {
    match d[2] {
        0x01 if d.len() >= 5 => kprintln!("      AC header  UAC {}  (ADC {})",
            if uac2 { 2 } else { 1 }, bcd(u16::from_le_bytes([d[3], d[4]]))),
        0x02 if d.len() >= 8 => {
            let ch = if uac2 { d.get(8) } else { d.get(7) }.copied().unwrap_or(0);
            kprintln!("      Input terminal {}  {}  {} channel{}", d[3],
                audio_terminal(u16::from_le_bytes([d[4], d[5]])), ch, if ch == 1 { "" } else { "s" });
        }
        0x03 if d.len() >= 8 => kprintln!("      Output terminal {}  {}  from {}", d[3],
            audio_terminal(u16::from_le_bytes([d[4], d[5]])), d[7]),
        0x04 => kprintln!("      Mixer unit {}", d.get(3).copied().unwrap_or(0)),
        0x05 => kprintln!("      Selector unit {}", d.get(3).copied().unwrap_or(0)),
        0x06 if d.len() >= 5 => kprintln!("      Feature unit {}  from {}", d[3], d[4]),
        0x0A if uac2 && d.len() >= 5 => kprintln!("      Clock source {}  {}", d[3],
            match d[4] & 3 { 0 => "external", 1 => "internal fixed", 2 => "internal variable", _ => "internal programmable" }),
        0x0B if uac2 => kprintln!("      Clock selector {}", d.get(3).copied().unwrap_or(0)),
        _ => raw(d),
    }
}

fn audio_terminal(t: u16) -> String {
    match t {
        0x0101 => "USB streaming".into(),
        0x0201 => "microphone".into(),
        0x0202 => "desktop microphone".into(),
        0x0203 => "personal microphone".into(),
        0x0204 => "omnidirectional microphone".into(),
        0x0205 => "microphone array".into(),
        0x0301 => "speaker".into(),
        0x0302 => "headphones".into(),
        0x0402 => "headset".into(),
        0x0403 => "speakerphone".into(),
        0x0603 => "line".into(),
        0x0605 => "S/PDIF".into(),
        _ => format!("type {:#06x}", t),
    }
}

fn audio_streaming(d: &[u8], uac2: bool) {
    match d[2] {
        0x01 if uac2 && d.len() >= 11 => kprintln!("      AS general  terminal {}  {} channel{}",
            d[3], d[10], if d[10] == 1 { "" } else { "s" }),
        0x01 if d.len() >= 7 => kprintln!("      AS general  terminal {}  format {}",
            d[3], match u16::from_le_bytes([d[5], d[6]]) { 1 => "PCM".into(), f => format!("{:#06x}", f) }),
        0x02 if uac2 && d.len() >= 6 => kprintln!("      Format type {}  {} bytes per sample  {} bits",
            d[3], d[4], d[5]),
        0x02 if d.len() >= 8 => {
            let n = d[7] as usize;
            let rate = |i: usize| -> u32 {
                let at = 8 + 3 * i;
                if d.len() >= at + 3 { u32::from_le_bytes([d[at], d[at + 1], d[at + 2], 0]) } else { 0 }
            };
            let rates = if n == 0 {
                format!("{} to {} Hz", rate(0), rate(1))
            } else {
                let r: Vec<String> = (0..n).map(|i| format!("{}", rate(i))).collect();
                format!("{} Hz", r.join(", "))
            };
            kprintln!("      Format type {}  {} channel{}  {} bits  {}",
                d[3], d[4], if d[4] == 1 { "" } else { "s" }, d[6], rates);
        }
        _ => raw(d),
    }
}

/// A descriptor this file does not decode: type and bytes.
fn raw(d: &[u8]) {
    let show = d.len().min(16);
    let hex: Vec<String> = d[..show].iter().map(|b| format!("{:02x}", b)).collect();
    kprintln!("      descriptor {:#04x}  {} bytes  {}{}", d[1], d.len(), hex.join(" "),
        if d.len() > show { " ..." } else { "" });
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    if d.len() < at + 4 { return 0; }
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

/// A binary-coded version such as 0x0201 → `2.01`.
fn bcd(v: u16) -> String {
    format!("{:x}.{:02x}", v >> 8, v & 0xFF)
}

/// A UVC format GUID by its FourCC (first four bytes), else in full.
fn fourcc(g: &[u8]) -> String {
    if g.len() >= 4 && g[..4].iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ') {
        return g[..4].iter().map(|&b| b as char).collect::<String>().trim_end().into();
    }
    guid(g)
}

fn guid(g: &[u8]) -> String {
    if g.len() < 16 { return String::from("?"); }
    format!("{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]), u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]), g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15])
}

fn speed_name(speed: u32) -> &'static str {
    match speed { 1 => "full", 2 => "low", 3 => "high", 4 => "super", 5 => "super+", _ => "unknown" }
}

/// xHCI completion codes (xHCI 1.2 §6.4.5) that enumeration can meet.
fn cc_name(cc: u32) -> String {
    match cc {
        0 => "no answer (timed out)".into(),
        2 => "data buffer error".into(),
        3 => "babble detected".into(),
        4 => "USB transaction error".into(),
        5 => "TRB error".into(),
        6 => "stall".into(),
        7 => "resource error".into(),
        8 => "bandwidth error".into(),
        9 => "no slots available".into(),
        11 => "slot not enabled".into(),
        12 => "endpoint not enabled".into(),
        17 => "parameter error".into(),
        19 => "context state error".into(),
        c => format!("completion code {}", c),
    }
}

/// usbutils' class names, by base class, subclass and protocol.
pub fn class_name(class: u8, sub: u8, proto: u8) -> &'static str {
    match (class, sub, proto) {
        (0x00, _, _) => "per interface",
        (0x01, 0x01, _) => "Audio control",
        (0x01, 0x02, _) => "Audio streaming",
        (0x01, 0x03, _) => "MIDI streaming",
        (0x01, _, _) => "Audio",
        (0x02, _, _) => "Communications (CDC)",
        (0x03, 0x01, 0x01) => "HID keyboard",
        (0x03, 0x01, 0x02) => "HID mouse",
        (0x03, _, _) => "HID",
        (0x05, _, _) => "Physical",
        (0x06, _, _) => "Still image",
        (0x07, _, _) => "Printer",
        (0x08, _, _) => "Mass storage",
        (0x09, _, _) => "Hub",
        (0x0A, _, _) => "CDC data",
        (0x0B, _, _) => "Smart card",
        (0x0D, _, _) => "Content security",
        (0x0E, 0x01, _) => "Video control",
        (0x0E, 0x02, _) => "Video streaming",
        (0x0E, _, _) => "Video",
        (0x0F, _, _) => "Personal healthcare",
        (0x10, _, _) => "Audio/video",
        (0x11, _, _) => "Billboard",
        (0x12, _, _) => "Type-C bridge",
        (0xDC, _, _) => "Diagnostic",
        (0xE0, 0x01, 0x01) => "Bluetooth",
        (0xE0, 0x01, 0x03) => "RNDIS",
        (0xE0, _, _) => "Wireless",
        (0xEF, 0x02, 0x01) => "Composite (interface association)",
        (0xEF, _, _) => "Miscellaneous",
        (0xFE, _, _) => "Application specific",
        (0xFF, _, _) => "Vendor specific",
        _ => "Unknown class",
    }
}
