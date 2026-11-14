//! Engine-neutral host functions.
//!
//! Every function here works on `(&mut HostState, args)` and nothing else.
//! Engines differ only in how they obtain the state: the interpreter passes
//! `caller.data_mut()`, the compiler takes it from the vmctx. Both share
//! this one implementation.
//!
//! Functions that need guest memory additionally take `mem: &mut [u8]`.
#![allow(clippy::too_many_arguments)]

use super::{
    HostState, get_debug_target, pop_app_key, extract_wasm_custom_section, spawn_on_worker_inner,
    MAX_APP_BUFS, app_is_focused, bench_sys_info, fsck_sys_info, append_entry, is_trust_critical_path,
    spawn_on_worker, picker_module_name, PICKER_W, PICKER_H,
};
use super::{
    HwDriverState, MAX_DMA_ALLOCS, MAX_DMA_ALLOCS_GPU, MAX_DMA_PAGES, MAX_DMA_PAGES_GPU,
    MAX_DMA_PAGES_PER_CALL,
};
// What the host functions not yet built on AArch64 use.
#[cfg(target_arch = "x86_64")]
use super::{MAX_MMIO_MAPS, TERM_IDX_ACTIVE};
use crate::{kprint, kprintln, capability};
#[cfg(target_arch = "x86_64")]
use crate::drivers::pci;
use alloc::vec::Vec;
use alloc::string::String;

/// The guest bytes `ptr .. ptr + len`, or `None` when the pointer is
/// negative, the end overflows, or the range leaves guest memory. Every
/// guest range goes through here or `guest_mut`; nothing indexes guest
/// memory with its own arithmetic.
pub(crate) fn guest(mem: &[u8], ptr: i32, len: usize) -> Option<&[u8]> {
    let start = usize::try_from(ptr).ok()?;
    mem.get(start..start.checked_add(len)?)
}

/// Writable form of `guest`.
pub(crate) fn guest_mut(mem: &mut [u8], ptr: i32, len: usize) -> Option<&mut [u8]> {
    let start = usize::try_from(ptr).ok()?;
    mem.get_mut(start..start.checked_add(len)?)
}

/// A guest-supplied length; `None` when negative.
pub(crate) fn glen(len: i32) -> Option<usize> {
    usize::try_from(len).ok()
}

/// A UTF-8 string from guest memory; `None` if the range does not fit or
/// is not UTF-8.
/// Longest string a host call copies out of a guest: paths, URLs, log lines
/// and metadata are far shorter, and the copy lands on the kernel heap.
const MAX_GUEST_STR: usize = 1024 * 1024;
/// Largest byte buffer a host call copies out of a guest (request bodies,
/// frames, driver messages).
const MAX_GUEST_BYTES: usize = 64 * 1024 * 1024;
/// Most response body one HTTP request makes the kernel hold: the module
/// names the buffer, the server decides how much arrives, and no request may
/// take more than a quarter of the free RAM (the async path's per-owner
/// share, `fetch::reserve_limits`).
fn max_http_body() -> usize {
    crate::memory::stats().0.saturating_mul(crate::memory::PAGE_SIZE) / 4
}

pub(crate) fn read_str(data: &[u8], ptr: i32, len: i32) -> Option<String> {
    let n = glen(len)?;
    if n > MAX_GUEST_STR { return None; }
    let bytes = guest(data, ptr, n)?;
    if bytes.is_empty() { return None; }
    core::str::from_utf8(bytes).ok().map(String::from)
}

/// Bytes from guest memory, or `None` if the range does not fit.
pub(crate) fn read_bytes(data: &[u8], ptr: i32, len: i32) -> Option<alloc::vec::Vec<u8>> {
    if len <= 0 { return None; }
    let n = glen(len)?;
    if n > MAX_GUEST_BYTES { return None; }
    guest(data, ptr, n).and_then(copy_out)
}

/// A kernel copy of guest bytes, or `None` when the heap cannot grow that
/// far: the size is the module's choice, so running out is the module's
/// refusal, not a kernel halt.
pub(crate) fn copy_out(bytes: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(bytes.len()).ok()?;
    v.extend_from_slice(bytes);
    Some(v)
}

/// Writes `bytes` to guest memory at `ptr`; returns the length, or -1 if
/// the range does not fit.
pub(crate) fn write_bytes(data: &mut [u8], ptr: i32, bytes: &[u8]) -> i32 {
    match guest_mut(data, ptr, bytes.len()) {
        Some(dst) => { dst.copy_from_slice(bytes); bytes.len() as i32 }
        None => -1,
    }
}

/// The answer to a buffer too small for one complete object: `-(needed + 2)`.
/// -1 stays "error or denied" and 0 a valid empty answer, so a caller grows
/// its buffer to `needed` and asks again (the SDK's `read_all`).
pub(crate) fn too_small(needed: usize) -> i32 {
    -(i32::try_from(needed).unwrap_or(i32::MAX - 2).min(i32::MAX - 2) + 2)
}

/// Writes one complete object into the guest buffer `[buf_ptr, buf_ptr +
/// buf_max)`: its length, `too_small` when it does not fit, -1 for a bad
/// range. Never a cut object.
pub(crate) fn write_whole(mem: &mut [u8], buf_ptr: i32, buf_max: i32, bytes: &[u8]) -> i32 {
    let Some(max) = glen(buf_max) else { return -1 };
    if bytes.len() > max { return too_small(bytes.len()); }
    write_bytes(mem, buf_ptr, bytes)
}

#[cfg(target_arch = "x86_64")]
/// Shared body of the WLAN queue polls: moves one message from `dequeue`
/// into the guest buffer. Its length, 0 for an empty queue, `too_small`
/// with the message left queued, or -1 for a bad range.
fn wifi_poll_into(
    mem: &mut [u8],
    buf_ptr: i32,
    max: i32,
    dequeue: fn(usize) -> crate::wifi::Pop,
) -> i32 {
    let Some(max) = glen(max) else { return -1 };
    let cap = max.min(crate::wifi::WIFI_MSG_MAX);
    // Checked before the dequeue: a message taken out cannot be put back.
    if guest(mem, buf_ptr, cap).is_none() { return -1; }
    match dequeue(cap) {
        crate::wifi::Pop::Empty => 0,
        crate::wifi::Pop::Short(needed) => too_small(needed),
        crate::wifi::Pop::Got(msg) => write_bytes(mem, buf_ptr, &msg),
    }
}

pub(crate) fn npk_http_status(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_http_status") {
        return 0;
    }
    ctx.http_status as i32
}

pub(crate) fn npk_fs_usage(ctx: &mut HostState) -> i64 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::READ, "npk_fs_usage") {
        return -1;
    }
    let Some((total_blocks, free_blocks, _, _)) = crate::npkfs::stats() else {
        return -1;
    };
    let block = crate::npkfs::BLOCK_SIZE as u64;
    let to_mib = |blocks: u64| (blocks.saturating_mul(block)) >> 20;
    let total = to_mib(total_blocks);
    let used = to_mib(total_blocks.saturating_sub(free_blocks));
    (((used & 0xFFFF_FFFF) << 32) | (total & 0xFFFF_FFFF)) as i64
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_clipboard_len(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_clipboard_len") {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    crate::shade::clipboard::text_len() as i32
}

pub(crate) fn npk_window_set_close_guard(ctx: &mut HostState, on: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_window_set_close_guard") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    crate::shade::widgets::set_close_guard(wid, on != 0);
    0
}

pub(crate) fn npk_window_close_cancel(ctx: &mut HostState) -> i32 {
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    if crate::shade::widgets::cancel_close_request(wid) { 0 } else { -1 }
}

pub(crate) fn npk_screen_size(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    let ok = capability::holds(&cap_id, capability::Rights::RENDER)
        || capability::holds(&cap_id, capability::Rights::CAPTURE);
    if !ok { return 0; }
    let info = crate::framebuffer::get_info();
    let (w, h) = (logical(info.width), logical(info.height));
    (((w & 0xFFFF) << 16) | (h & 0xFFFF)) as i32
}

/// Physical size of what `npk_capture_screen` writes, packed like
/// `npk_screen_size`. The screen size apps lay out against is logical.
pub(crate) fn npk_capture_size(ctx: &mut HostState) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::CAPTURE, "npk_capture_size") {
        return 0;
    }
    let info = crate::framebuffer::get_info();
    (((info.width & 0xFFFF) << 16) | (info.height & 0xFFFF)) as i32
}

/// Window geometry crosses the ABI in logical px.
fn phys(v: i32) -> i32 { crate::shade::widgets::scale::len_i(v) }
fn logical(v: u32) -> u32 { crate::shade::widgets::scale::logical(v) }

pub(crate) fn npk_ticks(_ctx: &mut HostState) -> i64 {
    (crate::time::ticks() as i64).saturating_mul(10)
}

pub(crate) fn npk_now_us(_ctx: &mut HostState) -> i64 {
    let f = crate::time::counter_hz();
    if f == 0 { return 0; }
    ((crate::time::counter() as u128 * 1_000_000) / f as u128) as i64
}

pub(crate) fn npk_unix_time(_ctx: &mut HostState) -> i64 {
    crate::time::now_or_zero() as i64
}

/// Fills module memory from the kernel CSPRNG.
///
/// Not capability-gated, like `npk_unix_time`: randomness is not a user
/// resource and reveals nothing about the user.
///
/// The source is the same ChaCha20 stream used for capability tokens
/// (seeded from RDRAND, rekeyed every 64 blocks). There is deliberately no
/// weaker second source: page code builds session tokens from
/// `crypto.getRandomValues`.
///
/// Returns the number of bytes written, or -1.
pub(crate) fn npk_random_bytes(mem: &mut [u8], _ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if buf_ptr < 0 || len <= 0 { return -1; }
    // Same 64 KiB cap as WebCrypto 10.1.1; without it a module could hold
    // the RNG mutex arbitrarily long.
    if len > 65_536 { return -1; }
    let Some(dst) = guest_mut(mem, buf_ptr, len as usize) else { return -1 };
    crate::security::csprng::fill(dst);
    len
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_theme_token(ctx: &mut HostState, token_id: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_theme_token") {
        return 0;
    }
    // The only token table lives in palette.rs; do not copy it here.
    if token_id < 0 { return 0; }
    let token = match crate::shade::widgets::palette::token_from_id(token_id as usize) {
        Some(t) => t,
        None => return 0,
    };
    // The palette the caller's window is drawn with: panels are glass.
    crate::shade::widgets::palette::for_window(ctx.widget_window_id).colors[token as u8 as usize] as i32
}

pub(crate) fn npk_cursor_pos(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_cursor_pos") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    if crate::shade::focused_widget_id() != Some(wid) { return -1; }
    let (x, y) = crate::shade::cursor::atomic_pos();
    if x < 0 || y < 0 || x > 0xFFFF || y > 0xFFFF { return -1; }
    (x << 16) | y
}

pub(crate) fn npk_screen_flash(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::CAPTURE, "npk_screen_flash") {
        return -1;
    }
    // Worker cores may only set the state; core 0 ticks and paints
    // it from poll_render.
    crate::shade::with_compositor(|comp| comp.start_flash());
    crate::shade::request_render();
    0
}

pub(crate) fn npk_window_set_overlay(ctx: &mut HostState, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_window_set_overlay") {
        return -1;
    }
    if w <= 0 || h <= 0 { return -1; }
    let (w, h) = (phys(w), phys(h));

    let mut wid = ctx.widget_window_id;
    if wid == 0 {
        // Prefer promoting the spawning terminal to a widget so
        // the app owns a single window. Only create a fresh one
        // if no terminal backed this worker (direct-launch path).
        let terminal_idx = ctx.terminal_idx;
        let promoted = if terminal_idx != 255 {
            crate::shade::with_compositor(|c|
                c.promote_terminal_to_widget(terminal_idx)
            ).flatten()
        } else {
            None
        };

        let new_id = match promoted {
            Some(id) => {
                ctx.terminal_idx = 255;
                // Overlay path wants focus on the new widget; promotion
                // does not focus, so fix up.
                crate::shade::with_compositor(|comp| comp.focus_window(id));
                id.0
            }
            None => {
                let title = ctx.module_name.clone();
                match crate::shade::with_compositor(|comp| {
                    let id = comp.create_widget_window(
                        if title.is_empty() { "widget" } else { title.as_str() });
                    comp.focus_window(id);
                    id.0
                }) {
                    Some(v) => v,
                    None => return -1,
                }
            }
        };
        ctx.widget_window_id = new_id;
        wid = new_id;
    }

    let anchor = take_launch_anchor(&ctx.module_name);
    let ok = crate::shade::with_compositor(|comp| {
        let ok = comp.set_overlay(crate::shade::WindowId(wid), w as u32, h as u32);
        if ok {
            if let Some(rect) = anchor {
                let id = crate::shade::WindowId(wid);
                comp.place_popup(id, rect, w as u32, h as u32, phys(LAUNCH_GAP) as u32);
                comp.set_light_dismiss(id, true);
            }
        }
        // The overlay path always wants this window focused — a
        // launcher style app drives keyboard from
        // here. The branch above already focuses, but a repeated
        // set_overlay (or a focus shift in between) must re-claim it
        // so keys are not routed to a stale window.
        if ok {
            comp.focus_window(crate::shade::WindowId(wid));
        }
        ok
    }).unwrap_or(false);

    if ok {
        crate::shade::request_render();
        0
    } else {
        -1
    }
}

pub(crate) fn npk_window_set_modal(ctx: &mut HostState, modal: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_window_set_modal") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    let ok = crate::shade::with_compositor(|comp|
        comp.set_modal(crate::shade::WindowId(wid), modal != 0)
    ).unwrap_or(false);
    if ok { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_window_set_overlay_at(ctx: &mut HostState, x: i32, y: i32, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_window_set_overlay_at") {
        return -1;
    }
    if w <= 0 || h <= 0 || x < 0 || y < 0 { return -1; }
    let (x, y, w, h) = (phys(x), phys(y), phys(w), phys(h));

    let mut wid = ctx.widget_window_id;
    if wid == 0 {
        let terminal_idx = ctx.terminal_idx;
        let promoted = if terminal_idx != 255 {
            crate::shade::with_compositor(|c|
                c.promote_terminal_to_widget(terminal_idx)
            ).flatten()
        } else {
            None
        };
        let new_id = match promoted {
            Some(id) => {
                ctx.terminal_idx = 255;
                crate::shade::with_compositor(|comp| comp.focus_window(id));
                id.0
            }
            None => {
                let title = ctx.module_name.clone();
                match crate::shade::with_compositor(|comp| {
                    let id = comp.create_widget_window(
                        if title.is_empty() { "widget" } else { title.as_str() });
                    comp.focus_window(id);
                    id.0
                }) {
                    Some(v) => v,
                    None => return -1,
                }
            }
        };
        ctx.widget_window_id = new_id;
        wid = new_id;
    }

    let ok = crate::shade::with_compositor(|comp| {
        let ok = comp.set_overlay_at(crate::shade::WindowId(wid),
            x, y, w as u32, h as u32);
        if ok { comp.focus_window(crate::shade::WindowId(wid)); }
        ok
    }).unwrap_or(false);

    if ok { crate::shade::request_render(); 0 } else { -1 }
}

pub(crate) fn npk_window_set_light_dismiss(ctx: &mut HostState, on: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_window_set_light_dismiss") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    let ok = crate::shade::with_compositor(|comp|
        comp.set_light_dismiss(crate::shade::WindowId(wid), on != 0)
    ).unwrap_or(false);
    if ok { 0 } else { -1 }
}

pub(crate) fn npk_window_set_clipboard_sink(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_window_set_clipboard_sink") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    crate::shade::widgets::set_clipboard_sink(wid);
    0
}

pub(crate) fn npk_window_set_panel(ctx: &mut HostState, edge: i32, behavior: i32, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_window_set_panel") {
        return -1;
    }
    // Only the bottom strut panel exists.
    if w <= 0 || h <= 0 || (edge, behavior) != (0, 1) { return -1; }
    let (w, h) = (phys(w), phys(h));
    // A panel is a strip along an edge, and its size becomes a pixel buffer
    // on the next commit: clamp it to a strip of the screen.
    let fb = crate::framebuffer::get_info();
    let w = (w as u32).min(fb.width) as i32;
    let h = (h as u32).min(fb.height / 4) as i32;

    let mut wid = ctx.widget_window_id;
    if wid == 0 {
        let terminal_idx = ctx.terminal_idx;
        let promoted = if terminal_idx != 255 {
            crate::shade::with_compositor(|c|
                c.promote_terminal_to_widget(terminal_idx)
            ).flatten()
        } else {
            None
        };
        let new_id = match promoted {
            Some(id) => { ctx.terminal_idx = 255; id.0 }
            None => {
                let title = ctx.module_name.clone();
                match crate::shade::with_compositor(|comp| {
                    comp.create_widget_window(
                        if title.is_empty() { "panel" } else { title.as_str() }).0
                }) {
                    Some(v) => v,
                    None => return -1,
                }
            }
        };
        ctx.widget_window_id = new_id;
        wid = new_id;
    }

    let ok = crate::shade::with_compositor(|comp|
        comp.set_panel(crate::shade::WindowId(wid),
            edge as u8, behavior as u8, w as u32, h as u32)
    ).unwrap_or(false);

    if ok { crate::shade::request_render(); 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
/// Reads one byte from an ACPI SystemMemory operation region.
///
/// Some firmware talks to its embedded controller through a memory-mapped
/// window instead of the ISA ports (Lenovo IdeaPad battery `_STA`/`_BST`
/// read `0xFE800008`). Without this the interpreter would return 0 and the
/// firmware would conclude there is no battery.
///
/// Sandbox bounds:
///  * `Rights::HARDWARE`, the same right as for the EC ports.
///  * Read only. Writes to arbitrary MMIO could reprogram devices; they
///    stay in the interpreter's scratch space.
///  * Never RAM. Any address inside a usable RAM range is refused (kernel,
///    heap and every module's linear memory live there). Without a memory
///    map, everything counts as RAM.
pub(crate) fn npk_acpi_mem_read(ctx: &mut HostState, hi: i32, lo: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_acpi_mem_read") {
        return -1;
    }
    let addr = ((hi as u32 as u64) << 32) | (lo as u32 as u64);
    if !crate::memory::is_device_window(addr) {
        // Log only the first few: the driver polls forever.
        use core::sync::atomic::{AtomicU32, Ordering};
        static REFUSED: AtomicU32 = AtomicU32::new(0);
        let n = REFUSED.fetch_add(1, Ordering::Relaxed);
        if n < 3 {
            kprintln!("[npk] acpi: refused SystemMemory read at {:#x} — that is RAM", addr);
        } else if n == 3 {
            kprintln!("[npk] acpi: (further SystemMemory refusals silenced)");
        }
        return -1;
    }
    match crate::paging::read_phys_u8(addr) {
        Some(v) => v as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// Takes a pending EC query. -1 = nothing pending or no right.
pub(crate) fn npk_ec_query(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_ec_query") {
        return -1;
    }
    match crate::ec::query() {
        Some(q) => q as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ec_read(ctx: &mut HostState, addr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_ec_read") {
        return -1;
    }
    if !(0..=255).contains(&addr) { return -1; }
    match crate::ec::read(addr as u8) {
        Some(v) => v as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ec_write(ctx: &mut HostState, addr: i32, val: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_ec_write") {
        return -1;
    }
    if !(0..=255).contains(&addr) || !(0..=255).contains(&val) { return -1; }
    if crate::ec::write(addr as u8, val as u8) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_battery_report(ctx: &mut HostState, packed: i32) {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_battery_report") {
        return;
    }
    crate::battery::report(packed);
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_battery_detail(ctx: &mut HostState, rate: i32, remaining: i32,
    full: i32, voltage_mv: i32, unit: i32) {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_battery_detail") {
        return;
    }
    crate::battery::report_detail(crate::battery::Detail {
        rate: rate as u32, remaining: remaining as u32, full: full as u32,
        voltage_mv: voltage_mv as u32, unit: unit as u32,
    });
}

/// Audio slots belong to the module that opened them (`ctx.pid`); submit,
/// buffered and close answer only to that module. pid 0 is the shared
/// identity of the inline execution paths, not an owner (same rule as
/// `fetch::submit`).
pub(crate) fn npk_audio_open(ctx: &mut HostState) -> i32 {
    if ctx.pid == 0 || crate::ext::is_instance(&ctx.cap_id) { return -1; }
    crate::audio::open_for(ctx.pid)
}

#[cfg(target_arch = "x86_64")]
/// The format of the caller's stream `slot` (`audio::Encoding` numbering);
/// empties its ring. -1 for a foreign slot or a format the mixer does not
/// take.
pub(crate) fn npk_audio_format(ctx: &mut HostState, slot: i32, rate: i32, encoding: i32, channels: i32) -> i32 {
    let Some(fmt) = audio_format(rate, encoding, channels) else { return -1 };
    if slot < 0 { return -1; }
    if crate::audio::set_format_for(slot as usize, ctx.pid, fmt) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
/// The audio driver's device format, what `npk_audio_poll_mix` produces.
/// Requires `Rights::HARDWARE`, like the poll itself.
pub(crate) fn npk_audio_device_format(ctx: &mut HostState, rate: i32, encoding: i32, channels: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_audio_device_format") {
        return -1;
    }
    let Some(fmt) = audio_format(rate, encoding, channels) else { return -1 };
    if crate::audio::set_device_format(ctx.pid, fmt) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
fn audio_format(rate: i32, encoding: i32, channels: i32) -> Option<crate::audio::Format> {
    let encoding = crate::audio::Encoding::from_abi(encoding)?;
    let rate = u32::try_from(rate).ok()?;
    let channels = u8::try_from(channels).ok()?;
    Some(crate::audio::Format { rate, encoding, channels })
}

pub(crate) fn npk_audio_close(ctx: &mut HostState, slot: i32) -> i32 {
    if slot < 0 { return -1; }
    if crate::audio::close_for(slot as usize, ctx.pid) { 0 } else { -1 }
}

/// The system volume: the shell's control, or the app the user is using.
pub(crate) fn npk_audio_set_volume(ctx: &mut HostState, pct: i32) -> i32 {
    let shell = capability::holds(&ctx.cap_id, capability::Rights::SHELL);
    if !shell && !app_is_focused(ctx) { return -1; }
    if pct < 0 { return -1; }
    crate::audio::set_volume(pct.min(100) as u8);
    0
}

pub(crate) fn npk_audio_get_volume(_ctx: &mut HostState) -> i32 {
 crate::audio::get_volume() as i32 
}

pub(crate) fn npk_workspace_switch(ctx: &mut HostState, n: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_workspace_switch") {
        return -1;
    }
    if !(0..=255).contains(&n) { return -1; }
    crate::shade::with_compositor(|c| c.switch_workspace(n as u8));
    crate::shade::request_render();
    0
}

/// Popups an app may hold open at once (`npk_popup`'s number).
const MAX_POPUPS: i32 = 8;

/// Open or update popup `popup` of the app's window: a window of its own,
/// as large as `ptr..ptr+len` (a scene tree) wants, next to anchor node
/// `anchor` in the app's own tree, `gap` px away.
pub(crate) fn npk_popup(mem: &mut [u8], ctx: &mut HostState, popup: i32, anchor: i32, gap: i32,
    ptr: i32, len: i32) -> i32
{
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_popup") {
        return -1;
    }
    let parent = ctx.widget_window_id;
    if parent == 0 || !(0..MAX_POPUPS).contains(&popup) || !(0..=256).contains(&gap) { return -1; }
    // The same cap as a scene commit.
    const MAX_SCENE_BYTES: usize = 16 * 1024 * 1024;
    let payload: alloc::vec::Vec<u8> = match glen(len)
        .filter(|&n| n <= MAX_SCENE_BYTES)
        .and_then(|n| guest(mem, ptr, n))
    {
        Some(b) if !b.is_empty() => match copy_out(b) {
            Some(v) => v,
            None => return -1,
        },
        _ => return -1,
    };
    let Some((w, h)) = crate::shade::widgets::commit_size(&payload) else { return -1 };
    let Some(rect) = crate::shade::widgets::anchor_rect(parent, anchor as u32) else { return -1 };
    let parent_id = crate::shade::WindowId(parent);
    let slot = popup as u8;
    let id = crate::shade::with_compositor(|c| {
        let id = c.popup_window(parent_id, slot).or_else(|| c.open_popup(parent_id, slot))?;
        c.place_popup(id, rect, w, h, phys(gap) as u32);
        Some(id)
    }).flatten();
    let Some(id) = id else { return -1 };
    let module_name = ctx.module_name.clone();
    let r = crate::shade::widgets::scene_commit(&payload, id.0, &module_name);
    crate::shade::request_render();
    if r >= 0 { 0 } else { -1 }
}

/// Close popup `popup` of the app's window. No `PopupDone`: the app asked.
pub(crate) fn npk_popup_close(ctx: &mut HostState, popup: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_popup_close") {
        return -1;
    }
    let parent = ctx.widget_window_id;
    if parent == 0 || !(0..MAX_POPUPS).contains(&popup) { return -1; }
    crate::shade::with_compositor(|c| {
        if let Some(id) = c.popup_window(crate::shade::WindowId(parent), popup as u8) {
            c.close_popup(id, false);
        }
    });
    crate::shade::request_render();
    0
}

pub(crate) fn npk_window_activate(ctx: &mut HostState, id: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_window_activate") {
        return -1;
    }
    if id <= 0 { return -1; }
    let ok = crate::shade::with_compositor(|c|
        c.activate_window(crate::shade::WindowId(id as u32))
    ).unwrap_or(false);
    if ok { crate::shade::request_render(); 0 } else { -1 }
}

pub(crate) fn npk_power(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_power") {
        return -1;
    }
    crate::shutdown::request(crate::shutdown::Kind::PowerOff);
    0
}

pub(crate) fn npk_restart(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL, "npk_restart") {
        return -1;
    }
    crate::shutdown::request(crate::shutdown::Kind::Restart);
    0
}

pub(crate) fn npk_close_widget(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_close_widget") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    crate::shade::with_compositor(|comp| {
        comp.close_window(crate::shade::WindowId(wid));
    });
    crate::shade::request_render();
    0
}

pub(crate) fn npk_get_fb_size(ctx: &mut HostState) -> i64 {
    // Same rights as `npk_screen_size`.
    let ok = capability::holds(&ctx.cap_id, capability::Rights::RENDER)
        || capability::holds(&ctx.cap_id, capability::Rights::CAPTURE);
    if !ok { return 0; }
    let (w, h) = crate::framebuffer::get_resolution();
    ((w as i64) << 32) | (h as i64)
}

pub(crate) fn npk_sys_info(ctx: &mut HostState, key: i32) -> i64 {
    match key & 0xFF {
        0 => crate::smp::per_core::core_count() as i64,
        1 => crate::time::uptime_secs() as i64,
        2 => { let (_, mb) = crate::memory::stats(); mb as i64 },
        3 => { let (used, _) = crate::heap::stats(); used as i64 },
        4 => { let (_, total) = crate::heap::stats(); total as i64 },
        5 => { let (s, _, _, _) = crate::smp::scheduler::stats(); s as i64 },
        6 => { let (_, c, _, _) = crate::smp::scheduler::stats(); c as i64 },
        7 => { let (_, _, st, _) = crate::smp::scheduler::stats(); st as i64 },
        8 => { let (_, _, _, w) = crate::smp::scheduler::stats(); w as i64 },
        #[cfg(target_arch = "x86_64")]
        9 => if crate::smp::per_core::has_mwait() { 1 } else { 0 },
        #[cfg(not(target_arch = "x86_64"))]
        9 => 0,
        10 => (crate::time::counter_hz() / 1_000_000) as i64,
        11 => {
            let core = (key >> 8) as usize;
            crate::smp::scheduler::queue_len(core) as i64
        },
        12 => {
            let core = (key >> 8) as usize;
            crate::smp::per_core::update_core_freq(core);
            crate::smp::per_core::core_freq_mhz(core) as i64
        },
        #[cfg(target_arch = "x86_64")]
        13 => crate::smp::per_core::max_turbo_mhz() as i64,
        #[cfg(target_arch = "x86_64")]
        14 => crate::smp::per_core::min_eff_mhz() as i64,
        15 => {
            let core = (key >> 8) as usize;
            crate::smp::per_core::update_core_freq(core);
            crate::smp::per_core::core_usage(core) as i64
        },
        // CPUID 0x15 raw values for diagnostics
        #[cfg(target_arch = "x86_64")]
        16 => { let (eax, _, _) = crate::arch::x86_64::time::cpuid15(); eax as i64 },
        #[cfg(target_arch = "x86_64")]
        17 => { let (_, ebx, _) = crate::arch::x86_64::time::cpuid15(); ebx as i64 },
        #[cfg(target_arch = "x86_64")]
        18 => { let (_, _, ecx) = crate::arch::x86_64::time::cpuid15(); ecx as i64 },

        // 19 → the raw counter (x86 TSC); combine with key 10 (its MHz) to
        // convert to time. The sign bit stays clear for ~150 years at 2 GHz.
        19 => crate::time::counter() as i64,

        // ── Process tracking (keys 20-29) → process table ──
        // 20: count, 21: pid_at_index, 22-29: query by PID
        20..=29 => crate::process::sys_info(key),

        // ── Bench probes (keys 30-34) → cached on first call ──
        // 30: BLAKE3 MB/s, 31: AES-GCM enc MB/s,
        // 32: AES-GCM dec(in-place) MB/s,
        // 33: raw blkdev write MB/s, 34: raw blkdev read MB/s.
        // The first call runs the measurement; results live in
        // BENCH_CACHE until reboot.
        // Benchmarks write raw blocks and the check holds the file system
        // lock for a full scan: disk tooling with WRITE only.
        30..=34 | 40 if !capability::require(&ctx.cap_id, capability::Rights::WRITE, "npk_sys_info") => -1,
        30..=34 => bench_sys_info(key),

        // ── fsck self-check (key 40) → read-only integrity scan ──
        // Runs on every call (not cached), logs a full report to
        // serial, and returns the total problem count (0 = clean,
        // -1 = scan error). testdisk calls it at the end of its run so
        // corruption surfaces before a reboot would fail the mount.
        40 => fsck_sys_info(),

        // ── 50: should drivers print their diagnostic lines? ──
        // Raw descriptors, register traces and counters are kept in the
        // drivers but stay quiet in normal operation. A driver asks once
        // at start. Default off; `set log.drivers 1` enables them.
        50 => if crate::config::get("log.drivers").as_deref() == Some("1") { 1 } else { 0 },

        _ => -1,
    }
}

/// An app may wake on another core than it parked on (`fiber::steal_for`):
/// account its time and list it there from now on.
fn follow_core(ctx: &mut HostState) {
    let cid = crate::smp::per_core::current_core_id();
    if cid != ctx.core_id {
        ctx.core_id = cid;
        crate::process::set_core(ctx.pid, cid);
    }
}

pub(crate) fn npk_sleep(ctx: &mut HostState, ms: i32) -> i32 {
    if ms <= 0 || ms > 60000 { return -1; }

    // The normal path: we run inside a fiber → yield to the scheduler.
    if crate::smp::fiber::yield_sleep(ms as u64) {
        follow_core(ctx);
        return 0;
    }

    // Fallback: not inside a fiber (degenerate no-worker host, or a
    // one-shot wasm on Core 0) → HLT-idle until the deadline. No
    // core-stealing helper: that would risk nesting.
    let freq = crate::time::counter_hz();
    let target = crate::time::counter() + (ms as u64) * (freq / 1000);
    while crate::time::counter() < target {
        crate::smp::per_core::halt_until(Some(target), crate::smp::per_core::WAKE_NPK_SLEEP);
    }

    0
}

/// `npk_wait` mask bit: an input event is waiting — a widget event
/// (`npk_event_poll`), a key in the terminal buffer (`npk_input_poll`), or
/// the app's window is gone.
const WAIT_INPUT: i32 = 1;
/// Driver: the device IRQ it registered (`npk_irq_register`) fired since
/// `npk_wait` last reported it.
const WAIT_IRQ: i32 = 2;
/// Driver registered as the WASM NIC: the IP stack queued a frame.
const WAIT_NET_TX: i32 = 4;
/// Driver: wifid queued a command (`npk_wifi_poll_cmd`).
const WAIT_WIFI_CMD: i32 = 8;
/// Manager (NETCTL): the driver queued an event (`npk_wifi_poll_event`).
const WAIT_WIFI_EVENT: i32 = 16;
/// A watched topic changed — windows, battery, volume (`crate::notify`).
/// RENDER-gated like the calls that read them (`npk_bar_state`).
const WAIT_STATE: i32 = 32;
/// Driver registered as the GPU service: the compositor queued a job
/// (`npk_gpudev_poll`), or the audio driver a display audio request
/// (`npk_display_audio_serve`).
const WAIT_GPU_CMD: i32 = 64;

/// Display audio waits for this module: it drives the display controller
/// whose HDMI/DP audio the kernel has queued work for.
fn display_audio_pending(ctx: &HostState) -> bool {
    #[cfg(target_arch = "x86_64")]
    return owned_pci_class(ctx) == Some(0x03) && crate::audio::display_audio_pending();
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = ctx;
        false
    }
}

/// Which of `mask`'s conditions hold right now. Bits the caller may not
/// wait on (a driver bit without a driver, an event bit without NETCTL)
/// never fire.
fn wait_ready(ctx: &mut HostState, mask: i32) -> i32 {
    let mut r = 0;
    if mask & WAIT_INPUT != 0 && input_ready(ctx) {
        r |= WAIT_INPUT;
    }
    let display_audio = mask & WAIT_GPU_CMD != 0 && display_audio_pending(ctx);
    if let Some(hw) = ctx.hw.as_mut() {
        if mask & WAIT_IRQ != 0 {
            for (irq, seen) in hw.irqs.iter_mut() {
                let n = crate::irq::fired_count(*irq);
                if n != *seen {
                    *seen = n;
                    r |= WAIT_IRQ;
                }
            }
        }
        if mask & WAIT_NET_TX != 0 && hw.registered_as_netdev
            && crate::netdev::wasm_nic_tx_pending()
        {
            r |= WAIT_NET_TX;
        }
        if mask & WAIT_WIFI_CMD != 0 && hw.registered_as_netdev && crate::wifi::cmd_pending() {
            r |= WAIT_WIFI_CMD;
        }
        if mask & WAIT_GPU_CMD != 0 && let Some(dev) = hw.dev
            && (crate::gpu::gpudev::pending(dev) || display_audio)
        {
            r |= WAIT_GPU_CMD;
        }
    }
    if mask & WAIT_WIFI_EVENT != 0
        && is_wifi_manager(ctx)
        && crate::wifi::event_pending()
    {
        r |= WAIT_WIFI_EVENT;
    }
    if mask & WAIT_STATE != 0 {
        if let Some(w) = crate::smp::fiber::current_waker() {
            if crate::notify::take(w) != 0 {
                r |= WAIT_STATE;
            }
        }
    }
    r
}

/// Is input waiting for this app? Also true once its widget window is
/// gone, so a parked app wakes and leaves its loop.
fn input_ready(ctx: &HostState) -> bool {
    let wid = ctx.widget_window_id;
    if wid != 0
        && (crate::shade::widgets::has_event(wid)
            || !crate::shade::widgets::widget_window_exists(wid))
    {
        return true;
    }
    crate::wasm::has_app_key(ctx.terminal_idx)
}

/// `npk_wait(mask, timeout_ms)` — park until something in `mask` happens,
/// or `timeout_ms` passes (< 0: no timeout). Returns the bits that fired,
/// 0 on timeout. Bits: `WAIT_INPUT` 1, `WAIT_IRQ` 2, `WAIT_NET_TX` 4,
/// `WAIT_WIFI_CMD` 8, `WAIT_WIFI_EVENT` 16, `WAIT_STATE` 32, `WAIT_GPU_CMD` 64.
///
/// The event-driven replacement for `loop { poll; npk_sleep(16) }`: the
/// app's fiber gives up its core and costs nothing until an event is pushed
/// for it (`widgets::push_event`, `wasm::push_app_key` signal it) or its
/// deadline comes.
///
/// Every bit is gated by what the caller already owns: the IRQ bit by the
/// vector its driver registered, the NIC bit by being the registered WASM
/// NIC, the command bit by being a driver, the event bit by NETCTL — the
/// same rights the matching `npk_*_poll` calls check. It parks only the
/// caller's own fiber.
pub(crate) fn npk_wait(ctx: &mut HostState, mask: i32, timeout_ms: i32) -> i32 {
    let freq = crate::time::counter_hz();
    let deadline = if timeout_ms < 0 {
        crate::smp::fiber::NO_DEADLINE
    } else {
        crate::time::counter() + (timeout_ms as u64) * (freq / 1000)
    };
    // Register where the app's input will be pushed. The widget window is
    // created lazily by the app's first scene commit, so do it here, every
    // time — a map insert and a store, at the rate the app waits.
    use crate::smp::fiber::{SIG_EVENT, SIG_IRQ, SIG_TX, SIG_WIFI};
    let mut sig = 0;
    if let Some(w) = crate::smp::fiber::current_waker() {
        if mask & WAIT_INPUT != 0 {
            if ctx.widget_window_id != 0 {
                crate::shade::widgets::set_event_waker(ctx.widget_window_id, w);
            }
            crate::wasm::set_key_waker(ctx.terminal_idx, w);
            sig |= SIG_EVENT;
        }
        if let Some(hw) = ctx.hw.as_ref() {
            if mask & WAIT_IRQ != 0 && !hw.irqs.is_empty() {
                // The interrupts must wake this core, and a level line the
                // ISR masked is released: the driver waits again, so it has
                // serviced the device (`irq::arm`).
                for &(irq, _) in &hw.irqs {
                    let _ = crate::irq::arm(irq);
                    crate::irq::set_waiter(irq, w);
                }
                sig |= SIG_IRQ;
            }
            if mask & WAIT_NET_TX != 0 && hw.registered_as_netdev {
                crate::netdev::set_nic_waker(w);
                sig |= SIG_TX;
            }
            if mask & WAIT_WIFI_CMD != 0 && hw.registered_as_netdev {
                crate::wifi::set_cmd_waker(w);
                sig |= SIG_WIFI;
            }
            if mask & WAIT_GPU_CMD != 0 && let Some(dev) = hw.dev {
                crate::gpu::gpudev::set_waker(dev, w);
                sig |= crate::smp::fiber::SIG_GPU;
            }
        }
        if mask & WAIT_WIFI_EVENT != 0 && is_wifi_manager(ctx) {
            crate::wifi::set_event_waker(w);
            sig |= SIG_WIFI;
        }
        if mask & WAIT_STATE != 0
            && capability::holds(&ctx.cap_id, capability::Rights::RENDER)
        {
            crate::notify::subscribe(w);
            sig |= crate::smp::fiber::SIG_STATE;
        }
    }
    let flushed = crate::smp::per_core::flush_busy(ctx.core_id);
    crate::process::add_busy_tsc(ctx.pid, flushed);
    let fired = loop {
        let r = wait_ready(ctx, mask);
        if r != 0 {
            break r;
        }
        if crate::time::counter() >= deadline {
            break 0;
        }
        if crate::smp::fiber::wait(sig, deadline).is_none() {
            // Not in a fiber: halt in place, looking again every 10 ms.
            let recheck = crate::time::counter() + freq / 100;
            crate::smp::per_core::halt_until(
                Some(deadline.min(recheck)), crate::smp::per_core::WAKE_NPK_SLEEP);
        }
    };
    follow_core(ctx);
    crate::smp::per_core::start_work(ctx.core_id);
    fired
}

pub(crate) fn npk_input_poll(ctx: &mut HostState) -> i32 {
    match pop_app_key(ctx.terminal_idx) {
        Some(k) => k as i32,
        None => -1,
    }
}

pub(crate) fn npk_input_wait(ctx: &mut HostState, timeout_ms: i32) -> i32 {
    let term_idx = ctx.terminal_idx;
    let core_id = ctx.core_id;
    if timeout_ms <= 0 {
        return match pop_app_key(term_idx) {
            Some(k) => k as i32,
            None => -1,
        };
    }

    // Flush work done since last checkpoint, update process table
    let flushed = crate::smp::per_core::flush_busy(core_id);
    crate::process::add_busy_tsc(ctx.pid, flushed);
    crate::smp::per_core::update_core_freq(core_id);
    crate::smp::per_core::set_active(core_id, false);

    let ms = (timeout_ms as u64).min(60_000);
    let freq = crate::time::counter_hz();
    let ticks_per_ms = freq / 1000;
    let deadline = crate::time::counter() + ms * ticks_per_ms;

    if let Some(w) = crate::smp::fiber::current_waker() {
        crate::wasm::set_key_waker(term_idx, w);
    }
    let result = loop {
        if let Some(k) = pop_app_key(term_idx) {
            break k as i32;
        }
        if crate::time::counter() >= deadline {
            break -1;
        }
        // Park until a key is pushed (it signals this fiber) or the
        // deadline. Halting here would hold the whole core and starve other
        // fibers on it.
        if crate::smp::fiber::wait(crate::smp::fiber::SIG_EVENT, deadline).is_none() {
            // Not in a fiber (a one-shot on Core 0): halt in place.
            let recheck = crate::time::counter() + freq / 100;
            crate::smp::per_core::halt_until(
                Some(deadline.min(recheck)), crate::smp::per_core::WAKE_NPK_SLEEP);
        }
    };

    // Resume work tracking, on the core it woke on.
    follow_core(ctx);
    crate::smp::per_core::set_active(ctx.core_id, true);
    crate::smp::per_core::start_work(ctx.core_id);

    result
}

pub(crate) fn npk_clear(ctx: &mut HostState) {
    let idx = ctx.terminal_idx;
    if (idx as usize) < MAX_APP_BUFS {
        crate::shade::terminal::clear_idx(idx as usize);
    } else {
        crate::shade::terminal::clear();
    }
}

pub(crate) fn npk_self_terminal(ctx: &mut HostState) -> i32 {
    ctx.terminal_idx as i32
}

/// Terminal output sinks read what any terminal prints, which is a remote
/// console's job and nobody else's.
fn stream_allowed(ctx: &HostState) -> bool {
    capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "terminal stream")
}

pub(crate) fn npk_stream_open(ctx: &mut HostState, idx: i32) -> i32 {
    if !stream_allowed(ctx) { return -1; }
    // -1 = the everything-sink: every write, whichever terminal it was
    // routed to. A remote console bound to one index goes silent as soon
    // as output is redirected elsewhere.
    if idx < 0 {
        crate::serial::mirror_open();
        return 0;
    }
    if crate::shade::terminal::stream_open(idx as usize) { return 0; }
    -1
}

pub(crate) fn npk_stream_close(ctx: &mut HostState, idx: i32) -> i32 {
    if !stream_allowed(ctx) { return -1; }
    if idx < 0 {
        crate::serial::mirror_close();
    } else {
        crate::shade::terminal::stream_close(idx as usize);
    }
    0
}

/// Injects a key byte into the shell's input queue.
///
/// Requires `Rights::HARDWARE`: the shell executes what arrives there with
/// its own authority, so an ungated injection would let any module run
/// commands.
pub(crate) fn npk_key_inject(ctx: &mut HostState, byte: i32) -> i32 {
    // Typed keys reach whatever has the focus, so only the remote console
    // may inject them, not every module that drives hardware.
    if ctx.module_name != "debug" {
        return -1;
    }
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_key_inject") {
        return -1;
    }
    crate::input::inject_byte((byte & 0xFF) as u8);
    0
}

/// A raw socket is at least as powerful as `npk_http_*` and needs the same
/// right.
///
/// `forge_glue::resolve` binds imports by name and does not check them
/// against capabilities at load time, so every socket call must check
/// `NET` itself. A module without `.npk.caps` gets `READ | EXECUTE |
/// RENDER`, i.e. no `NET`.
fn net_allowed(ctx: &mut HostState) -> bool {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        crate::kprintln!("[npk] tcp: module {} has no NET right ({:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "socket", e.reason());
        return false;
    }
    true
}

// ── TLS stream socket ────────────────────────────────────────────────────
//
// Exposes `crypto::tls` (`tls_connect`/`tls_send`/`tls_poll`/`tls_close` on
// a plain `tcp_handle`) to modules, for `wss://`. The WebSocket handshake
// and framing belong in the engine, not here.
//
// Unlike the `npk_http_*` job system (a request with a start and an end),
// this is a long-lived, mostly idle connection, hence a slot table instead
// of a queue.

#[cfg(target_arch = "x86_64")]
struct TlsSlot {
    /// Owner. A handle is served only to the process that opened it, as
    /// with the HTTP handles.
    pid: u32,
    /// `None` while the handshake runs or a send/poll has it checked out:
    /// those may yield the fiber, so they run without the table lock.
    session: Option<crate::crypto::tls::TlsSession>,
    /// Closed while checked out; whoever returns the session closes it.
    closed: bool,
}

/// Grows with the connections; how many one module holds is bounded by its
/// TCP connections (`tcp::connect_as`).
#[cfg(target_arch = "x86_64")]
static TLS_SLOTS: spin::Mutex<alloc::vec::Vec<Option<TlsSlot>>> =
    spin::Mutex::new(alloc::vec::Vec::new());

#[cfg(target_arch = "x86_64")]
/// `cap` reaches the host of `url`; the journal keeps the host, not the
/// path, which may carry what the user typed.
fn journal_url(cap: &crate::capability::CapId, url: &str) {
    if let Ok((host, _, _)) = crate::intent::http::parse_url(url) {
        crate::audit::record(cap, crate::audit::Kind::Net, &host);
    }
}

/// The TCP owner a module's connections are charged to. A run without a
/// process entry (pid 0) is still a module, never the kernel.
fn tcp_owner(ctx: &HostState) -> crate::net::tcp::Owner {
    if ctx.pid != 0 { ctx.pid } else { u32::MAX }
}

#[cfg(target_arch = "x86_64")]
/// Common gate for every access to an existing handle.
fn tls_slot_ok(ctx: &mut HostState, handle: i32) -> Option<usize> {
    if !net_allowed(ctx) { return None }
    let i = usize::try_from(handle).ok()?;
    let g = TLS_SLOTS.lock();
    match g.get(i)? {
        Some(sl) if sl.pid == ctx.pid => Some(i),
        _ => None,
    }
}

#[cfg(target_arch = "x86_64")]
/// Close every TLS handle of module `pid` (it has ended). A session checked
/// out by a running call cannot be: there is none once the module is gone.
pub(crate) fn tls_release_owner(pid: u32) {
    let mut sessions = alloc::vec::Vec::new();
    {
        let mut g = TLS_SLOTS.lock();
        for slot in g.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.pid == pid) {
                if let Some(sl) = slot.take() {
                    if let Some(sess) = sl.session { sessions.push(sess); }
                }
            }
        }
    }
    for mut s in sessions {
        let _ = crate::crypto::tls::tls_close(&mut s);
    }
}

#[cfg(target_arch = "x86_64")]
/// Check the session of slot `i` out of the table; `None` if it is busy.
fn tls_checkout(i: usize, pid: u32) -> Option<crate::crypto::tls::TlsSession> {
    let mut g = TLS_SLOTS.lock();
    match g[i].as_mut() {
        Some(sl) if sl.pid == pid && !sl.closed => sl.session.take(),
        _ => None,
    }
}

#[cfg(target_arch = "x86_64")]
/// Return a checked-out session, or close it if the handle was closed
/// meanwhile.
fn tls_checkin(i: usize, pid: u32, mut session: crate::crypto::tls::TlsSession) {
    let leftover = {
        let mut g = TLS_SLOTS.lock();
        match g[i].as_mut() {
            Some(sl) if sl.pid == pid && !sl.closed && sl.session.is_none() => {
                sl.session = Some(session);
                return;
            }
            Some(sl) if sl.pid == pid && sl.closed => {
                g[i] = None;
            }
            _ => {}
        }
        session
    };
    session = leftover;
    let _ = crate::crypto::tls::tls_close(&mut session);
}

#[cfg(target_arch = "x86_64")]
/// `npk_tls_connect(ip, port, host_ptr, host_len) -> handle | -1`
///
/// Blocks for the handshake, once per connection, like `open_tls` on the
/// HTTP path. The module's fiber holds its worker core meanwhile; if that
/// ever matters, split it into `start` and `status` like `npk_tcp_connect`.
pub(crate) fn npk_tls_connect(mem: &mut [u8], ctx: &mut HostState,
                              host_ptr: i32, host_len: i32, port: i32) -> i32 {
    if !net_allowed(ctx) { return -1 }
    // pid 0 is the shared identity of the inline execution paths, not an
    // owner (same rule as `fetch::submit`).
    if ctx.pid == 0 { return -1 }
    if port <= 0 || port > 65535 { return -1 }
    let Some(host) = read_str(mem, host_ptr, host_len) else { return -1 };
    // The bare name, without port, goes into SNI and certificate checks.
    let bare: String = String::from(host.split(':').next().unwrap_or(&host));
    if bare.is_empty() || bare.len() > 253 { return -1 }
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::Net, &bare);
    // The module passes a name, not an address, so `resolve_checked` both
    // resolves and checks the reach class (`ctx.net_reach`) in one step.
    // Resolving in the module would bypass the reach check every request of
    // the current page is held to.
    let ip = match crate::intent::http::resolve_checked(&bare, Some(ctx.net_reach)) {
        Ok(ip) => ip,
        Err(e) => {
            kprintln!("[npk] tls: {} — {}", bare, e);
            return -1;
        }
    };

    // Reserved before the handshake, so two connects cannot pick one slot.
    let free = {
        let mut g = TLS_SLOTS.lock();
        let slot = Some(TlsSlot { pid: ctx.pid, session: None, closed: false });
        match g.iter().position(|s| s.is_none()) {
            Some(i) => { g[i] = slot; i }
            None => { g.push(slot); g.len() - 1 }
        }
    };
    let release = || { TLS_SLOTS.lock()[free] = None; };
    let tcp = match crate::net::tcp::connect_as(tcp_owner(ctx), ip, port as u16) {
        Ok(h) => h,
        Err(_) => { release(); return -1 }
    };
    let session = match crate::crypto::tls::tls_connect(tcp, &bare) {
        Ok(s) => s,
        Err(e) => {
            kprintln!("[npk] tls: handshake with {} failed ({:?})", bare, e);
            let _ = crate::net::tcp::close(tcp);
            release();
            return -1;
        }
    };
    tls_checkin(free, ctx.pid, session);
    free as i32
}

#[cfg(target_arch = "x86_64")]
/// `npk_tls_send(handle, ptr, len) -> 0 | -1`
pub(crate) fn npk_tls_send(mem: &mut [u8], ctx: &mut HostState,
                           handle: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    if buf_len <= 0 { return -1 }
    // Straight from guest memory: `tls_send` copies one record at a time,
    // so the kernel never holds more than a record of it.
    let Some(data) = guest(mem, buf_ptr, buf_len as usize) else { return -1 };
    let Some(mut session) = tls_checkout(i, ctx.pid) else { return -1 };
    let r = match crate::crypto::tls::tls_send(&mut session, data) {
        Ok(()) => 0,
        Err(_) => -1,
    };
    tls_checkin(i, ctx.pid, session);
    r
}

#[cfg(target_arch = "x86_64")]
/// `npk_tls_recv(handle, ptr, cap) -> n | 0 (nothing yet) | -1 (closed/error)`
///
/// Returns immediately: `tls_poll` only collects what the TCP stack already
/// has. A browser polls this every frame.
pub(crate) fn npk_tls_recv(mem: &mut [u8], ctx: &mut HostState,
                           handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    if buf_max <= 0 { return -1 }
    let Some(dst) = guest_mut(mem, buf_ptr, buf_max as usize) else { return -1 };
    let Some(mut session) = tls_checkout(i, ctx.pid) else { return -1 };
    let r = match crate::crypto::tls::tls_poll(&mut session, dst) {
        Ok(n) => n as i32,
        Err(_) => -1,
    };
    tls_checkin(i, ctx.pid, session);
    r
}

#[cfg(target_arch = "x86_64")]
/// `npk_tls_close(handle) -> 0`
pub(crate) fn npk_tls_close(ctx: &mut HostState, handle: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    let session = {
        let mut g = TLS_SLOTS.lock();
        match g[i].as_mut() {
            Some(sl) if sl.session.is_some() => g[i].take().and_then(|sl| sl.session),
            // Checked out or still connecting: the holder closes it.
            Some(sl) => { sl.closed = true; None }
            None => None,
        }
    };
    if let Some(mut session) = session {
        let _ = crate::crypto::tls::tls_close(&mut session);
    }
    0
}

pub(crate) fn npk_tcp_connect(ctx: &mut HostState, ip_packed: i32, port: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    let ip = [
        ((ip_packed >> 24) & 0xFF) as u8,
        ((ip_packed >> 16) & 0xFF) as u8,
        ((ip_packed >> 8) & 0xFF) as u8,
        (ip_packed & 0xFF) as u8,
    ];
    if port <= 0 || port > 65535 { return -1; }
    // The reach class is deliberately not applied here. `ctx.net_reach` is
    // a page rule (a public page must not reach the local network); a
    // module is installed software with a declared capability, and for
    // `debug` the local network is the point (it logs to `nc -lk`).
    // The TLS path above is used by beak on behalf of a page and does apply
    // it.
    let peer = alloc::format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port);
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::Net, &peer);
    let Ok(h) = crate::net::tcp::connect_start_as(tcp_owner(ctx), ip, port as u16) else { return -1 };
    // The lowest free descriptor, as `open` hands out file descriptors.
    let fd = match ctx.tcp_handles.iter().position(Option::is_none) {
        Some(fd) => { ctx.tcp_handles[fd] = Some(h); fd }
        None => { ctx.tcp_handles.push(Some(h)); ctx.tcp_handles.len() - 1 }
    };
    fd as i32
}

/// The kernel handle behind descriptor `fd` of this module, or `None`.
fn own_tcp(ctx: &HostState, fd: i32) -> Option<usize> {
    *ctx.tcp_handles.get(usize::try_from(fd).ok()?)?
}

pub(crate) fn npk_tcp_status(ctx: &mut HostState, handle: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    let Some(h) = own_tcp(ctx, handle) else { return -1 };
    crate::net::tcp::connect_status(h)
}

pub(crate) fn npk_tcp_close(ctx: &mut HostState, handle: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    if let Some(h) = own_tcp(ctx, handle) {
        ctx.tcp_handles[handle as usize] = None;
        let _ = crate::net::tcp::close(h);
    }
    0
}

pub(crate) fn npk_debug_target_ip(_ctx: &mut HostState) -> i32 {
    get_debug_target().0 as i32
}

pub(crate) fn npk_debug_target_port(_ctx: &mut HostState) -> i32 {
    get_debug_target().1 as i32
}

#[cfg(target_arch = "x86_64")]
/// Binds the module to a PCI device. Allowed with a capability for that
/// exact device (the `driver` intent) or with global `Rights::HARDWARE`:
/// a bound device gives MMIO, bus mastering and DMA, i.e. reach into all
/// physical memory, so `EXECUTE` alone is not enough.
pub(crate) fn npk_pci_bind(ctx: &mut HostState, vendor: i32, device: i32) -> i32 {
    let vid = vendor as u16;
    let did = device as u16;
    let dev = match pci::find_device(vid, did) {
        Some(d) => d,
        None => return -1,
    };
    let cap_id = ctx.cap_id;
    let a = dev.addr;
    if capability::check_pci_device(&cap_id, capability::Rights::EXECUTE, a.bus, a.device, a.function).is_err()
        && !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_pci_bind") {
        kprintln!("[npk] WASM: npk_pci_bind DENIED {:04x}:{:04x}", vid, did);
        return -2;
    }
    if !take_binding(ctx, a) {
        kprintln!("[npk] WASM: npk_pci_bind {:04x}:{:04x} refused, owned by {:?}", vid, did, pci::owner(a));
        return -3;
    }
    crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
    ctx.hw = Some(HwDriverState {
        dev: Some(crate::drivers::DevId::Pci(dev.addr)),
        vendor_id: vid,
        device_id: did,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
        irqs: Vec::new(),
        dma_limit: DMA_LIMIT_DEFAULT,
        dma_pages: 0,
    });
    crate::kdebug!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, vid, did);
    0
}

#[cfg(target_arch = "x86_64")]
/// Make `addr` this module's device: free, already its own, or handed over
/// by the kernel driver that had it. The GPU is the only kernel-driven
/// device that hands over (`gpu::yield_to_module`); every other kernel
/// owner, and any other module, keeps it.
/// Bind `addr` to this module in place of its previous binding, whose
/// buffers and vector are released. Binding the same device again releases
/// the old state first: releasing it afterwards would give the device away.
fn take_binding(ctx: &mut HostState, addr: pci::PciAddr) -> bool {
    if ctx.hw.as_ref().is_some_and(|h| h.pci() == Some(addr)) {
        if let Some(old) = ctx.hw.take() { crate::wasm::release_hw(old); }
    }
    if !take_device(ctx.pid, addr) {
        return false;
    }
    if let Some(old) = ctx.hw.take() { crate::wasm::release_hw(old); }
    true
}

#[cfg(target_arch = "x86_64")]
fn take_device(pid: u32, addr: pci::PciAddr) -> bool {
    // pid 0 is the shared identity of the inline paths, not an owner.
    if pid == 0 { return false; }
    let me = pci::Owner::Module(pid);
    match pci::owner(addr) {
        Some(o) if o == me => return true,
        Some(pci::Owner::Module(_)) => return false,
        Some(pci::Owner::Kernel(_)) | None => {
            if !crate::gpu::yield_to_module(addr) { return false; }
        }
    }
    pci::claim(addr, me).is_ok()
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_pci_bind_class(ctx: &mut HostState, class: i32, subclass: i32) -> i32 {
    npk_pci_bind_class_n(ctx, class, subclass, 0)
}

#[cfg(target_arch = "x86_64")]
/// Bind the `index`-th PCI device of a class. Same rights check as
/// `npk_pci_bind_class`; `index` 0 is the first match.
///
/// Lets a driver walk the devices of its class; deciding which one fits is
/// the driver's job.
pub(crate) fn npk_pci_bind_class_n(ctx: &mut HostState, class: i32, subclass: i32, index: i32) -> i32 {
    let cls = class as u8;
    let sub = subclass as u8;
    if index < 0 { return -1; }
    let dev = match pci::find_by_class_n(cls, sub, index as u32) {
        Some(d) => d,
        None => return -1,
    };
    let cap_id = ctx.cap_id;
    let a = dev.addr;
    if capability::check_pci_device(&cap_id, capability::Rights::EXECUTE, a.bus, a.device, a.function).is_err()
        && !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_pci_bind_class") {
        kprintln!("[npk] WASM: npk_pci_bind_class DENIED {:02x}:{:02x}", cls, sub);
        return -2;
    }
    if !take_binding(ctx, a) {
        kprintln!("[npk] WASM: npk_pci_bind_class {:02x}:{:02x} refused, owned by {:?}", cls, sub, pci::owner(a));
        return -3;
    }
    crate::kdebug!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, dev.vendor_id, dev.device_id);
    crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
    ctx.hw = Some(HwDriverState {
        dev: Some(crate::drivers::DevId::Pci(dev.addr)),
        vendor_id: dev.vendor_id,
        device_id: dev.device_id,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
        irqs: Vec::new(),
        dma_limit: DMA_LIMIT_DEFAULT,
        dma_pages: 0,
    });
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_pci_read_config(ctx: &mut HostState, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    // Without a PCI device there is no config address; answering
    // would access 00:00.0.
    let Some(addr) = hw.pci() else { return -1 };
    if offset < 0 || offset > 0xFFF { return -1; }
    if offset > 255 {
        return pci::read32_ext(addr, offset as u16).map_or(-1, |v| v as i32);
    }
    pci::read32(addr, offset as u8) as i32
}

#[cfg(target_arch = "x86_64")]
/// Read a config dword of any PCI function, read-only. Linux drivers read
/// other functions' config to learn the platform: i915 asks the ISA bridge
/// for the PCH type and the host bridge for MCHBAR. Needs global
/// `HARDWARE`; config reads have no side effects in the header.
/// `bdf` = bus << 8 | device << 3 | function.
pub(crate) fn npk_pci_read_config_at(ctx: &mut HostState, bdf: i32, offset: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_pci_read_config_at") {
        return -1;
    }
    if !(0..=0xffff).contains(&bdf) || !(0..=0xFFC).contains(&offset) || offset & 3 != 0 {
        return -1;
    }
    let addr = pci::PciAddr {
        bus: (bdf >> 8) as u8,
        device: ((bdf >> 3) & 0x1f) as u8,
        function: (bdf & 7) as u8,
    };
    if offset > 255 {
        return pci::read32_ext(addr, offset as u16).map_or(-1, |v| v as i32);
    }
    pci::read32(addr, offset as u8) as i32
}

#[cfg(target_arch = "x86_64")]
/// Base and size of memory BAR `bar` of the bound device, as the boot scan
/// sized it: `npk_pci_bar_size` returns the size (0 = no such BAR).
pub(crate) fn npk_pci_bar_size(ctx: &mut HostState, bar: i32) -> i64 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let Some(addr) = hw.pci() else { return -1 };
    if !(0..6).contains(&bar) { return -1; }
    pci::bar(addr, bar as usize).map_or(0, |b| b.size as i64)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_pci_write_config(ctx: &mut HostState, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    // Without a PCI device there is no config address; answering
    // would access 00:00.0.
    let Some(addr) = hw.pci() else { return -1 };
    if offset < 0 || offset > 0xFFF { return -1; }
    if offset > 255 {
        if offset & 3 != 0 || ext_write_refused(addr, offset as u16) {
            kprintln!("[npk] WASM: PCI extended config write at {:#x} refused", offset);
            return -1;
        }
        return if pci::write32_ext(addr, offset as u16, value as u32) { 0 } else { -1 };
    }
    if config_write_refused(addr, offset as u8 & !3) {
        kprintln!("[npk] WASM: PCI config write at {:#x} refused (address or interrupt routing)", offset);
        return -1;
    }
    pci::write32(addr, offset as u8, value as u32);
    0
}

#[cfg(target_arch = "x86_64")]
/// `npk_pci_write_config16(offset, value)`: a word write to the bound
/// device's config space, with the same refusals as the dword form.
pub(crate) fn npk_pci_write_config16(ctx: &mut HostState, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let Some(addr) = hw.pci() else { return -1 };
    if offset < 0 || offset > 254 || offset & 1 != 0 { return -1; }
    if config_write_refused(addr, offset as u8 & !3) {
        kprintln!("[npk] WASM: PCI config write at {:#x} refused (address or interrupt routing)", offset);
        return -1;
    }
    pci::write16(addr, offset as u8, value as u16);
    0
}

#[cfg(target_arch = "x86_64")]
/// `npk_vga_msr_touch()`: `outb(inb(VGA_MIS_R), VGA_MIS_W)`, Linux
/// intel_vga_reset_io_mem. A display engine whose VGA power well was just
/// enabled raises unclaimed-register errors on 0x3d5 until the MSR has been
/// written once. Only for a bound VGA-compatible controller; the value
/// written is the one read back, so the legacy decode does not change.
pub(crate) fn npk_vga_msr_touch(ctx: &mut HostState) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let Some(addr) = hw.pci() else { return -1 };
    if pci::read8(addr, 0x0B) != 0x03 || pci::read8(addr, 0x0A) != 0x00 {
        return -1;
    }
    // SAFETY: the VGA MSR belongs to the legacy VGA decode of the display
    // controller this module owns; it is rewritten with its own value.
    let (read, write) = unsafe { (crate::arch::x86_64::Port::new(0x3CC), crate::arch::x86_64::Port::new(0x3C2)) };
    write.outb(read.inb());
    0
}

#[cfg(target_arch = "x86_64")]
/// Config dwords a driver must not write: they decide where the device sits
/// in physical memory (BARs, expansion ROM, a bridge's windows) or where its
/// interrupts go (MSI address and data, MSI-X control). The kernel sizes and
/// assigns BARs and programs interrupts itself; a driver that moved a BAR
/// onto RAM could map that RAM through `npk_mmio_map_bar`.
/// Extended capabilities a module may not write: they move BARs (SR-IOV,
/// Resizable BAR, VF Resizable BAR) or change how the device's DMA is
/// translated (ATS, PRI, PASID). Power management (L1 PM Substates, LTR)
/// and error reporting stay writable.
fn ext_write_refused(dev: pci::PciAddr, offset: u16) -> bool {
    let mut ptr: u16 = 0x100;
    for _ in 0..(0x1000 - 0x100) / 4 {
        let Some(hdr) = pci::read32_ext(dev, ptr) else { return true };
        if hdr == 0 || hdr == u32::MAX { return false; }
        let (id, next) = ((hdr & 0xFFFF) as u16, (hdr >> 20) as u16 & 0xFFC);
        // Sizes from the PCIe base spec: ATS 8, PRI 16, PASID 8, SR-IOV 64,
        // (VF) Resizable BAR up to 52 bytes.
        let span: u16 = match id { 0x0F | 0x1B => 8, 0x13 => 16, 0x10 => 0x40, 0x15 | 0x24 => 0x34, _ => 4 };
        // Every capability header is refused too: it links the list.
        if offset >= ptr && offset < ptr + span { return true; }
        if next < 0x100 { return false; }
        ptr = next;
    }
    true
}

#[cfg(target_arch = "x86_64")]
fn config_write_refused(dev: pci::PciAddr, dword: u8) -> bool {
    let header = pci::read8(dev, 0x0E) & 0x7F;
    let fixed = match header {
        0 => (0x10..0x28).contains(&dword) || dword == 0x30,
        1 => (0x10..0x18).contains(&dword) || (0x1C..0x30).contains(&dword) || dword == 0x38,
        _ => (0x10..0x40).contains(&dword),
    };
    if fixed { return true; }
    // Capability list, when the status register says there is one.
    if pci::read16(dev, 0x06) & 0x10 == 0 { return false; }
    let mut ptr = pci::read8(dev, 0x34) & 0xFC;
    for _ in 0..48 {
        if ptr < 0x40 { break; }
        let id = pci::read8(dev, ptr);
        let span: u8 = match id { 0x05 => 24, 0x11 => 12, _ => 0 };
        if span != 0 && dword >= ptr && (dword as u16) < ptr as u16 + span as u16 {
            return true;
        }
        ptr = pci::read8(dev, ptr + 1) & 0xFC;
    }
    false
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_pci_enable_bus_master(ctx: &mut HostState) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    // Without a PCI device there is no config address; answering
    // would access 00:00.0.
    let Some(addr) = hw.pci() else { return -1 };
    // A display controller goes behind its IOMMU before it masters
    // anything the driver hands it: from here on it reaches only what the
    // kernel grants (`drivers::iommu`).
    if pci::read8(addr, 0x0B) == 0x03 && crate::gpu::iommu_wanted() {
        match crate::drivers::iommu::protect(crate::drivers::DevId::Pci(addr)) {
            Ok(()) => {
                for &(phys, pages) in &hw.dma_allocs {
                    crate::drivers::iommu::grant(crate::drivers::DevId::Pci(addr), phys, pages);
                }
            }
            Err(e) => crate::kprintln!("[npk] iommu: {:02x}:{:02x}.{} not protected: {}",
                addr.bus, addr.device, addr.function, e),
        }
    }
    pci::enable_bus_master(addr);
    // Also enable memory space
    let cmd = pci::read32(addr, 0x04);
    pci::write32(addr, 0x04, cmd | 0x06);
    // And on every bridge above: without bus mastering there, the bridge
    // does not forward the device's requests upstream and the device gets
    // a master abort on valid RAM.
    pci::enable_bus_master_path(addr);
    hw.bus_master_enabled = true;
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_irq_register(ctx: &mut HostState, entry: i32) -> i32 {
    // Without a PCI device there is no config address.
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    let Some(addr) = hw.pci() else { return -1 };
    if !(0..2048).contains(&entry) { return -1; }
    // One interrupt per driver: registering again returns the same one, so
    // a loop around this call cannot take the vectors of its core.
    if let Some(&(v, _)) = hw.irqs.first() { return v as i32; }
    match crate::irq::register(addr, entry as u16) {
        Some(v) => { hw.irqs.push((v, 0)); v as i32 }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// Register I/O APIC input `gsi` for this driver — the interrupt line of a
/// device that is not on PCI (e.g. a GPIO controller for I2C touchpads,
/// found in its ACPI `_CRS`). `flags`: bit 0 level-triggered, bit 1 active-low.
///
/// Gated like `npk_mmio_map_phys`, whose register window such a driver
/// needs anyway: HARDWARE, and a bound or mapped device (`ctx.hw`). One
/// vector per driver, and a GSI that already has an owner (the keyboard,
/// another driver) is refused by `ioapic::route` — a module cannot take a
/// line away from its owner. Level lines are one-shot: the kernel ISR masks
/// them, `npk_wait` unmasks them when the driver waits again.
pub(crate) fn npk_irq_register_gsi(ctx: &mut HostState, gsi: i32, flags: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_irq_register_gsi") {
        return -1;
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if let Some(&(v, _)) = hw.irqs.first() { return v as i32; }
    if !(0..256).contains(&gsi) { return -1; }
    match crate::irq::register_gsi(gsi as u32, flags & 1 != 0, flags & 2 != 0) {
        Some(v) => { hw.irqs.push((v, 0)); v as i32 }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_sci_arm(gpe) -> vector | -1`: the AML driver takes the ACPI SCI for
/// its EC's GPE. Same ownership as `npk_irq_register_gsi` — the vector is
/// this module's, and `npk_wait(WAIT_IRQ)` arms it. Once per boot.
pub(crate) fn npk_sci_arm(ctx: &mut HostState, gpe: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_sci_arm") {
        return -1;
    }
    if !(0..256).contains(&gpe) { return -1; }
    if ctx.hw.is_none() {
        crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
        // A new binding replaces the old one: release it, or its buffers and
        // vector stay taken for the rest of the boot.
        if let Some(old) = ctx.hw.take() { crate::wasm::release_hw(old); }
        ctx.hw = Some(HwDriverState {
            dev: None,
            vendor_id: 0,
            device_id: 0,
            mmio_maps: Vec::new(),
            dma_allocs: Vec::new(),
            bus_master_enabled: false,
            registered_as_netdev: false,
            irqs: Vec::new(),
            dma_limit: DMA_LIMIT_DEFAULT,
            dma_pages: 0,
        });
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if !hw.irqs.is_empty() { return -1; }
    let Some((gsi, level, low)) = crate::sci::arm_ec(gpe as u32) else { return -1 };
    match crate::irq::register_gsi(gsi, level, low) {
        Some(v) => {
            hw.irqs.push((v, 0));
            crate::sci::VECTOR.store(v as u32, core::sync::atomic::Ordering::Relaxed);
            v as i32
        }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_sci_service() -> mask`: ack the SCI sources (see `sci::service`).
pub(crate) fn npk_sci_service(ctx: &mut HostState) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_sci_service") {
        return -1;
    }
    crate::sci::service() as i32
}

#[cfg(target_arch = "x86_64")]
/// `npk_acpi_video_notify(event) -> 0 | -1`: the AML driver heard a
/// `Notify` on an ACPI video output. Linux' `acpi_video_device_notify`
/// turns 0x86/0x87 into KEY_BRIGHTNESSUP/DOWN, and the desktop steps the
/// native backlight by a twentieth of its range; here the kernel does that
/// step. HARDWARE-gated, the right the AML driver holds.
pub(crate) fn npk_acpi_video_notify(ctx: &mut HostState, event: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_acpi_video_notify") {
        return -1;
    }
    match event {
        0x86 => crate::keyboard::step_brightness(5),
        0x87 => crate::keyboard::step_brightness(-5),
        _ => crate::kprintln!("[npk] acpi video: event 0x{:02x} not acted on", event),
    }
    0
}

#[cfg(target_arch = "x86_64")]
/// `npk_acpi_proc_caps() -> caps | -1`: the capabilities for the
/// processors' `_OSC`/`_PDC` (see `acpi_processor::proc_cap_bits`).
/// HARDWARE-gated.
pub(crate) fn npk_acpi_proc_caps(ctx: &mut HostState) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_acpi_proc_caps") {
        return -1;
    }
    crate::drivers::acpi_processor::proc_cap_bits() as i32
}

#[cfg(target_arch = "x86_64")]
/// `npk_acpi_cst(ptr, count) -> usable states | -1`: a processor's `_CST`
/// entries for MWAIT idle (see `acpi_processor::take_cst`). HARDWARE-gated.
pub(crate) fn npk_acpi_cst(mem: &mut [u8], ctx: &mut HostState, ptr: i32, count: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_acpi_cst") {
        return -1;
    }
    if !(1..=16).contains(&count) { return -1; }
    let Some(b) = read_bytes(mem, ptr, count * 24) else { return -1 };
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let cst: alloc::vec::Vec<crate::drivers::acpi_processor::Cx> = b.chunks_exact(24).map(|e| {
        let o = e.as_ptr() as usize - b.as_ptr() as usize;
        crate::drivers::acpi_processor::Cx {
            kind: u32_at(o),
            latency: u32_at(o + 4),
            power: u32_at(o + 8),
            space_id: e[12],
            bit_offset: e[13],
            address: u64::from_le_bytes(e[16..24].try_into().unwrap()),
        }
    }).collect();
    crate::drivers::acpi_processor::take_cst(&cst) as i32
}

/// `npk_dt_bind(compat_ptr, compat_len) -> 0 | -1`: bind the device-tree
/// display set whose HVS is compatible with `compat` (only
/// `brcm,bcm2712-hvs`, `platform::pi5_display_set`). HARDWARE. The HVS
/// goes behind its IOMMU before the module touches anything: no IOMMU, no
/// binding, since the HVS reads whatever memory its display list names.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_dt_bind(mem: &mut [u8], ctx: &mut HostState, compat_ptr: i32, compat_len: i32) -> i32 {
    use crate::platform::pi5_display_set as set;
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_dt_bind") {
        return -1;
    }
    let Ok(len) = usize::try_from(compat_len) else { return -1 };
    let Some(compat) = guest(mem, compat_ptr, len) else { return -1 };
    if compat != b"brcm,bcm2712-hvs" { return -1; }
    let Some(s) = set::get() else { return -1 };
    let dev = crate::drivers::DevId::Dt(s.hvs);
    if ctx.hw.as_ref().is_some_and(|h| h.dev == Some(dev)) { return 0; }
    if !set::take(ctx.pid) { return -1; }
    if let Err(e) = crate::drivers::iommu::protect(dev) {
        crate::kprintln!("[npk] display: not bound, the HVS is not behind its IOMMU: {}", e);
        set::release();
        return -1;
    }
    if let Some(old) = ctx.hw.take() { crate::wasm::release_hw(old); }
    crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
    ctx.hw = Some(HwDriverState {
        dev: Some(dev),
        vendor_id: 0,
        device_id: 0,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
        irqs: Vec::new(),
        dma_limit: DMA_LIMIT_DEFAULT,
        dma_pages: 0,
    });
    crate::kdebug!("[npk] WASM driver bound to the display set, HVS {}", dev);
    0
}

/// The display set, when this module holds it.
#[cfg(target_arch = "aarch64")]
fn bound_set(ctx: &HostState) -> Option<&'static crate::platform::pi5_display_set::Set> {
    let hw = ctx.hw.as_ref()?;
    if !hw.is_display() { return None; }
    crate::platform::pi5_display_set::get()
}

/// `npk_mmio_map_dt(part, reg) -> handle | -1`: window `reg` (the index in
/// the node's `reg`, the binding's order) of display-set part `part`,
/// exactly as long as the entry. Mapping a window again returns its handle.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_mmio_map_dt(ctx: &mut HostState, part: i32, reg: i32) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    let (Ok(part), Ok(reg)) = (usize::try_from(part), usize::try_from(reg)) else { return -1 };
    let Some((base, len)) = s.window(part, reg) else { return -1 };
    let Ok(len) = usize::try_from(len) else { return -1 };
    let Some(hw) = ctx.hw.as_mut() else { return -1 };
    if let Some(h) = hw.mmio_maps.iter().position(|&m| m == (base, len)) {
        return h as i32;
    }
    hw.mmio_maps.push((base, len));
    (hw.mmio_maps.len() - 1) as i32
}

/// `npk_irq_register_dt(part, index) -> vector | -1`: interrupt `index` of
/// display-set part `part`, a level GIC SPI, masked by the kernel's ISR
/// until the driver waits again (oneshot). The L2 controllers behind
/// their SPIs serve only display blocks; the module drives them.
/// Registering one again returns its vector.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_irq_register_dt(ctx: &mut HostState, part: i32, index: i32) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    let (Ok(part), Ok(index)) = (usize::try_from(part), usize::try_from(index)) else { return -1 };
    let Some(intid) = s.spi(part, index) else { return -1 };
    let Some(hw) = ctx.hw.as_mut() else { return -1 };
    if let Some(&(v, _)) = hw.irqs.iter().find(|&&(v, _)| crate::irq::intid(v) == Some(intid)) {
        return v as i32;
    }
    match crate::irq::register_spi(intid, false) {
        Some(v) => { hw.irqs.push((v, 0)); v as i32 }
        None => -1,
    }
}

/// `npk_fw_clock(id, op, rate) -> rate | -1`: a firmware clock of the
/// display, through the kernel's mailbox. `op` 0 rate, 1 max, 2 min,
/// 3 set `rate` (clamped to min..max), 4 enable, 5 disable. Only the bound display's
/// clocks: core 4, disp 16, M2MC 13, PIXEL_BVB 14.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_fw_clock(ctx: &mut HostState, id: i32, op: i32, rate: i64) -> i64 {
    use crate::platform::pi5_display_set::{fw_clock, ClockOp};
    if bound_set(ctx).is_none() { return -1; }
    let op = match op {
        0 => ClockOp::Rate,
        1 => ClockOp::MaxRate,
        2 => ClockOp::MinRate,
        3 => match u32::try_from(rate) { Ok(r) => ClockOp::SetRate(r), Err(_) => return -1 },
        4 => ClockOp::Enable,
        5 => ClockOp::Disable,
        _ => return -1,
    };
    let Ok(id) = u32::try_from(id) else { return -1 };
    fw_clock(id, op).map_or(-1, |r| r as i64)
}

/// `npk_fw_display_done() -> 0 | -1`: the firmware stops driving the
/// display (`vc4_drm_bind`'s `NOTIFY_DISPLAY_DONE`). Once per boot.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_fw_display_done(ctx: &mut HostState) -> i32 {
    if bound_set(ctx).is_none() { return -1; }
    if crate::platform::pi5_display_set::display_done() { 0 } else { -1 }
}

/// Pages one `npk_v3d_map` maps at most: 256 MiB.
#[cfg(target_arch = "aarch64")]
const V3D_MAP_MAX_PAGES: i32 = 1 << 16;

/// `npk_v3d_open() -> rev << 8 | QPUs, or -1`: the bound display set's V3D
/// powered up with the kernel's page table (`pi5_v3d::open`); the number
/// of QPUs (a program's spill buffer) and the hardware revision (the CSD
/// batch count).
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_open(ctx: &mut HostState) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    if !s.has_v3d() { return -1; }
    crate::platform::pi5_v3d::open().map_or(-1, |i| ((i.rev & 0xff) << 8 | (i.qpus & 0xff)) as i32)
}

/// `npk_v3d_map(pages_ptr, n) -> V3D address | -1`: the `n` 4 KiB pages
/// whose physical addresses (u64 each) are at `pages_ptr`, mapped in order
/// at a new V3D address. Every page must be granted to the display device
/// (a gpudev buffer, a DMA buffer of the module); the mapping goes when
/// any of its pages is revoked, or with `npk_v3d_unmap`.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_map(mem: &mut [u8], ctx: &mut HostState, pages_ptr: i32, n: i32) -> i64 {
    if bound_set(ctx).is_none() || n <= 0 || n > V3D_MAP_MAX_PAGES { return -1; }
    let Some(b) = guest(mem, pages_ptr, n as usize * 8) else { return -1 };
    let pages: alloc::vec::Vec<u64> = b.chunks_exact(8)
        .map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
        .collect();
    crate::platform::pi5_v3d::map(&pages).map_or(-1, |va| va as i64)
}

/// `npk_v3d_unmap(va) -> 0 | -1`: the mapping at V3D address `va` taken
/// away.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_unmap(ctx: &mut HostState, va: i64) -> i32 {
    if bound_set(ctx).is_none() { return -1; }
    let Ok(va) = u32::try_from(va) else { return -1 };
    if crate::platform::pi5_v3d::unmap(va) { 0 } else { -1 }
}

/// `npk_v3d_power(on) -> 0 | -1`: V3D's runtime PM (`pi5_v3d::power`).
/// Before `on = 0` the module masks V3D's interrupts and cleans its
/// caches; after `on = 1` it enables them.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_power(ctx: &mut HostState, on: i32) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    if !s.has_v3d() { return -1; }
    if crate::platform::pi5_v3d::power(on != 0) { 0 } else { -1 }
}

/// `npk_v3d_reset() -> 0 | -1`: the kernel's share of `v3d_reset` after a
/// hang (`pi5_v3d::reset`), between the module's `v3d_irq_disable` and
/// `v3d_irq_reset`.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_reset(ctx: &mut HostState) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    if !s.has_v3d() { return -1; }
    if crate::platform::pi5_v3d::reset() { 0 } else { -1 }
}

/// `npk_v3d_mmu_fault(out_ptr) -> 0 | -1`: after an MMU interrupt, the
/// violating AXI ID (u32) and address (u64) at `out_ptr`, the error cleared
/// (`v3d_hub_irq`).
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_v3d_mmu_fault(mem: &mut [u8], ctx: &mut HostState, out_ptr: i32) -> i32 {
    let Some(s) = bound_set(ctx) else { return -1 };
    if !s.has_v3d() { return -1; }
    let Some(f) = crate::platform::pi5_v3d::mmu_fault() else { return -1 };
    let Some(out) = guest_mut(mem, out_ptr, 12) else { return -1 };
    out[..4].copy_from_slice(&f.axi_id.to_le_bytes());
    out[4..12].copy_from_slice(&f.addr.to_le_bytes());
    0
}

/// Is `vector` one this module's driver registered?
fn owns_vector(ctx: &HostState, vector: i32) -> bool {
    ctx.hw.as_ref().is_some_and(|h| h.irqs.iter().any(|&(v, _)| v != 0 && v as i32 == vector))
}

pub(crate) fn npk_irq_arm(ctx: &mut HostState, vector: i32) -> i64 {
    // Arming re-routes the device's MSI to the calling core — only for the
    // module that owns it.
    if !owns_vector(ctx, vector) { return -1; }
    crate::irq::arm(vector as crate::irq::Irq) as i64
}

pub(crate) fn npk_irq_wait(ctx: &mut HostState, vector: i32, since: i64, timeout_ms: i32) -> i32 {
    if !owns_vector(ctx, vector) || since < 0 { return -1; }
    let t = if timeout_ms <= 0 { 1000 } else { timeout_ms as u64 };
    if crate::irq::wait(vector as crate::irq::Irq, since as u64, t) { 1 } else { 0 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_mmio_map_bar(ctx: &mut HostState, bar_idx: i32, pages: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    // Without a PCI device there is no config address; answering
    // would access 00:00.0.
    let Some(addr) = hw.pci() else { return -1 };
    // Up to 16 MB: a GPU's register BAR holds its registers and, in the
    // upper half, the GGTT (TGL GTTMMADR is 16 MB).
    if bar_idx < 0 || bar_idx > 5 || pages <= 0 || pages > 4096 { return -1; }
    if hw.mmio_maps.len() >= MAX_MMIO_MAPS { return -1; }

    let bar_offset = 0x10 + (bar_idx as u8) * 4;
    let bar_raw = pci::read32(addr, bar_offset);
    let is_64bit = bar_raw & 0x04 != 0;
    let mut bar_base = if is_64bit {
        pci::read_bar64(addr, bar_offset)
    } else {
        (bar_raw & 0xFFFF_FFF0) as u64
    };

    // If BAR is unassigned (UEFI didn't configure it), assign it now.
    // assign_bar_mmio sizes the BAR internally; we just need the base.
    if bar_base == 0 && bar_raw & 0x01 == 0 {
        bar_base = pci::assign_bar_mmio(addr, bar_offset);
        if bar_base == 0 { return -1; }
    }
    if bar_base == 0 { return -1; }

    // The size the boot scan measured. Only a BAR the firmware left
    // unassigned (placed just above) is sized here, before anyone uses it;
    // measuring a live device would turn its decoding off under whoever
    // is using it (a GPU's aperture holds the picture on screen).
    let bar_size = match pci::bar(addr, bar_idx as usize) {
        Some(b) if b.base == bar_base => b.size,
        _ => {
            let cmd = pci::read32(addr, 0x04);
            pci::write32(addr, 0x04, cmd & !0x02);
            let saved_lo = pci::read32(addr, bar_offset);
            pci::write32(addr, bar_offset, 0xFFFF_FFFF);
            let size_lo = pci::read32(addr, bar_offset);
            pci::write32(addr, bar_offset, saved_lo);
            pci::write32(addr, 0x04, cmd);
            (!((size_lo & !0xF) as u64)).wrapping_add(1) & 0xFFFF_FFFF
        }
    };

    let max_pages = (bar_size as usize) / 4096;
    let requested = pages as usize;
    let page_count = if requested > max_pages { max_pages } else { requested };

    for i in 0..page_count {
        let addr = bar_base + (i * 4096) as u64;
        if let Some(why) = mmio_refusal(addr) {
            kprintln!("[npk] WASM: BAR{} at {:#x} refused: {}", bar_idx, addr, why);
            return -1;
        }
    }
    // The BAR of the bound device, uncached; a refusal is the caller's -1.
    let virt = match crate::paging::ioremap(
        bar_base, (page_count * 4096) as u64,
        crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::NO_CACHE,
    ) {
        Ok(v) => v,
        Err(e) => {
            kprintln!("[npk] WASM MMIO map BAR{} at {:#x}: {}", bar_idx, bar_base, e);
            return -1;
        }
    };
    let handle = hw.mmio_maps.len();
    hw.mmio_maps.push((virt, page_count * 4096));
    crate::kdebug!("[npk] WASM driver: MMIO BAR{} mapped at {:#x} — BAR size {:#x}, requested {} pages, mapped {} pages",
        bar_idx, bar_base, bar_size, requested, page_count);
    handle as i32
}

/// Maps a physical MMIO range that does not belong to a PCI device.
///
/// For hardware announced only via ACPI: on AMD Renoir/Lucienne the I2C
/// controllers sit at fixed MMIO in the FCH and do not appear in PCI space,
/// so `npk_mmio_map_bar` cannot reach them.
///
/// The mapping goes into the same handle table as a BAR, so the existing
/// `npk_mmio_read*`/`write*` work on it unchanged.
///
/// Sandbox bounds:
///  * `Rights::HARDWARE`, the same right as EC ports and DSDT.
///  * Never RAM. Every page of the span is checked, not just the first,
///    so a span starting at the edge of a hole cannot reach into RAM.
///    Without a memory map, everything counts as RAM.
///  * Never LAPIC or IOAPIC. A write to `0xFEE0_0000` is an interrupt to
///    any vector on any core, i.e. code execution in the kernel; the IOAPIC
///    routes other devices' interrupts.
///  * Caps on the span and on the number of mappings (`MAX_MMIO_MAPS`): a
///    span inside one region the firmware typed as not RAM may be as long
///    as that region (an APU's frame buffer), any other at most
///    `MMIO_MAP_PAGES_UNDECLARED` pages.
/// Platform ranges no driver may map (ECAM, HPET, IOMMU), read once from
/// the ACPI tables.
static PLATFORM_MMIO: spin::Mutex<Option<Vec<(u64, u64)>>> = spin::Mutex::new(None);

#[cfg(target_arch = "x86_64")]
/// Why page `a` must not be mapped for a module, or `None` if it may.
/// One rule for every mapping path: a BAR the module can rewrite is no
/// more trustworthy than an address it names.
fn mmio_refusal(a: u64) -> Option<&'static str> {
    if !crate::memory::is_device_window(a) {
        return Some("RAM or the kernel image");
    }
    // LAPIC [0xFEE00000, 0xFEF00000) and IOAPIC [0xFEC00000, 0xFED00000).
    if (0xFEE0_0000..0xFEF0_0000).contains(&a) || (0xFEC0_0000..0xFED0_0000).contains(&a) {
        return Some("interrupt controller");
    }
    let mut g = PLATFORM_MMIO.lock();
    let ranges = g.get_or_insert_with(crate::acpi::platform_mmio_ranges);
    if ranges.iter().any(|&(b, l)| a >= b && a < b.saturating_add(l)) {
        return Some("platform registers (ECAM, HPET, IOMMU)");
    }
    None
}

/// The most pages one `npk_mmio_map_phys` maps outside a firmware region.
const MMIO_MAP_PAGES_UNDECLARED: i32 = 16;

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_mmio_map_phys(ctx: &mut HostState, hi: i32, lo: i32, pages: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_mmio_map_phys") {
        kprintln!("[npk] WASM: npk_mmio_map_phys DENIED — needs HARDWARE");
        return -1;
    }
    if pages <= 0 { return -1; }
    let base = ((hi as u32 as u64) << 32) | (lo as u32 as u64);
    if base == 0 || base & 0xFFF != 0 { return -1; }
    let n = pages as usize;
    // More than a few pages only inside one region the firmware declared:
    // an address no region covers also passes the per-page check, and a
    // mapping of it as large as the module asks would cost page tables
    // without bound.
    if pages > MMIO_MAP_PAGES_UNDECLARED
        && !crate::memory::is_firmware_device_region(base, (n * 4096) as u64)
    {
        kprintln!("[npk] WASM: npk_mmio_map_phys refused {:#x}+{} pages: not inside one firmware region",
            base, pages);
        return -1;
    }

    for i in 0..n {
        let a = match base.checked_add((i * 4096) as u64) { Some(a) => a, None => return -1 };
        if let Some(why) = mmio_refusal(a) {
            kprintln!("[npk] WASM: npk_mmio_map_phys refused {:#x}: {}", a, why);
            return -1;
        }
        // A BAR of a device another driver owns is that driver's, whatever
        // the address it is reached by.
        if let Some(dev) = pci::bar_owner_of(a) {
            if pci::owner(dev).is_some_and(|o| o != pci::Owner::Module(ctx.pid)) {
                kprintln!("[npk] WASM: npk_mmio_map_phys refused {:#x}: BAR of {:02x}:{:02x}.{}, owned by {:?}",
                    a, dev.bus, dev.device, dev.function, pci::owner(dev));
                return -1;
            }
        }
    }

    // A driver state without a PCI device if the module never bound one.
    if ctx.hw.is_none() {
        crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
        // A new binding replaces the old one: release it, or its buffers and
        // vector stay taken for the rest of the boot.
        if let Some(old) = ctx.hw.take() { crate::wasm::release_hw(old); }
        ctx.hw = Some(HwDriverState {
            dev: None,
            vendor_id: 0,
            device_id: 0,
            mmio_maps: Vec::new(),
            dma_allocs: Vec::new(),
            bus_master_enabled: false,
            registered_as_netdev: false,
            irqs: Vec::new(),
            dma_limit: DMA_LIMIT_DEFAULT,
            dma_pages: 0,
        });
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if hw.mmio_maps.len() >= MAX_MMIO_MAPS { return -1; }

    // Checked above to be neither RAM nor an interrupt controller. NO_CACHE
    // because device registers must not be cached.
    let virt = match crate::paging::ioremap(
        base, (n * 4096) as u64,
        crate::paging::PageFlags::PRESENT
            | crate::paging::PageFlags::WRITABLE
            | crate::paging::PageFlags::NO_CACHE,
    ) {
        Ok(v) => v,
        Err(e) => {
            kprintln!("[npk] WASM: npk_mmio_map_phys map {:#x}: {}", base, e);
            return -1;
        }
    };
    let handle = hw.mmio_maps.len();
    hw.mmio_maps.push((virt, n * 4096));
    crate::kdebug!("[npk] WASM driver: MMIO {:#x}+{:#x} mapped (handle {})",
        base, n * 4096, handle);
    handle as i32
}

/// Injects a pointer movement.
///
/// Uses the same queue as PS/2 and USB (`xhci::inject_mouse`), so all
/// pointing devices coexist and there is no second path into the
/// compositor.
///
/// `dx`/`dy` are `i32` here but `i8` in the event: a fast movement is split
/// into steps rather than clamped, so the pointer does not lag behind.
///
/// Requires `Rights::HARDWARE`, like `npk_key_inject`.
pub(crate) fn npk_pointer_inject(
    ctx: &mut HostState, dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_pointer_inject") {
        return -1;
    }
    let b = (buttons & 0x07) as u8;
    let s = scroll.clamp(-127, 127) as i8;
    let h = hscroll.clamp(-127, 127) as i8;
    let (mut rx, mut ry) = (dx, dy);
    // Bounded: an absurd value must not stall the loop, and more than
    // 16 x 127 points is not a hand movement.
    for step in 0..16 {
        let sx = rx.clamp(-127, 127);
        let sy = ry.clamp(-127, 127);
        rx -= sx;
        ry -= sy;
        let last = rx == 0 && ry == 0;
        crate::xhci::inject_mouse(crate::xhci::MouseEvent {
            buttons: b,
            dx: sx as i8,
            dy: sy as i8,
            // Scroll is sent once, not with every partial step.
            scroll: if step == 0 { s } else { 0 },
            hscroll: if step == 0 { h } else { 0 },
        });
        if last { break; }
    }
    0
}

/// A pointing device that reports where it points (touch screen, pen):
/// `x`, `y` on 0..=65535 of the screen, clamped. Requires `Rights::HARDWARE`,
/// like `npk_pointer_inject`.
pub(crate) fn npk_pointer_abs(ctx: &mut HostState, x: i32, y: i32, buttons: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_pointer_abs") {
        return -1;
    }
    crate::xhci::inject_pointer_abs(
        x.clamp(0, 65535) as u16, y.clamp(0, 65535) as u16, (buttons & 0x07) as u8);
    0
}

/// An address inside one of the module's own MMIO or DMA regions, checked
/// once on construction. Only `hw_addr` makes one.
#[derive(Clone, Copy)]
struct HwAddr(u64);

impl HwAddr {
    fn read<T: Copy>(self) -> T {
        // SAFETY: `hw_addr` checked that the access lies inside a region the
        // kernel mapped for this module (a validated BAR or physical window,
        // or its own DMA allocation), which stays mapped while it runs.
        unsafe { core::ptr::read_volatile(self.0 as *const T) }
    }

    fn write<T: Copy>(self, v: T) {
        // SAFETY: as in `read`.
        unsafe { core::ptr::write_volatile(self.0 as *mut T, v) }
    }
}

/// `width` bytes at `offset` in a region of `len` bytes at `base`.
fn hw_addr(base: u64, len: usize, offset: i32, width: usize) -> Option<HwAddr> {
    let off = usize::try_from(offset).ok()?;
    if off.checked_add(width)? > len { return None; }
    Some(HwAddr(base + off as u64))
}

fn mmio_at(ctx: &HostState, handle: i32, offset: i32, width: usize) -> Option<HwAddr> {
    let &(base, len) = ctx.hw.as_ref()?.mmio_maps.get(usize::try_from(handle).ok()?)?;
    hw_addr(base, len, offset, width)
}

fn dma_at(ctx: &HostState, handle: i32, offset: i32, width: usize) -> Option<HwAddr> {
    let &(phys, pages) = ctx.hw.as_ref()?.dma_allocs.get(usize::try_from(handle).ok()?)?;
    hw_addr(phys, pages.checked_mul(4096)?, offset, width)
}

pub(crate) fn npk_mmio_read32(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    mmio_at(ctx, handle, offset, 4).map_or(-1, |a| a.read::<u32>() as i32)
}

pub(crate) fn npk_mmio_write32(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let Some(a) = mmio_at(ctx, handle, offset, 4) else { return -1 };
    a.write(value as u32);
    0
}

pub(crate) fn npk_mmio_read16(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    if offset & 1 != 0 { return -1; }
    mmio_at(ctx, handle, offset, 2).map_or(-1, |a| a.read::<u16>() as i32)
}

pub(crate) fn npk_mmio_write16(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    if offset & 1 != 0 { return -1; }
    let Some(a) = mmio_at(ctx, handle, offset, 2) else { return -1 };
    a.write(value as u16);
    0
}

// 8-bit MMIO. rtw88 (RTL8822CE) addresses much of its register set in
// bytes: the power sequence is a read8/write8 interpreter, and registers
// like REG_SYS_FUNC_EN+1 mean a single byte. A 32-bit RMW also touches
// three neighbouring bytes, which differs on registers with read side
// effects. Linux chooses the width deliberately; we follow it.
pub(crate) fn npk_mmio_read8(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    mmio_at(ctx, handle, offset, 1).map_or(-1, |a| a.read::<u8>() as i32)
}

pub(crate) fn npk_mmio_write8(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let Some(a) = mmio_at(ctx, handle, offset, 1) else { return -1 };
    a.write(value as u8);
    0
}

/// Two 32-bit accesses, low half first: not every device decodes a 64-bit
/// transaction.
pub(crate) fn npk_mmio_read64(ctx: &mut HostState, handle: i32, offset: i32) -> i64 {
    let (Some(lo), Some(hi)) = (mmio_at(ctx, handle, offset, 8), mmio_at(ctx, handle, offset.wrapping_add(4), 4))
        else { return -1 };
    ((hi.read::<u32>() as u64) << 32 | lo.read::<u32>() as u64) as i64
}

pub(crate) fn npk_mmio_write64(ctx: &mut HostState, handle: i32, offset: i32, value: i64) -> i32 {
    let (Some(lo), Some(hi)) = (mmio_at(ctx, handle, offset, 8), mmio_at(ctx, handle, offset.wrapping_add(4), 4))
        else { return -1 };
    lo.write(value as u32);
    hi.write((value as u64 >> 32) as u32);
    0
}

/// One 64-bit read, as Linux `readq`. `npk_mmio_read64` is two 32-bit reads
/// (low, then high), which is what iwlwifi's `iwl_read64` does; a GPU's
/// page-table entries are read in one access.
pub(crate) fn npk_mmio_readq(ctx: &mut HostState, handle: i32, offset: i32) -> i64 {
    if offset & 7 != 0 { return -1; }
    mmio_at(ctx, handle, offset, 8).map_or(-1, |a| a.read::<u64>() as i64)
}

/// One 64-bit write, as Linux `writeq`: the device never sees half of the
/// value. i915 writes GGTT entries this way.
pub(crate) fn npk_mmio_writeq(ctx: &mut HostState, handle: i32, offset: i32, value: i64) -> i32 {
    if offset & 7 != 0 { return -1; }
    let Some(a) = mmio_at(ctx, handle, offset, 8) else { return -1 };
    a.write(value as u64);
    0
}

/// DMA allocations and pages a module may hold. A GPU driver binds every
/// buffer the compositor hands it through its own page tables, one page
/// per 2 MB bound, and keeps a compute heap, scene and blur buffers; the
/// cap still protects the frame pool from a runaway module.
fn dma_caps(hw: &HwDriverState) -> (usize, usize) {
    if hw.is_display() {
        (MAX_DMA_ALLOCS_GPU, MAX_DMA_PAGES_GPU)
    } else {
        (MAX_DMA_ALLOCS, MAX_DMA_PAGES)
    }
}

/// DMA limit of a driver that names no mask: 32-bit descriptors (a PCIe TX
/// BD has a 32-bit address field), as Linux's default mask.
const DMA_LIMIT_DEFAULT: u64 = 0x1_0000_0000;

/// `dma_set_mask(DMA_BIT_MASK(bits))`: the device reaches `bits` of
/// address, so its later buffers may lie anywhere below `1 << bits`. 0, or
/// -1 for a width outside 32..=64.
pub(crate) fn npk_dma_set_mask(ctx: &mut HostState, bits: i32) -> i32 {
    let Some(hw) = ctx.hw.as_mut() else { return -1 };
    if !(32..=64).contains(&bits) { return -1; }
    hw.dma_limit = if bits == 64 { u64::MAX } else { 1u64 << bits };
    // Behind a bcm2712-iommu the IOVA is the physical address: the device
    // reaches nothing above the aperture, whatever it decodes.
    #[cfg(target_arch = "aarch64")]
    if let Some(crate::drivers::DevId::Dt(base)) = hw.dev
        && let Some(end) = crate::drivers::bcm2712_iommu::aperture_end(base)
    {
        hw.dma_limit = hw.dma_limit.min(end);
    }
    0
}

/// A handle for the new buffer `phys`: a slot `npk_dma_free` emptied, else
/// a new one.
fn dma_slot(hw: &mut HwDriverState, phys: u64, pages: usize) -> usize {
    match hw.dma_allocs.iter().position(|&(_, p)| p == 0) {
        Some(h) => {
            hw.dma_allocs[h] = (phys, pages);
            h
        }
        None => {
            hw.dma_allocs.push((phys, pages));
            hw.dma_allocs.len() - 1
        }
    }
}

/// `dma_free_coherent`: the driver's buffer `handle` goes back. Behind an
/// IOMMU the device loses it first; the handle is then empty and every
/// access through it fails. 0, or -1 for a handle that holds nothing.
pub(crate) fn npk_dma_free(ctx: &mut HostState, handle: i32) -> i32 {
    let Some(hw) = ctx.hw.as_mut() else { return -1 };
    let Some(&(phys, pages)) = usize::try_from(handle).ok().and_then(|h| hw.dma_allocs.get(h)) else { return -1 };
    if pages == 0 { return -1; }
    if let Some(dev) = hw.dev {
        crate::drivers::iommu::revoke(dev, phys, pages);
    }
    crate::wasm::dma_give(phys, pages);
    hw.dma_allocs[handle as usize] = (0, 0);
    hw.dma_pages -= pages;
    0
}

pub(crate) fn npk_dma_alloc(ctx: &mut HostState, pages: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if pages <= 0 || pages as usize > MAX_DMA_PAGES_PER_CALL { return -1; }
    let page_count = pages as usize;
    let (max_allocs, max_pages) = dma_caps(hw);
    if hw.dma_live() >= max_allocs { return -1; }
    if hw.dma_pages + page_count > max_pages { return -1; }

    // Below the device's DMA mask.
    let phys = match crate::wasm::dma_take(page_count, hw.dma_limit) {
        Some(p) => p,
        None => return -1,
    };
    // SAFETY: zeroing freshly allocated DMA memory
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }
    let handle = dma_slot(hw, phys, page_count);
    hw.dma_pages += page_count;
    if let Some(dev) = hw.dev {
        crate::drivers::iommu::grant(dev, phys, page_count);
    }
    handle as i32
}

/// Like `npk_dma_alloc`, but the driver chooses the upper limit.
///
/// `allocate_contiguous_below` searches top-down, so a 4 GB limit always
/// lands right below the PCI MMIO hole. On AMD platforms that is where
/// TSEG/DPR sits: the CPU can access it, a device is rejected and gets a
/// master abort on what looks like valid RAM.
///
/// Which addresses a device can reach is device knowledge and belongs in
/// the driver. `limit_mb <= 0` means the DMA mask (`npk_dma_set_mask`, 4 GB
/// unless set); the limit never goes above the mask.
///
/// Same path, caps and bitmap as `npk_dma_alloc`. The caller picks a limit,
/// not an address, so it cannot select foreign memory.
pub(crate) fn npk_dma_alloc_below(ctx: &mut HostState, pages: i32, limit_mb: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if pages <= 0 || pages as usize > MAX_DMA_PAGES_PER_CALL { return -1; }
    let page_count = pages as usize;
    let (max_allocs, max_pages) = dma_caps(hw);
    if hw.dma_live() >= max_allocs { return -1; }
    if hw.dma_pages + page_count > max_pages { return -1; }

    let limit = if limit_mb <= 0 {
        hw.dma_limit
    } else {
        ((limit_mb as u64) * 1024 * 1024).min(hw.dma_limit)
    };

    let phys = match crate::wasm::dma_take(page_count, limit) {
        Some(p) => p,
        None => return -1,
    };
    // SAFETY: zeroing freshly allocated DMA memory
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }
    let handle = dma_slot(hw, phys, page_count);
    hw.dma_pages += page_count;
    if let Some(dev) = hw.dev {
        crate::drivers::iommu::grant(dev, phys, page_count);
    }
    handle as i32
}

pub(crate) fn npk_dma_phys_addr(ctx: &mut HostState, handle: i32) -> i64 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.dma_allocs.len() { return -1; }
    hw.dma_allocs[h].0 as i64
}

pub(crate) fn npk_dma_read32(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    dma_at(ctx, handle, offset, 4).map_or(-1, |a| a.read::<u32>() as i32)
}

pub(crate) fn npk_dma_write32(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let Some(a) = dma_at(ctx, handle, offset, 4) else { return -1 };
    a.write(value as u32);
    0
}

pub(crate) fn npk_memory_fence(_ctx: &mut HostState) -> i32 {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    0
}

#[cfg(target_arch = "x86_64")]
/// The NIC's own driver: a hardware module that registered as the network
/// device. Only it may move frames and wifi commands for that device; any
/// other module with driver state (an ACPI or input driver) may not.
fn is_netdev(ctx: &HostState) -> bool {
    ctx.hw.as_ref().is_some_and(|h| h.registered_as_netdev)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_set_link(ctx: &mut HostState, up: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    crate::netdev::set_wasm_nic_link(up != 0);
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_set_link_state(ctx: &mut HostState, carrier: i32, dormant: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    crate::netdev::set_wasm_nic_link_state(carrier != 0, dormant != 0);
    0
}

/// Every entry below `base` on an external volume, as relative paths,
/// folders before what they hold. Bounded like one listing.
fn ext_tree(base: &str, out: &mut alloc::vec::Vec<u8>) -> bool {
    const MAX_TREE: usize = 1 << 16;
    let mut todo: alloc::vec::Vec<String> = alloc::vec![String::new()];
    let mut count = 0usize;
    while let Some(rel) = todo.pop() {
        let abs = if rel.is_empty() { String::from(base) } else { alloc::format!("{}/{}", base, rel) };
        let entries = match crate::ext::list(&abs) {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(_) => return false,
        };
        for e in entries {
            count += 1;
            if count > MAX_TREE { return false; }
            let child = if rel.is_empty() { e.name.clone() } else { alloc::format!("{}/{}", rel, e.name) };
            append_entry(out, &child, e.size, e.is_dir, e.mtime);
            if e.is_dir { todo.push(child); }
        }
    }
    true
}

/// May the module of `ctx` reach `name` in `ext/` for `want`? With
/// EXTERNAL and the global right, or through a grant from the open dialog.
fn ext_allowed(ctx: &HostState, name: &str, want: capability::Rights) -> bool {
    (capability::holds(&ctx.cap_id, capability::Rights::EXTERNAL)
        && capability::holds(&ctx.cap_id, want))
        || capability::check_path_grant(&ctx.cap_id, name.trim_matches('/'), want)
}

/// What a file gate journals for `call` once it lets it through; `None`
/// for the calls on a file already open, which its open journalled.
fn file_kind(call: &str) -> Option<crate::audit::Kind> {
    use crate::audit::Kind;
    match call {
        "npk_fetch" | "npk_file_open" | "npk_drag_start" => Some(Kind::Read),
        "npk_store" | "npk_file_create" => Some(Kind::Write),
        "npk_fs_delete" => Some(Kind::Delete),
        "npk_fs_mkdir" => Some(Kind::Mkdir),
        _ => None,
    }
}

/// A file gate let `call` on `name` through.
fn journal_file(ctx: &HostState, name: &str, call: &str) {
    if let Some(kind) = file_kind(call) {
        crate::audit::record(&ctx.cap_id, kind, name);
    }
}

/// `old` became `new`.
fn journal_move(ctx: &HostState, kind: crate::audit::Kind, old: &str, new: &str) {
    let what = alloc::format!("{} -> {}", old, new);
    crate::audit::record(&ctx.cap_id, kind, &what);
}

/// A file gate refused `call` on `name`; `why` says which rule.
fn journal_refusal(ctx: &HostState, want: capability::Rights, name: &str, call: &str, why: &str) {
    let what = alloc::format!("{} {}", call, name);
    crate::audit::denied(&ctx.cap_id, want, &what, why);
}

/// May the module of `ctx` read file `name`? The rule of `npk_fetch` and
/// `npk_file_*`: another module's private area is closed, even with global
/// READ; the module's own area is open even without READ
/// (`wasm::is_own_private`).
fn read_allowed(ctx: &HostState, name: &str, call: &str) -> bool {
    let read = capability::Rights::READ;
    if crate::ext::is_ext(name) {
        let ok = ext_allowed(ctx, name, read);
        if !ok {
            kprintln!("[npk] WASM: {} DENIED (no EXTERNAL for {})", call, name);
            journal_refusal(ctx, read | capability::Rights::EXTERNAL, name, call, "");
            return false;
        }
        journal_file(ctx, name, call);
        return true;
    }
    if !crate::wasm::private_area_allows(name, &ctx.module_name) {
        kprintln!("[npk] WASM: {} DENIED ({} belongs to another module)", call, name);
        journal_refusal(ctx, read, name, call, "another app's private area");
        return false;
    }
    if !crate::wasm::is_own_private(name, &ctx.module_name) {
        if let Err(e) = capability::check_global(&ctx.cap_id, read) {
            kprintln!("[npk] WASM: {} DENIED (cap_id={:08x}, {:?})",
                call, capability::short_id(&ctx.cap_id), e);
            journal_refusal(ctx, read, name, call, e.reason());
            return false;
        }
    }
    journal_file(ctx, name, call);
    true
}

pub(crate) fn npk_fetch(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    if !read_allowed(ctx, &name, "npk_fetch") {
        return -1;
    }
    if crate::ext::is_ext(&name) {
        let Some(max) = glen(buf_max) else { return -1 };
        let st = match crate::ext::stat(&name) {
            Ok(Some(st)) if !st.is_dir => st,
            _ => return -1,
        };
        let Ok(len) = usize::try_from(st.size) else { return -1 };
        if len > max { return too_small(len); }
        let Some(dst) = guest_mut(mem, buf_ptr, len) else { return -1 };
        let mut at = 0;
        while at < len {
            match crate::ext::read(&name, at as u64, &mut dst[at..]) {
                Ok(n) if n > 0 => at += n,
                _ => return -1,
            }
        }
        return len as i32;
    }

    // Size first, then straight into the guest: a file the buffer cannot
    // take is never read, and one it can take is never copied whole onto
    // the kernel heap.
    let Some(max) = glen(buf_max) else { return -1 };
    let budget = file_budget(ctx, usize::MAX);
    let Ok(mut reader) = crate::npkfs::open_range(&name, budget) else { return -1 };
    let len = usize::try_from(reader.size()).unwrap_or(usize::MAX);
    if len > max { return too_small(len); }
    let Some(dst) = guest_mut(mem, buf_ptr, len) else { return -1 };
    let mut at = 0;
    while at < len {
        match reader.read_at(at as u64, &mut dst[at..], budget) {
            Ok(n) if n > 0 => at += n,
            _ => return -1,
        }
    }
    len as i32
}

// ── Keyvault ────────────────────────────────────
//
// The kernel is the vault authority (crate::security::keyvault). These are
// usable only by the signed `vault` GUI module, checked by its kernel-assigned
// name (not spoofable), so an unlocked user tier is NOT ambient: no other app
// can read user secrets while the GUI holds it open.

fn is_vault_module(ctx: &HostState) -> bool {
    ctx.module_name == "vault"
}

/// Raw guest bytes `[ptr, ptr+len)`, bounds-checked. None on overflow or OOB.
fn read_mem(mem: &[u8], ptr: i32, len: i32) -> Option<&[u8]> {
    let len = glen(len)?;
    let start = usize::try_from(ptr).ok()?;
    let end = start.checked_add(len)?;
    if end > mem.len() {
        return None;
    }
    Some(&mem[start..end])
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_status(ctx: &mut HostState) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    use crate::security::keyvault as kv;
    let mut s = 0;
    if kv::exists() { s |= 1; }
    if kv::user_tier_present() { s |= 2; }
    if kv::user_unlocked() { s |= 4; }
    s
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_init(ctx: &mut HostState) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    if crate::security::keyvault::init() { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_set_passphrase(
    mem: &mut [u8], ctx: &mut HostState,
    old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32,
) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let Some(new) = read_mem(mem, new_ptr, new_len) else { return -1 };
    let old = if old_len > 0 {
        match read_mem(mem, old_ptr, old_len) {
            Some(o) => Some(o),
            None => return -1,
        }
    } else {
        None
    };
    if crate::security::keyvault::set_passphrase(old, new) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_unlock(mem: &mut [u8], ctx: &mut HostState, pass_ptr: i32, pass_len: i32) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let Some(pass) = read_mem(mem, pass_ptr, pass_len) else { return -1 };
    if crate::security::keyvault::unlock_user(pass) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_lock(ctx: &mut HostState) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    crate::security::keyvault::lock_user();
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_user_set(
    mem: &mut [u8], ctx: &mut HostState,
    name_ptr: i32, name_len: i32, val_ptr: i32, val_len: i32,
) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let (Some(name), Some(val)) =
        (read_str(mem, name_ptr, name_len), read_mem(mem, val_ptr, val_len))
    else {
        return -1;
    };
    if crate::security::keyvault::user_set(&name, val) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_user_get(
    mem: &mut [u8], ctx: &mut HostState,
    name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32,
) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    let Some(val) = crate::security::keyvault::user_get(&name) else { return -1 };
    write_whole(mem, buf_ptr, buf_max, &val)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_user_list(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let joined = crate::security::keyvault::user_list().join("\n");
    write_whole(mem, buf_ptr, buf_max, joined.as_bytes())
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_system_set(
    mem: &mut [u8], ctx: &mut HostState,
    name_ptr: i32, name_len: i32, val_ptr: i32, val_len: i32,
) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let (Some(name), Some(val)) =
        (read_str(mem, name_ptr, name_len), read_mem(mem, val_ptr, val_len))
    else {
        return -1;
    };
    // A network password: the WLAN manager owns the record format.
    // The name is the escaped form `npk_vault_system_list` showed.
    if let Some(rest) = name.strip_prefix("net:") {
        let ssid = crate::drivers::wifi::ssid_of(rest);
        return if crate::drivers::wifi::save_password(&ssid, val).is_ok() { 0 } else { -1 };
    }
    if crate::security::keyvault::system_set(&name, val) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_system_list(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let joined = crate::security::keyvault::system_list().join("\n");
    write_whole(mem, buf_ptr, buf_max, joined.as_bytes())
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_user_delete(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    if crate::security::keyvault::user_delete(&name) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_vault_system_delete(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    if !is_vault_module(ctx) {
        return -1;
    }
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    if !crate::security::keyvault::system_delete(&name) {
        return -1;
    }
    if name.starts_with("net:") {
        crate::drivers::wifi::known_changed();
    }
    0
}

pub(crate) fn npk_http_response_headers(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_http_response_headers") {
        return -1;
    }
    let Some(hdrs) = &ctx.http_reply_headers else { return -1 };
    write_whole(mem, buf_ptr, buf_max, hdrs.as_bytes())
}

pub(crate) fn npk_http_final_url(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_http_final_url") {
        return -1;
    }
    let Some(url) = &ctx.http_final_url else { return -1 };
    write_whole(mem, buf_ptr, buf_max, url.as_bytes())
}

pub(crate) fn npk_http_content_type(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_http_content_type") {
        return -1;
    }
    let Some(ct) = &ctx.http_content_type else { return -1 };
    write_whole(mem, buf_ptr, buf_max, ct.as_bytes())
}

pub(crate) fn npk_http_last_error(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_http_last_error") {
        return -1;
    }
    let Some(err) = &ctx.http_last_error else { return -1 };
    write_whole(mem, buf_ptr, buf_max, err.as_bytes())
}

/// A module's private area may hold this share of the store: 1/32.
const PRIVATE_QUOTA_DIVISOR: u64 = 32;

/// How the module of `ctx` may write file `name`, the rule of `npk_store`
/// and `npk_file_*`: `Some(false)` by capability or as its own settings
/// file, `Some(true)` only through its private area (the quota applies),
/// `None` refused.
fn write_allowed(ctx: &HostState, name: &str, call: &str) -> Option<bool> {
    let write = capability::Rights::WRITE;
    if crate::ext::is_ext(name) {
        if !ext_allowed(ctx, name, write) {
            kprintln!("[npk] WASM: {} DENIED (no EXTERNAL for {})", call, name);
            journal_refusal(ctx, write | capability::Rights::EXTERNAL, name, call, "");
            return None;
        }
        if !crate::ext::writable(name) {
            kprintln!("[npk] WASM: {} DENIED ({} is read-only)", call, name);
            journal_refusal(ctx, write, name, call, "read-only drive");
            return None;
        }
        journal_file(ctx, name, call);
        return Some(false);
    }
    // Apps may not write the module store or the trust store — see
    // is_trust_critical_path.
    // Checked before the grant path so a per-file grant can never
    // become a way in there.
    if is_trust_critical_path(name) {
        kprintln!("[npk] WASM: {} DENIED ({} is read-only to apps)", call, name);
        journal_refusal(ctx, write, name, call, "module or trust store");
        return None;
    }
    // Another module's private area is closed, even with global READ and
    // WRITE. See `wasm::private_area_owner`.
    if !crate::wasm::private_area_allows(name, &ctx.module_name) {
        kprintln!("[npk] WASM: {} DENIED ({} belongs to another module)", call, name);
        journal_refusal(ctx, write, name, call, "another app's private area");
        return None;
    }

    // Four ways to be allowed to write, narrowest last:
    //   1. blanket WRITE from `.npk.caps`
    //   2. a grant for exactly this file — what the user handed over
    //      by picking the path in a trusted dialog
    //   3. the app's own settings file, `sys/config/<module>`. An
    //      app that keeps preferences shouldn't need write access to
    //      the whole store for it, and the name is the kernel's to
    //      derive — a module can't claim someone else's.
    //   4. the app's own private area, `priv/<module>/…`. It needs no
    //      capability: it is this program's state, not a file of the
    //      machine's store.
    let cap_id = ctx.cap_id;
    let own_config = alloc::format!("sys/config/{}", ctx.module_name);
    let has_write = capability::holds(&cap_id, write)
        || capability::check_path_grant(&cap_id, name, write)
        || (name == own_config && ctx.module_name != crate::ext::MODULE);
    if has_write {
        journal_file(ctx, name, call);
        return Some(false);
    }
    if crate::wasm::is_own_private(name, &ctx.module_name) {
        journal_file(ctx, name, call);
        return Some(true);
    }
    kprintln!("[npk] WASM: {} DENIED (no WRITE, no grant for {})", call, name);
    journal_refusal(ctx, write, name, call, "");
    None
}

/// The module's private area and its quota, a share of the store's size.
fn private_area(ctx: &HostState) -> (String, u64) {
    let area = alloc::format!("{}/{}", crate::wasm::PRIVATE_ROOT, ctx.module_name);
    let quota = crate::npkfs::stats()
        .map_or(0, |(total, ..)| total * crate::npkfs::BLOCK_SIZE as u64 / PRIVATE_QUOTA_DIVISOR);
    (area, quota)
}

/// Bytes the module's open writers into its private area have taken in and
/// not yet committed; they count against the quota like stored ones.
fn private_pending(ctx: &HostState) -> u64 {
    ctx.files.iter().flatten().map(|f| match f {
        OpenFile::Write { name, writer, .. }
            if crate::wasm::is_own_private(name, &ctx.module_name) => writer.written(),
        _ => 0,
    }).sum()
}

/// Whether the private area takes `len` more bytes, where `committed` is
/// what it holds besides the file being written.
fn private_quota_ok(ctx: &HostState, committed: u64, len: usize) -> bool {
    let (_, quota) = private_area(ctx);
    if committed.saturating_add(private_pending(ctx)).saturating_add(len as u64) > quota {
        kprintln!("[npk] WASM: write DENIED (private area of {} full, {} MiB)",
            ctx.module_name, quota / (1024 * 1024));
        return false;
    }
    true
}

/// What the private area holds besides file `name`.
fn private_committed(ctx: &HostState, name: &str) -> u64 {
    let (area, _) = private_area(ctx);
    crate::npkfs::tree_bytes(&area).saturating_sub(crate::npkfs::tree_bytes(name))
}

pub(crate) fn npk_store(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    let Some(by_private) = write_allowed(ctx, &name, "npk_store") else { return -1 };
    let Some(payload) = glen(data_len).and_then(|n| guest(mem, data_ptr, n)) else { return -1 };
    if crate::ext::is_ext(&name) {
        return if ext_store(&name, payload) { 0 } else { -1 };
    }

    if !crate::npkfs::room_for(payload.len()) {
        kprintln!("[npk] WASM: npk_store DENIED ({} B, npkFS is down to the system reserve)",
            payload.len());
        return -1;
    }
    // The private area must not be a way to fill the disk without WRITE: a
    // quota per module (Linux `dquot`), a share of the store's size.
    if by_private && !private_quota_ok(ctx, private_committed(ctx, &name), payload.len()) {
        return -1;
    }

    // Insert-or-replace: apps with state to persist (panel
    // configs, etc.) re-write the same key on every change. The
    // strict-create `store` would fail on the second write and
    // leave the app's state diverging from disk.
    match crate::npkfs::upsert(&name, payload, cap_id) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

// ── Ranged file access ───────────────────────────────────────────────────
//
// `npk_file_*`: a file opened once and read at any offset, or written as a
// sequential stream, so neither side holds the whole file. Same gates as
// `npk_fetch` and `npk_store`, checked at open and again on every call.
// The handle table lives in the instance state: a handle names nothing in
// another module, and every handle goes when the module ends.

/// One open file of a module.
pub(crate) enum OpenFile {
    /// A file in `ext/`: read by path and offset from its volume.
    Ext {
        name: String,
        size: u64,
    },
    /// A file being written in `ext/`: the bytes go to `temp` beside it,
    /// which takes the name at close, so the name keeps its old content
    /// until then.
    ExtWrite {
        name: String,
        temp: String,
        written: u64,
    },
    Read {
        name: String,
        reader: crate::npkfs::fs::RangeReader,
    },
    Write {
        name: String,
        writer: crate::npkfs::fs::StreamingWriter,
        /// `private_committed` at a root: recomputed only after a commit.
        committed: Option<([u8; 32], u64)>,
    },
}

impl OpenFile {
    /// Kernel memory the handle holds: a reader's current chunk (or its
    /// whole Blob), a writer's chunk buffer.
    fn held(&self) -> usize {
        match self {
            OpenFile::Ext { .. } | OpenFile::ExtWrite { .. } => 0,
            OpenFile::Read { reader, .. } => reader.held(),
            OpenFile::Write { .. } => crate::npkfs::STREAMING_CHUNK_SIZE,
        }
    }
}

/// Files one module may hold open at once, like its TCP connections
/// (`net_core::tcp::PER_OWNER_MAX`, Linux `RLIMIT_NOFILE`).
const FILES_PER_MODULE: usize = 32;

/// Kernel memory the module's open files other than `except` leave for it:
/// all together stay within half of the free RAM, the rule wasi descriptors
/// follow (`wasi::grow_file`).
fn file_budget(ctx: &HostState, except: usize) -> usize {
    let held: usize = ctx.files.iter().enumerate()
        .filter(|(i, _)| *i != except)
        .filter_map(|(_, f)| f.as_ref().map(OpenFile::held))
        .sum();
    (crate::memory::stats().0.saturating_mul(crate::memory::PAGE_SIZE) / 2).saturating_sub(held)
}

/// A free slot in the module's handle table, or `None` at the limit.
fn file_slot(ctx: &mut HostState) -> Option<usize> {
    if let Some(i) = ctx.files.iter().position(|f| f.is_none()) {
        return Some(i);
    }
    if ctx.files.len() >= FILES_PER_MODULE {
        kprintln!("[npk] WASM: {} holds {} files open, none more", ctx.module_name, FILES_PER_MODULE);
        return None;
    }
    ctx.files.push(None);
    Some(ctx.files.len() - 1)
}

/// The slot of an open handle.
fn file_index(ctx: &HostState, handle: i32) -> Option<usize> {
    let i = usize::try_from(handle).ok()?;
    ctx.files.get(i)?.as_ref().map(|_| i)
}

/// `npk_file_open(name_ptr, name_len) -> handle | -1`
pub(crate) fn npk_file_open(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    if !read_allowed(ctx, &name, "npk_file_open") {
        return -1;
    }
    if crate::ext::is_ext(&name) {
        let size = match crate::ext::stat(&name) {
            Ok(Some(st)) if !st.is_dir => st.size,
            _ => return -1,
        };
        let Some(i) = file_slot(ctx) else { return -1 };
        ctx.files[i] = Some(OpenFile::Ext { name, size });
        return i as i32;
    }
    let Some(i) = file_slot(ctx) else { return -1 };
    let Ok(reader) = crate::npkfs::open_range(&name, file_budget(ctx, i)) else { return -1 };
    ctx.files[i] = Some(OpenFile::Read { name, reader });
    i as i32
}

/// `npk_file_read(handle, offset, buf_ptr, buf_max) -> n | 0 (end) | -1`
///
/// A stream read: what fits from `offset` on, never past the end of the
/// file; a short read is no error.
pub(crate) fn npk_file_read(mem: &mut [u8], ctx: &mut HostState,
                            handle: i32, offset: i64, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(i) = file_index(ctx, handle) else { return -1 };
    let Ok(offset) = u64::try_from(offset) else { return -1 };
    let Some(dst) = glen(buf_max).and_then(|n| guest_mut(mem, buf_ptr, n)) else { return -1 };
    let allowed = match &ctx.files[i] {
        Some(OpenFile::Read { name, .. }) | Some(OpenFile::Ext { name, .. }) => read_allowed(ctx, name, "npk_file_read"),
        _ => false,
    };
    if !allowed { return -1; }
    if let Some(OpenFile::Ext { name, size }) = &ctx.files[i] {
        if offset >= *size { return 0; }
        let n = dst.len().min(crate::ext::MAX_IO).min((*size - offset) as usize);
        return match crate::ext::read(name, offset, &mut dst[..n]) {
            Ok(n) => n as i32,
            Err(_) => -1,
        };
    }
    let budget = file_budget(ctx, i);
    let Some(OpenFile::Read { reader, .. }) = ctx.files[i].as_mut() else { return -1 };
    match reader.read_at(offset, dst, budget) {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

/// `npk_file_size(handle) -> bytes | -1`: a reader's file length, a
/// writer's bytes so far.
pub(crate) fn npk_file_size(ctx: &mut HostState, handle: i32) -> i64 {
    let Some(i) = file_index(ctx, handle) else { return -1 };
    let n = match &ctx.files[i] {
        Some(OpenFile::Ext { size, .. }) => *size,
        Some(OpenFile::ExtWrite { written, .. }) => *written,
        Some(OpenFile::Read { reader, .. }) => reader.size(),
        Some(OpenFile::Write { writer, .. }) => writer.written(),
        None => return -1,
    };
    i64::try_from(n).unwrap_or(-1)
}

/// `npk_file_create(name_ptr, name_len) -> handle | -1`
///
/// The file appears at `npk_file_close`, whole and at once; until then the
/// name keeps its old content, if it had one.
pub(crate) fn npk_file_create(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    if write_allowed(ctx, &name, "npk_file_create").is_none() {
        return -1;
    }
    if crate::ext::is_ext(&name) {
        let Some(temp) = ext_temp_name(&name) else { return -1 };
        let Some(i) = file_slot(ctx) else { return -1 };
        if crate::ext::create(&temp).is_err() { return -1; }
        ctx.files[i] = Some(OpenFile::ExtWrite { name, temp, written: 0 });
        return i as i32;
    }
    let Some(i) = file_slot(ctx) else { return -1 };
    // The writer allocates its chunk buffer as it opens.
    if file_budget(ctx, i) < crate::npkfs::STREAMING_CHUNK_SIZE {
        return -1;
    }
    let Ok(writer) = crate::npkfs::open_streaming_write(&name) else { return -1 };
    ctx.files[i] = Some(OpenFile::Write { name, writer, committed: None });
    i as i32
}

/// `npk_file_write(handle, buf_ptr, len) -> len | -1`: append `len` bytes.
pub(crate) fn npk_file_write(mem: &mut [u8], ctx: &mut HostState,
                             handle: i32, buf_ptr: i32, len: i32) -> i32 {
    let Some(i) = file_index(ctx, handle) else { return -1 };
    let Some(data) = glen(len).and_then(|n| guest(mem, buf_ptr, n)) else { return -1 };
    if let Some(OpenFile::ExtWrite { name, temp, written }) = &ctx.files[i] {
        let (name, temp, mut at) = (name.clone(), temp.clone(), *written);
        if write_allowed(ctx, &name, "npk_file_write").is_none() { return -1; }
        for chunk in data.chunks(crate::ext::MAX_IO) {
            if crate::ext::write(&temp, at, chunk).is_err() { return -1; }
            at += chunk.len() as u64;
            if let Some(OpenFile::ExtWrite { written, .. }) = ctx.files[i].as_mut() { *written = at; }
        }
        return data.len() as i32;
    }
    let (name, cached) = match &ctx.files[i] {
        Some(OpenFile::Write { name, committed, .. }) => (name.clone(), *committed),
        _ => return -1,
    };
    let Some(by_private) = write_allowed(ctx, &name, "npk_file_write") else { return -1 };
    if !crate::npkfs::room_for(data.len()) {
        kprintln!("[npk] WASM: npk_file_write DENIED ({} B, npkFS is down to the system reserve)",
            data.len());
        return -1;
    }
    if by_private {
        let root = crate::npkfs::storage::current_root();
        let committed = match (root, cached) {
            (Some(r), Some((at, bytes))) if r == at => bytes,
            _ => {
                let bytes = private_committed(ctx, &name);
                if let (Some(r), Some(OpenFile::Write { committed, .. })) = (root, ctx.files[i].as_mut()) {
                    *committed = Some((r, bytes));
                }
                bytes
            }
        };
        if !private_quota_ok(ctx, committed, data.len()) {
            return -1;
        }
    }
    let Some(OpenFile::Write { writer, .. }) = ctx.files[i].as_mut() else { return -1 };
    match writer.write(data) {
        Ok(()) => data.len() as i32,
        Err(_) => -1,
    }
}

/// `npk_file_close(handle) -> 0 | -1`. Closing a writer publishes the file;
/// -1 then means it was not written and the name keeps its old content.
pub(crate) fn npk_file_close(ctx: &mut HostState, handle: i32) -> i32 {
    let Some(i) = file_index(ctx, handle) else { return -1 };
    match ctx.files[i].take() {
        Some(OpenFile::Write { name, writer, .. }) => {
            if write_allowed(ctx, &name, "npk_file_close").is_none() {
                return -1;
            }
            match writer.finish() {
                Ok(_) => 0,
                Err(_) => -1,
            }
        }
        Some(OpenFile::ExtWrite { name, temp, .. }) => {
            if write_allowed(ctx, &name, "npk_file_close").is_none()
                || crate::ext::rename(&temp, &name, true).is_err()
            {
                let _ = crate::ext::delete(&temp);
                return -1;
            }
            0
        }
        Some(OpenFile::Read { .. }) | Some(OpenFile::Ext { .. }) => 0,
        None => -1,
    }
}

/// `npk_file_abort(handle) -> 0 | -1`. Closes without publishing: a
/// writer's file is not written. For a reader the same as close.
pub(crate) fn npk_file_abort(ctx: &mut HostState, handle: i32) -> i32 {
    let Some(i) = file_index(ctx, handle) else { return -1 };
    if let Some(OpenFile::ExtWrite { temp, .. }) = ctx.files[i].take() {
        let _ = crate::ext::delete(&temp);
    }
    0
}

#[cfg(target_arch = "x86_64")]
/// The files a module left open on external drives: their partial copies
/// go, the names keep what they had.
pub(crate) fn drop_ext_writers(ctx: &mut HostState) {
    for f in ctx.files.iter_mut() {
        if let Some(OpenFile::ExtWrite { temp, .. }) = f.take() {
            let _ = crate::ext::delete(&temp);
        }
    }
}

/// The partial copy of `name` while it is written: a hidden name beside it.
fn ext_temp_name(name: &str) -> Option<String> {
    let p = name.trim_matches('/');
    let (dir, base) = p.rsplit_once('/')?;
    if base.is_empty() { return None; }
    Some(alloc::format!("{}/.{}.npk-part", dir, base))
}

/// `npk_store` on an external drive: written beside, then renamed over.
fn ext_store(name: &str, data: &[u8]) -> bool {
    let Some(temp) = ext_temp_name(name) else { return false };
    if crate::ext::create(&temp).is_err() { return false; }
    let mut at = 0u64;
    for chunk in data.chunks(crate::ext::MAX_IO) {
        if crate::ext::write(&temp, at, chunk).is_err() {
            let _ = crate::ext::delete(&temp);
            return false;
        }
        at += chunk.len() as u64;
    }
    if crate::ext::rename(&temp, name, true).is_err() {
        let _ = crate::ext::delete(&temp);
        return false;
    }
    true
}

pub(crate) fn npk_home_dir(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::READ, "npk_home_dir") {
        return -1;
    }
    let home = crate::intent::home_dir();
    write_whole(mem, buf_ptr, buf_max, home.as_bytes())
}

pub(crate) fn npk_locale(mem: &mut [u8], _ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let lang = crate::config::get("lang").unwrap_or_default();
    let lang = lang.trim();
    let bytes = if lang.is_empty() { b"en".as_slice() } else { lang.as_bytes() };
    write_whole(mem, buf_ptr, buf_max, bytes)
}

pub(crate) fn npk_launch_arg(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(arg) = &ctx.launch_arg else { return 0 };
    write_whole(mem, buf_ptr, buf_max, arg.as_bytes())
}

pub(crate) fn npk_clipboard_set(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_clipboard_set") {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    let Some(slice) = glen(len).and_then(|n| guest(mem, ptr, n)) else { return -1 };
    if !crate::shade::clipboard::set_text(slice) { return -1; }
    slice.len() as i32
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_clipboard_get(mem: &mut [u8], ctx: &mut HostState, ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_clipboard_get") {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    let text = match crate::shade::clipboard::get_text() {
        Some(t) => t,
        None => return 0,
    };
    write_whole(mem, ptr, max, &text)
}

pub(crate) fn npk_scene_commit(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_scene_commit") {
        kprintln!("[npk] WASM: npk_scene_commit DENIED (no RENDER)");
        return -1;
    }
    // Remember which capability owns this window, so a later grant
    // (loft opening a file in an already-running editor) can find it.
    let owner_wid = ctx.widget_window_id;
    if owner_wid != 0 { crate::shade::widgets::set_window_cap(owner_wid, cap_id); }

    // Heap copy; a typical tree is a few hundred bytes. Capped before the
    // copy: the tree is stored and cloned on every hover and keystroke.
    const MAX_SCENE_BYTES: usize = 16 * 1024 * 1024;
    let payload: alloc::vec::Vec<u8> = match glen(len)
        .filter(|&n| n <= MAX_SCENE_BYTES)
        .and_then(|n| guest(mem, ptr, n))
    {
        Some(b) if !b.is_empty() => match copy_out(b) {
            Some(v) => v,
            None => return -1,
        },
        _ => return -1,
    };

    let mut prev_window = ctx.widget_window_id;

    // First commit from a module that was spawned as a terminal:
    // promote that terminal to a widget in place so the app only
    // owns one window (not a terminal + a widget side-by-side).
    if prev_window == 0 {
        let terminal_idx = ctx.terminal_idx;
        if terminal_idx != 255 {
            if let Some(promoted) = crate::shade::with_compositor(|c|
                c.promote_terminal_to_widget(terminal_idx)
            ).flatten() {
                ctx.widget_window_id = promoted.0;
                ctx.terminal_idx = 255;
                prev_window = promoted.0;
            }
        }
    }

    let module_name = ctx.module_name.clone();
    let result = crate::shade::widgets::scene_commit(&payload, prev_window, &module_name);

    // Positive return = newly allocated window id → store so
    // subsequent commits from this app reuse the same slot.
    if result > 0 && ctx.widget_window_id == 0 {
        ctx.widget_window_id = result as u32;
    }
    // Collapse "new-window id" into success for the callee —
    // the WASM ABI contract is that any non-negative return
    // means "commit accepted". Negatives still propagate.
    if result < 0 { result } else { 0 }
}

pub(crate) fn npk_canvas_commit(mem: &mut [u8], ctx: &mut HostState, canvas_id: i32, ptr: i32, len: i32, width: i32, height: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::CANVAS, "npk_canvas_commit") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 || width <= 0 || height <= 0 { return -1; }
    let Some(pixel_bytes) = (width as usize).checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4)) else { return -1 };
    if glen(len) != Some(pixel_bytes) { return -1; }
    // The canvas caps, before anything is copied onto the kernel heap.
    use crate::shade::widgets::canvas::{window_budget, MAX_DIM};
    if width as u32 > MAX_DIM || height as u32 > MAX_DIM || pixel_bytes > window_budget() { return -1; }
    let Some(px) = guest(mem, ptr, pixel_bytes).and_then(copy_out) else { return -1 };
    if !crate::shade::widgets::canvas::commit(
        wid, canvas_id as u32, width as u32, height as u32, px) {
        return -1;
    }
    crate::shade::widgets::rerender_window_pixels(wid);
    crate::shade::request_render();
    0
}

#[cfg(target_arch = "x86_64")]
/// `npk_canvas_frame(canvas_id, ptr, len, w, h, format)`: a frame shown as
/// a scene layer over the window (`canvas::commit_frame`): the window is
/// not rasterized again, only the canvas rect is composed, and the GPU
/// scales the frame. Format 0 = BGRA32, 1 = packed YUYV BT.601 limited
/// (a camera's YUY2), 2 = packed YUYV full range; YUYV has an even width.
/// 3 = an MJPEG frame (baseline 4:2:2, `len` bytes) the GPU module decodes
/// into a YUYV layer: -1 without GPU decode, -2 while the last is still
/// being decoded (dropped).
pub(crate) fn npk_canvas_frame(mem: &mut [u8], ctx: &mut HostState, canvas_id: i32, ptr: i32, len: i32,
                               width: i32, height: i32, format: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::CANVAS, "npk_canvas_frame") { return -1; }
    let wid = ctx.widget_window_id;
    use crate::shade::scene::Yuv;
    if format == 3 {
        if wid == 0 || width <= 0 || height <= 0 || len <= 0 { return -1; }
        let n = len as usize;
        if n > crate::shade::widgets::canvas::window_budget() { return -1; }
        let Some(src) = guest(mem, ptr, n) else { return -1 };
        let r = crate::shade::widgets::canvas::commit_jpeg(wid, canvas_id as u32, width as u32, height as u32, src);
        if r == 0 { crate::shade::request_render(); }
        return r;
    }
    let yuv = match format { 0 => Yuv::None, 1 => Yuv::Limited, 2 => Yuv::Full, _ => return -1 };
    if wid == 0 || width <= 0 || height <= 0 { return -1; }
    let bpp = if yuv == Yuv::None { 4 } else { 2 };
    let Some(bytes) = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(bpp)) else { return -1 };
    if glen(len) != Some(bytes) { return -1; }
    use crate::shade::widgets::canvas::{window_budget, MAX_DIM};
    if width as u32 > MAX_DIM || height as u32 > MAX_DIM || bytes > window_budget() { return -1; }
    let Some(src) = guest(mem, ptr, bytes) else { return -1 };
    if !crate::shade::widgets::canvas::commit_frame(wid, canvas_id as u32, width as u32, height as u32, yuv, src) {
        return -1;
    }
    crate::shade::request_render();
    0
}

/// `npk_canvas_commit_yuv(canvas_id, y_ptr, u_ptr, v_ptr, ys, cs, w, h, flags)`
///
/// Upload a planar 4:2:0 frame (I420) instead of BGRA. `ys`/`cs` are row
/// strides in bytes, so a decoder can hand over its padded planes without
/// repacking them. `flags`: bit 0 = Rec. 709 (else Rec. 601), bit 1 = full
/// range (else limited/studio).
///
/// Why this exists next to `npk_canvas_commit`: converting Y′CbCr to BGRA
/// inside a module costs more than decoding the frame. Here the conversion
/// happens natively in the blit, only for the pixels that land in the
/// canvas rect, and the frame crosses the boundary at 1.5 bytes per pixel
/// instead of 4.
pub(crate) fn npk_canvas_commit_yuv(
    mem: &mut [u8], ctx: &mut HostState,
    canvas_id: i32, y_ptr: i32, u_ptr: i32, v_ptr: i32,
    ys: i32, cs: i32, width: i32, height: i32, flags: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::CANVAS, "npk_canvas_commit_yuv") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 || width <= 0 || height <= 0 || ys <= 0 || cs <= 0 { return -1; }
    let (w, h) = (width as u32, height as u32);
    let (ys, cs) = (ys as usize, cs as usize);
    let ch = ((h + 1) / 2) as usize;

    // How much of each plane the blit may touch. `canvas::commit_i420`
    // checks the same thing against the copied Vec; this check is about a
    // different question — whether the module's own memory holds it.
    let Some(need_y) = ys.checked_mul(h as usize) else { return -1 };
    let Some(need_c) = cs.checked_mul(ch) else { return -1 };
    // The canvas caps, before anything is copied onto the kernel heap.
    use crate::shade::widgets::canvas::{window_budget, MAX_DIM};
    if w > MAX_DIM || h > MAX_DIM
        || need_y.saturating_add(need_c.saturating_mul(2)) > window_budget() { return -1; }

    let data = &*mem;
    let take = |ptr: i32, n: usize| guest(data, ptr, n).and_then(copy_out);
    let (Some(y), Some(u), Some(v)) =
        (take(y_ptr, need_y), take(u_ptr, need_c), take(v_ptr, need_c))
        else { return -1 };

    let coding = crate::shade::widgets::canvas::YuvCoding {
        bt709: flags & 1 != 0,
        full_range: flags & 2 != 0,
    };
    if !crate::shade::widgets::canvas::commit_i420(
        wid, canvas_id as u32, w, h, y, u, v, ys, cs, coding) {
        return -1;
    }
    crate::shade::widgets::rerender_window_pixels(wid);
    crate::shade::request_render();
    0
}

pub(crate) fn npk_canvas_rect(mem: &mut [u8], ctx: &mut HostState, canvas_id: i32, out_ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_canvas_rect") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 {
        return -1;
    }
    let (x, y, w, h) = match crate::shade::widgets::canvas::rect_of(wid, canvas_id as u32) {
        Some(r) => r,
        None => return -1,
    };
    let Some(out) = guest_mut(mem, out_ptr, 16) else { return -1 };
    out[0..4].copy_from_slice(&x.to_le_bytes());
    out[4..8].copy_from_slice(&y.to_le_bytes());
    out[8..12].copy_from_slice(&(w as i32).to_le_bytes());
    out[12..16].copy_from_slice(&(h as i32).to_le_bytes());
    0
}

pub(crate) fn npk_canvas_drop(ctx: &mut HostState, canvas_id: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::RENDER, "npk_canvas_drop") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    if crate::shade::widgets::canvas::remove(wid, canvas_id as u32) { 0 } else { -1 }
}

/// Most bytes and paths one drag may carry.
const DRAG_MAX_BYTES: usize = 1024 * 1024;
const DRAG_MAX_PATHS: usize = 16 * 1024;

pub(crate) fn npk_drag_start(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::RENDER, "npk_drag_start") {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 || crate::shade::focused_widget_id() != Some(wid) { return -1; }
    // A drag starts from a press the user is still holding, and one the
    // app has already seen: with a press or release still queued, the
    // call comes from motion of an earlier gesture.
    if crate::shade::cursor::atomic_buttons().0 & 1 == 0 { return -1; }
    if crate::shade::widgets::button_pending(wid) { return -1; }
    // Bounded so the list and the `Event::Drop` that carries it stay small
    // kernel allocations.
    if glen(len).is_none_or(|n| n > DRAG_MAX_BYTES) { return -1; }
    let Some(bytes) = read_bytes(mem, ptr, len) else { return -1 };
    let Ok(text) = core::str::from_utf8(&bytes) else { return -1 };
    let mut paths = alloc::vec::Vec::new();
    for p in text.split('\0').filter(|p| !p.is_empty()) {
        if paths.len() == DRAG_MAX_PATHS || paths.try_reserve(1).is_err() { return -1; }
        if !read_allowed(ctx, p, "npk_drag_start") { return -1; }
        paths.push(String::from(p));
    }
    if paths.is_empty() { return -1; }
    if crate::shade::widgets::drag::start(&ctx.module_name, paths) { 0 } else { -1 }
}

pub(crate) fn npk_capture_screen(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::CAPTURE, "npk_capture_screen") {
        return -1;
    }
    crate::audit::record(&cap_id, crate::audit::Kind::Read, "screen");
    let info = crate::framebuffer::get_info();
    let w = info.width as usize;
    let h = info.height as usize;
    let pitch = info.pitch as usize;
    if w == 0 || h == 0 { return -1; }
    let need = w * h * 4;
    if info.addr == 0 { return -1; }
    if glen(buf_max).is_none_or(|m| m < need) { return too_small(need); }
    // Read the actual displayed MMIO framebuffer (not a shadow
    // buffer): it always holds the final composite (background +
    // windows + cursor) that's physically on screen. The shadow
    // double-buffer can be mid-swap when we (on a worker core)
    // read it, yielding a stale background-only frame.
    // Row by row straight into the guest (pitch may exceed w*4): a
    // screen-sized kernel buffer would need a contiguous heap grow. The
    // reads are uncached, so other fibers of this core get a turn between
    // bands.
    let Some(dst) = guest_mut(mem, buf_ptr, need) else { return -1 };
    let src = info.addr as *const u8;
    for y in 0..h {
        // SAFETY: the GOP framebuffer is identity-mapped and valid
        // for pitch*height bytes; we read w*4 <= pitch per row.
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(y * pitch), dst.as_mut_ptr().add(y * w * 4), w * 4);
        }
        if y % CAPTURE_BAND == CAPTURE_BAND - 1 {
            crate::smp::fiber::yield_if_contended();
        }
    }
    need as i32
}

/// Rows `npk_capture_screen` copies between two looks at the other fibers
/// of its core.
const CAPTURE_BAND: usize = 64;

pub(crate) fn npk_event_poll(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_event_poll") {
        return -1;
    }
    let window_id = ctx.widget_window_id;
    if window_id == 0 { return -1; }
    // -1 also covers "window was closed by shade" (e.g. Mod+Shift+Q)
    // so the app can fall out of its poll loop instead of spinning.
    if !crate::shade::widgets::widget_window_exists(window_id) { return -1; }

    let event = match crate::shade::widgets::poll_event(window_id) {
        Some(e) => e,
        None => return 0,
    };
    let encoded = match postcard::to_allocvec(&event) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    // Too small a buffer is not a closed window: the event stays queued and
    // the answer says how much it needs, as -(needed + 2). -1 keeps meaning
    // "window gone", so an app that knows only that one still stops cleanly
    // instead of reading a size as an event.
    if buf_max < 0 || encoded.len() > buf_max as usize {
        crate::shade::widgets::unpoll_event(window_id, event);
        return too_small(encoded.len());
    }

    write_bytes(mem, buf_ptr, &encoded[..])
}

pub(crate) fn npk_list_modules(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_list_modules") {
        return -1;
    }

    // `sys/wasm` is a real directory: list its immediate children.
    let entries = match crate::npkfs::fs::list("sys/wasm") {
        Ok(Some(v)) => v,
        Ok(None) => alloc::vec::Vec::new(),
        Err(_) => return -1,
    };

    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for e in &entries {
        if !matches!(e.kind, crate::npkfs::object::EntryKind::File) { continue; }
        if e.name.ends_with(".version") { continue; }
        if !out.is_empty() { out.push(0); }
        out.extend_from_slice(e.name.as_bytes());
    }

    write_whole(mem, buf_ptr, buf_max, &out)
}

pub(crate) fn npk_app_meta(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_app_meta") {
        return -1;
    }
    if name_len <= 0 || name_len > 64 { return -1; }

    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    // Confine to sys/wasm/<bare-name>: reject any separator so a caller
    // can't traverse out of the module directory.
    if name.contains('/') { return -1; }
    let path = alloc::format!("sys/wasm/{}", name);

    let (content, _) = match crate::npkfs::fetch(&path) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    // A module that asks for the hardware is a driver, not an app: no
    // launcher lists it, whatever metadata it carries.
    if capability::widget_rights_from_wasm(&content).contains(capability::Rights::HARDWARE) {
        return -1;
    }
    let meta = match extract_wasm_custom_section(&content, ".npk.app_meta") {
        Some(m) => m,
        None => return -1,
    };

    write_whole(mem, buf_ptr, buf_max, meta)
}

pub(crate) fn npk_spawn_module(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER | capability::Rights::EXECUTE, "npk_spawn_module") {
        return -1;
    }
    if name_len <= 0 || name_len > 64 { return -1; }

    let Some(name) = glen(name_len).and_then(|n| guest(mem, name_ptr, n))
        .and_then(|b| core::str::from_utf8(b).ok()).map(String::from) else { return -1 };

    // Path validation — refuse absolute paths, traversal, prefix reuse.
    if name.contains('/') || name.contains("..") || name.is_empty() {
        return -1;
    }

    let path = alloc::format!("sys/wasm/{}", name);
    let (bytes, _hash) = match crate::npkfs::fetch(&path) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    // Grant exactly the rights the app declares in its `.npk.caps`
    // section (e.g. spell asks for WRITE to save files); apps with
    // no declaration get the safe default (no WRITE). Per-app, not
    // a blanket grant.
    let rights = capability::widget_rights_from_wasm(&bytes);
    let module_cap = match capability::create_module_cap(&name, rights, None) {
        Ok(id) => id,
        Err(_) => return -1,
    };

    // Create a new terminal-kind window with its own terminal
    // buffer and focus it. The widget-kind launcher that called
    // us then closes itself (`npk_close_widget`), leaving the
    // new loop + running app on screen.
    let spawned = crate::shade::with_compositor(|comp| {
        let id = comp.create_window(&name, 0, 0, 800, 600)?;
        comp.focus_window(id);
        let term_idx = comp.windows.iter()
            .find(|w| w.id == id)
            .map(|w| w.terminal_idx)?;
        Some((id, term_idx))
    }).flatten();

    let (win_id, term_idx) = match spawned {
        Some(v) => v,
        None => { capability::release_module_cap(&module_cap); return -1; }
    };

    // Fresh session prompt so the terminal isn't stuck on the
    // caller's old prompt state when the app exits.
    crate::intent::reset_session_prompt(term_idx);

    if !spawn_on_worker(bytes.to_vec(), module_cap, term_idx, &name) {
        crate::shade::with_compositor(|comp| comp.close_window(win_id));
        return -1;
    }
    crate::shade::request_render();
    0
}

pub(crate) fn npk_run_intent(mem: &mut [u8], ctx: &mut HostState, verb_ptr: i32, verb_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    // Launching a microvm hands a guest the network, the user's home and
    // sound: a shell role (launcher, panel), not something every module with
    // the default EXECUTE may do.
    let need = capability::Rights::EXECUTE | capability::Rights::SHELL;
    if !capability::require(&cap_id, need, "npk_run_intent") {
        return -1;
    }
    if verb_len <= 0 || verb_len > 64 { return -1; }
    let Some(verb) = glen(verb_len).and_then(|n| guest(mem, verb_ptr, n))
        .and_then(|b| core::str::from_utf8(b).ok()).map(String::from) else { return -1 };
    // Reject only on the pure cooperative Core-0 path (BSP-only),
    // so the caller falls back to typing the intent at the prompt.
    // Fiber mode (vCPU as a pool fiber) AND the dedicated-core path
    // both support launching from a worker, so allow those.
    #[cfg(target_arch = "x86_64")]
    {
        if !crate::microvm::cpu::vm_fiber_mode()
            && crate::smp::per_core::dedicated_vm_core().is_none()
        {
            return -1;
        }
        match verb.as_str() {
            "firefox" => {
                crate::intent::launch_app(&crate::microvm::app::FIREFOX);
                0
            }
            _ => -1,
        }
    }
    // No guest to launch into.
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = verb;
        -1
    }
}

pub(crate) fn npk_bar_state(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_bar_state") {
        return -1;
    }
    let local = crate::time::local(crate::time::now_or_zero());
    let secs = local.rem_euclid(86_400);
    let (ws_count, ws_active, title) =
        crate::shade::with_compositor(|c| c.bar_info())
            .unwrap_or((0, 0, alloc::string::String::new()));
    let s = alloc::format!("{:02}:{:02}\n{}\n{}\n{}",
        secs / 3600, (secs % 3600) / 60, ws_count, ws_active, title);

    write_whole(mem, buf_ptr, max, s.as_bytes())
}

pub(crate) fn npk_window_titles(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_window_titles") {
        return -1;
    }
    let full = capability::holds(&cap_id, capability::Rights::SHELL);
    let s = crate::shade::with_compositor(|c| c.window_lines(full))
        .unwrap_or_default();
    write_whole(mem, buf_ptr, max, s.as_bytes())
}

#[cfg(target_arch = "x86_64")]
/// Copies the `index`-th ACPI table with this signature into module memory.
///
/// `sig` holds the four characters little-endian in an `i32`, as they sit
/// in memory (`SSDT` = 0x54445353).
///
/// Firmware spreads its namespace over the DSDT and any number of SSDTs,
/// and Linux loads them all into one (`acpi_tb_load_namespace`); reading
/// only the DSDT misses names declared elsewhere.
///
/// Returns the length, `-(needed + 2)` if the buffer is too small, or -1
/// (no right / no such table).
pub(crate) fn npk_acpi_table(
    mem: &mut [u8], ctx: &mut HostState, sig: i32, index: i32, buf_ptr: i32, buf_max: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_acpi_table") {
        return -1;
    }
    if index < 0 || index > 63 { return -1; }
    let sig_bytes = (sig as u32).to_le_bytes();
    let Some((addr, len)) = crate::acpi::find_table_nth(&sig_bytes, index as usize) else {
        return -1;
    };
    if glen(buf_max).is_none_or(|m| len > m) {
        return too_small(len);
    }
    // SAFETY: find_table_nth mapped [addr, addr+len).
    let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    write_bytes(mem, buf_ptr, &src[..])
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_acpi_dsdt(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_acpi_dsdt") {
        return -1;
    }
    let Some((addr, len)) = crate::acpi::dsdt() else { return -1 };
    if glen(buf_max).is_none_or(|m| len > m) {
        return too_small(len);
    }
    // SAFETY: acpi::dsdt() mapped [addr, addr+len) for us.
    let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    write_bytes(mem, buf_ptr, &src[..])
}

pub(crate) fn npk_audio_submit(mem: &mut [u8], ctx: &mut HostState, slot: i32, ptr: i32, len: i32) -> i32 {
    if slot < 0 || ptr < 0 || len < 0 { return -1; }
    let Some(pcm) = guest(mem, ptr, len as usize) else { return -1 };
    match crate::audio::submit_for(slot as usize, ctx.pid, pcm) {
        Some(n) => n as i32,
        None => -1,
    }
}

/// `npk_audio_buffered(slot)` — bytes still sitting in the slot's ring,
/// or -1 for a closed/invalid slot.
///
/// This is the play clock. Without it an app can only estimate what has
/// been heard from the wall clock (`submitted - elapsed * rate`), and the
/// wall clock and the audio crystal drift apart. For music nobody notices;
/// for lipsync over a film the error accumulates, which is why every player
/// that shows pictures makes the audio output its master clock.
pub(crate) fn npk_audio_buffered(ctx: &mut HostState, slot: i32) -> i32 {
    if slot < 0 { return -1; }
    match crate::audio::buffered_for(slot as usize, ctx.pid) {
        Some(n) => n as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// Drains the mixed output of every slot: the audio driver's side.
/// Requires `Rights::HARDWARE`; anyone else could record or starve all
/// playback.
pub(crate) fn npk_audio_poll_mix(mem: &mut [u8], ctx: &mut HostState, ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::HARDWARE, "npk_audio_poll_mix") {
        return -1;
    }
    if ptr < 0 || max < 0 { return -1; }
    let Some(out) = guest_mut(mem, ptr, max as usize) else { return -1 };
    crate::audio::poll_mix(ctx.pid, out) as i32
}

#[cfg(target_arch = "x86_64")]
/// The PCI class (config 0x0B) of the device this driver owns, if it owns
/// one.
fn owned_pci_class(ctx: &HostState) -> Option<u8> {
    let addr = ctx.hw.as_ref()?.pci()?;
    (pci::owner(addr) == Some(pci::Owner::Module(ctx.pid))).then(|| pci::read8(addr, 0x0b))
}

#[cfg(target_arch = "x86_64")]
/// The display driver reports the ELD of `port`'s sink, `len` 0 for none
/// (`pin_eld_notify` of Linux' audio component). Only the driver of a
/// display device may.
pub(crate) fn npk_display_audio_set(mem: &mut [u8], ctx: &mut HostState, port: i32, ptr: i32, len: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_set")
        || owned_pci_class(ctx) != Some(0x03)
    {
        return -1;
    }
    if port < 0 || ptr < 0 || len < 0 || len as usize > crate::audio::ELD_MAX { return -1; }
    let Some(eld) = guest_mut(mem, ptr, len as usize) else { return -1 };
    if crate::audio::display_audio_set(port as usize, eld) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
/// The audio driver reads the ELD of `port` (`get_eld`): its length, 0 when
/// no sink is there, `-(needed + 2)` for a short buffer. Only the driver of
/// a multimedia device may.
pub(crate) fn npk_display_audio_get(mem: &mut [u8], ctx: &mut HostState, port: i32, ptr: i32, max: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_get")
        || owned_pci_class(ctx) != Some(0x04)
    {
        return -1;
    }
    if port < 0 { return -1; }
    let mut eld = [0u8; crate::audio::ELD_MAX];
    match crate::audio::display_audio_get(port as usize, &mut eld) {
        Some(n) => write_whole(mem, ptr, max, &eld[..n]),
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// 0 until the display driver reported once, then a count that moves with
/// every report.
pub(crate) fn npk_display_audio_generation(ctx: &mut HostState) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_generation") {
        return -1;
    }
    crate::audio::display_audio_generation() as i32
}

#[cfg(target_arch = "x86_64")]
/// The display driver binds the audio component, says request `done` is
/// carried out, and learns what is wanted now: `(number << 2) | bits`.
pub(crate) fn npk_display_audio_serve(ctx: &mut HostState, done: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_serve")
        || owned_pci_class(ctx) != Some(0x03)
    {
        return -1;
    }
    crate::audio::display_audio_serve(done as u32) as i32
}

#[cfg(target_arch = "x86_64")]
/// The audio driver asks for the display's audio power and the codec wake
/// signal (`bits`): the request's number, -2 while no display driver is
/// bound, -3 when none will be.
pub(crate) fn npk_display_audio_request(ctx: &mut HostState, bits: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_request")
        || owned_pci_class(ctx) != Some(0x04)
        || bits < 0
    {
        return -1;
    }
    match crate::audio::display_audio_request(ctx.pid, bits as u32) {
        Some(n) => n as i32,
        None if crate::audio::intel_display_present() => -2,
        None => -3,
    }
}

#[cfg(target_arch = "x86_64")]
/// The last request the display driver carried out, -2 while none is bound.
pub(crate) fn npk_display_audio_done(ctx: &mut HostState) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_display_audio_done")
        || owned_pci_class(ctx) != Some(0x04)
    {
        return -1;
    }
    crate::audio::display_audio_done().map_or(-2, |n| n as i32)
}

/// Blocks per stretch of the sequential device read: a stretch lies in one
/// allocated run, so a read never crosses into free space.
const BENCH_WINDOW_BLOCKS: u64 = 2048;

#[cfg(target_arch = "x86_64")]
/// `testdisk`'s layered measurements, as u64 values into `out`:
///
/// - 0: device sequential read in calls of `a` blocks over `b` blocks of
///   stored data → bytes, ticks
/// - 1: device read of `a` random stored blocks in batches of `b` → ops, ticks
/// - 2: device sequential write of `b` blocks in calls of `a`, then flush
///   → bytes, ticks in all, ticks of the flush, free areas written
/// - 3: write back and drop the block cache → nothing
/// - 4: read the largest stored objects up to `a` bytes, cold → objects,
///   bytes, ticks
/// - 5: TSC Hz, the object read and write stage counters
///   (`npkfs::bench`), blocks per NVMe command, the path-layer write and
///   commit stage counters
///
/// Needs WRITE, as the other benchmarks: a device write takes free blocks
/// for its duration.
pub(crate) fn npk_disk_bench(mem: &mut [u8], ctx: &mut HostState, kind: i32, a: i64, b: i64,
                             out_ptr: i32, out_len: i32) -> i32 {
    use crate::storage::npkfs::{bench, storage as st};
    if !capability::require(&ctx.cap_id, capability::Rights::WRITE, "npk_disk_bench") || a < 0 || b < 0 {
        return -1;
    }
    let (a, b) = (a as u64, b as u64);
    let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let ok = match kind {
        0 => {
            let window = a.max(BENCH_WINDOW_BLOCKS);
            st::bench_used_windows(window, b)
                .and_then(|w| st::bench_raw_read_seq(&w, window, a))
                .map(|(bytes, t)| v.extend([bytes, t]))
                .is_some()
        }
        1 => st::bench_raw_read_random(a, b as usize).map(|(n, t)| v.extend([n, t])).is_some(),
        2 => st::bench_raw_write_seq(b, a).map(|(bytes, t, flush, areas)| v.extend([bytes, t, flush, areas])).is_some(),
        3 => st::bench_drop_caches(),
        4 => st::bench_read_objects(a).map(|(n, bytes, t)| v.extend([n, bytes, t])).is_some(),
        5 => {
            v.push(crate::time::counter_hz());
            bench::GET.snapshot(&mut v);
            bench::PUT.snapshot(&mut v);
            v.push(crate::nvme::max_blocks_per_cmd() as u64);
            bench::WRITE.snapshot(&mut v);
            bench::COMMIT.snapshot(&mut v);
            bench::LOOKUP.snapshot(&mut v);
            v.push(bench::CACHE_HITS.load(core::sync::atomic::Ordering::Relaxed));
            v.push(bench::CACHE_MISSES.load(core::sync::atomic::Ordering::Relaxed));
            true
        }
        _ => false,
    };
    if !ok { return -1; }
    let bytes: alloc::vec::Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    match write_whole(mem, out_ptr, out_len, &bytes) {
        n if n >= 0 => v.len() as i32,
        e => e,
    }
}

pub(crate) fn npk_fs_list(mem: &mut [u8], ctx: &mut HostState, prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32 {
    if prefix_len > 0 {
        if let Some(prefix) = read_str(mem, prefix_ptr, prefix_len).filter(|p| crate::ext::is_ext(p)) {
            if !ext_allowed(ctx, &prefix, capability::Rights::READ) { return -1; }
            let mut out = alloc::vec::Vec::new();
            if recursive == 0 {
                let entries = match crate::ext::list(&prefix) {
                    Ok(Some(v)) => v,
                    Ok(None) => alloc::vec::Vec::new(),
                    Err(_) => return -1,
                };
                for e in &entries {
                    append_entry(&mut out, &e.name, e.size, e.is_dir, e.mtime);
                }
            } else if !ext_tree(&prefix, &mut out) {
                return -1;
            }
            return write_whole(mem, out_ptr, out_cap, &out);
        }
    }
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::READ, "npk_fs_list") {
        return -1;
    }
    if prefix_len < 0 { return -1; }

    let prefix = if prefix_len == 0 {
        alloc::string::String::new()
    } else {
        match read_str(mem, prefix_ptr, prefix_len) {
            Some(s) => s,
            None => return -1,
        }
    };

    // Directories are real Tree objects; listings come straight from them.
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let prefix_for_list = prefix.trim_matches('/');

    // Listing is reading. Another module's private area must not appear
    // even as a name, since its existence is information. So the requested
    // path must be allowed, and every entry is checked again at its full
    // path; otherwise `list("")` would leak it.
    if !crate::wasm::private_area_allows(prefix_for_list, &ctx.module_name) {
        return -1;
    }
    let module = ctx.module_name.clone();
    let visible = |full: &str| crate::wasm::private_area_allows(full, &module);
    let full_of = |base: &str, rel: &str| -> alloc::string::String {
        if base.is_empty() { alloc::string::String::from(rel) }
        else { alloc::format!("{}/{}", base, rel) }
    };

    if recursive == 0 {
        // Non-recursive: one directory's immediate children.
        let entries = match crate::npkfs::fs::list(prefix_for_list) {
            Ok(Some(v)) => v,
            Ok(None) => alloc::vec::Vec::new(),
            Err(_) => return -1,
        };
        for e in &entries {
            if !visible(&full_of(prefix_for_list, &e.name)) { continue }
            let is_dir = matches!(e.kind, crate::npkfs::object::EntryKind::Dir);
            append_entry(&mut out, &e.name, e.size, is_dir, e.mtime);
        }
    } else {
        // Recursive: DFS the subtree, emit relative paths.
        fn dfs(
            base: &str, rel: alloc::string::String,
            out: &mut alloc::vec::Vec<u8>,
            visible: &dyn Fn(&str) -> bool,
        ) -> Result<(), ()> {
            let abs = if rel.is_empty() {
                alloc::string::String::from(base)
            } else if base.is_empty() {
                rel.clone()
            } else {
                alloc::format!("{}/{}", base, rel)
            };
            let entries = match crate::npkfs::fs::list(&abs) {
                Ok(Some(v)) => v,
                Ok(None) => return Ok(()),
                Err(_) => return Err(()),
            };
            for e in &entries {
                let child_rel = if rel.is_empty() {
                    e.name.clone()
                } else {
                    alloc::format!("{}/{}", rel, e.name)
                };
                let child_abs = if abs.is_empty() {
                    e.name.clone()
                } else {
                    alloc::format!("{}/{}", abs, e.name)
                };
                // Skip it and do not descend into it.
                if !visible(&child_abs) { continue }
                match e.kind {
                    crate::npkfs::object::EntryKind::File => {
                        append_entry(out, &child_rel, e.size, false, e.mtime);
                    }
                    crate::npkfs::object::EntryKind::Dir => {
                        append_entry(out, &child_rel, 0, true, e.mtime);
                        dfs(base, child_rel, out, visible)?;
                    }
                }
            }
            Ok(())
        }
        if dfs(prefix_for_list, alloc::string::String::new(), &mut out, &visible).is_err() {
            return -1;
        }
    }

    write_whole(mem, out_ptr, out_cap, &out)
}

/// `npk_config_get(key, buf)`: a `set` value of the module's own
/// namespace, `<module>.<name>`; its length, 0 when the key is unset,
/// `-(needed + 2)` for a buffer too short, -1 for a key outside the
/// namespace. Read only: the user sets it, the module reads it.
pub(crate) fn npk_config_get(mem: &mut [u8], ctx: &mut HostState, key_ptr: i32, key_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(key) = read_str(mem, key_ptr, key_len) else { return -1 };
    let Some(name) = key.strip_prefix(ctx.module_name.as_str()).and_then(|r| r.strip_prefix('.')) else { return -1 };
    if name.is_empty() || ctx.module_name.is_empty() {
        return -1;
    }
    let Some(value) = crate::config::get(&key) else { return 0 };
    write_whole(mem, buf_ptr, buf_max, value.as_bytes())
}

/// `npk_setting_get(key, buf)`: a system setting the settings table lets
/// apps read (docs/spec/SETTINGS.md); RENDER.
pub(crate) fn npk_setting_get(mem: &mut [u8], ctx: &mut HostState, key_ptr: i32, key_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::RENDER, "npk_setting_get") {
        return -1;
    }
    if glen(key_len).is_none_or(|n| n > crate::settings::MAX_KEY) { return -1; }
    let Some(key) = read_str(mem, key_ptr, key_len) else { return -1 };
    if !crate::settings::app_reads(&key) { return -1; }
    let Some(value) = crate::settings::get_for_app(&key) else { return 0 };
    write_whole(mem, buf_ptr, buf_max, value.as_bytes())
}

/// `npk_setting_set(key, value)`: change a system setting the settings
/// table lets apps write, through the loop's `set` path; SETTINGS.
pub(crate) fn npk_setting_set(mem: &mut [u8], ctx: &mut HostState, key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::SETTINGS, "npk_setting_set") {
        return -1;
    }
    if glen(key_len).is_none_or(|n| n > crate::settings::MAX_KEY) { return -1; }
    if glen(val_len).is_none_or(|n| n > crate::settings::MAX_VALUE) { return -2; }
    let Some(key) = read_str(mem, key_ptr, key_len) else { return -1 };
    let Some(value) = read_str(mem, val_ptr, val_len) else { return -2 };
    match crate::settings::set(&key, &value, crate::settings::Origin::App) {
        Ok(_) => {
            let what = alloc::format!("{} = {}", key, value);
            crate::audit::record(&ctx.cap_id, crate::audit::Kind::Setting, &what);
            0
        }
        Err(crate::settings::Error::NotWritable) => -1,
        Err(crate::settings::Error::Invalid(_)) => -2,
    }
}

pub(crate) fn npk_fs_stat(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, out_ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::READ, "npk_fs_stat") {
        return -1;
    }
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };

    if crate::ext::is_ext(&name) {
        if !ext_allowed(ctx, &name, capability::Rights::READ) { return -1; }
        let st = match crate::ext::stat(&name) {
            Ok(Some(st)) => st,
            Ok(None) => return 0,
            Err(_) => return -1,
        };
        let mut buf = [0u8; 17];
        buf[0..8].copy_from_slice(&st.size.to_le_bytes());
        buf[8] = st.is_dir as u8;
        buf[9..17].copy_from_slice(&st.mtime.to_le_bytes());
        return write_bytes(mem, out_ptr, &buf);
    }
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        return -1;   // silently: another module's private area does not exist
    }
    let (size, is_dir, mtime) = match crate::npkfs::fs::stat(&name) {
        Ok(Some(s)) => {
            let is_dir = matches!(s.kind, crate::npkfs::object::EntryKind::Dir);
            (s.size, if is_dir { 1u8 } else { 0u8 }, s.mtime)
        }
        Ok(None) => return 0,
        Err(_) => return -1,
    };

    let mut buf = [0u8; 17];
    buf[0..8].copy_from_slice(&size.to_le_bytes());
    buf[8] = is_dir;
    buf[9..17].copy_from_slice(&mtime.to_le_bytes());
    write_bytes(mem, out_ptr, &buf)
}

pub(crate) fn npk_set_wallpaper(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32, width: i32, height: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::WRITE, "npk_set_wallpaper") {
        kprintln!("[npk] WASM: npk_set_wallpaper DENIED (no WRITE)");
        return -1;
    }

    if width <= 0 || height <= 0 { return -1; }
    let Some(pixel_bytes) = (width as usize).checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4)) else { return -1 };
    if glen(len).is_none_or(|n| n < pixel_bytes) { return -1; }
    let Some(pixels) = guest(mem, ptr, pixel_bytes) else { return -1 };

    let info = crate::framebuffer::get_info();
    crate::gui::background::set_wallpaper(pixels, width as u32, height as u32, &info);

    // Force compositor full redraw
    crate::shade::force_redraw();
    kprintln!("[npk] Wallpaper set ({}x{})", width, height);
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_set_theme(mem: &mut [u8], ctx: &mut HostState, ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::WRITE, "npk_set_theme") {
        return -1;
    }

    let Some(raw) = guest(mem, ptr, 64) else { return -1 };
    let mut colors = [0u32; 16];
    for (c, b) in colors.iter_mut().zip(raw.chunks_exact(4)) {
        *c = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    }
    crate::theme::set_palette(&colors);
    crate::shade::force_redraw();
    0
}

pub(crate) fn npk_stream_read(mem: &mut [u8], ctx: &mut HostState, idx: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    if !stream_allowed(ctx) { return -1; }
    if buf_len <= 0 { return 0; }
    let Some(out) = guest_mut(mem, buf_ptr, buf_len as usize) else { return -1 };
    if idx < 0 {
        return crate::serial::mirror_read(out) as i32;
    }
    crate::shade::terminal::stream_read(idx as usize, out) as i32
}

/// Most one `npk_tcp_send` may hand the kernel. The send buffer takes a
/// whole call when it is empty, and module memory is far larger than the
/// kernel's reserve; a bigger send is the caller's loop to make.
const MAX_TCP_SEND: usize = 1024 * 1024;

pub(crate) fn npk_tcp_send(mem: &mut [u8], ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    if buf_len <= 0 || buf_len as usize > MAX_TCP_SEND { return -1; }
    let Some(h) = own_tcp(ctx, handle) else { return -1 };
    let Some(bytes) = guest(mem, buf_ptr, buf_len as usize) else { return -1 };
    match crate::net::tcp::send(h, bytes) {
        Ok(_) => 0,
        // Backpressure, not a failure: too much is still unacked.
        // A module that treats this as fatal drops a live connection.
        Err(crate::net::tcp::TcpError::WouldBlock) => -2,
        Err(_) => -1,
    }
}

pub(crate) fn npk_tcp_recv(mem: &mut [u8], ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    if buf_max <= 0 { return -1; }
    let Some(h) = own_tcp(ctx, handle) else { return -1 };
    let Some(out) = guest_mut(mem, buf_ptr, buf_max as usize) else { return -1 };
    match crate::net::tcp::recv(h, out) {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

/// The bytes `off .. off + len` of a module's DMA buffer, as a physical
/// address, or `None` when the handle is not the module's or the range
/// leaves the buffer. Negative values are refused before any arithmetic.
fn dma_range(ctx: &HostState, handle: i32, off: i32, len: i32) -> Option<(u64, usize)> {
    let len = usize::try_from(len).ok()?;
    Some((dma_at(ctx, handle, off, len)?.0, len))
}

pub(crate) fn npk_dma_read(mem: &mut [u8], ctx: &mut HostState, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    let Some((phys, len)) = dma_range(ctx, handle, dma_off, len) else { return -1 };
    let Some(dst) = guest_mut(mem, wasm_ptr, len) else { return -1 };
    // SAFETY: `phys .. phys + len` lies inside one of this module's own live
    // DMA allocations (`dma_range`; a freed one has no pages), which are
    // identity-mapped. The device may write it concurrently; the
    // bytes are copied as they are.
    let src = unsafe { core::slice::from_raw_parts(phys as *const u8, len) };
    dst.copy_from_slice(src);
    0
}

pub(crate) fn npk_dma_write(mem: &mut [u8], ctx: &mut HostState, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    let Some((phys, len)) = dma_range(ctx, handle, dma_off, len) else { return -1 };
    let Some(src) = guest(mem, wasm_ptr, len) else { return -1 };
    // SAFETY: as in `npk_dma_read`; the range is the module's own buffer and
    // nothing in the kernel holds a reference into it.
    let dst = unsafe { core::slice::from_raw_parts_mut(phys as *mut u8, len) };
    dst.copy_from_slice(src);
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_register(mem: &mut [u8], ctx: &mut HostState, mac_ptr: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if hw.registered_as_netdev { return -1; } // already registered

    let Some(raw) = guest(mem, mac_ptr, 6) else { return -1 };
    let mut mac = [0u8; 6];
    mac.copy_from_slice(raw);

    if !crate::netdev::register_wasm_nic(mac) {
        kprintln!("[npk] {}: NIC refused, another driver holds it", ctx.module_name);
        return -1;
    }
    // Re-borrow after register call
    if let Some(h) = ctx.hw.as_mut() {
        h.registered_as_netdev = true;
    }
    kprintln!("[npk] WASM driver registered as NIC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    0
}

pub(crate) fn npk_print(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) {
    if let Some(s) = read_str(mem, ptr, len) {
        if ctx.direct_output {
            let idx = ctx.terminal_idx;
            #[cfg(target_arch = "x86_64")]
            if idx != TERM_IDX_ACTIVE && (idx as usize) < MAX_APP_BUFS {
                // Write to specific terminal (worker-core safe)
                crate::shade::terminal::write_idx(idx as usize, &s);
                return;
            }
            // The active terminal, or the console where there are no
            // terminals.
            let _ = idx;
            kprint!("{}", s);
        } else {
            ctx.output.push_str(&s);
        }
    }
}

/// Module diagnostics (`npk_log`, `npk_log_serial`) follow `kdebug!`: always
/// in the capture buffer (`dmesg`), on screen and serial only with
/// `bootlog verbose`. Program output goes through `npk_print`.
pub(crate) fn npk_log(mem: &mut [u8], _ctx: &mut HostState, ptr: i32, len: i32) {
    if let Some(s) = read_str(mem, ptr, len) {
        crate::kdebug!("{}", s);
    }
}

pub(crate) fn npk_log_serial(mem: &mut [u8], _ctx: &mut HostState, ptr: i32, len: i32) {
    if let Some(s) = read_str(mem, ptr, len) {
        crate::drivers::serial::capture_fmt(format_args!("{}\n", s));
        if crate::drivers::serial::verbose() {
            let serial = crate::drivers::serial::SERIAL.lock();
            for byte in s.bytes() {
                if byte == b'\n' { serial.write_byte(b'\r'); }
                serial.write_byte(byte);
            }
            serial.write_byte(b'\r');
            serial.write_byte(b'\n');
        }
        // ...and to the remote mirror. Outside the SERIAL lock: the sink
        // takes its own, and holding both risks a deadlock.
        crate::shade::terminal::stream_push_global(&s);
        crate::shade::terminal::stream_push_global("\n");
    }
}

#[cfg(target_arch = "x86_64")]
/// Fetch a document: the navigation itself, which goes wherever it is
/// pointed, a home-network address included, as in any browser. Reach is
/// therefore not applied here. It is applied to everything a page fetches
/// in turn (`npk_http_begin`, `npk_http_send`, `npk_http_request_many`,
/// `npk_tls_connect`), so a public page cannot reach into the home network.
/// It separates pages inside a module from each other; against the module
/// itself, which declares NET, it cannot be enforced and is not meant to be.
pub(crate) fn npk_http_request(mem: &mut [u8], ctx: &mut HostState, url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_request DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "npk_http_request", e.reason());
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = (buf_max as usize).min(max_http_body());

    let url = match read_str(mem, url_ptr, url_len) {
        Some(s) => s,
        None => return -1,
    };
    let (host, path, tls) = match crate::intent::http::parse_url(&url) {
        Ok(hp) => hp,
        Err(e) => {
            kprintln!("[npk] WASM: npk_http_request bad url: {}", e);
            return -1;
        }
    };
    crate::audit::record(&cap_id, crate::audit::Kind::Net, &host);

    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut info = crate::intent::http::FetchInfo::default();
    let res = crate::intent::http::https_get_streaming_ex(
        &host, &path, crate::intent::http::limit_for(cap),
        &mut |chunk: &[u8]| crate::intent::http::collect_within(&mut out, cap, chunk),
        Some(&mut info),
        &crate::intent::http::HttpRequest {
            // The document itself compresses well.
            accept_gzip: tls,
            // HTTP/2 avoids per-connection throttling by some servers
            // (BROWSER.md §8.1).
            try_h2: tls,
            plain: !tls,
            ..Default::default()
        },
    );
    // A failed request must not leave the previous request's final
    // URL readable as if it were this one's.
    let ok = res.is_ok();
    ctx.http_final_url =
        if ok && !info.final_url.is_empty() { Some(info.final_url) } else { None };
    // Same rule: a stale Content-Type would make the next document
    // decode against the last one's charset.
    ctx.http_content_type =
        if ok && !info.content_type.is_empty() { Some(info.content_type) } else { None };
    // Same rule for the reason: cleared on success, so a caller can
    // never read a stale error and attribute it to this request.
    ctx.http_last_error = match &res {
        Ok(_) => None,
        Err(e) => Some(alloc::format!("{}\t{}", crate::intent::http::error_kind(e), e)),
    };
    if res.is_err() { return -1; }
    // Bounds-checked write: buf_ptr is guest-controlled, and a
    // wrapping `start + len` would panic the kernel on the slice index.
    write_bytes(mem, buf_ptr, &out)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_http_send(mem: &mut [u8], ctx: &mut HostState, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_send DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "npk_http_send", e.reason());
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = (buf_max as usize).min(max_http_body());

    let method = match read_str(mem, method_ptr, method_len) {
        Some(s) => s,
        None => return -1,
    };
    // The method sits at the very front of the request line and the
    // headers end it — a newline in either rewrites the request, and
    // everything after it is read as a second one. This is the check
    // that stops a sandboxed app from smuggling requests through us.
    if !crate::intent::http::method_is_safe(&method) {
        kprintln!("[npk] WASM: npk_http_send rejected method");
        return -1;
    }
    let url = match read_str(mem, url_ptr, url_len) {
        Some(s) => s,
        None => return -1,
    };
    journal_url(&cap_id, &url);
    let hdr_blob = if hdrs_len > 0 {
        match read_str(mem, hdrs_ptr, hdrs_len) {
            Some(s) => s,
            None => return -1,
        }
    } else {
        String::new()
    };
    let mut headers: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for line in hdr_blob.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
        if !crate::intent::http::header_line_is_safe(line) {
            kprintln!("[npk] WASM: npk_http_send rejected a header");
            return -1;
        }
        headers.push(String::from(line));
    }
    let body = if body_len > 0 {
        match read_bytes(mem, body_ptr, body_len) {
            Some(b) => b,
            None => return -1,
        }
    } else {
        alloc::vec::Vec::new()
    };

    let (host, path, tls) = match crate::intent::http::parse_url(&url) {
        Ok(hp) => hp,
        Err(e) => {
            kprintln!("[npk] WASM: npk_http_send bad url: {}", e);
            return -1;
        }
    };

    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut info = crate::intent::http::FetchInfo::default();
    let req = crate::intent::http::HttpRequest {
        method: &method, headers: &headers, body: &body,
        // The browser asks for gzip and the kernel unpacks it; an app
        // cannot set `Accept-Encoding` itself (RESERVED_HEADERS).
        accept_gzip: tls,
        try_h2: tls,
        plain: !tls,
        // Everything crossing the WASM boundary counts as page code, even
        // when beak forwards it. The reach comes from the context, which
        // the kernel computed itself, not from the request.
        from_reach: Some(ctx.net_reach),
        require_partial: false,
        addr: None,
    };
    let res = crate::intent::http::https_request_streaming(
        &host, &path, &req, crate::intent::http::limit_for(cap),
        &mut |chunk: &[u8]| crate::intent::http::collect_within(&mut out, cap, chunk),
        Some(&mut info),
        true,
    );
    // Same rule as npk_http_request throughout: everything is cleared
    // on failure, so a caller can never read one request's answer and
    // attribute it to the next.
    let ok = res.is_ok();
    ctx.http_final_url =
        if ok && !info.final_url.is_empty() { Some(info.final_url) } else { None };
    ctx.http_content_type =
        if ok && !info.content_type.is_empty() { Some(info.content_type) } else { None };
    ctx.http_reply_headers =
        if ok { Some(info.headers) } else { None };
    ctx.http_status = if ok { info.status } else { 0 };
    ctx.http_last_error = match &res {
        Ok(_) => None,
        Err(e) => Some(alloc::format!("{}\t{}", crate::intent::http::error_kind(e), e)),
    };
    if res.is_err() { return -1; }
    write_bytes(mem, buf_ptr, &out)
}

// ── Fetching without standing still ────────────────────────────────────────
//
// The same two requests as `npk_http_send` / `npk_http_request_many`, split
// into "start it" and "collect it". Between the two the module keeps running:
// it paints, it reads keys, its peer fibers get their turns. The wait happens
// on a worker fiber on another core (`intent::fetch`).
//
// A handle is answered only to the process that opened it — `ctx.pid`, not
// the caller's word — so guessing a small integer cannot read another app's
// document.

#[cfg(target_arch = "x86_64")]
/// Sets this module's reach context.
///
/// The kernel trusts the module for the document URL but not for its class:
/// beak passes the address, and this function computes the network class
/// itself. Otherwise the sandbox would be drawing its own boundary.
///
/// An address without an origin (`beak:selftest`, `about:blank`) and
/// anything that does not resolve falls back to `Public`, the strictest
/// class.
pub(crate) fn npk_net_context(
    mem: &mut [u8], ctx: &mut HostState, url_ptr: i32, url_len: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::NET, "npk_net_context") {
        return -1;
    }
    let url = match read_str(mem, url_ptr, url_len) {
        Some(s) => s,
        None => return -1,
    };
    let reach = crate::intent::http::reach_of_url(&url);
    if reach != ctx.net_reach {
        kprintln!("[npk] net context: {:?} ({})", reach, url);
    }
    ctx.net_reach = reach;
    0
}

#[cfg(target_arch = "x86_64")]
/// Start one request. Returns a handle (>= 1), or -1 with the reason in
/// `npk_http_last_error`. Same validation as `npk_http_send`: it is the same
/// request, only nobody waits for it here.
pub(crate) fn npk_http_begin(
    mem: &mut [u8], ctx: &mut HostState,
    method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32,
    hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32,
    buf_max: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_begin DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "npk_http_begin", e.reason());
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = (buf_max as usize).min(max_http_body());

    let method = match read_str(mem, method_ptr, method_len) {
        Some(s) => s,
        None => return -1,
    };
    if !crate::intent::http::method_is_safe(&method) {
        kprintln!("[npk] WASM: npk_http_begin rejected method");
        return -1;
    }
    let url = match read_str(mem, url_ptr, url_len) {
        Some(s) => s,
        None => return -1,
    };
    journal_url(&cap_id, &url);
    let hdr_blob = if hdrs_len > 0 {
        match read_str(mem, hdrs_ptr, hdrs_len) {
            Some(s) => s,
            None => return -1,
        }
    } else {
        String::new()
    };
    let mut headers: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for line in hdr_blob.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
        if !crate::intent::http::header_line_is_safe(line) {
            kprintln!("[npk] WASM: npk_http_begin rejected a header");
            return -1;
        }
        headers.push(String::from(line));
    }
    let body = if body_len > 0 {
        match read_bytes(mem, body_ptr, body_len) {
            Some(b) => b,
            None => return -1,
        }
    } else {
        alloc::vec::Vec::new()
    };
    let (host, path, tls) = match crate::intent::http::parse_url(&url) {
        Ok(hp) => hp,
        Err(e) => {
            // A refusal at the door has to name itself the same way a failed
            // exchange does, or the caller's error page says "unknown".
            ctx.http_last_error = Some(alloc::format!("url\t{}", e));
            return -1;
        }
    };

    match crate::intent::fetch::begin_one(
        ctx.pid, ctx.core_id, method, host, path, headers, body, cap, tls,
        Some(ctx.net_reach),
    ) {
        Ok(h) => h,
        Err(e) => {
            ctx.http_last_error = Some(alloc::format!("queue\t{}", e));
            if crate::intent::fetch::is_busy(e) { -2 } else { -1 }
        }
    }
}

#[cfg(target_arch = "x86_64")]
/// Start a batch. Returns a handle (>= 1) or -1; the answer comes back
/// through `npk_http_take_many`.
///
/// Without headers. Kept so modules built against this signature still
/// link: a changed signature is a bind error, and the app would not start.
pub(crate) fn npk_http_begin_many(
    mem: &mut [u8], ctx: &mut HostState,
    urls_ptr: i32, urls_len: i32, out_max: i32,
) -> i32 {
    npk_http_begin_many_hdr(mem, ctx, urls_ptr, urls_len, 0, 0, out_max)
}

#[cfg(target_arch = "x86_64")]
/// Like [`npk_http_begin_many`], but with one cookie line per URL, so
/// subresources of a logged-in page (images, stylesheets, scripts) carry
/// the session too.
///
/// `hdrs` is a block of lines positional to `urls`: line `i` is the value
/// of the `Cookie` header for `urls[i]`, or empty. Empty lines are not
/// filtered out, otherwise positions shift and a cookie goes to the wrong
/// URL.
///
/// The cookie jar belongs to the browser; the kernel only validates and
/// forwards.
pub(crate) fn npk_http_begin_many_hdr(
    mem: &mut [u8], ctx: &mut HostState,
    urls_ptr: i32, urls_len: i32, hdrs_ptr: i32, hdrs_len: i32, out_max: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_begin_many DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "npk_http_begin_many", e.reason());
        return -1;
    }
    if out_max <= 0 { return -1; }
    let blob = match read_str(mem, urls_ptr, urls_len) {
        Some(s) => s,
        None => return -1,
    };
    let urls: alloc::vec::Vec<String> = blob
        .split('\n')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for u in &urls { journal_url(&cap_id, u); }
    // One cookie line per URL, in order, checked like a header on the
    // single-request path. A line that fails becomes no cookie rather than
    // a refused request: the resource then loads anonymously instead of
    // not at all.
    let cookies: alloc::vec::Vec<String> = if hdrs_len > 0 {
        match read_str(mem, hdrs_ptr, hdrs_len) {
            Some(blob) => blob
                .split('\n')
                .map(|line| {
                    let v = line.trim();
                    if v.is_empty() { return String::new() }
                    if crate::intent::http::header_line_is_safe(&alloc::format!("cookie: {v}")) {
                        String::from(v)
                    } else {
                        kprintln!("[npk] WASM: npk_http_begin_many dropped an unsafe cookie line");
                        String::new()
                    }
                })
                .collect(),
            None => return -1,
        }
    } else {
        alloc::vec::Vec::new()
    };
    match crate::intent::fetch::begin_many(ctx.pid, ctx.core_id, urls, cookies, (out_max as usize).min(max_http_body()),
                                          Some(ctx.net_reach)) {
        Ok(h) => h,
        Err(e) => {
            ctx.http_last_error = Some(alloc::format!("queue\t{}", e));
            -1
        }
    }
}

#[cfg(target_arch = "x86_64")]
/// 1 = an answer is waiting, 0 = still running, -1 = it failed (call
/// `npk_http_take` for the reason), -2 = no such handle.
pub(crate) fn npk_http_poll(ctx: &mut HostState, handle: i32) -> i32 {
    crate::intent::fetch::poll(ctx.pid, handle)
}

#[cfg(target_arch = "x86_64")]
/// Collect a finished single request. Bytes written on success; -1 if the
/// request failed; -2 if the handle is unknown; -3 while it is still running;
/// -4 if the body is longer than `buf_max` (it is at most the `buf_max` of
/// `npk_http_begin`). The job survives -3 and -4 only.
///
/// Fills exactly the five getters `npk_http_send` fills, and clears them the
/// same way — a caller must never read one request's answer and attribute it
/// to the next.
pub(crate) fn npk_http_take(mem: &mut [u8], ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if buf_max < 0 { return -1; }
    if crate::intent::fetch::body_len(ctx.pid, handle).is_some_and(|n| n > buf_max as usize) {
        return -4;
    }
    let reply = match crate::intent::fetch::take(ctx.pid, handle) {
        crate::intent::fetch::Take::Got(r) => r,
        crate::intent::fetch::Take::NotReady => return -3,
        crate::intent::fetch::Take::Unknown => return -2,
    };
    let ok = reply.error.is_empty();
    ctx.http_final_url =
        if ok && !reply.final_url.is_empty() { Some(reply.final_url) } else { None };
    ctx.http_content_type =
        if ok && !reply.content_type.is_empty() { Some(reply.content_type) } else { None };
    ctx.http_reply_headers = if ok { Some(reply.headers) } else { None };
    ctx.http_status = if ok { reply.status } else { 0 };
    ctx.http_last_error = if ok { None } else { Some(reply.error) };
    if !ok { return -1; }
    write_bytes(mem, buf_ptr, &reply.body)
}

#[cfg(target_arch = "x86_64")]
/// Collect a finished batch: the bodies back to back in `out`, one
/// little-endian i32 length per URL in `lens` (-1 for one that failed, -2
/// for one left out because the batch's buffer was full).
/// Returns how many URLs the batch had, or -1 / -2 / -3 as above.
///
/// Touches none of the response getters — a batch has one status per URL and
/// no headers, exactly as `npk_http_request_many` has always had it, and
/// clobbering the document's headers with a picture's would be worse than
/// silence.
pub(crate) fn npk_http_take_many(
    mem: &mut [u8], ctx: &mut HostState,
    handle: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32,
) -> i32 {
    if out_max <= 0 || lens_max <= 0 || out_ptr < 0 || lens_ptr < 0 { return -1; }
    // Asked before taking: `take` destroys the job, so a length table too
    // small to hold the answer has to be refused while the answer still
    // exists — otherwise a caller that sized it wrong loses the batch.
    match crate::intent::fetch::result_count(ctx.pid, handle) {
        Some(n) if (lens_max as usize) < n * 4 => return -1,
        _ => {}
    }
    let reply = match crate::intent::fetch::take(ctx.pid, handle) {
        crate::intent::fetch::Take::Got(r) => r,
        crate::intent::fetch::Take::NotReady => return -3,
        crate::intent::fetch::Take::Unknown => return -2,
    };
    if !reply.error.is_empty() {
        ctx.http_last_error = Some(reply.error);
        return -1;
    }
    let mut lens: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for n in &reply.lens {
        lens.extend_from_slice(&n.to_le_bytes());
    }
    // Refused, not truncated. The length table describes the whole blob, so a
    // short write would leave the caller slicing bodies out of bytes that were
    // never written. (`npk_http_take` may truncate — there is no table there.)
    if reply.body.len() > out_max as usize { return -1; }
    if write_bytes(mem, lens_ptr, &lens) < 0 { return -1; }
    if !reply.body.is_empty() && write_bytes(mem, out_ptr, &reply.body) < 0 { return -1; }
    reply.lens.len() as i32
}

#[cfg(target_arch = "x86_64")]
/// Stop caring about a handle. Always 0 — a browser cancels on every
/// navigation and must not have to know which state it caught.
pub(crate) fn npk_http_cancel(ctx: &mut HostState, handle: i32) -> i32 {
    crate::intent::fetch::cancel(ctx.pid, handle);
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_http_request_many(mem: &mut [u8], ctx: &mut HostState, urls_ptr: i32, urls_len: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_request_many DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        crate::audit::denied(&cap_id, capability::Rights::NET, "npk_http_request_many", e.reason());
        return -1;
    }
    if out_max <= 0 || lens_max <= 0 || out_ptr < 0 || lens_ptr < 0 { return -1; }

    let blob = match read_str(mem, urls_ptr, urls_len) {
        Some(s) => s,
        None => return -1,
    };
    let urls: alloc::vec::Vec<String> = blob
        .split('\n')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for u in &urls { journal_url(&cap_id, u); }
    // Bound the work a single call can ask for, and make sure the
    // guest actually gave us room for one length per URL.
    const MAX_URLS: usize = 64;
    if urls.is_empty() || urls.len() > MAX_URLS { return -1; }
    if (lens_max as usize) < urls.len() * 4 { return -1; }

    let total_cap = (out_max as usize).min(max_http_body());
    let bodies = crate::intent::http::https_get_many(
        &urls, &[], crate::intent::http::limit_for(total_cap), Some(ctx.net_reach));

    let mut blobs: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut lens: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for body in bodies {
        let n: i32 = match body {
            // Left out rather than truncated: half an image decodes to
            // garbage, and the caller can fetch a missing one on its own.
            Some(b) if blobs.len() + b.len() <= total_cap => {
                blobs.extend_from_slice(&b);
                b.len() as i32
            }
            Some(_) => crate::intent::fetch::OVER_BUDGET,
            None => -1,
        };
        lens.extend_from_slice(&n.to_le_bytes());
    }

    if write_bytes(mem, lens_ptr, &lens) < 0 { return -1; }
    if !blobs.is_empty() && write_bytes(mem, out_ptr, &blobs) < 0 { return -1; }
    urls.len() as i32
}

/// What a caller may pass on for `path`: READ and WRITE as far as it holds
/// them itself, globally or through a grant on that path. Opening a file in
/// another app hands over the user's choice, never more than the caller had.
fn passable_rights(cap: &capability::CapId, path: &str) -> capability::Rights {
    use capability::Rights;
    let mut r = Rights::empty();
    let ext = crate::ext::is_ext(path);
    for want in [Rights::READ, Rights::WRITE] {
        let global = capability::holds(cap, want)
            && (!ext || capability::holds(cap, Rights::EXTERNAL));
        if global || capability::check_path_grant(cap, path, want)
        {
            r |= want;
        }
    }
    r
}

pub(crate) fn npk_open(mem: &mut [u8], ctx: &mut HostState, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::EXECUTE, "npk_open") {
        return -1;
    }
    let app = match read_str(mem, app_ptr, app_len) {
        Some(s) => s,
        None => return -1,
    };
    // Module name only — no path traversal into the store.
    if app.is_empty() || app.contains('/') || app.contains("..") { return -1; }
    let arg = if arg_len > 0 { read_str(mem, arg_ptr, arg_len) } else { None };

    // Singleton + tabs: if the target app already has a widget
    // window (titled with its module name), deliver the open as an
    // Event::Open to that instance and focus it instead of
    // spawning a duplicate. Only when there's something to open.
    if let Some(arg_str) = arg.clone() {
        let existing = crate::shade::with_compositor(|c| {
            c.windows.iter()
                .find(|w| w.kind == crate::shade::window::WindowKind::Widget
                    && w.title == app)
                .map(|w| w.id)
        }).flatten();
        if let Some(id) = existing {
            // Same deal as a pick: the user pointed at this file
            // (a double-click in the file manager), so the app may
            // read and save it — and nothing else.
            let pass = passable_rights(&cap_id, &arg_str);
            if let Some(cap) = crate::shade::widgets::window_cap(id.0) {
                if !pass.is_empty() { capability::grant_path(cap, &arg_str, pass); }
            }
            crate::shade::widgets::push_event(
                id.0, crate::shade::widgets::abi::Event::Open(arg_str));
            crate::shade::with_compositor(|c| c.focus_window(id));
            crate::shade::request_render();
            return 0;
        }
    }

    let path = alloc::format!("sys/wasm/{}", app);
    let bytes = match crate::npkfs::fetch(&path) {
        Ok((b, _)) => b,
        Err(_) => return -1,
    };
    let rights = capability::widget_rights_from_wasm(&bytes);
    let module_cap = match capability::create_module_cap(&app, rights, None) {
        Ok(id) => id,
        Err(_) => return -1,
    };
    // Launching an app on a file is the user pointing at it — grant
    // that one path so the app can save it back without holding
    // WRITE over the whole store.
    if let Some(a) = arg.as_deref() {
        let pass = passable_rights(&cap_id, a);
        if !pass.is_empty() { capability::grant_path(module_cap, a, pass); }
    }
    // Create the widget window now (synchronously, titled with the
    // module name) instead of lazily on first scene_commit. The
    // app spawns asynchronously, so without this a rapid second
    // open (e.g. a double-click = two opens) would see no window
    // yet and spawn a duplicate instance. Pre-creating lets the
    // next open find it and route an Event::Open tab instead.
    let win = match crate::shade::with_compositor(|c| c.create_widget_window(&app)) {
        Some(id) => id,
        None => {
            capability::revoke_path_grants(&module_cap);
            capability::release_module_cap(&module_cap);
            return -1;
        }
    };
    crate::shade::focus_window(win); // bring the editor to the front
    crate::shade::request_render();
    if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, win.0, arg) { 0 } else { -1 }
}

/// Where a tool started by `npk_launch_at` is to appear: the anchor's
/// screen rectangle, keyed by module name, taken by the app's first
/// `npk_window_set_overlay`. An entry older than `ANCHOR_TICKS` is stale.
static LAUNCH_ANCHORS: spin::Mutex<alloc::vec::Vec<(String, (i32, i32, u32, u32), u64)>> =
    spin::Mutex::new(alloc::vec::Vec::new());
const ANCHOR_TICKS: u64 = 500;
/// Logical px between the anchor and the tool's window.
const LAUNCH_GAP: i32 = 8;

fn take_launch_anchor(module: &str) -> Option<(i32, i32, u32, u32)> {
    let now = crate::time::ticks();
    let mut v = LAUNCH_ANCHORS.lock();
    v.retain(|e| now.saturating_sub(e.2) <= ANCHOR_TICKS);
    let i = v.iter().position(|e| e.0 == module)?;
    Some(v.remove(i).1)
}

pub(crate) fn npk_launch_at(mem: &mut [u8], ctx: &mut HostState, app_ptr: i32, app_len: i32,
    arg_ptr: i32, arg_len: i32, anchor: i32) -> i32
{
    let Some(app) = read_str(mem, app_ptr, app_len) else { return -1 };
    // The anchor is a node of the caller's own window, resolved now.
    let rect = (ctx.widget_window_id != 0).then(||
        crate::shade::widgets::anchor_rect(ctx.widget_window_id, anchor as u32)).flatten();
    let r = npk_launch(mem, ctx, app_ptr, app_len, arg_ptr, arg_len);
    if r == 0 {
        if let Some(rect) = rect {
            let mut v = LAUNCH_ANCHORS.lock();
            v.retain(|e| e.0 != app);
            v.push((app, rect, crate::time::ticks()));
        }
    }
    r
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_status_set(mem: &mut [u8], ctx: &mut HostState, key_ptr: i32, key_len: i32,
    icon: i32, value: i32, flags: i32, text_ptr: i32, text_len: i32, opens_ptr: i32, opens_len: i32) -> i32
{
    // A file system instance reads crafted input; it gets no say on screen.
    if crate::ext::is_instance(&ctx.cap_id) { return -1; }
    let Some(key) = read_str(mem, key_ptr, key_len) else { return -1 };
    if key.is_empty() || key.len() > crate::status::MAX_KEY { return -1; }
    let text = if text_len > 0 { read_str(mem, text_ptr, text_len) } else { Some(String::new()) };
    let Some(text) = text.filter(|t| t.len() <= crate::status::MAX_TEXT) else { return -1 };
    let opens = if opens_len > 0 { read_str(mem, opens_ptr, opens_len) } else { Some(String::new()) };
    let Some(opens) = opens.filter(|o| o.len() <= 32 && !o.contains('/')) else { return -1 };
    let Some(waker) = crate::smp::fiber::current_waker() else { return -1 };
    if !(0..=u16::MAX as i32).contains(&icon) { return -1; }
    let ok = crate::status::set(&ctx.module_name, waker, &key, icon as u16, value,
        (flags & 0xFF) as u8, ((flags >> 8) & 0xFF) as u8, &text, &opens);
    if ok { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_status_clear(mem: &mut [u8], ctx: &mut HostState, key_ptr: i32, key_len: i32) -> i32 {
    let Some(key) = read_str(mem, key_ptr, key_len) else { return -1 };
    crate::status::clear(&ctx.module_name, &key);
    0
}

pub(crate) fn npk_status_list(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::RENDER, "npk_status_list") {
        return -1;
    }
    write_whole(mem, buf_ptr, max, &crate::status::serialize())
}

/// The journal's reader is `peek` by its kernel-assigned name and AUDIT:
/// the journal names every file and host of every app, and AUDIT alone is
/// a right a second module could declare.
fn is_journal_reader(ctx: &HostState) -> bool {
    ctx.module_name == "peek"
        && capability::require(&ctx.cap_id, capability::Rights::AUDIT, "npk_audit_read")
}

/// One field of a journal line: no tab, no line break.
fn audit_field(out: &mut String, s: &str) {
    out.push('\t');
    out.extend(s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }));
}

/// Lines, tab separated, times as local seconds and uptime ms:
/// `S checks_passed entries now_ms now_local`, then per live holder
/// `H inst name checks held used`, then per entry touched since `since_ms`
/// `E seq local up_ms last_up_ms inst holder kind rights count checks denied object`.
pub(crate) fn npk_audit_read(mem: &mut [u8], ctx: &mut HostState, since_ms: i64, buf_ptr: i32, max: i32) -> i32 {
    use core::fmt::Write;
    if !is_journal_reader(ctx) { return -1; }
    let local = |w: u64| if w == 0 { 0 } else { crate::time::wall::local(w).max(0) as u64 };
    let now_ms = crate::time::uptime_us() / 1000;
    let mut out = String::new();
    let _ = write!(out, "S\t{}\t{}\t{}\t{}\n", crate::audit::checks_passed(), crate::audit::total_count(),
        now_ms, local(crate::time::wall::now_or_zero()));
    for (inst, name, cap) in crate::audit::holders() {
        let _ = write!(out, "H\t{}", inst);
        audit_field(&mut out, &name);
        let (held, used, checks) = capability::usage_of(&cap)
            .unwrap_or((capability::Rights::empty(), capability::Rights::empty(), 0));
        let _ = write!(out, "\t{}", checks);
        audit_field(&mut out, &capability::rights_names(held));
        audit_field(&mut out, &capability::rights_names(used));
        out.push('\n');
    }
    for e in crate::audit::touched_since(since_ms.max(0) as u64) {
        let _ = write!(out, "E\t{}\t{}\t{}\t{}\t{}", e.seq, local(e.wall), e.up_ms, e.last_up_ms, e.inst);
        audit_field(&mut out, &e.holder);
        audit_field(&mut out, e.kind.label());
        audit_field(&mut out, &capability::rights_names(e.rights));
        let _ = write!(out, "\t{}\t{}\t{}", e.count, e.checks, e.denied);
        audit_field(&mut out, &e.object);
        out.push('\n');
    }
    write_whole(mem, buf_ptr, max, out.as_bytes())
}

pub(crate) fn npk_launch(mem: &mut [u8], ctx: &mut HostState, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::EXECUTE, "npk_launch") {
        return -1;
    }
    let app = match read_str(mem, app_ptr, app_len) {
        Some(s) => s,
        None => return -1,
    };
    if app.is_empty() || app.contains('/') || app.contains("..") { return -1; }
    let arg = if arg_len > 0 { read_str(mem, arg_ptr, arg_len) } else { None };
    let path = alloc::format!("sys/wasm/{}", app);
    let bytes = match crate::npkfs::fetch(&path) {
        Ok((b, _)) => b,
        Err(_) => return -1,
    };
    let rights = capability::widget_rights_from_wasm(&bytes);
    let module_cap = match capability::create_module_cap(&app, rights, None) {
        Ok(id) => id,
        Err(_) => return -1,
    };
    if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, 0, arg) { 0 } else { -1 }
}

pub(crate) fn npk_pick(mem: &mut [u8], ctx: &mut HostState, mode: i32, start_ptr: i32, start_len: i32, suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::RENDER, "npk_pick") {
        return -1;
    }
    let pick_mode = match mode {
        0 => crate::shade::widgets::PickMode::Open,
        1 => crate::shade::widgets::PickMode::Save,
        2 => crate::shade::widgets::PickMode::Folder,
        _ => return -1,
    };

    // Only a windowed app can receive the reply event.
    let requester = ctx.widget_window_id;
    if requester == 0 { return -1; }
    if crate::shade::widgets::has_open_pick(requester) { return -2; }

    let start = if start_len > 0 {
        read_str(mem, start_ptr, start_len).unwrap_or_default()
    } else {
        String::new()
    };
    let suggest = if suggest_len > 0 {
        read_str(mem, suggest_ptr, suggest_len).unwrap_or_default()
    } else {
        String::new()
    };
    // A start dir is a hint, not authority — the picker re-resolves
    // it and the user can navigate anywhere regardless.
    let start = if start.trim().is_empty() || start.contains("..") {
        crate::intent::home_dir()
    } else {
        start
    };
    // The suggestion is a bare filename; a path here would let a
    // caller pre-aim the save at a directory the user never saw.
    let suggest = if suggest.contains('/') || suggest.contains("..") {
        String::new()
    } else {
        suggest
    };

    let module = picker_module_name();
    let path = alloc::format!("sys/wasm/{}", module);
    let bytes = match crate::npkfs::fetch(&path) {
        Ok((b, _)) => b,
        Err(_) => {
            kprintln!("[npk] npk_pick: picker module `{}` not installed", module);
            return -1;
        }
    };
    let rights = capability::widget_rights_from_wasm(&bytes);
    let module_cap = match capability::create_module_cap(&module, rights, None) {
        Ok(id) => id,
        Err(_) => return -1,
    };

    // Wire the request as the launch argument:
    //   "<open|save|folder>\0<start-dir>\0<suggested-name>"
    let verb = match pick_mode {
        crate::shade::widgets::PickMode::Open => "open",
        crate::shade::widgets::PickMode::Save => "save",
        crate::shade::widgets::PickMode::Folder => "folder",
        crate::shade::widgets::PickMode::Screen => return -1,
    };
    let arg = alloc::format!("{}\0{}\0{}", verb, start, suggest);

    // Floating + centred, like the launcher — a dialog must not
    // re-tile the workspace behind it.
    let win = match crate::shade::with_compositor(|c| {
        let id = c.create_widget_window(&module);
        c.set_overlay(id, PICKER_W, PICKER_H);
        id
    }) {
        Some(id) => id,
        None => { capability::release_module_cap(&module_cap); return -1; }
    };
    crate::shade::widgets::register_pick(win.0, requester, tag as u32, cap_id, pick_mode);
    crate::shade::focus_window(win);
    crate::shade::request_render();

    if spawn_on_worker_inner(bytes, module_cap, 255, &module, false, win.0, Some(arg)) {
        0
    } else {
        // Undo the half-open session, else the requester can never
        // ask again (has_open_pick would keep saying "one is up").
        if let Some(s) = crate::shade::widgets::take_pick(win.0) {
            crate::shade::widgets::finish_pick(s, String::new());
        }
        -1
    }
}

/// The screen picker's overlay: one question and two buttons.
const SCREEN_PICKER_W: u32 = 480;
const SCREEN_PICKER_H: u32 = 260;

#[cfg(target_arch = "x86_64")]
/// The screen picker for a guest that asks to share the screen
/// (`shade::capture`), over the guest's window `requester`. Its window, or
/// None when it could not be started.
pub(crate) fn open_screen_pick(requester: u32, app: &str) -> Option<u32> {
    use crate::shade::widgets::{self, PickMode};
    if widgets::has_open_pick(requester) {
        return None;
    }
    let module = picker_module_name();
    let bytes = match crate::npkfs::fetch(&alloc::format!("sys/wasm/{}", module)) {
        Ok((b, _)) => b,
        Err(_) => {
            kprintln!("[npk] screen picker: module `{}` not installed", module);
            return None;
        }
    };
    let rights = capability::widget_rights_from_wasm(&bytes);
    let module_cap = capability::create_module_cap(&module, rights, None).ok()?;
    // "screen\0<app>\0": the app is shown, it grants nothing.
    let app: String = app.chars().filter(|c| *c != '\0').take(64).collect();
    let arg = alloc::format!("screen\0{}\0", app);
    let win = match crate::shade::with_compositor(|c| {
        let id = c.create_widget_window(&module);
        c.set_overlay(id, SCREEN_PICKER_W, SCREEN_PICKER_H);
        id
    }) {
        Some(id) => id,
        None => {
            capability::release_module_cap(&module_cap);
            return None;
        }
    };
    widgets::register_pick(win.0, requester, 0, [0; 32], PickMode::Screen);
    crate::shade::focus_window(win);
    crate::shade::request_render();
    if spawn_on_worker_inner(bytes, module_cap, 255, &module, false, win.0, Some(arg)) {
        Some(win.0)
    } else {
        widgets::take_pick(win.0);
        crate::shade::close_window(win);
        None
    }
}

pub(crate) fn npk_pick_result(mem: &mut [u8], ctx: &mut HostState, path_ptr: i32, path_len: i32) -> i32 {
    let me = ctx.widget_window_id;
    if me == 0 { return -1; }
    let session = match crate::shade::widgets::take_pick(me) {
        Some(s) => s,
        None => return -1,
    };
    let path = if path_len > 0 {
        read_str(mem, path_ptr, path_len).unwrap_or_default()
    } else {
        String::new()
    };
    crate::shade::widgets::finish_pick(session, path);
    // Hand focus back to the app that asked, so the user carries on
    // where they left off instead of on a closing dialog.
    crate::shade::focus_window(crate::shade::window::WindowId(session.requester));
    crate::shade::request_render();
    0
}

pub(crate) fn npk_pick_mkdir(mem: &mut [u8], ctx: &mut HostState, path_ptr: i32, path_len: i32) -> i32 {
    let me = ctx.widget_window_id;
    if me == 0 || !crate::shade::widgets::is_open_pick(me) { return -1; }
    let path = match read_str(mem, path_ptr, path_len) {
        Some(s) => s,
        None => return -1,
    };
    let clean = path.trim().trim_matches('/');
    if clean.is_empty() || clean.contains("..") { return -1; }
    if is_trust_critical_path(clean) || clean == "sys" || clean.starts_with("sys/") {
        kprintln!("[npk] npk_pick_mkdir DENIED (sys is off limits)");
        return -1;
    }
    if crate::ext::is_ext(clean) {
        if !crate::ext::writable(clean) { return -1; }
        return if crate::ext::mkdir(clean).is_ok() { 0 } else { -1 };
    }
    match crate::npkfs::fs::mkdir(clean) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_delete(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::WRITE, "npk_fs_delete") {
        return -1;
    }
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    if crate::ext::is_ext(&name) {
        if write_allowed(ctx, &name, "npk_fs_delete").is_none() { return -1; }
        return if crate::ext::delete(&name).is_ok() { 0 } else { -1 };
    }
    // Apps may not delete modules or trust anchors — see
    // is_trust_critical_path.
    if is_trust_critical_path(&name) {
        kprintln!("[npk] WASM: npk_fs_delete DENIED ({} is read-only to apps)", name);
        journal_refusal(ctx, capability::Rights::WRITE, &name, "npk_fs_delete", "module or trust store");
        return -1;
    }
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_delete DENIED ({} belongs to another module)", name);
        journal_refusal(ctx, capability::Rights::WRITE, &name, "npk_fs_delete", "another app's private area");
        return -1;
    }
    journal_file(ctx, &name, "npk_fs_delete");
    match crate::npkfs::delete(&name) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Create directory `name`. The parent must exist; an existing entry of
/// that name is refused. Same rights and areas as `npk_fs_delete`.
pub(crate) fn npk_fs_mkdir(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::WRITE, "npk_fs_mkdir") {
        return -1;
    }
    let Some(name) = read_str(mem, name_ptr, name_len) else { return -1 };
    if crate::ext::is_ext(&name) {
        if write_allowed(ctx, &name, "npk_fs_mkdir").is_none() { return -1; }
        return if crate::ext::mkdir(&name).is_ok() { 0 } else { -1 };
    }
    if is_trust_critical_path(&name) {
        kprintln!("[npk] WASM: npk_fs_mkdir DENIED ({} is read-only to apps)", name);
        journal_refusal(ctx, capability::Rights::WRITE, &name, "npk_fs_mkdir", "module or trust store");
        return -1;
    }
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_mkdir DENIED ({} belongs to another module)", name);
        journal_refusal(ctx, capability::Rights::WRITE, &name, "npk_fs_mkdir", "another app's private area");
        return -1;
    }
    journal_file(ctx, &name, "npk_fs_mkdir");
    match crate::storage::npkfs::fs::mkdir(&name) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_rename(mem: &mut [u8], ctx: &mut HostState, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::WRITE, "npk_fs_rename") {
        return -1;
    }
    let old = match read_str(mem, old_ptr, old_len) {
        Some(s) => s,
        None => return -1,
    };
    let new = match read_str(mem, new_ptr, new_len) {
        Some(s) => s,
        None => return -1,
    };
    // Within one external volume only; across volumes the bytes move.
    if crate::ext::is_ext(&old) || crate::ext::is_ext(&new) {
        if write_allowed(ctx, &old, "npk_fs_rename").is_none()
            || write_allowed(ctx, &new, "npk_fs_rename").is_none()
        {
            return -1;
        }
        journal_move(ctx, crate::audit::Kind::Rename, &old, &new);
        return if crate::ext::rename(&old, &new, false).is_ok() { 0 } else { -1 };
    }
    // Neither source nor destination may be module or trust store —
    // renaming in would plant an unverified module or anchor.
    if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
        kprintln!("[npk] WASM: npk_fs_rename DENIED (module/trust store is read-only to apps)");
        journal_refusal(ctx, capability::Rights::WRITE, &old, "npk_fs_rename", "module or trust store");
        return -1;
    }
    // Both sides: checking only the source would let a file be moved into
    // another module's private area, checking only the target would let
    // one be moved out.
    if !crate::wasm::private_area_allows(&old, &ctx.module_name)
        || !crate::wasm::private_area_allows(&new, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_rename DENIED (private area of another module)");
        journal_refusal(ctx, capability::Rights::WRITE, &old, "npk_fs_rename", "another app's private area");
        return -1;
    }
    journal_move(ctx, crate::audit::Kind::Rename, &old, &new);
    match crate::npkfs::rename(&old, &new) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_copy(mem: &mut [u8], ctx: &mut HostState, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if !capability::require(&cap_id, capability::Rights::WRITE, "npk_fs_copy") {
        return -1;
    }
    let old = match read_str(mem, old_ptr, old_len) {
        Some(s) => s,
        None => return -1,
    };
    let new = match read_str(mem, new_ptr, new_len) {
        Some(s) => s,
        None => return -1,
    };
    // An external file is bytes, not a store object: the caller copies it.
    if crate::ext::is_ext(&old) || crate::ext::is_ext(&new) { return -1; }
    if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
        kprintln!("[npk] WASM: npk_fs_copy DENIED (module/trust store is read-only to apps)");
        journal_refusal(ctx, capability::Rights::WRITE, &new, "npk_fs_copy", "module or trust store");
        return -1;
    }
    // Both sides, as for rename: copying out of another module's private
    // area is the same as reading it.
    if !crate::wasm::private_area_allows(&old, &ctx.module_name)
        || !crate::wasm::private_area_allows(&new, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_copy DENIED (private area of another module)");
        journal_refusal(ctx, capability::Rights::WRITE, &old, "npk_fs_copy", "another app's private area");
        return -1;
    }
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::Read, &old);
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::Write, &new);
    match crate::npkfs::copy(&old, &new) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_send_cmd(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_wifi_manager(ctx) {
        return -1;
    }
    match read_bytes(mem, buf_ptr, len) {
        Some(msg) if crate::wifi::send_cmd(&msg) => 0,
        _ => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_poll_event(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_wifi_manager(ctx) {
        return -1;
    }
    // The manager's own fiber is calling: remember its core so the
    // microvm keeps vCPUs off it (see wifi::note_manager_core).
    crate::wifi::note_manager_core();
    wifi_poll_into(mem, buf_ptr, max, crate::wifi::poll_event)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_poll_cmd(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    wifi_poll_into(mem, buf_ptr, max, crate::wifi::poll_cmd)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_send_event(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(msg) if crate::wifi::send_event(&msg) => 0,
        _ => -1,
    }
}

// ── WLAN manager ──
//
// The manager is `wifid` by its kernel-assigned name AND NETCTL: the known
// networks carry their passwords, and NETCTL alone is a capability a second
// module could declare. The deck asks and reads state, nothing more.

fn is_wifi_manager(ctx: &HostState) -> bool {
    ctx.module_name == "wifid"
        && capability::holds(&ctx.cap_id, capability::Rights::NETCTL)
}

/// The network tool the panel opens: it may read the
/// manager's state and ask it to scan, connect or forget.
fn is_wifi_ui(ctx: &HostState) -> bool {
    ctx.module_name == "link"
}

/// npk_net_info(buf, max) -> len / -1 / `-(needed + 2)`: the interfaces and the IPv4
/// configuration as `key value` lines (RENDER, like the status list):
/// `iface <name> <driver> <mac> <up 0|1> <primary 0|1>`, `ipv4 a.b.c.d/p`,
/// `gateway a.b.c.d`, `dns a.b.c.d`.
pub(crate) fn npk_net_info(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    use core::fmt::Write;
    if !capability::require(&ctx.cap_id, capability::Rights::RENDER, "npk_net_info") {
        return -1;
    }
    let mut t = String::new();
    for i in crate::netdev::list() {
        let m = i.mac;
        let _ = writeln!(t, "iface {} {} {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} {} {}",
            i.name, i.driver.replace(' ', "_"), m[0], m[1], m[2], m[3], m[4], m[5],
            i.link_up as u8, i.primary as u8);
    }
    let ip = crate::net::arp::our_ip();
    if ip != [0, 0, 0, 0] {
        let gw = crate::net::ipv4::gateway();
        let dns = crate::net::dns::server();
        let _ = writeln!(t, "ipv4 {}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], crate::net::ipv4::prefix_len());
        let _ = writeln!(t, "gateway {}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3]);
        let _ = writeln!(t, "dns {}.{}.{}.{}", dns[0], dns[1], dns[2], dns[3]);
    }
    write_whole(mem, buf_ptr, max, t.as_bytes())
}

#[cfg(target_arch = "x86_64")]
/// npk_wifi_known(buf, max) -> len / -1 / `-(needed + 2)`: every known
/// network with its record.
pub(crate) fn npk_wifi_known(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_wifi_manager(ctx) { return -1; }
    write_whole(mem, buf_ptr, max, &crate::drivers::wifi::known_wire())
}

#[cfg(target_arch = "x86_64")]
/// npk_wifi_new_network() -> 0 / -1: the link just came up on a different
/// network than before; drop the lease and ask for a new one.
pub(crate) fn npk_wifi_new_network(ctx: &mut HostState) -> i32 {
    if !is_wifi_manager(ctx) { return -1; }
    crate::net::network_changed();
    0
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_poll_request(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_wifi_manager(ctx) { return -1; }
    wifi_poll_into(mem, buf_ptr, max, crate::drivers::wifi::poll_request)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_wifi_publish(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_wifi_manager(ctx) { return -1; }
    if len < 0 || len as usize > crate::drivers::wifi::STATUS_MAX { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(b) => { crate::drivers::wifi::publish_status(&b); 0 }
        None => -1,
    }
}

pub(crate) fn npk_wifi_status(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_wifi_ui(ctx) { return -1; }
    write_whole(mem, buf_ptr, max, &crate::drivers::wifi::status())
}

/// npk_wifi_request(buf, len) -> 0 / -1. `[op]` or `[op][ssid_len][ssid]`;
/// CONNECT may append `[psk_len][psk]`, which the kernel stores in the vault;
/// SET_AUTO appends `[on]`.
pub(crate) fn npk_wifi_request(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    use crate::drivers::wifi as w;
    if !is_wifi_ui(ctx) { return -1; }
    let Some(m) = read_bytes(mem, buf_ptr, len) else { return -1 };
    let ssid = || -> Option<&[u8]> {
        let n = *m.get(1)? as usize;
        m.get(2..2 + n)
    };
    let ok = match m.first().copied() {
        Some(w::REQ_SCAN) => w::request_scan(),
        Some(w::REQ_DISCONNECT) => w::request_disconnect(),
        Some(w::REQ_CONNECT) => {
            let Some(s) = ssid() else { return -1 };
            let at = 2 + s.len();
            let psk = m.get(at).and_then(|&n| m.get(at + 1..at + 1 + n as usize));
            w::request_connect(s, psk).is_ok()
        }
        Some(w::REQ_FORGET) => ssid().is_some_and(|s| w::forget(s).is_ok()),
        Some(w::REQ_SET_AUTO) => match ssid() {
            Some(s) => w::set_auto(s, m.get(2 + s.len()).is_some_and(|&b| b != 0)).is_ok(),
            None => false,
        },
        _ => false,
    };
    if ok { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_driver_report(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
    if len <= 0 || len as usize > crate::drivers::report::REPORT_MAX { return -1; }
    match read_str(mem, buf_ptr, len) {
        Some(s) => {
            let name = ctx.module_name.clone();
            crate::drivers::report::store(&name, &s);
            0
        }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_submit_rx(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::netdev::wasm_nic_submit_rx(&frame); 0 }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_rx_deliver(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::net::wasm_deliver_rx(&frame); 0 }
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_netdev_poll_tx(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    // Checked before the dequeue: a frame taken out cannot be put back.
    if max < 0 || (max as usize) < crate::netdev::MTU { return -1; }
    let mut frame = [0u8; crate::netdev::MTU];
    let len = match crate::netdev::wasm_nic_poll_tx(&mut frame) {
        Some(n) => n,
        None => return -1,
    };
    if max < 0 || (max as usize) < len { return -1; }
    write_bytes(mem, buf_ptr, &frame[..len])
}

/// The display controller this module drives, if it holds one with
/// HARDWARE: what every `npk_gpudev_*` call requires.
fn gpudev_device(ctx: &HostState) -> Option<crate::drivers::DevId> {
    if !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "gpudev_device") {
        return None;
    }
    let hw = ctx.hw.as_ref()?;
    if !hw.is_display() {
        return None;
    }
    hw.dev
}

/// `npk_gpudev_register(version) -> 0 | <0`.
pub(crate) fn npk_gpudev_register(ctx: &mut HostState, version: i32) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::register(dev, version),
        None => -1,
    }
}

/// `npk_gpudev_framebuffer(width, height) -> 0 | -1`: a framebuffer of the
/// module's mode, `gpudev::set_framebuffer`.
#[cfg(target_arch = "aarch64")]
pub(crate) fn npk_gpudev_framebuffer(ctx: &mut HostState, width: i32, height: i32) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::set_framebuffer(dev, width, height),
        None => -1,
    }
}

/// `npk_gpudev_bo_info(id, out) -> 0 | -1`: `{phys, size: u64, pitch,
/// width, height, format, offset, pages: u32}`, 40 bytes.
pub(crate) fn npk_gpudev_bo_info(mem: &mut [u8], ctx: &mut HostState, id: i32, out_ptr: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    let Some(info) = crate::gpu::gpudev::bo_info(dev, id) else { return -1 };
    let Some(out) = guest_mut(mem, out_ptr, info.len()) else { return -1 };
    out.copy_from_slice(&info);
    0
}

/// `npk_gpudev_bo_pages(id, first, out_ptr, max) -> pages | -1`: physical
/// addresses, 8 bytes each.
pub(crate) fn npk_gpudev_bo_pages(mem: &mut [u8], ctx: &mut HostState, id: i32, first: i32, out_ptr: i32,
                                  max: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if first < 0 || max <= 0 {
        return if max == 0 { 0 } else { -1 };
    }
    let Some(out) = guest_mut(mem, out_ptr, (max as usize).saturating_mul(8)) else { return -1 };
    crate::gpu::gpudev::bo_pages(dev, id, first as u32, out)
}

/// `npk_gpudev_job_data(seq, offset, out_ptr, len) -> bytes | -1`.
pub(crate) fn npk_gpudev_job_data(mem: &mut [u8], ctx: &mut HostState, seq: i32, offset: i32, out_ptr: i32,
                                  len: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if offset < 0 || len < 0 {
        return -1;
    }
    let Some(out) = guest_mut(mem, out_ptr, len as usize) else { return -1 };
    crate::gpu::gpudev::job_data(dev, seq as u32, offset as u32, out)
}

/// `npk_gpudev_bo_bound(id, gpu_addr) -> 0 | -1`.
pub(crate) fn npk_gpudev_bo_bound(ctx: &mut HostState, id: i32, gpu_addr: i64) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::bo_bound(dev, id, gpu_addr),
        None => -1,
    }
}

/// `npk_gpudev_bo_gpu_only(id) -> 0 | -1`.
pub(crate) fn npk_gpudev_bo_gpu_only(ctx: &mut HostState, id: i32) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::bo_gpu_only(dev, id),
        None => -1,
    }
}

/// `npk_gpudev_ready(caps) -> 0 | -1`.
pub(crate) fn npk_gpudev_ready(ctx: &mut HostState, caps: i32) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::ready(dev, caps),
        None => -1,
    }
}

/// `npk_gpudev_poll(buf_ptr, max) -> jobs | -1`, 32 bytes each.
pub(crate) fn npk_gpudev_poll(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if max <= 0 {
        return 0;
    }
    let Some(out) = guest_mut(mem, buf_ptr, max as usize) else { return -1 };
    crate::gpu::gpudev::poll(dev, out)
}

/// `npk_gpudev_complete(seq, status) -> 0 | -1`.
pub(crate) fn npk_gpudev_complete(ctx: &mut HostState, seq: i32, status: i32) -> i32 {
    match gpudev_device(ctx) {
        Some(dev) => crate::gpu::gpudev::complete(dev, seq as u32, status),
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_reply(seq, ptr, len) -> 0 | -1`: the reply of `OP_CCMD` job
/// `seq`, at most `gpudev::MAX_REPLY` bytes, before `npk_gpudev_complete`.
pub(crate) fn npk_gpudev_reply(mem: &mut [u8], ctx: &mut HostState, seq: i32, ptr: i32, len: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if len < 0 || len as usize > crate::gpu::gpudev::MAX_REPLY {
        return -1;
    }
    let Some(data) = guest_mut(mem, ptr, len as usize) else { return -1 };
    crate::gpu::gpudev::reply(dev, seq as u32, data)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_tt_alloc(client, pages) -> tt | -1`: GTT pages for a guest
/// client's object.
pub(crate) fn npk_gpudev_tt_alloc(ctx: &mut HostState, client: i32, pages: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if pages <= 0 {
        return -1;
    }
    crate::gpu::gpudev::tt_alloc(dev, client as u32, pages as u32)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_tt_buffer(tt) -> id | -1`: the buffer GTT pages `tt` are
/// read under with `npk_gpudev_bo_pages`.
pub(crate) fn npk_gpudev_tt_buffer(ctx: &mut HostState, tt: i32) -> i32 {
    if gpudev_device(ctx).is_none() || tt <= 0 {
        return -1;
    }
    crate::gpu::gpudev::tt_buffer(tt as u32).map_or(-1, |id| id as i32)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_tt_read(tt, offset, out_ptr, len) -> bytes | -1`: what the
/// module's GTT pages `tt` hold, for a command stream checked in a copy.
pub(crate) fn npk_gpudev_tt_read(mem: &mut [u8], ctx: &mut HostState, tt: i32, offset: i64, out_ptr: i32,
                                 len: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if tt <= 0 || offset < 0 || len < 0 {
        return -1;
    }
    let Some(out) = guest_mut(mem, out_ptr, len as usize) else { return -1 };
    crate::gpu::gpudev::tt_read(dev, tt as u32, offset as u64, out)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_tt_free(tt) -> 0 | -1`.
pub(crate) fn npk_gpudev_tt_free(ctx: &mut HostState, tt: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if tt <= 0 {
        return -1;
    }
    crate::gpu::gpudev::tt_free(dev, tt as u32)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_capset(ptr, len) -> 0 | -1`: the card's DRM capset, for
/// native contexts, before `npk_gpudev_ready`.
pub(crate) fn npk_gpudev_capset(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if len < 0 || len as usize > crate::gpu::gpudev::MAX_CAPSET {
        return -1;
    }
    let Some(data) = guest_mut(mem, ptr, len as usize) else { return -1 };
    crate::gpu::gpudev::set_capset(dev, data)
}

/// `npk_gpudev_released(id) -> 0 | -1`: the module let go of a GEM object
/// it kept past its detach.
pub(crate) fn npk_gpudev_released(ctx: &mut HostState, id: i32) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if id < 0 {
        return -1;
    }
    crate::gpu::gpudev::released(dev, id as u32)
}

/// `npk_gpudev_stats(ptr, len) -> 0 | -1`: the module's cumulative GPU
/// counters (`gpudev::GPU_STATS_WORDS` little-endian u64s), for `cores`.
pub(crate) fn npk_gpudev_stats(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    use crate::gpu::gpudev::GPU_STATS_WORDS;
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if len != (GPU_STATS_WORDS * 8) as i32 {
        return -1;
    }
    let Some(data) = guest_mut(mem, ptr, len as usize) else { return -1 };
    let words: [u64; GPU_STATS_WORDS] =
        core::array::from_fn(|i| u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap()));
    crate::gpu::gpudev::stats(dev, words)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_vram(base, size) -> 0 | -1`: the window through which the
/// CPU reaches all of the card's VRAM, before `npk_gpudev_ready`.
pub(crate) fn npk_gpudev_vram(ctx: &mut HostState, base: i64, size: i64) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if base <= 0 || size <= 0 {
        return -1;
    }
    crate::gpu::gpudev::set_vram_window(dev, base as u64, size as u64)
}

#[cfg(target_arch = "x86_64")]
/// `npk_gpudev_ring(client, ring, submitted, completed) -> 0 | -1`.
pub(crate) fn npk_gpudev_ring(ctx: &mut HostState, client: i32, ring: i32, submitted: i64, completed: i64) -> i32 {
    let Some(dev) = gpudev_device(ctx) else { return -1 };
    if ring < 0 || submitted < 0 || completed < 0 {
        return -1;
    }
    crate::gpu::gpudev::ring(dev, client as u32, ring as u32, submitted as u64, completed as u64)
}

// === External volumes: the block handle and the request channel of an
// `extfs` instance (`storage::ext`) ===

#[cfg(target_arch = "x86_64")]
fn blk_code(e: crate::ext::BlkError) -> i32 {
    match e {
        crate::ext::BlkError::Range => -1,
        crate::ext::BlkError::Gone => -2,
        crate::ext::BlkError::Io => -3,
        crate::ext::BlkError::ReadOnly => -4,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_blk_info(mem: &mut [u8], ctx: &mut HostState, out_ptr: i32) -> i32 {
    let Some((role, bs, blocks, ro)) = crate::ext::blk_info(&ctx.cap_id) else { return -1 };
    let mut b = [0u8; 20];
    b[0..4].copy_from_slice(&role.to_le_bytes());
    b[4..8].copy_from_slice(&bs.to_le_bytes());
    b[8..16].copy_from_slice(&blocks.to_le_bytes());
    b[16..20].copy_from_slice(&(ro as u32).to_le_bytes());
    write_bytes(mem, out_ptr, &b)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_blk_read(mem: &mut [u8], ctx: &mut HostState, lba: i64, len: i32, buf_ptr: i32) -> i32 {
    let (Ok(lba), Some(n)) = (u64::try_from(lba), glen(len)) else { return -1 };
    if n > crate::ext::MAX_IO { return -1; }
    let Some(buf) = guest_mut(mem, buf_ptr, n) else { return -1 };
    match crate::ext::blk_read(&ctx.cap_id, lba, buf) {
        Ok(()) => 0,
        Err(e) => blk_code(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_blk_write(mem: &mut [u8], ctx: &mut HostState, lba: i64, len: i32, buf_ptr: i32) -> i32 {
    let (Ok(lba), Some(n)) = (u64::try_from(lba), glen(len)) else { return -1 };
    if n > crate::ext::MAX_IO { return -1; }
    let Some(buf) = guest(mem, buf_ptr, n) else { return -1 };
    match crate::ext::blk_write(&ctx.cap_id, lba, buf) {
        Ok(()) => 0,
        Err(e) => blk_code(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_blk_flush(ctx: &mut HostState) -> i32 {
    match crate::ext::blk_flush(&ctx.cap_id) {
        Ok(()) => 0,
        Err(e) => blk_code(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ext_parts(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let Some(n) = glen(len) else { return -1 };
    if n % 16 != 0 || n > 16 * 16 { return -1; }
    let Some(b) = guest(mem, ptr, n) else { return -1 };
    let parts: alloc::vec::Vec<(u64, u64)> = b.chunks_exact(16).map(|c| (
        u64::from_le_bytes(c[0..8].try_into().unwrap_or([0; 8])),
        u64::from_le_bytes(c[8..16].try_into().unwrap_or([0; 8])),
    )).collect();
    match crate::ext::report_parts(&ctx.cap_id, &parts) {
        Some(k) => k as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ext_mount(mem: &mut [u8], ctx: &mut HostState, label_ptr: i32, label_len: i32, total: i64, free: i64) -> i32 {
    let label = if label_len == 0 { String::new() } else {
        match read_str(mem, label_ptr, label_len) { Some(s) => s, None => return -1 }
    };
    let (Ok(total), Ok(free)) = (u64::try_from(total), u64::try_from(free)) else { return -1 };
    match crate::ext::mount(&ctx.cap_id, &label, total, free) {
        Some(_) => 0,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ext_next(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32, timeout_ms: i32) -> i32 {
    let (Some(max), Ok(t)) = (glen(buf_max), u64::try_from(timeout_ms)) else { return -1 };
    if !crate::ext::is_instance(&ctx.cap_id) { return -1; }
    if guest(mem, buf_ptr, max).is_none() { return -1; }
    match crate::ext::next(&ctx.cap_id, t.min(10_000), max) {
        crate::ext::Next::Req(b) => write_bytes(mem, buf_ptr, &b),
        crate::ext::Next::Idle => 0,
        crate::ext::Next::Gone => -2,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_ext_reply(mem: &mut [u8], ctx: &mut HostState, seq: i32, status: i32, ptr: i32, len: i32) -> i32 {
    let Some(n) = glen(len) else { return -1 };
    let Some(b) = guest(mem, ptr, n) else { return -1 };
    if crate::ext::reply(&ctx.cap_id, seq as u32, status, b) { 0 } else { -1 }
}

pub(crate) fn npk_ext_volumes(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    if !ext_allowed(ctx, crate::ext::ROOT, capability::Rights::READ) { return -1; }
    let mut out = alloc::vec::Vec::new();
    for v in crate::ext::volumes() {
        out.extend_from_slice(&(v.label.len() as u16).to_le_bytes());
        out.push(v.read_only as u8);
        out.extend_from_slice(&v.total.to_le_bytes());
        out.extend_from_slice(&v.free.to_le_bytes());
        out.extend_from_slice(v.label.as_bytes());
    }
    write_whole(mem, buf_ptr, buf_max, &out)
}

pub(crate) fn npk_ext_eject(mem: &mut [u8], ctx: &mut HostState, label_ptr: i32, label_len: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::EXTERNAL, "npk_ext_eject") { return -1; }
    let Some(label) = read_str(mem, label_ptr, label_len) else { return -1 };
    if crate::ext::eject(&label).is_ok() { 0 } else { -1 }
}

// === USB interfaces for modules (`xhci::usbfn`) ===

#[cfg(target_arch = "x86_64")]
/// The right a USB class needs: video → CAMERA, audio → MIC. Other
/// classes have no module path.
fn usb_class_right(class: u8) -> Option<capability::Rights> {
    match class {
        crate::xhci::usbfn::CLASS_VIDEO => Some(capability::Rights::CAMERA),
        crate::xhci::usbfn::CLASS_AUDIO => Some(capability::Rights::MIC),
        _ => None,
    }
}

#[cfg(target_arch = "x86_64")]
fn usb_err(e: crate::xhci::usbfn::FnError) -> i32 {
    use crate::xhci::usbfn::FnError::*;
    match e {
        Gone => -1,
        NotFound => -2,
        Denied => -3,
        Failed => -4,
        Bandwidth => -5,
        NoMemory => -6,
        BadArgument => -7,
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_usb_claim(class, buf, max)`: claim the interfaces of `class` on the
/// first free device; the device and configuration descriptors go to
/// `buf`. Returns the handle (> 0) or an error (< 0).
pub(crate) fn npk_usb_claim(mem: &mut [u8], ctx: &mut HostState, class: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(right) = usb_class_right(class as u8) else { return -3 };
    if ctx.pid == 0 || !capability::require(&ctx.cap_id, right, "npk_usb_claim") {
        kprintln!("[npk] WASM: npk_usb_claim DENIED (class {:02x})", class as u8);
        return -3;
    }
    if buf_max < 0 { return -7; }
    let Some(out) = guest_mut(mem, buf_ptr, buf_max as usize) else { return -7 };
    match crate::xhci::usbfn::claim(class as u8, ctx.pid) {
        Ok((id, descs)) => {
            let n = descs.len().min(out.len());
            out[..n].copy_from_slice(&descs[..n]);
            id as i32
        }
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_usb_control(handle, setup, data, len)`: `setup` is the 8-byte
/// SETUP packet; `data` holds OUT bytes or receives IN bytes. Returns the
/// bytes moved or an error.
pub(crate) fn npk_usb_control(mem: &mut [u8], ctx: &mut HostState, handle: i32, setup_ptr: i32,
                              data_ptr: i32, data_len: i32) -> i32 {
    let Some(setup) = guest(mem, setup_ptr, 8) else { return -7 };
    let (bm, req) = (setup[0], setup[1]);
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let w_length = u16::from_le_bytes([setup[6], setup[7]]);
    if data_len < w_length as i32 { return -7; }
    let out: Vec<u8> = if bm & 0x80 == 0 {
        match guest(mem, data_ptr, w_length as usize) { Some(b) => b.to_vec(), None => return -7 }
    } else {
        Vec::new()
    };
    match crate::xhci::usbfn::control(handle as u32, ctx.pid, bm, req, value, index, &out, w_length) {
        Ok(v) => {
            if bm & 0x80 != 0 {
                let Some(dst) = guest_mut(mem, data_ptr, v.len()) else { return -7 };
                dst.copy_from_slice(&v);
            }
            w_length as i32
        }
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_usb_set_alt(ctx: &mut HostState, handle: i32, ifnum: i32, alt: i32) -> i32 {
    match crate::xhci::usbfn::set_alt(handle as u32, ctx.pid, ifnum as u8, alt as u8) {
        Ok(()) => 0,
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_usb_stream_open(handle, ep, td_len, count)`: an IN stream on
/// endpoint `ep`; `td_len` 0 = one service interval (isochronous).
pub(crate) fn npk_usb_stream_open(ctx: &mut HostState, handle: i32, ep: i32, td_len: i32, count: i32) -> i32 {
    if td_len < 0 || count <= 0 { return -7; }
    match crate::xhci::usbfn::stream_open(handle as u32, ctx.pid, ep as u8, td_len as usize, count as usize) {
        Ok(sid) => sid as i32,
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
/// `npk_usb_stream_read(handle, stream, buf, max)`: records of `len: u16,
/// status: u8, 0` and `len` bytes. Returns the bytes written.
pub(crate) fn npk_usb_stream_read(mem: &mut [u8], ctx: &mut HostState, handle: i32, sid: i32,
                                  buf_ptr: i32, buf_max: i32) -> i32 {
    if buf_max <= 0 { return -7; }
    if guest_mut(mem, buf_ptr, buf_max as usize).is_none() { return -7; }
    match crate::xhci::usbfn::stream_read(handle as u32, ctx.pid, sid as u32, buf_max as usize) {
        Ok(v) => {
            let Some(dst) = guest_mut(mem, buf_ptr, v.len()) else { return -7 };
            dst.copy_from_slice(&v);
            v.len() as i32
        }
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_usb_stream_close(ctx: &mut HostState, handle: i32, sid: i32) -> i32 {
    match crate::xhci::usbfn::stream_close(handle as u32, ctx.pid, sid as u32) {
        Ok(()) => 0,
        Err(e) => usb_err(e),
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_usb_release(ctx: &mut HostState, handle: i32) -> i32 {
    crate::xhci::usbfn::release(handle as u32, ctx.pid);
    0
}

#[cfg(target_arch = "x86_64")]
/// `npk_device_register`: an audio driver enters one of its devices.
/// Cameras are entered by the kernel only.
pub(crate) fn npk_device_register(mem: &mut [u8], ctx: &mut HostState, kind: i32, rank: i32, id_ptr: i32,
                                  id_len: i32, name_ptr: i32, name_len: i32) -> i32 {
    use crate::devices::{self, Kind, Owner};
    let Some(kind) = Kind::from_abi(kind).filter(|k| *k != Kind::Camera) else { return -1 };
    if ctx.pid == 0 || !capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "npk_device_register") {
        return -1;
    }
    let Ok(rank) = u8::try_from(rank) else { return -1 };
    let (Some(id), Some(name)) = (read_str(mem, id_ptr, id_len), read_str(mem, name_ptr, name_len)) else { return -1 };
    match devices::register(kind, rank, &id, &name, Owner::Module(ctx.pid)) {
        Some(h) => h as i32,
        None => -1,
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_device_remove(ctx: &mut HostState, handle: i32) -> i32 {
    if crate::devices::remove(handle as u32, crate::devices::Owner::Module(ctx.pid)) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
/// Whether the caller's own entry `handle` is the active one of its kind.
pub(crate) fn npk_device_active(ctx: &mut HostState, handle: i32) -> i32 {
    let mine = crate::devices::get(handle as u32).is_some_and(|e| e.owner == crate::devices::Owner::Module(ctx.pid));
    if !mine { return -1; }
    match crate::devices::is_active(handle as u32) {
        Some(a) => a as i32,
        None => -1,
    }
}

/// `npk_device_list(kind, buf, max)`: the shell's device lists.
pub(crate) fn npk_device_list(mem: &mut [u8], ctx: &mut HostState, kind: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::SHELL, "npk_device_list") {
        return -1;
    }
    let Some(kind) = crate::devices::Kind::from_abi(kind) else { return -1 };
    let (entries, active) = crate::devices::list(kind);
    let mut out = alloc::vec::Vec::new();
    for e in &entries {
        out.extend_from_slice(&e.handle.to_le_bytes());
        out.push((active == Some(e.handle)) as u8);
        out.push(e.id.len() as u8);
        out.push(e.name.len() as u8);
        out.push(0);
        out.extend_from_slice(e.id.as_bytes());
        out.extend_from_slice(e.name.as_bytes());
    }
    write_whole(mem, buf_ptr, buf_max, &out)
}

/// `npk_device_select(handle)`: the user picks the active one of its kind.
pub(crate) fn npk_device_select(ctx: &mut HostState, handle: i32) -> i32 {
    if !capability::require(&ctx.cap_id, capability::Rights::SHELL, "npk_device_select") {
        return -1;
    }
    if crate::devices::select(handle as u32) { 0 } else { -1 }
}

fn hardware(ctx: &HostState) -> bool {
    ctx.pid != 0 && capability::require(&ctx.cap_id, capability::Rights::HARDWARE, "audio capture device")
}

fn mic(ctx: &HostState) -> bool {
    ctx.pid != 0 && capability::require(&ctx.cap_id, capability::Rights::MIC, "microphone")
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_device_format(ctx: &mut HostState, rate: i32, encoding: i32, channels: i32) -> i32 {
    if !hardware(ctx) { return -1; }
    let Some(fmt) = audio_format(rate, encoding, channels) else { return -1 };
    if crate::capture::set_device_format(ctx.pid, fmt) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_wanted(ctx: &mut HostState) -> i32 {
    if !hardware(ctx) { return -1; }
    crate::capture::wanted(ctx.pid) as i32
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_push(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    if !hardware(ctx) { return -1; }
    let Some(data) = glen(len).and_then(|n| guest(mem, ptr, n)) else { return -1 };
    crate::capture::push(ctx.pid, data) as i32
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_open(ctx: &mut HostState) -> i32 {
    if !mic(ctx) {
        kprintln!("[npk] WASM: npk_audio_capture_open DENIED");
        return -1;
    }
    let s = crate::capture::open(Some(ctx.pid));
    if s.is_some() { crate::audit::record(&ctx.cap_id, crate::audit::Kind::DeviceOn, "microphone"); }
    s.map_or(-1, |i| i as i32)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_format(ctx: &mut HostState, stream: i32, rate: i32, encoding: i32, channels: i32) -> i32 {
    let (Ok(i), Some(fmt)) = (usize::try_from(stream), audio_format(rate, encoding, channels)) else { return -1 };
    if crate::capture::set_format(i, Some(ctx.pid), fmt) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_read(mem: &mut [u8], ctx: &mut HostState, stream: i32, ptr: i32, max: i32) -> i32 {
    let Ok(i) = usize::try_from(stream) else { return -1 };
    if max < 0 { return -1; }
    let Some(out) = guest_mut(mem, ptr, max as usize) else { return -1 };
    crate::capture::read(i, Some(ctx.pid), out).map_or(-1, |n| n as i32)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_audio_capture_close(ctx: &mut HostState, stream: i32) -> i32 {
    let Ok(i) = usize::try_from(stream) else { return -1 };
    if !crate::capture::close(i, Some(ctx.pid)) { return -1; }
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::DeviceOff, "microphone");
    0
}

fn camera_right(ctx: &HostState) -> bool {
    ctx.pid != 0 && capability::require(&ctx.cap_id, capability::Rights::CAMERA, "camera")
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_camera_wanted(ctx: &mut HostState) -> i64 {
    if !camera_right(ctx) { return -1; }
    let (n, w, h) = crate::camfeed::wanted();
    n.min(0xFFFF) as i64 | (w as i64) << 16 | (h as i64) << 32
}

#[cfg(target_arch = "x86_64")]
/// A frame from the camera's driver; only the module that holds the
/// active camera may, so no other module can show a picture as the camera.
pub(crate) fn npk_camera_push(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32, width: i32, height: i32,
                              format: i32) -> i32 {
    if !camera_right(ctx) || !crate::xhci::usbfn::holds_active_camera(ctx.pid) { return -1; }
    let (Ok(w), Ok(h), Ok(f)) = (u16::try_from(width), u16::try_from(height), u8::try_from(format)) else { return -1 };
    let Some(data) = glen(len).and_then(|n| guest(mem, ptr, n)) else { return -1 };
    if crate::camfeed::push(data, w, h, f) { 0 } else { -1 }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_camera_open(ctx: &mut HostState, width: i32, height: i32) -> i32 {
    if !camera_right(ctx) { return -1; }
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else { return -1 };
    let r = crate::camfeed::open(Some(ctx.pid), w, h);
    if r.is_some() { crate::audit::record(&ctx.cap_id, crate::audit::Kind::DeviceOn, "camera"); }
    r.map_or(-1, |i| i as i32)
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_camera_read(mem: &mut [u8], ctx: &mut HostState, reader: i32, buf_ptr: i32, buf_max: i32,
                              info_ptr: i32) -> i32 {
    let Ok(id) = usize::try_from(reader) else { return -1 };
    let Some(max) = glen(buf_max) else { return -1 };
    let owner = Some(ctx.pid);
    let Some(n) = crate::camfeed::pending(id, owner) else { return -1 };
    if n == 0 { return 0; }
    if n > max { return too_small(n); }
    let Some(out) = guest_mut(mem, buf_ptr, n) else { return -1 };
    let Some((got, info)) = crate::camfeed::read(id, owner, out) else { return -1 };
    let mut rec = [0u8; 16];
    rec[0..2].copy_from_slice(&info.width.to_le_bytes());
    rec[2..4].copy_from_slice(&info.height.to_le_bytes());
    rec[4] = info.format;
    rec[8..16].copy_from_slice(&info.seq.to_le_bytes());
    if write_bytes(mem, info_ptr, &rec) < 0 { return -1; }
    got as i32
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn npk_camera_close(ctx: &mut HostState, reader: i32) -> i32 {
    let Ok(id) = usize::try_from(reader) else { return -1 };
    if !crate::camfeed::close(id, Some(ctx.pid)) { return -1; }
    crate::audit::record(&ctx.cap_id, crate::audit::Kind::DeviceOff, "camera");
    0
}
