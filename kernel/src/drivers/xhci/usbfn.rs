//! USB interfaces handed to a module: the device configured, control
//! requests limited to the claimed interfaces, alternate settings with
//! their bandwidth, and bulk or isochronous IN streams.
//!
//! Linux `usb_set_configuration`, `usb_set_interface`,
//! `usb_hcd_alloc_bandwidth` for the device side; `xhci_endpoint_init`,
//! `xhci_queue_isoc_tx`, `process_isoc_td` and the isochronous cases of
//! `handle_tx_event` for the rings. Streams queue like uvcvideo: a fixed
//! number of TDs in flight, each re-queued once its data was read, every
//! isochronous TD with SIA (`URB_ISO_ASAP`).

use alloc::collections::VecDeque;
use core::sync::atomic::AtomicU32;

use super::*;

/// Endpoint rings of claimed devices: one page.
const FN_RING_TRBS: usize = 256;
/// TDs queued on a ring at most (one ring pass).
const FN_MAX_TDS: usize = FN_RING_TRBS - 2;
/// Buffers per stream at most. More than the ring holds: a TD that comes
/// back is queued again from the event, as uvcvideo resubmits from its
/// completion handler, while the module reads the ones before.
const FN_MAX_BUFS: usize = 4096;
/// DMA memory one stream may hold: a guard against a module asking for
/// gigabytes, not a size any camera needs (4 MiB is 128 ms at 3 KiB per
/// microframe).
const FN_MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
/// Largest TD: one TRB, which holds up to 64 KiB.
const FN_MAX_TD: usize = 64 * 1024;
/// `trb_block_event_intr`: an interrupt every this many TDs.
const FN_BEI_GROUP: usize = 32;

const TRB_ISOC: u32 = 5 << 10;
const TRB_BEI: u32 = 1 << 9;
const TRB_SIA: u32 = 1 << 31;

const EP_TYPE_ISOC_OUT: u32 = 1;
const EP_TYPE_BULK_OUT_FN: u32 = 2;
const EP_TYPE_ISOC_IN: u32 = 5;
const EP_TYPE_BULK_IN_FN: u32 = 6;

const CC_BABBLE: u32 = 3;
const CC_TRANSACTION: u32 = 4;
const CC_BANDWIDTH: u32 = 8;
const CC_RING_UNDERRUN: u32 = 14;
const CC_RING_OVERRUN: u32 = 15;
const CC_BANDWIDTH_OVERRUN: u32 = 18;
const CC_MISSED_SERVICE: u32 = 23;
const CC_STOPPED: u32 = 26;
const CC_STOPPED_LENGTH_INVALID: u32 = 27;
const CC_STOPPED_SHORT: u32 = 28;
const CC_ISOCH_BUFFER_OVERRUN: u32 = 31;

const DT_INTERFACE_FN: u8 = 0x04;
const DT_ENDPOINT_FN: u8 = 0x05;
const DT_SS_EP_COMP_FN: u8 = 0x30;

/// Per-packet status in `stream_read` records.
pub const PKT_OK: u8 = 0;
pub const PKT_MISSED: u8 = 1;
pub const PKT_ERROR: u8 = 2;
pub const PKT_OVERRUN: u8 = 3;

/// Class codes a module may claim, with the right each needs.
pub const CLASS_AUDIO: u8 = 0x01;
pub const CLASS_VIDEO: u8 = 0x0E;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnError {
    /// No such claim, stream or device, or it was unplugged.
    Gone,
    /// Nothing unclaimed of that class.
    NotFound,
    /// The request is outside the claimed interfaces.
    Denied,
    /// The device or controller refused.
    Failed,
    /// Configure Endpoint answered Bandwidth Error: pick a smaller alt.
    Bandwidth,
    NoMemory,
    BadArgument,
}

/// Open camera streams, for the kernel's status item.
static CAMERA_STREAMS: AtomicU32 = AtomicU32::new(0);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

pub fn camera_streams() -> u32 {
    CAMERA_STREAMS.load(Ordering::Relaxed)
}

/// An endpoint descriptor with its SuperSpeed companion.
#[derive(Clone, Copy)]
struct EpInfo {
    addr: u8,
    attr: u8,
    w_max: u16,
    interval: u8,
    ss_burst: u8,
    ss_attr: u8,
    ss_bpi: u16,
}

impl EpInfo {
    fn dci(&self) -> u8 {
        (self.addr & 0x0F) * 2 + u8::from(self.addr & 0x80 != 0)
    }
    fn xfer(&self) -> u8 { self.attr & 0x03 }
    fn is_in(&self) -> bool { self.addr & 0x80 != 0 }
}

/// A configured endpoint of a claimed device and its ring.
struct FnEp {
    ep: EpInfo,
    ifnum: u8,
    ring: DmaRegion,
    cycle: u32,
    enq: usize,
}

/// A device configured for modules: its configuration and the endpoints
/// of the current alternate settings.
pub(super) struct FnDev {
    slot: u8,
    cfg: Vec<u8>,
    alts: Vec<(u8, u8)>,
    eps: Vec<FnEp>,
}

/// One TD that came back, waiting to be read.
struct Done {
    buf: u16,
    len: u32,
    status: u8,
}

struct Stream {
    id: u32,
    dci: u8,
    iso: bool,
    td_len: usize,
    /// Buffer slots, `slot` bytes each, aligned to `slot` so no TD crosses
    /// a 64 KiB boundary.
    bufs: DmaRegion,
    pages: usize,
    base: u64,
    slot: usize,
    free: Vec<u16>,
    /// Ring index -> buffer of the TD queued there.
    trb_buf: [u16; FN_RING_TRBS],
    /// Ring index of the oldest TD still queued, and how many are.
    oldest: usize,
    armed: usize,
    done: VecDeque<Done>,
    /// `ep->skip`: a Missed Service Error said TDs were passed over.
    skip: bool,
    queued: usize,
    camera: bool,
}

/// What a module holds.
pub(super) struct UsbFn {
    id: u32,
    slot: u8,
    owner: u32,
    class: u8,
    ifaces: Vec<u8>,
    streams: Vec<Stream>,
}

// === Descriptors ===

/// Endpoints of interface `ifnum`, alternate setting `alt`.
fn endpoints_of(cfg: &[u8], ifnum: u8, alt: u8) -> Vec<EpInfo> {
    let mut out: Vec<EpInfo> = Vec::new();
    let mut inside = false;
    let mut pos = 0;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        match d[1] {
            DT_INTERFACE_FN if len >= 9 => inside = d[2] == ifnum && d[3] == alt,
            DT_ENDPOINT_FN if inside && len >= 7 => out.push(EpInfo {
                addr: d[2], attr: d[3], w_max: u16::from_le_bytes([d[4], d[5]]), interval: d[6],
                ss_burst: 0, ss_attr: 0, ss_bpi: 0,
            }),
            DT_SS_EP_COMP_FN if inside && len >= 6 => {
                if let Some(e) = out.last_mut() {
                    e.ss_burst = d[2];
                    e.ss_attr = d[3];
                    e.ss_bpi = u16::from_le_bytes([d[4], d[5]]);
                }
            }
            _ => {}
        }
        pos += len;
    }
    out
}

/// Interface numbers (alt 0) of class `class`, and of every interface.
fn interfaces(cfg: &[u8], class: Option<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        if d[1] == DT_INTERFACE_FN && len >= 9 && d[3] == 0 && class.is_none_or(|c| d[5] == c)
            && !out.contains(&d[2])
        {
            out.push(d[2]);
        }
        pos += len;
    }
    out
}

/// Whether interface `ifnum` has an alternate setting `alt`, and how many
/// it has.
fn alt_count(cfg: &[u8], ifnum: u8) -> usize {
    let mut n = 0;
    let mut pos = 0;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        if d[1] == DT_INTERFACE_FN && len >= 9 && d[2] == ifnum { n += 1; }
        pos += len;
    }
    n
}

fn alt_exists(cfg: &[u8], ifnum: u8, alt: u8) -> bool {
    let mut pos = 0;
    while pos + 2 <= cfg.len() {
        let len = cfg[pos] as usize;
        if len < 2 || pos + len > cfg.len() { break; }
        let d = &cfg[pos..pos + len];
        if d[1] == DT_INTERFACE_FN && len >= 9 && d[2] == ifnum && d[3] == alt { return true; }
        pos += len;
    }
    false
}

// === Endpoint context — xhci_endpoint_init ===

/// `xhci_get_endpoint_interval`.
fn ep_interval(speed: u32, ep: &EpInfo) -> u32 {
    let exponent = |b: u8| (b.clamp(1, 16) - 1) as u32;
    match (speed, ep.xfer()) {
        (SPEED_HIGH, 0 | 2) => {
            if ep.interval == 0 { 0 } else { (31 - (ep.interval as u32).leading_zeros()).min(15) }
        }
        (SPEED_HIGH | SPEED_SUPER | 5, 1 | 3) => exponent(ep.interval),
        (SPEED_FULL, 1) => exponent(ep.interval) + 3,
        (SPEED_FULL | SPEED_LOW, 3) => xhci_interval(speed, ep.interval),
        _ => 0,
    }
}

/// `usb_endpoint_max_periodic_payload`.
fn max_esit_payload(speed: u32, ep: &EpInfo) -> u32 {
    if ep.xfer() & 1 == 0 { return 0; }
    let maxp = (ep.w_max & 0x7FF) as u32;
    match speed {
        SPEED_SUPER | 5 => ep.ss_bpi as u32,
        SPEED_HIGH => maxp * (((ep.w_max >> 11) & 3) as u32 + 1),
        _ => maxp,
    }
}

/// Fill the input context's endpoint context for `ep` on a ring starting
/// at `deq` (`xhci_endpoint_init`).
fn ep_ctx_init(state: &XhciState, speed: u32, ep: &EpInfo, deq: u64, cycle: u32) {
    let iso = ep.xfer() == 1;
    let ep_type = match (ep.xfer(), ep.is_in()) {
        (1, false) => EP_TYPE_ISOC_OUT,
        (1, true) => EP_TYPE_ISOC_IN,
        (2, false) => EP_TYPE_BULK_OUT_FN,
        (2, true) => EP_TYPE_BULK_IN_FN,
        (3, false) => EP_TYPE_INTERRUPT_OUT,
        _ => EP_TYPE_INTERRUPT_IN,
    };
    let lec = state.hci_version > 0x100 && state.mmio.r32(CAP_HCCPARAMS2) & (1 << 4) != 0;
    let mult = if speed >= SPEED_SUPER && iso && !lec { (ep.ss_attr & 3) as u32 } else { 0 };
    let burst = match speed {
        SPEED_SUPER | 5 => ep.ss_burst as u32,
        SPEED_HIGH if ep.xfer() & 1 == 1 => ((ep.w_max >> 11) & 3) as u32,
        _ => 0,
    };
    let mut maxp = (ep.w_max & 0x7FF) as u32;
    if ep.xfer() == 2 {
        if speed == SPEED_HIGH { maxp = 512; }
        if speed == SPEED_FULL { maxp = (1u32 << (31 - maxp.max(1).leading_zeros())).clamp(8, 64); }
    }
    let esit = max_esit_payload(speed, ep);
    let interval = ep_interval(speed, ep);
    let cerr = if iso { 0 } else { 3 };
    let off = state.ctx_size * (ep.dci() as usize + 1);
    let input = state.input_ctx;
    input.w32(off, (interval << 16) | (mult << 8) | ((esit >> 16) << 24));
    input.w32(off + 4, (cerr << 1) | (ep_type << 3) | (burst << 8) | (maxp << 16));
    input.w32(off + 8, (deq as u32) | cycle);
    input.w32(off + 12, (deq >> 32) as u32);
    input.w32(off + 16, ((esit & 0xFFFF) << 16) | esit.min(0xFFFF));
}

/// One Configure Endpoint: drop `drop`, add `add` (`usb_hcd_alloc_bandwidth`
/// → `xhci_check_bandwidth`). The new rings are in `add`.
fn configure(state: &mut XhciState, slot: u8, drop: &[u8], add: &[(EpInfo, u64)], last: u8) -> Result<(), FnError> {
    let d = dev_idx(state, slot).ok_or(FnError::Gone)?;
    let speed = state.devs[d].speed;
    state.input_ctx.fill(0u32, 0, 4096);
    let mut drop_flags = 0u32;
    for &dci in drop { drop_flags |= 1 << dci; }
    let mut add_flags = 1u32;
    for (ep, _) in add { add_flags |= 1 << ep.dci(); }
    // An endpoint in both is dropped and added again: that is how the
    // controller resets it (`xhci_endpoint_reset`).
    state.input_ctx.w32(0u32, drop_flags);
    state.input_ctx.w32(4u32, add_flags);
    input_slot_copy(state, d, last.max(1));
    // Context Entries: the last endpoint left, not the largest ever.
    let ctx = state.ctx_size;
    let s0 = state.input_ctx.r32(ctx);
    state.input_ctx.w32(ctx, (s0 & !(0x1F << 27)) | ((last.max(1) as u32) << 27));
    for (ep, ring) in add {
        ep_ctx_init(state, speed, ep, *ring, 1);
    }
    post_command(state, state.input_ctx.phys(), 0, TRB_CONFIGURE_EP | ((slot as u32) << 24));
    match wait_command_completion(state) {
        Some((cc, _)) if cc == CC_SUCCESS => Ok(()),
        Some((CC_BANDWIDTH, _)) => Err(FnError::Bandwidth),
        Some((cc, _)) => {
            kprintln!("[npk] xhci: slot {} configure endpoint failed, completion code {}", slot, cc);
            Err(FnError::Failed)
        }
        None => Err(FnError::Failed),
    }
}

fn new_ring(state: &mut XhciState) -> Option<DmaRegion> {
    let r = take_page(state, "usb fn ring")?;
    write_trb(r, FN_RING_TRBS - 1, r.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    Some(r)
}

fn last_dci(eps: &[FnEp]) -> u8 {
    eps.iter().map(|e| e.ep.dci()).max().unwrap_or(1)
}

// === Claim ===

/// Whether a kernel driver has the device.
fn kernel_owned(state: &XhciState, slot: u8) -> bool {
    state.hid.iter().any(|h| h.slot == slot)
        || state.msc.iter().any(|m| m.slot == slot)
        || state.nic.as_ref().is_some_and(|n| n.slot_id == slot)
        || state.devs.iter().any(|d| d.slot == slot && d.hub.is_some())
}

/// `usb_set_configuration`: the endpoints of every interface's alt 0, then
/// SET_CONFIGURATION, as Linux's generic driver does for every device.
fn configure_device(state: &mut XhciState, slot: u8) -> Result<(), FnError> {
    if state.fn_devs.iter().any(|f| f.slot == slot) { return Ok(()); }
    let d = dev_idx(state, slot).ok_or(FnError::Gone)?;
    let cfg = state.devs[d].seen.cfg.clone();
    if cfg.len() < 9 { return Err(FnError::Failed); }
    let config_val = cfg[5];
    let mut eps: Vec<FnEp> = Vec::new();
    let ifs = interfaces(&cfg, None);
    for &i in &ifs {
        for ep in endpoints_of(&cfg, i, 0) {
            let Some(ring) = new_ring(state) else {
                state.free_pages.extend(eps.iter().map(|e| e.ring));
                return Err(FnError::NoMemory);
            };
            eps.push(FnEp { ep, ifnum: i, ring, cycle: 1, enq: 0 });
        }
    }
    let add: Vec<(EpInfo, u64)> = eps.iter().map(|e| (e.ep, e.ring.phys())).collect();
    if !add.is_empty() {
        if let Err(e) = configure(state, slot, &[], &add, last_dci(&eps)) {
            state.free_pages.extend(eps.iter().map(|e| e.ring));
            return Err(e);
        }
    }
    if !usb_set_config(state, slot, config_val) {
        kprintln!("[npk] xhci: slot {} SET_CONFIGURATION {} failed", slot, config_val);
        state.free_pages.extend(eps.iter().map(|e| e.ring));
        return Err(FnError::Failed);
    }
    let alts = ifs.iter().map(|&i| (i, 0)).collect();
    state.fn_devs.push(FnDev { slot, cfg, alts, eps });
    Ok(())
}

/// Claim every interface of class `class` on the first device that has
/// one and no driver. Returns the claim and the device and configuration
/// descriptors.
pub fn claim(class: u8, owner: u32) -> Result<(u32, Vec<u8>), FnError> {
    let mut g = CTRLS.lock();
    for s in g.iter_mut() {
        let slots: Vec<u8> = s.devs.iter().map(|d| d.slot).collect();
        for slot in slots {
            if kernel_owned(s, slot) { continue; }
            let Some(d) = dev_idx(s, slot) else { continue };
            let cfg = s.devs[d].seen.cfg.clone();
            let ifaces = interfaces(&cfg, Some(class));
            if ifaces.is_empty() { continue; }
            let taken = s.fns.iter().any(|f| f.slot == slot && f.ifaces.iter().any(|i| ifaces.contains(i)));
            if taken { continue; }
            configure_device(s, slot)?;
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let mut descs = s.devs[d].seen.device.clone();
            descs.extend_from_slice(&cfg);
            kprintln!("[npk] xhci: port {} interfaces {:?} (class {:02x}) to a module",
                s.devs[d].seen.port, ifaces, class);
            s.fns.push(UsbFn { id, slot, owner, class, ifaces, streams: Vec::new() });
            return Ok((id, descs));
        }
    }
    Err(FnError::NotFound)
}

/// `f(state, fn index)` on claim `id` of `owner`, under the controller lock.
fn with_fn<R>(id: u32, owner: u32, f: impl FnOnce(&mut XhciState, usize) -> Result<R, FnError>) -> Result<R, FnError> {
    let mut g = CTRLS.lock();
    for s in g.iter_mut() {
        if let Some(i) = s.fns.iter().position(|x| x.id == id && x.owner == owner) {
            return f(s, i);
        }
    }
    Err(FnError::Gone)
}

/// A control request on the claimed interfaces (`usb_control_msg`). A
/// request to the device is allowed only for GET_DESCRIPTOR (strings);
/// SET_INTERFACE goes through `set_alt`. IN returns `w_length` bytes.
pub fn control(id: u32, owner: u32, bm: u8, req: u8, value: u16, index: u16, out: &[u8], w_length: u16)
               -> Result<Vec<u8>, FnError> {
    with_fn(id, owner, |s, i| {
        let f = &s.fns[i];
        let slot = f.slot;
        let dir_in = bm & 0x80 != 0;
        let ty = (bm >> 5) & 3;
        let allowed = match bm & 0x1F {
            0 => ty == 0 && dir_in && req == USB_GET_DESCRIPTOR,
            1 => f.ifaces.contains(&(index as u8)) && !(ty == 0 && req == USB_SET_INTERFACE),
            2 => {
                let ep = index as u8;
                s.fn_devs.iter().find(|d| d.slot == slot)
                    .is_some_and(|d| d.eps.iter().any(|e| e.ep.addr == ep && f.ifaces.contains(&e.ifnum)))
            }
            _ => false,
        };
        if !allowed { return Err(FnError::Denied); }
        if !dir_in && out.len() != w_length as usize { return Err(FnError::BadArgument); }
        if !dir_in && !out.is_empty() { s.data_buf.copy_in(0u32, out); }
        if dir_in { s.data_buf.fill(0u32, 0, w_length as u64); }
        if !usb_control_transfer(s, slot, bm, req, value, index, w_length, dir_in) {
            return Err(FnError::Failed);
        }
        let mut v = alloc::vec![0u8; if dir_in { w_length as usize } else { 0 }];
        if dir_in { s.data_buf.copy_out(0u32, &mut v); }
        // `usb_clear_halt`: the device starts the endpoint at DATA0, so
        // the host's side is reset too (`usb_reset_endpoint`).
        if bm == 0x02 && req == USB_REQ_CLEAR_FEATURE && value == USB_ENDPOINT_HALT {
            reset_endpoint(s, i, index as u8)?;
        }
        Ok(v)
    })
}

const USB_REQ_CLEAR_FEATURE: u8 = 0x01;
const USB_ENDPOINT_HALT: u16 = 0;

/// `xhci_endpoint_reset`: a stream on the endpoint ends, the endpoint is
/// stopped, dropped and added again on a fresh ring.
fn reset_endpoint(s: &mut XhciState, i: usize, ep_addr: u8) -> Result<(), FnError> {
    let slot = s.fns[i].slot;
    let fd = s.fn_devs.iter().position(|d| d.slot == slot).ok_or(FnError::Gone)?;
    let e = s.fn_devs[fd].eps.iter().position(|e| e.ep.addr == ep_addr).ok_or(FnError::BadArgument)?;
    let dci = s.fn_devs[fd].eps[e].ep.dci();
    if matches!(s.fn_devs[fd].eps[e].ep.xfer(), 0 | 1) { return Ok(()); }
    let open: Vec<u32> = s.fns[i].streams.iter().filter(|st| st.dci == dci).map(|st| st.id).collect();
    for sid in open { stream_close_locked(s, i, sid); }
    post_command(s, 0, 0, TRB_STOP_RING | ((slot as u32) << 24) | ((dci as u32) << 16));
    let _ = wait_command_completion(s);
    let ring = s.fn_devs[fd].eps[e].ring;
    ring.zero();
    write_trb(ring, FN_RING_TRBS - 1, ring.phys(), 0, TRB_LINK | TRB_CYCLE | (1 << 1));
    let ep = s.fn_devs[fd].eps[e].ep;
    s.fn_devs[fd].eps[e].enq = 0;
    s.fn_devs[fd].eps[e].cycle = 1;
    let last = last_dci(&s.fn_devs[fd].eps);
    configure(s, slot, &[dci], &[(ep, ring.phys())], last)
}

/// `usb_set_interface`: bandwidth for the new alternate setting first
/// (the old one's endpoints dropped, the new one's added), then
/// SET_INTERFACE; on its failure the old setting's bandwidth comes back.
pub fn set_alt(id: u32, owner: u32, ifnum: u8, alt: u8) -> Result<(), FnError> {
    with_fn(id, owner, |s, i| {
        if !s.fns[i].ifaces.contains(&ifnum) { return Err(FnError::Denied); }
        let slot = s.fns[i].slot;
        let fd = s.fn_devs.iter().position(|d| d.slot == slot).ok_or(FnError::Gone)?;
        if !alt_exists(&s.fn_devs[fd].cfg, ifnum, alt) { return Err(FnError::BadArgument); }
        // Streams on this interface end with its setting.
        let dcis: Vec<u8> = s.fn_devs[fd].eps.iter().filter(|e| e.ifnum == ifnum).map(|e| e.ep.dci()).collect();
        let gone: Vec<u32> = s.fns[i].streams.iter().filter(|st| dcis.contains(&st.dci)).map(|st| st.id).collect();
        for sid in gone { stream_close_locked(s, i, sid); }
        let cur = s.fn_devs[fd].alts.iter().find(|a| a.0 == ifnum).map_or(0, |a| a.1);
        let cfg = s.fn_devs[fd].cfg.clone();
        let mut new_eps = Vec::new();
        for ep in endpoints_of(&cfg, ifnum, alt) {
            let Some(ring) = new_ring(s) else {
                s.free_pages.extend(new_eps.iter().map(|e: &FnEp| e.ring));
                return Err(FnError::NoMemory);
            };
            new_eps.push(FnEp { ep, ifnum, ring, cycle: 1, enq: 0 });
        }
        let kept: Vec<u8> = s.fn_devs[fd].eps.iter().filter(|e| e.ifnum != ifnum).map(|e| e.ep.dci()).collect();
        let last = kept.iter().copied().chain(new_eps.iter().map(|e| e.ep.dci())).max().unwrap_or(1);
        let add: Vec<(EpInfo, u64)> = new_eps.iter().map(|e| (e.ep, e.ring.phys())).collect();
        if let Err(e) = configure(s, slot, &dcis, &add, last) {
            s.free_pages.extend(new_eps.iter().map(|e| e.ring));
            return Err(e);
        }
        let ok = usb_control_transfer(s, slot, 0x01, USB_SET_INTERFACE, alt as u16, ifnum as u16, 0, false);
        // A device with one setting may stall the request; Linux carries on.
        if !ok && !(s.last_cc == CC_STALL && alt_count(&cfg, ifnum) == 1) {
            let old: Vec<(EpInfo, u64)> = s.fn_devs[fd].eps.iter().filter(|e| e.ifnum == ifnum)
                .map(|e| (e.ep, e.ring.phys())).collect();
            let back_dcis: Vec<u8> = new_eps.iter().map(|e| e.ep.dci()).collect();
            let back_last = kept.iter().copied().chain(old.iter().map(|e| e.0.dci())).max().unwrap_or(1);
            let _ = configure(s, slot, &back_dcis, &old, back_last);
            s.free_pages.extend(new_eps.iter().map(|e| e.ring));
            kprintln!("[npk] xhci: slot {} SET_INTERFACE {} alt {} failed (was {})", slot, ifnum, alt, cur);
            return Err(FnError::Failed);
        }
        let fdev = &mut s.fn_devs[fd];
        let mut old_rings = Vec::new();
        fdev.eps.retain(|e| if e.ifnum == ifnum { old_rings.push(e.ring); false } else { true });
        fdev.eps.extend(new_eps);
        match fdev.alts.iter_mut().find(|a| a.0 == ifnum) {
            Some(a) => a.1 = alt,
            None => fdev.alts.push((ifnum, alt)),
        }
        // Dropped: the controller no longer reads these rings.
        for r in old_rings { r.zero(); s.free_pages.push(r); }
        Ok(())
    })
}

// === Streams — xhci_queue_isoc_tx, process_isoc_td ===

/// Start an IN stream on endpoint `ep_addr` of the current setting with
/// `count` buffers of `td_len` bytes (0 = one service interval's payload,
/// for isochronous endpoints); up to a ring's worth of them is queued.
pub fn stream_open(id: u32, owner: u32, ep_addr: u8, td_len: usize, count: usize) -> Result<u32, FnError> {
    with_fn(id, owner, |s, i| {
        let slot = s.fns[i].slot;
        let fd = s.fn_devs.iter().position(|d| d.slot == slot).ok_or(FnError::Gone)?;
        let e = s.fn_devs[fd].eps.iter().find(|e| e.ep.addr == ep_addr).ok_or(FnError::BadArgument)?;
        if !s.fns[i].ifaces.contains(&e.ifnum) { return Err(FnError::Denied); }
        let ep = e.ep;
        if !ep.is_in() || !matches!(ep.xfer(), 1 | 2) { return Err(FnError::BadArgument); }
        let dci = ep.dci();
        if s.fns.iter().any(|f| f.streams.iter().any(|st| st.dci == dci) && f.slot == slot) {
            return Err(FnError::Denied);
        }
        let speed = s.devs[dev_idx(s, slot).ok_or(FnError::Gone)?].speed;
        let iso = ep.xfer() == 1;
        let td_len = if td_len == 0 && iso { max_esit_payload(speed, &ep) as usize } else { td_len };
        if td_len == 0 || td_len > FN_MAX_TD || count == 0 || count > FN_MAX_BUFS {
            return Err(FnError::BadArgument);
        }
        let slot_bytes = td_len.next_power_of_two().max(4096);
        if count * slot_bytes > FN_MAX_STREAM_BYTES { return Err(FnError::BadArgument); }
        let pages = (count * slot_bytes).div_ceil(4096);
        let align = slot_bytes / 4096;
        let base = crate::memory::allocate_contiguous_aligned_below(pages, align, u64::MAX)
            .ok_or(FnError::NoMemory)?;
        // SAFETY: `pages` frames just allocated for this stream, identity
        // mapped; freed in `stream_close_locked` after the endpoint stopped.
        let bufs = unsafe { DmaRegion::from_raw(base, (pages * 4096) as u64) };
        bufs.zero();
        let sid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let enq = s.fn_devs[fd].eps.iter().find(|e| e.ep.dci() == dci).map_or(0, |e| e.enq);
        let camera = s.fns[i].class == CLASS_VIDEO;
        s.fns[i].streams.push(Stream {
            id: sid, dci, iso, td_len, bufs, pages, base, slot: slot_bytes,
            free: (0..count as u16).rev().collect(), trb_buf: [0; FN_RING_TRBS],
            oldest: enq, armed: 0, done: VecDeque::new(), skip: false, queued: 0, camera,
        });
        let st = s.fns[i].streams.len() - 1;
        refill(s, i, st);
        if camera {
            CAMERA_STREAMS.fetch_add(1, Ordering::Relaxed);
            crate::notify::notify(crate::notify::TOPIC_STATUS);
        }
        Ok(sid)
    })
}

/// Queue a TD for every free buffer the ring has room for.
fn refill(s: &mut XhciState, i: usize, st: usize) {
    let slot = s.fns[i].slot;
    let Some(fd) = s.fn_devs.iter().position(|d| d.slot == slot) else { return };
    let dci = s.fns[i].streams[st].dci;
    let Some(e) = s.fn_devs[fd].eps.iter().position(|e| e.ep.dci() == dci) else { return };
    let speed = dev_idx(s, slot).map_or(SPEED_HIGH, |d| s.devs[d].speed);
    let hci = s.hci_version;
    let mut rang = false;
    loop {
        let stream = &mut s.fns[i].streams[st];
        if stream.armed >= FN_MAX_TDS { break; }
        let Some(b) = stream.free.pop() else { break };
        let ring_ep = &mut s.fn_devs[fd].eps[e];
        let (ring, idx, cycle) = (ring_ep.ring, ring_ep.enq, ring_ep.cycle);
        let addr = stream.base + (b as usize * stream.slot) as u64;
        let len = stream.td_len;
        stream.queued += 1;
        let bei = if stream.queued % FN_BEI_GROUP != 0 { TRB_BEI } else { 0 };
        let ctl = if stream.iso {
            let maxp = (ring_ep.ep.w_max & 0x7FF).max(1) as usize;
            let packets = len.div_ceil(maxp).max(1);
            // `xhci_get_burst_count`, `xhci_get_last_burst_packet_count`.
            let (tbc, tlbpc) = if hci < 0x100 {
                (0, 0)
            } else if speed >= SPEED_SUPER {
                let mb = ring_ep.ep.ss_burst as usize + 1;
                let res = packets % mb;
                (packets.div_ceil(mb) - 1, if res == 0 { mb - 1 } else { res - 1 })
            } else {
                (0, packets - 1)
            };
            TRB_ISOC | ((tlbpc as u32 & 0xF) << 16) | ((tbc as u32 & 3) << 7) | TRB_SIA
        } else {
            TRB_NORMAL
        };
        write_trb(ring, idx, addr, len as u32, ctl | TRB_ISP | TRB_IOC | bei | cycle);
        stream.trb_buf[idx] = b;
        stream.armed += 1;
        ring_ep.enq += 1;
        if ring_ep.enq >= FN_RING_TRBS - 1 {
            write_trb(ring, FN_RING_TRBS - 1, ring.phys(), 0, TRB_LINK | ring_ep.cycle | (1 << 1));
            ring_ep.cycle ^= 1;
            ring_ep.enq = 0;
        }
        rang = true;
    }
    if rang { ring_doorbell(s, slot as u32, dci as u32); }
}

fn next_idx(i: usize) -> usize {
    if i + 1 >= FN_RING_TRBS - 1 { 0 } else { i + 1 }
}

/// Complete the oldest queued TD with `status`.
fn complete_oldest(st: &mut Stream, len: u32, status: u8) {
    let b = st.trb_buf[st.oldest];
    st.done.push_back(Done { buf: b, len, status });
    st.oldest = next_idx(st.oldest);
    st.armed -= 1;
}

/// A transfer event on a stream's ring. True when it was one.
pub(super) fn dispatch(s: &mut XhciState, e: &Evt) -> bool {
    let mut hit = None;
    for fd in &s.fn_devs {
        for ep in &fd.eps {
            let base = ep.ring.phys();
            let slot_dci = (fd.slot, ep.ep.dci());
            let on = e.param >= base && e.param < base + (FN_RING_TRBS * 16) as u64;
            let id_hit = e.param == 0 && ((e.control >> 24) as u8, ((e.control >> 16) & 0x1F) as u8) == slot_dci;
            if on || id_hit {
                hit = Some((fd.slot, ep.ep.dci(), if on { Some(((e.param - base) / 16) as usize) } else { None }));
            }
        }
    }
    let Some((slot, dci, idx)) = hit else { return false };
    let Some(i) = s.fns.iter().position(|f| f.slot == slot && f.streams.iter().any(|st| st.dci == dci)) else {
        return true;
    };
    let st_i = s.fns[i].streams.iter().position(|st| st.dci == dci).unwrap_or(0);
    let mut halted = false;
    {
        let st = &mut s.fns[i].streams[st_i];
        match e.cc {
            CC_RING_OVERRUN | CC_RING_UNDERRUN => return true,
            CC_STOPPED | CC_STOPPED_LENGTH_INVALID | CC_STOPPED_SHORT => return true,
            CC_MISSED_SERVICE => st.skip = true,
            _ => {}
        }
        let Some(idx) = idx else { return true };
        if st.armed == 0 { return true; }
        // TDs before this one were passed over: the skip flag after a
        // Missed Service Error, or an event that went missing.
        while st.oldest != idx && st.armed > 0 {
            complete_oldest(st, 0, PKT_MISSED);
        }
        if st.armed == 0 { return true; }
        st.skip = false;
        let actual = (st.td_len as u32).saturating_sub(e.residual);
        match e.cc {
            CC_SUCCESS | CC_SHORT_PACKET => complete_oldest(st, actual, PKT_OK),
            CC_MISSED_SERVICE => complete_oldest(st, 0, PKT_MISSED),
            CC_BABBLE | CC_ISOCH_BUFFER_OVERRUN | CC_BANDWIDTH_OVERRUN => {
                complete_oldest(st, 0, PKT_OVERRUN);
                halted = !st.iso || e.cc == CC_BABBLE;
            }
            CC_TRANSACTION => {
                complete_oldest(st, if st.iso { actual } else { 0 }, PKT_ERROR);
                halted = !st.iso;
            }
            _ => {
                complete_oldest(st, 0, PKT_ERROR);
                halted = true;
            }
        }
    }
    if halted { recover(s, i, st_i); }
    refill(s, i, st_i);
    true
}

/// A halted stream endpoint: Reset Endpoint, the queued TDs given back as
/// errors, the dequeue pointer moved to where the next TD goes
/// (`xhci_handle_halted_endpoint`).
fn recover(s: &mut XhciState, i: usize, st_i: usize) {
    let slot = s.fns[i].slot;
    let dci = s.fns[i].streams[st_i].dci;
    let Some(d) = dev_idx(s, slot) else { return };
    let ep_state = s.devs[d].dctx.r32(s.ctx_size * dci as usize) & 0x7;
    if ep_state != EP_STATE_HALTED { return; }
    let sel = ((slot as u32) << 24) | ((dci as u32) << 16);
    post_command(s, 0, 0, TRB_RESET_EP | sel);
    let _ = wait_command_completion(s);
    let Some(fd) = s.fn_devs.iter().position(|f| f.slot == slot) else { return };
    let Some(e) = s.fn_devs[fd].eps.iter().position(|e| e.ep.dci() == dci) else { return };
    let (ring, enq, cycle) = (s.fn_devs[fd].eps[e].ring, s.fn_devs[fd].eps[e].enq, s.fn_devs[fd].eps[e].cycle);
    post_command(s, ring.phys() + (enq * 16) as u64 | cycle as u64, 0, TRB_SET_DEQ | sel);
    let _ = wait_command_completion(s);
    let st = &mut s.fns[i].streams[st_i];
    while st.armed > 0 { complete_oldest(st, 0, PKT_ERROR); }
    st.oldest = enq;
}

/// Read what came back: records of `len: u16, status: u8, 0`, then `len`
/// bytes, as many as fit in `max`. A buffer read is free for a TD again.
pub fn stream_read(id: u32, owner: u32, sid: u32, max: usize) -> Result<Vec<u8>, FnError> {
    with_fn(id, owner, |s, i| {
        drain(s);
        let st_i = s.fns[i].streams.iter().position(|st| st.id == sid).ok_or(FnError::Gone)?;
        let mut out = Vec::new();
        {
            let st = &mut s.fns[i].streams[st_i];
            while let Some(d) = st.done.front() {
                let len = d.len as usize;
                if out.len() + 4 + len > max { break; }
                out.extend_from_slice(&(len as u16).to_le_bytes());
                out.push(d.status);
                out.push(0);
                let at = out.len();
                out.resize(at + len, 0);
                st.bufs.copy_out(d.buf as usize * st.slot, &mut out[at..]);
                st.free.push(d.buf);
                st.done.pop_front();
            }
        }
        refill(s, i, st_i);
        Ok(out)
    })
}

/// Stop the endpoint, move its dequeue pointer past what was queued and
/// free the buffers.
fn stream_close_locked(s: &mut XhciState, i: usize, sid: u32) {
    let Some(st_i) = s.fns[i].streams.iter().position(|st| st.id == sid) else { return };
    let slot = s.fns[i].slot;
    let dci = s.fns[i].streams[st_i].dci;
    let sel = ((slot as u32) << 24) | ((dci as u32) << 16);
    let alive = dev_idx(s, slot).is_some();
    if alive {
        post_command(s, 0, 0, TRB_STOP_RING | sel);
        let _ = wait_command_completion(s);
        if let Some(fd) = s.fn_devs.iter().position(|f| f.slot == slot) {
            if let Some(e) = s.fn_devs[fd].eps.iter().position(|e| e.ep.dci() == dci) {
                let ep = &s.fn_devs[fd].eps[e];
                let deq = ep.ring.phys() + (ep.enq * 16) as u64 | ep.cycle as u64;
                post_command(s, deq, 0, TRB_SET_DEQ | sel);
                let _ = wait_command_completion(s);
            }
        }
    }
    let st = s.fns[i].streams.remove(st_i);
    if st.camera {
        CAMERA_STREAMS.fetch_sub(1, Ordering::Relaxed);
        crate::notify::notify(crate::notify::TOPIC_STATUS);
    }
    if alive {
        crate::memory::deallocate_contiguous(st.bufs.phys(), st.pages);
    }
}

pub fn stream_close(id: u32, owner: u32, sid: u32) -> Result<(), FnError> {
    with_fn(id, owner, |s, i| {
        stream_close_locked(s, i, sid);
        Ok(())
    })
}

/// Give back claim `id`: its streams end and its interfaces go back to alt
/// 0 (`usb_unbind_interface`). The device stays configured.
pub fn release(id: u32, owner: u32) {
    let mut alts = Vec::new();
    let _ = with_fn(id, owner, |s, i| {
        let sids: Vec<u32> = s.fns[i].streams.iter().map(|st| st.id).collect();
        for sid in sids { stream_close_locked(s, i, sid); }
        let slot = s.fns[i].slot;
        if let Some(fd) = s.fn_devs.iter().find(|d| d.slot == slot) {
            alts = fd.alts.iter().filter(|a| a.1 != 0 && s.fns[i].ifaces.contains(&a.0)).map(|a| a.0).collect();
        }
        Ok(())
    });
    for ifnum in alts { let _ = set_alt(id, owner, ifnum, 0); }
    let mut g = CTRLS.lock();
    for s in g.iter_mut() { s.fns.retain(|f| f.id != id); }
}

/// The module `owner` ended: every claim it held goes.
pub fn release_owner(owner: u32) {
    let ids: Vec<u32> = CTRLS.lock().iter().flat_map(|s| s.fns.iter().filter(|f| f.owner == owner).map(|f| f.id)
        .collect::<Vec<_>>()).collect();
    for id in ids { release(id, owner); }
}

/// Device `slot` is gone: its claims, streams and rings. The slot is still
/// enabled, so buffers are freed only by the caller after Disable Slot.
pub(super) fn detached(s: &mut XhciState, slot: u8) -> (Vec<DmaRegion>, Vec<(u64, usize)>) {
    let mut pages = Vec::new();
    let mut bufs = Vec::new();
    for f in s.fns.iter_mut().filter(|f| f.slot == slot) {
        for st in f.streams.drain(..) {
            if st.camera { CAMERA_STREAMS.fetch_sub(1, Ordering::Relaxed); }
            bufs.push((st.bufs.phys(), st.pages));
        }
    }
    s.fns.retain(|f| f.slot != slot);
    if let Some(p) = s.fn_devs.iter().position(|d| d.slot == slot) {
        let fd = s.fn_devs.remove(p);
        pages.extend(fd.eps.into_iter().map(|e| e.ring));
    }
    if !bufs.is_empty() { crate::notify::notify(crate::notify::TOPIC_STATUS); }
    (pages, bufs)
}

/// Everything of the controller, before a reset.
pub(super) fn forget(s: &mut XhciState) -> Vec<(u64, usize)> {
    let slots: Vec<u8> = s.fn_devs.iter().map(|d| d.slot).collect();
    let mut bufs = Vec::new();
    for slot in slots {
        let (_, b) = detached(s, slot);
        bufs.extend(b);
    }
    bufs
}
