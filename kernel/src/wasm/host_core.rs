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
    HostState, HwDriverState, MAX_APP_BUFS, MAX_DMA_ALLOCS, MAX_DMA_PAGES,
    MAX_DMA_PAGES_PER_CALL, MAX_MMIO_MAPS, app_is_focused, bench_sys_info,
    fsck_sys_info, get_debug_target, pop_app_key, append_entry,
    extract_wasm_custom_section, is_trust_critical_path, spawn_on_worker,
    spawn_on_worker_inner, picker_module_name, TERM_IDX_ACTIVE, PICKER_W,
    PICKER_H,
};
use crate::{kprint, kprintln, capability};
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
pub(crate) fn read_str(data: &[u8], ptr: i32, len: i32) -> Option<String> {
    let bytes = guest(data, ptr, glen(len)?)?;
    if bytes.is_empty() { return None; }
    core::str::from_utf8(bytes).ok().map(String::from)
}

/// Bytes from guest memory, or `None` if the range does not fit.
pub(crate) fn read_bytes(data: &[u8], ptr: i32, len: i32) -> Option<alloc::vec::Vec<u8>> {
    if len <= 0 { return None; }
    guest(data, ptr, glen(len)?).map(|b| b.to_vec())
}

/// Writes `bytes` to guest memory at `ptr`; returns the length, or -1 if
/// the range does not fit.
pub(crate) fn write_bytes(data: &mut [u8], ptr: i32, bytes: &[u8]) -> i32 {
    match guest_mut(data, ptr, bytes.len()) {
        Some(dst) => { dst.copy_from_slice(bytes); bytes.len() as i32 }
        None => -1,
    }
}

/// Shared body of both poll functions: copies one message from `dequeue`
/// into the guest buffer (capped by `max`); returns the length or -1.
fn wifi_poll_into(
    mem: &mut [u8],
    buf_ptr: i32,
    max: i32,
    dequeue: fn(&mut [u8]) -> Option<usize>,
) -> i32 {
    if max <= 0 { return -1; }
    let cap = (max as usize).min(crate::wifi::WIFI_MSG_MAX);
    let mut tmp = [0u8; crate::wifi::WIFI_MSG_MAX];
    let len = match dequeue(&mut tmp[..cap]) {
        Some(n) => n,
        None => return -1,
    };
    write_bytes(mem, buf_ptr, &tmp[..len])
}

pub(crate) fn npk_http_status(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return 0;
    }
    ctx.http_status as i32
}

pub(crate) fn npk_fs_usage(ctx: &mut HostState) -> i64 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
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

pub(crate) fn npk_clipboard_len(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    crate::shade::clipboard::text_len() as i32
}

pub(crate) fn npk_window_set_close_guard(ctx: &mut HostState, on: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    crate::shade::widgets::set_close_guard(wid, on != 0);
    0
}

pub(crate) fn npk_screen_size(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    let ok = capability::check_global(&cap_id, capability::Rights::RENDER).is_ok()
        || capability::check_global(&cap_id, capability::Rights::CAPTURE).is_ok();
    if !ok { return 0; }
    let info = crate::framebuffer::get_info();
    (((info.width & 0xFFFF) << 16) | (info.height & 0xFFFF)) as i32
}

pub(crate) fn npk_ticks(_ctx: &mut HostState) -> i64 {
    (crate::interrupts::ticks() as i64).saturating_mul(10)
}

pub(crate) fn npk_now_us(_ctx: &mut HostState) -> i64 {
    let f = crate::interrupts::tsc_freq();
    if f == 0 { return 0; }
    ((crate::interrupts::rdtsc() as u128 * 1_000_000) / f as u128) as i64
}

pub(crate) fn npk_unix_time(_ctx: &mut HostState) -> i64 {
    crate::rtc::read_unix_time().unwrap_or(0) as i64
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

pub(crate) fn npk_theme_token(ctx: &mut HostState, token_id: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return 0;
    }
    // The only token table lives in palette.rs; do not copy it here.
    if token_id < 0 { return 0; }
    let token = match crate::shade::widgets::palette::token_from_id(token_id as usize) {
        Some(t) => t,
        None => return 0,
    };
    crate::shade::widgets::palette::resolve(token) as i32
}

pub(crate) fn npk_cursor_pos(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    if capability::check_global(&cap_id, capability::Rights::CAPTURE).is_err() {
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
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 { return -1; }

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
                // Overlay path wants focus on the new widget (drun
                // style); promotion does not focus, so fix up.
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
        let ok = comp.set_overlay(crate::shade::WindowId(wid), w as u32, h as u32);
        // The overlay path always wants this window focused — drun
        // and any other launcher style app drives keyboard from
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
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    let ok = crate::shade::with_compositor(|comp|
        comp.set_modal(crate::shade::WindowId(wid), modal != 0)
    ).unwrap_or(false);
    if ok { 0 } else { -1 }
}

pub(crate) fn npk_window_set_overlay_at(ctx: &mut HostState, x: i32, y: i32, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 || x < 0 || y < 0 { return -1; }

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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 { return -1; }
    crate::shade::widgets::set_clipboard_sink(wid);
    0
}

pub(crate) fn npk_window_set_dock(ctx: &mut HostState, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 { return -1; }

    let mut wid = ctx.widget_window_id;
    if wid == 0 {
        // Promote the spawning terminal to a widget window if there
        // is one; otherwise create a fresh widget window. Unlike
        // the overlay path we do not focus it — the dock is a
        // background overlay that never owns keyboard focus.
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
                id.0
            }
            None => {
                let title = ctx.module_name.clone();
                match crate::shade::with_compositor(|comp| {
                    comp.create_widget_window(
                        if title.is_empty() { "dock" } else { title.as_str() }).0
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
        comp.set_dock(crate::shade::WindowId(wid), w as u32, h as u32)
    ).unwrap_or(false);

    if ok {
        crate::shade::request_render();
        0
    } else {
        -1
    }
}

pub(crate) fn npk_window_set_panel(ctx: &mut HostState, edge: i32, behavior: i32, w: i32, h: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 || edge < 0 || behavior < 0 { return -1; }
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

pub(crate) fn npk_battery(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    let cached = crate::battery::cached();
    if cached >= 0 {
        return cached;
    }
    match crate::battery::read() {
        Some(b) => ((b.status as i32) << 8) | b.percent as i32,
        None => -1,
    }
}

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
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    let addr = ((hi as u32 as u64) << 32) | (lo as u32 as u64);
    if !crate::memory::is_device_window(addr) {
        // Log only the first few: the driver polls forever.
        use core::sync::atomic::{AtomicU32, Ordering};
        static REFUSED: AtomicU32 = AtomicU32::new(0);
        let n = REFUSED.fetch_add(1, Ordering::Relaxed);
        if n < 3 {
            kprintln!("[npk] aml: refused SystemMemory read at {:#x} — that is RAM", addr);
        } else if n == 3 {
            kprintln!("[npk] aml: (further SystemMemory refusals silenced)");
        }
        return -1;
    }
    match crate::paging::read_phys_u8(addr) {
        Some(v) => v as i32,
        None => -1,
    }
}

/// Takes a pending EC query. -1 = nothing pending or no right.
pub(crate) fn npk_ec_query(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    match crate::ec::query() {
        Some(q) => q as i32,
        None => -1,
    }
}

pub(crate) fn npk_ec_read(ctx: &mut HostState, addr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    if !(0..=255).contains(&addr) { return -1; }
    match crate::ec::read(addr as u8) {
        Some(v) => v as i32,
        None => -1,
    }
}

pub(crate) fn npk_ec_write(ctx: &mut HostState, addr: i32, val: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    if !(0..=255).contains(&addr) || !(0..=255).contains(&val) { return -1; }
    if crate::ec::write(addr as u8, val as u8) { 0 } else { -1 }
}

pub(crate) fn npk_battery_report(ctx: &mut HostState, packed: i32) {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return;
    }
    crate::battery::report(packed);
}

pub(crate) fn npk_battery_detail(ctx: &mut HostState, rate: i32, remaining: i32,
    full: i32, voltage_mv: i32, unit: i32) {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
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
    if ctx.pid == 0 { return -1; }
    crate::audio::open_for(ctx.pid)
}

pub(crate) fn npk_audio_close(ctx: &mut HostState, slot: i32) -> i32 {
    if slot < 0 { return -1; }
    if crate::audio::close_for(slot as usize, ctx.pid) { 0 } else { -1 }
}

/// The system volume: the shell's control, or the app the user is using.
pub(crate) fn npk_audio_set_volume(ctx: &mut HostState, pct: i32) -> i32 {
    let shell = capability::check_global(&ctx.cap_id, capability::Rights::SHELL).is_ok();
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if !(0..=255).contains(&n) { return -1; }
    crate::shade::with_compositor(|c| c.switch_workspace(n as u8));
    crate::shade::request_render();
    0
}

pub(crate) fn npk_power(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER | capability::Rights::SHELL).is_err() {
        return -1;
    }
    crate::acpi::power_off();
    0
}

pub(crate) fn npk_close_widget(ctx: &mut HostState) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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

pub(crate) fn npk_get_fb_size(_ctx: &mut HostState) -> i64 {
    let (w, h) = crate::framebuffer::get_resolution();
    ((w as i64) << 32) | (h as i64)
}

pub(crate) fn npk_sys_info(ctx: &mut HostState, key: i32) -> i64 {
    match key & 0xFF {
        0 => crate::smp::per_core::core_count() as i64,
        1 => crate::interrupts::uptime_secs() as i64,
        2 => { let (_, mb) = crate::memory::stats(); mb as i64 },
        3 => { let (used, _) = crate::heap::stats(); used as i64 },
        4 => { let (_, total) = crate::heap::stats(); total as i64 },
        5 => { let (s, _, _, _) = crate::smp::scheduler::stats(); s as i64 },
        6 => { let (_, c, _, _) = crate::smp::scheduler::stats(); c as i64 },
        7 => { let (_, _, st, _) = crate::smp::scheduler::stats(); st as i64 },
        8 => { let (_, _, _, w) = crate::smp::scheduler::stats(); w as i64 },
        9 => if crate::smp::per_core::has_mwait() { 1 } else { 0 },
        10 => (crate::interrupts::tsc_freq() / 1_000_000) as i64,
        11 => {
            let core = (key >> 8) as usize;
            crate::smp::scheduler::queue_len(core) as i64
        },
        12 => {
            let core = (key >> 8) as usize;
            crate::smp::per_core::update_core_freq(core);
            crate::smp::per_core::core_freq_mhz(core) as i64
        },
        13 => crate::smp::per_core::max_turbo_mhz() as i64,
        14 => crate::smp::per_core::min_eff_mhz() as i64,
        15 => {
            let core = (key >> 8) as usize;
            crate::smp::per_core::update_core_freq(core);
            crate::smp::per_core::core_usage(core) as i64
        },
        // CPUID 0x15 raw values for diagnostics
        16 => { let (eax, _, _) = crate::interrupts::cpuid15(); eax as i64 },
        17 => { let (_, ebx, _) = crate::interrupts::cpuid15(); ebx as i64 },
        18 => { let (_, _, ecx) = crate::interrupts::cpuid15(); ecx as i64 },

        // 19 → raw TSC reading; combine with key 10 (TSC MHz) to convert
        // to time. The sign bit stays clear for ~150 years at 2 GHz.
        // SAFETY: rdtsc has no preconditions.
        19 => unsafe { core::arch::x86_64::_rdtsc() as i64 },

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
        30..=34 | 40 if capability::check_global(&ctx.cap_id, capability::Rights::WRITE).is_err() => -1,
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

pub(crate) fn npk_sleep(_ctx: &mut HostState, ms: i32) -> i32 {
    if ms <= 0 || ms > 60000 { return -1; }

    // The normal path: we run inside a fiber → yield to the scheduler.
    if crate::smp::fiber::yield_sleep(ms as u64) {
        return 0;
    }

    // Fallback: not inside a fiber (degenerate no-worker host, or a
    // one-shot wasm on Core 0) → HLT-idle until the deadline. No
    // core-stealing helper: that would risk nesting.
    let freq = crate::interrupts::tsc_freq();
    let target = crate::interrupts::rdtsc() + (ms as u64) * (freq / 1000);
    while crate::interrupts::rdtsc() < target {
        crate::interrupts::halt_until(Some(target), crate::smp::per_core::WAKE_NPK_SLEEP);
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
/// RENDER-gated like the calls that read them (`npk_bar_state`,
/// `npk_battery`).
const WAIT_STATE: i32 = 32;

/// Which of `mask`'s conditions hold right now. Bits the caller may not
/// wait on (a driver bit without a driver, an event bit without NETCTL)
/// never fire.
fn wait_ready(ctx: &mut HostState, mask: i32) -> i32 {
    let mut r = 0;
    if mask & WAIT_INPUT != 0 && input_ready(ctx) {
        r |= WAIT_INPUT;
    }
    if let Some(hw) = ctx.hw.as_mut() {
        if mask & WAIT_IRQ != 0 && hw.irq_vector != 0 {
            let n = crate::irq::fired_count(hw.irq_vector);
            if n != hw.irq_seen {
                hw.irq_seen = n;
                r |= WAIT_IRQ;
            }
        }
        if mask & WAIT_NET_TX != 0 && hw.registered_as_netdev
            && crate::netdev::wasm_nic_tx_pending()
        {
            r |= WAIT_NET_TX;
        }
        if mask & WAIT_WIFI_CMD != 0 && crate::wifi::cmd_pending() {
            r |= WAIT_WIFI_CMD;
        }
    }
    if mask & WAIT_WIFI_EVENT != 0
        && capability::check_global(&ctx.cap_id, capability::Rights::NETCTL).is_ok()
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
/// `WAIT_WIFI_CMD` 8, `WAIT_WIFI_EVENT` 16, `WAIT_STATE` 32.
///
/// The event-driven replacement for `loop { poll; npk_sleep(16) }`: the
/// app's fiber gives up its core and costs nothing until an event is pushed
/// for it (`widgets::push_event`, `wasm::push_app_key` signal it) or its
/// deadline comes. `docs/plan/CORES_AND_EVENTS.md` §3.3.
///
/// Every bit is gated by what the caller already owns: the IRQ bit by the
/// vector its driver registered, the NIC bit by being the registered WASM
/// NIC, the command bit by being a driver, the event bit by NETCTL — the
/// same rights the matching `npk_*_poll` calls check. It parks only the
/// caller's own fiber.
pub(crate) fn npk_wait(ctx: &mut HostState, mask: i32, timeout_ms: i32) -> i32 {
    let freq = crate::interrupts::tsc_freq();
    let deadline = if timeout_ms < 0 {
        crate::smp::fiber::NO_DEADLINE
    } else {
        crate::interrupts::rdtsc() + (timeout_ms as u64) * (freq / 1000)
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
            if mask & WAIT_IRQ != 0 && hw.irq_vector != 0 {
                // The interrupt must wake this core, and a level line the
                // ISR masked is released: the driver waits again, so it has
                // serviced the device (`irq::arm`).
                let _ = crate::irq::arm(hw.irq_vector);
                crate::irq::set_waiter(hw.irq_vector, w);
                sig |= SIG_IRQ;
            }
            if mask & WAIT_NET_TX != 0 && hw.registered_as_netdev {
                crate::netdev::set_nic_waker(w);
                sig |= SIG_TX;
            }
            if mask & WAIT_WIFI_CMD != 0 {
                crate::wifi::set_cmd_waker(w);
                sig |= SIG_WIFI;
            }
        }
        if mask & WAIT_WIFI_EVENT != 0
            && capability::check_global(&ctx.cap_id, capability::Rights::NETCTL).is_ok()
        {
            crate::wifi::set_event_waker(w);
            sig |= SIG_WIFI;
        }
        if mask & WAIT_STATE != 0
            && capability::check_global(&ctx.cap_id, capability::Rights::RENDER).is_ok()
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
        if crate::interrupts::rdtsc() >= deadline {
            break 0;
        }
        if crate::smp::fiber::wait(sig, deadline).is_none() {
            // Not in a fiber: halt in place, looking again every 10 ms.
            let recheck = crate::interrupts::rdtsc() + freq / 100;
            crate::interrupts::halt_until(
                Some(deadline.min(recheck)), crate::smp::per_core::WAKE_NPK_SLEEP);
        }
    };
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
    let freq = crate::interrupts::tsc_freq();
    let ticks_per_ms = freq / 1000;
    let deadline = crate::interrupts::rdtsc() + ms * ticks_per_ms;

    if let Some(w) = crate::smp::fiber::current_waker() {
        crate::wasm::set_key_waker(term_idx, w);
    }
    let result = loop {
        if let Some(k) = pop_app_key(term_idx) {
            break k as i32;
        }
        if crate::interrupts::rdtsc() >= deadline {
            break -1;
        }
        // Park until a key is pushed (it signals this fiber) or the
        // deadline. Halting here would hold the whole core and starve other
        // fibers on it.
        if crate::smp::fiber::wait(crate::smp::fiber::SIG_EVENT, deadline).is_none() {
            // Not in a fiber (a one-shot on Core 0): halt in place.
            let recheck = crate::interrupts::rdtsc() + freq / 100;
            crate::interrupts::halt_until(
                Some(deadline.min(recheck)), crate::smp::per_core::WAKE_NPK_SLEEP);
        }
    };

    // Resume work tracking
    crate::smp::per_core::set_active(core_id, true);
    crate::smp::per_core::start_work(core_id);

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
    capability::check_global(&ctx.cap_id, capability::Rights::HARDWARE).is_ok()
}

pub(crate) fn npk_stream_open(ctx: &mut HostState, idx: i32) -> i32 {
    if !stream_allowed(ctx) { return -1; }
    // -1 = the everything-sink: every write, whichever terminal it was
    // routed to. A remote console bound to one index goes silent as soon
    // as output is redirected elsewhere.
    if idx < 0 {
        return if crate::shade::terminal::stream_open_global() { 0 } else { -1 };
    }
    if crate::shade::terminal::stream_open(idx as usize) { 0 } else { -1 }
}

pub(crate) fn npk_stream_close(ctx: &mut HostState, idx: i32) -> i32 {
    if !stream_allowed(ctx) { return -1; }
    if idx >= 0 { crate::shade::terminal::stream_close(idx as usize); }
    else { crate::shade::terminal::stream_close_global(); }
    0
}

/// Injects a key byte into the shell's input queue.
///
/// Requires `Rights::HARDWARE`: the shell executes what arrives there with
/// its own authority, so an ungated injection would let any module run
/// commands.
pub(crate) fn npk_key_inject(ctx: &mut HostState, byte: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    crate::keyboard::inject_byte((byte & 0xFF) as u8);
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
        crate::kprintln!("[npk] tcp: Modul {} hat kein NET-Recht ({:?})",
            capability::short_id(&cap_id), e);
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

const MAX_TLS: usize = 8;

struct TlsSlot {
    /// Owner. A handle is served only to the process that opened it, as
    /// with the HTTP handles.
    pid: u32,
    session: crate::crypto::tls::TlsSession,
}

static TLS_SLOTS: spin::Mutex<[Option<TlsSlot>; MAX_TLS]> =
    spin::Mutex::new([const { None }; MAX_TLS]);

/// Common gate for every access to an existing handle.
fn tls_slot_ok(ctx: &mut HostState, handle: i32) -> Option<usize> {
    if !net_allowed(ctx) { return None }
    if handle < 0 || handle as usize >= MAX_TLS { return None }
    let i = handle as usize;
    let g = TLS_SLOTS.lock();
    match &g[i] {
        Some(sl) if sl.pid == ctx.pid => Some(i),
        _ => None,
    }
}

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

    let free = {
        let g = TLS_SLOTS.lock();
        match g.iter().position(|s| s.is_none()) { Some(i) => i, None => return -1 }
    };
    let tcp = match crate::net::tcp::connect(ip, port as u16) {
        Ok(h) => h,
        Err(_) => return -1,
    };
    let session = match crate::crypto::tls::tls_connect(tcp, &bare) {
        Ok(s) => s,
        Err(e) => {
            kprintln!("[npk] tls: Handschlag mit {} gescheitert ({:?})", bare, e);
            let _ = crate::net::tcp::close(tcp);
            return -1;
        }
    };
    TLS_SLOTS.lock()[free] = Some(TlsSlot { pid: ctx.pid, session });
    free as i32
}

/// `npk_tls_send(handle, ptr, len) -> 0 | -1`
pub(crate) fn npk_tls_send(mem: &mut [u8], ctx: &mut HostState,
                           handle: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    if buf_len <= 0 { return -1 }
    // Straight from guest memory: `tls_send` copies one record at a time,
    // so the kernel never holds more than a record of it.
    let Some(data) = guest(mem, buf_ptr, buf_len as usize) else { return -1 };
    let mut g = TLS_SLOTS.lock();
    let Some(sl) = g[i].as_mut() else { return -1 };
    match crate::crypto::tls::tls_send(&mut sl.session, data) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// `npk_tls_recv(handle, ptr, cap) -> n | 0 (nothing yet) | -1 (closed/error)`
///
/// Returns immediately: `tls_poll` only collects what the TCP stack already
/// has. A browser polls this every frame.
pub(crate) fn npk_tls_recv(mem: &mut [u8], ctx: &mut HostState,
                           handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    if buf_max <= 0 { return -1 }
    let Some(dst) = guest_mut(mem, buf_ptr, buf_max as usize) else { return -1 };
    let mut g = TLS_SLOTS.lock();
    let Some(sl) = g[i].as_mut() else { return -1 };
    match crate::crypto::tls::tls_poll(&mut sl.session, dst) {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

/// `npk_tls_close(handle) -> 0`
pub(crate) fn npk_tls_close(ctx: &mut HostState, handle: i32) -> i32 {
    let Some(i) = tls_slot_ok(ctx, handle) else { return -1 };
    if let Some(mut sl) = TLS_SLOTS.lock()[i].take() {
        let _ = crate::crypto::tls::tls_close(&mut sl.session);
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
    match crate::net::tcp::connect_start(ip, port as u16) {
        Ok(h) => { ctx.tcp_handles.push(h); h as i32 }
        Err(_) => -1,
    }
}

/// A raw TCP handle this module opened itself, or `None`.
fn own_tcp(ctx: &HostState, handle: i32) -> Option<usize> {
    let h = usize::try_from(handle).ok()?;
    ctx.tcp_handles.contains(&h).then_some(h)
}

pub(crate) fn npk_tcp_status(ctx: &mut HostState, handle: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    let Some(h) = own_tcp(ctx, handle) else { return -1 };
    crate::net::tcp::connect_status(h)
}

pub(crate) fn npk_tcp_close(ctx: &mut HostState, handle: i32) -> i32 {
    if !net_allowed(ctx) { return -1; }
    if let Some(h) = own_tcp(ctx, handle) {
        ctx.tcp_handles.retain(|&x| x != h);
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
        && capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        kprintln!("[npk] WASM: npk_pci_bind DENIED {:04x}:{:04x}", vid, did);
        return -2;
    }
    crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
    ctx.hw = Some(HwDriverState {
        is_pci: true,
        pci_addr: dev.addr,
        vendor_id: vid,
        device_id: did,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
        irq_vector: 0,
        irq_seen: 0,
    });
    crate::kdebug!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, vid, did);
    0
}

pub(crate) fn npk_pci_bind_class(ctx: &mut HostState, class: i32, subclass: i32) -> i32 {
    npk_pci_bind_class_n(ctx, class, subclass, 0)
}

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
        && capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        kprintln!("[npk] WASM: npk_pci_bind_class DENIED {:02x}:{:02x}", cls, sub);
        return -2;
    }
    crate::kdebug!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, dev.vendor_id, dev.device_id);
    crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
    ctx.hw = Some(HwDriverState {
        is_pci: true,
        pci_addr: dev.addr,
        vendor_id: dev.vendor_id,
        device_id: dev.device_id,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
        irq_vector: 0,
        irq_seen: 0,
    });
    0
}

pub(crate) fn npk_pci_read_config(ctx: &mut HostState, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) if h.is_pci => h,
        // Without a PCI device there is no config address; answering
        // would access 00:00.0.
        _ => return -1,
    };
    if offset < 0 || offset > 255 { return -1; }
    pci::read32(hw.pci_addr, offset as u8) as i32
}

pub(crate) fn npk_pci_write_config(ctx: &mut HostState, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) if h.is_pci => h,
        // Without a PCI device there is no config address; answering
        // would access 00:00.0.
        _ => return -1,
    };
    if offset < 0 || offset > 255 { return -1; }
    if config_write_refused(hw.pci_addr, offset as u8 & !3) {
        kprintln!("[npk] WASM: PCI config write at {:#x} refused (address or interrupt routing)", offset);
        return -1;
    }
    pci::write32(hw.pci_addr, offset as u8, value as u32);
    0
}

/// Config dwords a driver must not write: they decide where the device sits
/// in physical memory (BARs, expansion ROM, a bridge's windows) or where its
/// interrupts go (MSI address and data, MSI-X control). The kernel sizes and
/// assigns BARs and programs interrupts itself; a driver that moved a BAR
/// onto RAM could map that RAM through `npk_mmio_map_bar`.
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

pub(crate) fn npk_pci_enable_bus_master(ctx: &mut HostState) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) if h.is_pci => h,
        // Without a PCI device there is no config address; answering
        // would access 00:00.0.
        _ => return -1,
    };
    pci::enable_bus_master(hw.pci_addr);
    // Also enable memory space
    let cmd = pci::read32(hw.pci_addr, 0x04);
    pci::write32(hw.pci_addr, 0x04, cmd | 0x06);
    // And on every bridge above: without bus mastering there, the bridge
    // does not forward the device's requests upstream and the device gets
    // a master abort on valid RAM.
    pci::enable_bus_master_path(hw.pci_addr);
    hw.bus_master_enabled = true;
    0
}

pub(crate) fn npk_irq_register(ctx: &mut HostState, entry: i32) -> i32 {
    // Without a PCI device there is no config address.
    let hw = match ctx.hw.as_mut() { Some(h) if h.is_pci => h, _ => return -1 };
    if !(0..2048).contains(&entry) { return -1; }
    // One vector per driver. Registering again returns the same one — the
    // pool has 16 vectors and is never freed, so a loop around this call
    // would otherwise take them all.
    if hw.irq_vector != 0 { return hw.irq_vector as i32; }
    match crate::irq::register(hw.pci_addr, entry as u16) {
        Some(v) => { hw.irq_vector = v; v as i32 }
        None => -1,
    }
}

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
    if capability::check_global(&ctx.cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if hw.irq_vector != 0 { return hw.irq_vector as i32; }
    if !(0..256).contains(&gsi) { return -1; }
    match crate::irq::register_gsi(gsi as u32, flags & 1 != 0, flags & 2 != 0) {
        Some(v) => { hw.irq_vector = v; v as i32 }
        None => -1,
    }
}

/// `npk_sci_arm(gpe) -> vector | -1`: the AML driver takes the ACPI SCI for
/// its EC's GPE. Same ownership as `npk_irq_register_gsi` — the vector is
/// this module's, and `npk_wait(WAIT_IRQ)` arms it. Once per boot.
pub(crate) fn npk_sci_arm(ctx: &mut HostState, gpe: i32) -> i32 {
    if capability::check_global(&ctx.cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    if !(0..256).contains(&gpe) { return -1; }
    if ctx.hw.is_none() {
        crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
        ctx.hw = Some(HwDriverState {
            is_pci: false,
            pci_addr: pci::PciAddr { bus: 0, device: 0, function: 0 },
            vendor_id: 0,
            device_id: 0,
            mmio_maps: Vec::new(),
            dma_allocs: Vec::new(),
            bus_master_enabled: false,
            registered_as_netdev: false,
            irq_vector: 0,
            irq_seen: 0,
        });
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if hw.irq_vector != 0 { return -1; }
    let Some((gsi, level, low)) = crate::sci::arm_ec(gpe as u32) else { return -1 };
    match crate::irq::register_gsi(gsi, level, low) {
        Some(v) => {
            hw.irq_vector = v;
            crate::sci::VECTOR.store(v as u32, core::sync::atomic::Ordering::Relaxed);
            v as i32
        }
        None => -1,
    }
}

/// `npk_sci_service() -> mask`: ack the SCI sources (see `sci::service`).
pub(crate) fn npk_sci_service(ctx: &mut HostState) -> i32 {
    if capability::check_global(&ctx.cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    crate::sci::service() as i32
}

/// Is `vector` the one this module's driver registered?
fn owns_vector(ctx: &HostState, vector: i32) -> bool {
    matches!(ctx.hw.as_ref(), Some(h) if h.irq_vector != 0 && h.irq_vector as i32 == vector)
}

pub(crate) fn npk_irq_arm(ctx: &mut HostState, vector: i32) -> i64 {
    // Arming re-routes the device's MSI to the calling core — only for the
    // module that owns it.
    if !owns_vector(ctx, vector) { return -1; }
    crate::irq::arm(vector as u8) as i64
}

pub(crate) fn npk_irq_wait(ctx: &mut HostState, vector: i32, since: i64, timeout_ms: i32) -> i32 {
    if !owns_vector(ctx, vector) || since < 0 { return -1; }
    let t = if timeout_ms <= 0 { 1000 } else { timeout_ms as u64 };
    if crate::irq::wait(vector as u8, since as u64, t) { 1 } else { 0 }
}

pub(crate) fn npk_mmio_map_bar(ctx: &mut HostState, bar_idx: i32, pages: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) if h.is_pci => h,
        // Without a PCI device there is no config address; answering
        // would access 00:00.0.
        _ => return -1,
    };
    if bar_idx < 0 || bar_idx > 5 || pages <= 0 || pages > 256 { return -1; }
    if hw.mmio_maps.len() >= MAX_MMIO_MAPS { return -1; }

    let bar_offset = 0x10 + (bar_idx as u8) * 4;
    let bar_raw = pci::read32(hw.pci_addr, bar_offset);
    let is_64bit = bar_raw & 0x04 != 0;
    let mut bar_base = if is_64bit {
        pci::read_bar64(hw.pci_addr, bar_offset)
    } else {
        (bar_raw & 0xFFFF_FFF0) as u64
    };

    // If BAR is unassigned (UEFI didn't configure it), assign it now.
    // assign_bar_mmio sizes the BAR internally; we just need the base.
    if bar_base == 0 && bar_raw & 0x01 == 0 {
        bar_base = pci::assign_bar_mmio(hw.pci_addr, bar_offset);
        if bar_base == 0 { return -1; }
    }
    if bar_base == 0 { return -1; }

    // Size the BAR: disable memory, write 0xFFFFFFFF, read back, restore.
    // Safe at this point because the driver hasn't started using the
    // BAR yet (mmio_map_bar is the first access after pci_bind).
    let cmd = pci::read32(hw.pci_addr, 0x04);
    pci::write32(hw.pci_addr, 0x04, cmd & !0x02);
    let saved_lo = pci::read32(hw.pci_addr, bar_offset);
    pci::write32(hw.pci_addr, bar_offset, 0xFFFF_FFFF);
    let size_lo = pci::read32(hw.pci_addr, bar_offset);
    pci::write32(hw.pci_addr, bar_offset, saved_lo);
    let bar_size = (!((size_lo & !0xF) as u64)).wrapping_add(1) & 0xFFFF_FFFF;
    pci::write32(hw.pci_addr, 0x04, cmd);

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
    for i in 0..page_count {
        let addr = bar_base + (i * 4096) as u64;
        // SAFETY: identity-mapped MMIO region for bound PCI device BAR.
        // map_page splits huge pages to set NO_CACHE for MMIO access.
        if let Err(e) = crate::paging::map_page(
            addr, addr,
            crate::paging::PageFlags::PRESENT
                | crate::paging::PageFlags::WRITABLE
                | crate::paging::PageFlags::NO_CACHE,
        ) {
            kprintln!("[npk] WASM MMIO map {:#x}: {}", addr, e);
        }
    }
    let handle = hw.mmio_maps.len();
    hw.mmio_maps.push((bar_base, page_count));
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
///  * Caps on the span (16 pages = 64 KiB) and on the number of mappings
///    (`MAX_MMIO_MAPS`), as for a BAR.
/// Platform ranges no driver may map (ECAM, HPET, IOMMU), read once from
/// the ACPI tables.
static PLATFORM_MMIO: spin::Mutex<Option<Vec<(u64, u64)>>> = spin::Mutex::new(None);

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

pub(crate) fn npk_mmio_map_phys(ctx: &mut HostState, hi: i32, lo: i32, pages: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        kprintln!("[npk] WASM: npk_mmio_map_phys DENIED — needs HARDWARE");
        return -1;
    }
    if pages <= 0 || pages > 16 { return -1; }
    let base = ((hi as u32 as u64) << 32) | (lo as u32 as u64);
    if base == 0 || base & 0xFFF != 0 { return -1; }
    let n = pages as usize;

    for i in 0..n {
        let a = match base.checked_add((i * 4096) as u64) { Some(a) => a, None => return -1 };
        if let Some(why) = mmio_refusal(a) {
            kprintln!("[npk] WASM: npk_mmio_map_phys refused {:#x}: {}", a, why);
            return -1;
        }
    }

    // A driver state without a PCI device if the module never bound one.
    if ctx.hw.is_none() {
        crate::smp::per_core::mark_driver_core(crate::smp::per_core::current_core_id());
        ctx.hw = Some(HwDriverState {
            is_pci: false,
            pci_addr: pci::PciAddr { bus: 0, device: 0, function: 0 },
            vendor_id: 0,
            device_id: 0,
            mmio_maps: Vec::new(),
            dma_allocs: Vec::new(),
            bus_master_enabled: false,
            registered_as_netdev: false,
            irq_vector: 0,
            irq_seen: 0,
        });
    }
    let hw = match ctx.hw.as_mut() { Some(h) => h, None => return -1 };
    if hw.mmio_maps.len() >= MAX_MMIO_MAPS { return -1; }

    for i in 0..n {
        let a = base + (i * 4096) as u64;
        // SAFETY: checked above to be neither RAM nor an interrupt
        // controller. NO_CACHE because device registers must not be cached.
        if let Err(e) = crate::paging::map_page(
            a, a,
            crate::paging::PageFlags::PRESENT
                | crate::paging::PageFlags::WRITABLE
                | crate::paging::PageFlags::NO_CACHE,
        ) {
            kprintln!("[npk] WASM: npk_mmio_map_phys map {:#x}: {}", a, e);
            return -1;
        }
    }
    let handle = hw.mmio_maps.len();
    hw.mmio_maps.push((base, n));
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
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
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

pub(crate) fn npk_mmio_read32(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 4 > pages * 4096 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR
    unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) as i32 }
}

pub(crate) fn npk_mmio_write32(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 4 > pages * 4096 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR
    unsafe { core::ptr::write_volatile((base + off as u64) as *mut u32, value as u32) }
    0
}

pub(crate) fn npk_mmio_read16(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
    unsafe { core::ptr::read_volatile((base + off as u64) as *const u16) as i32 }
}

pub(crate) fn npk_mmio_write16(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
    unsafe { core::ptr::write_volatile((base + off as u64) as *mut u16, value as u16) }
    0
}

// 8-bit MMIO. rtw88 (RTL8822CE) addresses much of its register set in
// bytes: the power sequence is a read8/write8 interpreter, and registers
// like REG_SYS_FUNC_EN+1 mean a single byte. A 32-bit RMW also touches
// three neighbouring bytes, which differs on registers with read side
// effects. Linux chooses the width deliberately; we follow it.
pub(crate) fn npk_mmio_read8(ctx: &mut HostState, handle: i32, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 1 > pages * 4096 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR
    unsafe { core::ptr::read_volatile((base + off as u64) as *const u8) as i32 }
}

pub(crate) fn npk_mmio_write8(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 1 > pages * 4096 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR
    unsafe { core::ptr::write_volatile((base + off as u64) as *mut u8, value as u8) }
    0
}

pub(crate) fn npk_mmio_read64(ctx: &mut HostState, handle: i32, offset: i32) -> i64 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 8 > pages * 4096 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR
    let lo = unsafe { core::ptr::read_volatile((base + off as u64) as *const u32) } as u64;
    let hi = unsafe { core::ptr::read_volatile((base + off as u64 + 4) as *const u32) } as u64;
    (hi << 32 | lo) as i64
}

pub(crate) fn npk_mmio_write64(ctx: &mut HostState, handle: i32, offset: i32, value: i64) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.mmio_maps.len() { return -1; }
    let (base, pages) = hw.mmio_maps[h];
    let off = offset as usize;
    if offset < 0 || off + 8 > pages * 4096 { return -1; }
    let v = value as u64;
    // SAFETY: validated MMIO region within mapped BAR
    unsafe {
        core::ptr::write_volatile((base + off as u64) as *mut u32, v as u32);
        core::ptr::write_volatile((base + off as u64 + 4) as *mut u32, (v >> 32) as u32);
    }
    0
}

pub(crate) fn npk_dma_alloc(ctx: &mut HostState, pages: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if pages <= 0 || pages as usize > MAX_DMA_PAGES_PER_CALL { return -1; }
    let page_count = pages as usize;
    if hw.dma_allocs.len() >= MAX_DMA_ALLOCS { return -1; }
    let total: usize = hw.dma_allocs.iter().map(|(_, p)| *p).sum();
    if total + page_count > MAX_DMA_PAGES { return -1; }

    // DMA buffers must be below 4 GB: the PCIe TX BD has a 32-bit address field.
    let phys = match crate::memory::allocate_contiguous_below(page_count, 0x1_0000_0000) {
        Some(p) => p,
        None => return -1,
    };
    // SAFETY: zeroing freshly allocated DMA memory
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }
    let handle = hw.dma_allocs.len();
    hw.dma_allocs.push((phys, page_count));
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
/// the driver. `limit_mb <= 0` means 4 GB; nothing above 4 GB is allowed,
/// since devices with 32-bit descriptors would silently break.
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
    if hw.dma_allocs.len() >= MAX_DMA_ALLOCS { return -1; }
    let total: usize = hw.dma_allocs.iter().map(|(_, p)| *p).sum();
    if total + page_count > MAX_DMA_PAGES { return -1; }

    let limit = if limit_mb <= 0 {
        0x1_0000_0000u64
    } else {
        ((limit_mb as u64) * 1024 * 1024).min(0x1_0000_0000)
    };

    let phys = match crate::memory::allocate_contiguous_below(page_count, limit) {
        Some(p) => p,
        None => return -1,
    };
    // SAFETY: zeroing freshly allocated DMA memory
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, page_count * 4096) }
    let handle = hw.dma_allocs.len();
    hw.dma_allocs.push((phys, page_count));
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
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.dma_allocs.len() { return -1; }
    let (phys, pages) = hw.dma_allocs[h];
    let off = offset as usize;
    if offset < 0 || off + 4 > pages * 4096 { return -1; }
    // SAFETY: reading from validated DMA buffer
    unsafe { core::ptr::read_volatile((phys + off as u64) as *const u32) as i32 }
}

pub(crate) fn npk_dma_write32(ctx: &mut HostState, handle: i32, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    let h = handle as usize;
    if h >= hw.dma_allocs.len() { return -1; }
    let (phys, pages) = hw.dma_allocs[h];
    let off = offset as usize;
    if offset < 0 || off + 4 > pages * 4096 { return -1; }
    // SAFETY: writing to validated DMA buffer
    unsafe { core::ptr::write_volatile((phys + off as u64) as *mut u32, value as u32) }
    0
}

pub(crate) fn npk_memory_fence(_ctx: &mut HostState) -> i32 {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    0
}

/// The NIC's own driver: a hardware module that registered as the network
/// device. Only it may move frames and wifi commands for that device; any
/// other module with driver state (an ACPI or input driver) may not.
fn is_netdev(ctx: &HostState) -> bool {
    ctx.hw.as_ref().is_some_and(|h| h.registered_as_netdev)
}

pub(crate) fn npk_netdev_set_link(ctx: &mut HostState, up: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    crate::netdev::set_wasm_nic_link(up != 0);
    0
}

pub(crate) fn npk_netdev_set_link_state(ctx: &mut HostState, carrier: i32, dormant: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    crate::netdev::set_wasm_nic_link_state(carrier != 0, dormant != 0);
    0
}

pub(crate) fn npk_fetch(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    // Another module's private area is closed, even with global READ. The
    // module's own area is open even without READ (`wasm::is_own_private`).
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fetch DENIED ({} gehoert einem anderen Modul)", name);
        return -1;
    }
    if !crate::wasm::is_own_private(&name, &ctx.module_name) {
        if let Err(e) = capability::check_global(&cap_id, capability::Rights::READ) {
            kprintln!("[npk] WASM: npk_fetch DENIED (cap_id={:08x}, {:?})",
                capability::short_id(&cap_id), e);
            return -1;
        }
    }

    let (content, _) = match crate::npkfs::fetch(&name) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    let Some(max) = glen(buf_max) else { return -1 };
    write_bytes(mem, buf_ptr, &content[..content.len().min(max)])
}

pub(crate) fn npk_http_response_headers(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return -1;
    }
    if buf_ptr < 0 || buf_max <= 0 { return -1; }
    let hdrs = match &ctx.http_reply_headers {
        Some(h) => h.clone(),
        None => return -1,
    };
    let Some(max) = glen(buf_max) else { return -1 };
    let n = hdrs.len().min(max);
    write_bytes(mem, buf_ptr, &hdrs.as_bytes()[..n])
}

pub(crate) fn npk_http_final_url(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return -1;
    }
    if buf_ptr < 0 || buf_max <= 0 { return -1; }
    let url = match &ctx.http_final_url {
        Some(u) => u.clone(),
        None => return -1,
    };
    let Some(max) = glen(buf_max) else { return -1 };
    let n = url.len().min(max);
    write_bytes(mem, buf_ptr, &url.as_bytes()[..n])
}

pub(crate) fn npk_http_content_type(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return -1;
    }
    if buf_ptr < 0 || buf_max <= 0 { return -1; }
    let ct = match &ctx.http_content_type {
        Some(c) => c.clone(),
        None => return -1,
    };
    let Some(max) = glen(buf_max) else { return -1 };
    let n = ct.len().min(max);
    write_bytes(mem, buf_ptr, &ct.as_bytes()[..n])
}

pub(crate) fn npk_http_last_error(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return -1;
    }
    if buf_ptr < 0 || buf_max <= 0 { return -1; }
    let err = match &ctx.http_last_error {
        Some(e) => e.clone(),
        None => return -1,
    };
    let Some(max) = glen(buf_max) else { return -1 };
    let n = err.len().min(max);
    write_bytes(mem, buf_ptr, &err.as_bytes()[..n])
}

pub(crate) fn npk_store(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };

    // Apps may not write the module store or the trust store — see
    // is_trust_critical_path.
    // Checked before the grant path so a per-file grant can never
    // become a way in there.
    if is_trust_critical_path(&name) {
        kprintln!("[npk] WASM: npk_store DENIED ({} is read-only to apps)", name);
        return -1;
    }
    // Another module's private area is closed, even with global READ and
    // WRITE. See `wasm::private_area_owner`.
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_store DENIED ({} gehoert einem anderen Modul)", name);
        return -1;
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
    let own_config = alloc::format!("sys/config/{}", ctx.module_name);
    let has_write = capability::check_global(&cap_id, capability::Rights::WRITE).is_ok()
        || capability::check_path_grant(&cap_id, &name, capability::Rights::WRITE)
        || name == own_config;
    let by_private = !has_write && crate::wasm::is_own_private(&name, &ctx.module_name);
    if !has_write && !by_private {
        kprintln!("[npk] WASM: npk_store DENIED (no WRITE, no grant for {})", name);
        return -1;
    }
    // The private area must not be a way to fill the disk without WRITE.
    // Larger data needs WRITE, and with it the user's consent.
    // Not implemented: a total quota. This caps each write, so many small
    // files still get past it.
    const PRIVATE_WRITE_MAX: i32 = 1024 * 1024;
    if by_private && data_len > PRIVATE_WRITE_MAX {
        kprintln!("[npk] WASM: npk_store DENIED ({} B in den privaten Bereich, Deckel {} B)",
            data_len, PRIVATE_WRITE_MAX);
        return -1;
    }

    let Some(payload) = glen(data_len).and_then(|n| guest(mem, data_ptr, n)) else { return -1 };
    if payload.is_empty() { return -1; }

    // Insert-or-replace: apps with state to persist (panel
    // configs, etc.) re-write the same key on every change. The
    // strict-create `store` would fail on the second write and
    // leave the app's state diverging from disk.
    match crate::npkfs::upsert(&name, payload, cap_id) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_home_dir(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
        return -1;
    }
    let home = crate::intent::home_dir();
    let bytes = home.as_bytes();
    if glen(buf_max).is_none_or(|m| bytes.len() > m) { return -1; }
    write_bytes(mem, buf_ptr, bytes)
}

pub(crate) fn npk_locale(mem: &mut [u8], _ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let lang = crate::config::get("lang").unwrap_or_default();
    let lang = lang.trim();
    let bytes = if lang.is_empty() { b"en".as_slice() } else { lang.as_bytes() };
    if buf_max < 0 || bytes.len() > buf_max as usize { return -1; }
    let data = &mut *mem;
    let Ok(start) = usize::try_from(buf_ptr) else { return -1 };
    let Some(end) = start.checked_add(bytes.len()) else { return -1 };
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
}

pub(crate) fn npk_launch_arg(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let arg = match &ctx.launch_arg {
        Some(s) => s.clone(),
        None => return 0,
    };
    let bytes = arg.as_bytes();
    if glen(buf_max).is_none_or(|m| bytes.len() > m) { return -1; }
    write_bytes(mem, buf_ptr, bytes)
}

pub(crate) fn npk_clipboard_set(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    let Some(slice) = glen(len).and_then(|n| guest(mem, ptr, n)) else { return -1 };
    crate::shade::clipboard::set_text(slice);
    slice.len() as i32
}

pub(crate) fn npk_clipboard_get(mem: &mut [u8], ctx: &mut HostState, ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    let text = match crate::shade::clipboard::get_text() {
        Some(t) => t,
        None => return 0,
    };
    let n = text.len().min(max.max(0) as usize);
    if write_bytes(mem, ptr, &text[..n]) < 0 { return -1; }
    text.len() as i32
}

pub(crate) fn npk_scene_commit(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        kprintln!("[npk] WASM: npk_scene_commit DENIED (no RENDER)");
        return -1;
    }
    // Remember which capability owns this window, so a later grant
    // (loft opening a file in an already-running editor) can find it.
    let owner_wid = ctx.widget_window_id;
    if owner_wid != 0 { crate::shade::widgets::set_window_cap(owner_wid, cap_id); }

    // Heap copy; a typical tree is a few hundred bytes.
    let payload: alloc::vec::Vec<u8> = match glen(len).and_then(|n| guest(mem, ptr, n)) {
        Some(b) if !b.is_empty() => b.to_vec(),
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
    if capability::check_global(&cap_id, capability::Rights::CANVAS).is_err() {
        return -1;
    }
    let wid = ctx.widget_window_id;
    if wid == 0 || width <= 0 || height <= 0 { return -1; }
    let Some(pixel_bytes) = (width as usize).checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4)) else { return -1 };
    if glen(len) != Some(pixel_bytes) { return -1; }
    let Some(px) = guest(mem, ptr, pixel_bytes).map(|b| b.to_vec()) else { return -1 };
    if !crate::shade::widgets::canvas::commit(
        wid, canvas_id as u32, width as u32, height as u32, px) {
        return -1;
    }
    crate::shade::widgets::rerender_window_pixels(wid);
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
    if capability::check_global(&cap_id, capability::Rights::CANVAS).is_err() {
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

    let data = &*mem;
    let take = |ptr: i32, n: usize| guest(data, ptr, n).map(|b| b.to_vec());
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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

pub(crate) fn npk_capture_screen(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::CAPTURE).is_err() {
        return -1;
    }
    let info = crate::framebuffer::get_info();
    let w = info.width as usize;
    let h = info.height as usize;
    let pitch = info.pitch as usize;
    if w == 0 || h == 0 { return -1; }
    let need = w * h * 4;
    if (buf_max as usize) < need { return -1; }
    if info.addr == 0 { return -1; }
    // Read the actual displayed MMIO framebuffer (not a shadow
    // buffer): it always holds the final composite (background +
    // windows + cursor) that's physically on screen. The shadow
    // double-buffer can be mid-swap when we (on a worker core)
    // read it, yielding a stale background-only frame.
    // Row-by-row into a tight BGRA temp (pitch may exceed w*4).
    let mut tmp = alloc::vec![0u8; need];
    let src = info.addr as *const u8;
    for y in 0..h {
        // SAFETY: the GOP framebuffer is identity-mapped and valid
        // for pitch*height bytes; we read w*4 ≤ pitch per row.
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.add(y * pitch),
                tmp.as_mut_ptr().add(y * w * 4),
                w * 4,
            );
        }
    }
    write_bytes(mem, buf_ptr, &tmp[..])
}

pub(crate) fn npk_event_poll(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    if encoded.len() > buf_max as usize { return -1; }

    write_bytes(mem, buf_ptr, &encoded[..])
}

pub(crate) fn npk_list_modules(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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

    if out.len() > buf_max as usize { return -1; }

    write_bytes(mem, buf_ptr, &out[..])
}

pub(crate) fn npk_app_meta(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if name_len <= 0 || name_len > 64 || buf_max <= 0 { return -1; }

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

    let meta = match extract_wasm_custom_section(&content, ".npk.app_meta") {
        Some(m) => m,
        None => return -1,
    };

    let Some(max) = glen(buf_max) else { return -1 };
    write_bytes(mem, buf_ptr, &meta[..meta.len().min(max)])
}

pub(crate) fn npk_spawn_module(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
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
        None => return -1,
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
    if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
        return -1;
    }
    if verb_len <= 0 || verb_len > 64 { return -1; }
    let Some(verb) = glen(verb_len).and_then(|n| guest(mem, verb_ptr, n))
        .and_then(|b| core::str::from_utf8(b).ok()).map(String::from) else { return -1 };
    // Reject only on the pure cooperative Core-0 path (BSP-only),
    // so the caller falls back to typing the intent at the prompt.
    // Fiber mode (vCPU as a pool fiber) AND the dedicated-core path
    // both support launching from a worker, so allow those.
    if !crate::microvm::cpu::vm_fiber_mode()
        && crate::smp::per_core::dedicated_vm_core().is_none()
    {
        return -1;
    }
    match verb.as_str() {
        "browser" => {
            crate::intent::launch_browser();
            0
        }
        _ => -1,
    }
}

pub(crate) fn npk_bar_state(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if max <= 0 { return -1; }

    let unix = crate::rtc::read_unix_time().unwrap_or(0);
    let tz = crate::config::timezone_offset_minutes();
    let local = unix as i64 + tz as i64 * 60;
    let secs = ((local % 86400) + 86400) % 86400;
    let (ws_count, ws_active, title) =
        crate::shade::with_compositor(|c| c.bar_info())
            .unwrap_or((0, 0, alloc::string::String::new()));
    let s = alloc::format!("{:02}:{:02}\n{}\n{}\n{}",
        secs / 3600, (secs % 3600) / 60, ws_count, ws_active, title);

    let bytes = s.as_bytes();
    let Some(max) = glen(max) else { return -1 };
    write_bytes(mem, buf_ptr, &bytes[..bytes.len().min(max)])
}

pub(crate) fn npk_window_titles(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if max <= 0 { return -1; }
    let s = crate::shade::with_compositor(|c| c.window_lines())
        .unwrap_or_default();
    let bytes = s.as_bytes();
    let write_len = bytes.len().min(max as usize);
    let data = &mut *mem;
    let Ok(start) = usize::try_from(buf_ptr) else { return -1 };
    let Some(end) = start.checked_add(write_len) else { return -1 };
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&bytes[..write_len]);
    write_len as i32
}

/// Copies the `index`-th ACPI table with this signature into module memory.
///
/// `sig` holds the four characters little-endian in an `i32`, as they sit
/// in memory (`SSDT` = 0x54445353).
///
/// Firmware spreads its namespace over the DSDT and any number of SSDTs,
/// and Linux loads them all into one (`acpi_tb_load_namespace`); reading
/// only the DSDT misses names declared elsewhere.
///
/// Returns the length, the required length if the buffer is too small, or
/// -1 (no right / no such table).
pub(crate) fn npk_acpi_table(
    mem: &mut [u8], ctx: &mut HostState, sig: i32, index: i32, buf_ptr: i32, buf_max: i32,
) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    if index < 0 || index > 63 { return -1; }
    let sig_bytes = (sig as u32).to_le_bytes();
    let Some((addr, len)) = crate::acpi::find_table_nth(&sig_bytes, index as usize) else {
        return -1;
    };
    if len > buf_max as usize {
        return len as i32;
    }
    // SAFETY: find_table_nth mapped [addr, addr+len).
    let src = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    write_bytes(mem, buf_ptr, &src[..])
}

pub(crate) fn npk_acpi_dsdt(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    let Some((addr, len)) = crate::acpi::dsdt() else { return -1 };
    if len > buf_max as usize {
        return len as i32; // too small: tell the caller the needed size
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

/// Drains the mixed output of every slot: the audio driver's side.
/// Requires `Rights::HARDWARE`; anyone else could record or starve all
/// playback.
pub(crate) fn npk_audio_poll_mix(mem: &mut [u8], ctx: &mut HostState, ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::HARDWARE).is_err() {
        return -1;
    }
    if ptr < 0 || max < 0 { return -1; }
    let Some(out) = guest_mut(mem, ptr, max as usize) else { return -1 };
    crate::audio::poll_mix(out) as i32
}

pub(crate) fn npk_fs_list(mem: &mut [u8], ctx: &mut HostState, prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
        return -1;
    }
    if prefix_len < 0 || out_cap <= 0 { return -1; }

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

    if out.len() > out_cap as usize { return -1; }

    write_bytes(mem, out_ptr, &out[..])
}

pub(crate) fn npk_fs_stat(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, out_ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::READ).is_err() {
        return -1;
    }
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };

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
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
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

pub(crate) fn npk_set_theme(mem: &mut [u8], ctx: &mut HostState, ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
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
        crate::shade::terminal::stream_read_global(out) as i32
    } else {
        crate::shade::terminal::stream_read(idx as usize, out) as i32
    }
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
    let hw = ctx.hw.as_ref()?;
    let &(phys, pages) = hw.dma_allocs.get(usize::try_from(handle).ok()?)?;
    let off = usize::try_from(off).ok()?;
    let len = usize::try_from(len).ok()?;
    if off.checked_add(len)? > pages.checked_mul(4096)? { return None; }
    Some((phys + off as u64, len))
}

pub(crate) fn npk_dma_read(mem: &mut [u8], ctx: &mut HostState, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    let Some((phys, len)) = dma_range(ctx, handle, dma_off, len) else { return -1 };
    let Some(dst) = guest_mut(mem, wasm_ptr, len) else { return -1 };
    // SAFETY: `phys .. phys + len` lies inside one of this module's own DMA
    // allocations (`dma_range`), which stay allocated while the module runs
    // and are identity-mapped. The device may write it concurrently; the
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

pub(crate) fn npk_netdev_register(mem: &mut [u8], ctx: &mut HostState, mac_ptr: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if hw.registered_as_netdev { return -1; } // already registered

    let Some(raw) = guest(mem, mac_ptr, 6) else { return -1 };
    let mut mac = [0u8; 6];
    mac.copy_from_slice(raw);

    crate::netdev::register_wasm_nic(mac);
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
            if idx != TERM_IDX_ACTIVE && (idx as usize) < MAX_APP_BUFS {
                // Write to specific terminal (worker-core safe)
                crate::shade::terminal::write_idx(idx as usize, &s);
            } else {
                // Fallback: write to active terminal via kprint
                kprint!("{}", s);
            }
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

pub(crate) fn npk_http_request(mem: &mut [u8], ctx: &mut HostState, url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_request DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = buf_max as usize;

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

    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut info = crate::intent::http::FetchInfo::default();
    let res = crate::intent::http::https_get_streaming_ex(
        &host, &path, cap,
        &mut |chunk: &[u8]| -> Result<(), &'static str> {
            if out.len() < cap {
                let take = chunk.len().min(cap - out.len());
                out.extend_from_slice(&chunk[..take]);
            }
            Ok(())
        },
        Some(&mut info),
        &crate::intent::http::HttpRequest {
            // The document itself compresses well
            // (docs/plan/JS_SCOPE_CONTENT_WEB.md §8).
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

    let write_len = out.len().min(cap);
    // Bounds-checked write: buf_ptr is guest-controlled, and a
    // wrapping `start + len` would panic the kernel on the slice index.
    write_bytes(mem, buf_ptr, &out[..write_len])
}

pub(crate) fn npk_http_send(mem: &mut [u8], ctx: &mut HostState, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_send DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = buf_max as usize;

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
    };
    let res = crate::intent::http::https_request_streaming(
        &host, &path, &req, cap,
        &mut |chunk: &[u8]| -> Result<(), &'static str> {
            if out.len() < cap {
                let take = chunk.len().min(cap - out.len());
                out.extend_from_slice(&chunk[..take]);
            }
            Ok(())
        },
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

    let write_len = out.len().min(cap);
    write_bytes(mem, buf_ptr, &out[..write_len])
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
    if capability::check_global(&cap_id, capability::Rights::NET).is_err() {
        return -1;
    }
    let url = match read_str(mem, url_ptr, url_len) {
        Some(s) => s,
        None => return -1,
    };
    let reach = crate::intent::http::reach_of_url(&url);
    if reach != ctx.net_reach {
        kprintln!("[npk] Netzkontext: {:?} ({})", reach, url);
    }
    ctx.net_reach = reach;
    0
}

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
        return -1;
    }
    if buf_max <= 0 { return -1; }
    let cap = buf_max as usize;

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
            -1
        }
    }
}

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
    match crate::intent::fetch::begin_many(ctx.pid, ctx.core_id, urls, cookies, out_max as usize,
                                          Some(ctx.net_reach)) {
        Ok(h) => h,
        Err(e) => {
            ctx.http_last_error = Some(alloc::format!("queue\t{}", e));
            -1
        }
    }
}

/// 1 = an answer is waiting, 0 = still running, -1 = it failed (call
/// `npk_http_take` for the reason), -2 = no such handle.
pub(crate) fn npk_http_poll(ctx: &mut HostState, handle: i32) -> i32 {
    crate::intent::fetch::poll(ctx.pid, handle)
}

/// Collect a finished single request. Bytes written on success; -1 if the
/// request failed; -2 if the handle is unknown; -3 while it is still running
/// (and only then does the job survive the call).
///
/// Fills exactly the five getters `npk_http_send` fills, and clears them the
/// same way — a caller must never read one request's answer and attribute it
/// to the next.
pub(crate) fn npk_http_take(mem: &mut [u8], ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if buf_max < 0 { return -1; }
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

    let write_len = reply.body.len().min(buf_max as usize);
    write_bytes(mem, buf_ptr, &reply.body[..write_len])
}

/// Collect a finished batch: the bodies back to back in `out`, one
/// little-endian i32 length per URL in `lens` (-1 for one that failed).
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

/// Stop caring about a handle. Always 0 — a browser cancels on every
/// navigation and must not have to know which state it caught.
pub(crate) fn npk_http_cancel(ctx: &mut HostState, handle: i32) -> i32 {
    crate::intent::fetch::cancel(ctx.pid, handle);
    0
}

pub(crate) fn npk_http_request_many(mem: &mut [u8], ctx: &mut HostState, urls_ptr: i32, urls_len: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::NET) {
        kprintln!("[npk] WASM: npk_http_request_many DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
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
    // Bound the work a single call can ask for, and make sure the
    // guest actually gave us room for one length per URL.
    const MAX_URLS: usize = 64;
    if urls.is_empty() || urls.len() > MAX_URLS { return -1; }
    if (lens_max as usize) < urls.len() * 4 { return -1; }

    let total_cap = out_max as usize;
    let bodies = crate::intent::http::https_get_many(&urls, &[], total_cap, Some(ctx.net_reach));

    let mut blobs: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut lens: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    for body in bodies {
        let n: i32 = match body {
            // Drop a body that would overrun the caller's buffer
            // rather than truncating it — half an image decodes to
            // garbage, whereas a missing one draws a placeholder.
            Some(b) if blobs.len() + b.len() <= total_cap => {
                blobs.extend_from_slice(&b);
                b.len() as i32
            }
            _ => -1,
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
    for want in [Rights::READ, Rights::WRITE] {
        if capability::check_global(cap, want).is_ok()
            || capability::check_path_grant(cap, path, want)
        {
            r |= want;
        }
    }
    r
}

pub(crate) fn npk_open(mem: &mut [u8], ctx: &mut HostState, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
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
    let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
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
        None => return -1,
    };
    crate::shade::focus_window(win); // bring the editor to the front
    crate::shade::request_render();
    if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, win.0, arg) { 0 } else { -1 }
}

pub(crate) fn npk_launch(mem: &mut [u8], ctx: &mut HostState, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
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
    let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
        Ok(id) => id,
        Err(_) => return -1,
    };
    if spawn_on_worker_inner(bytes, module_cap, 255, &app, false, 0, arg) { 0 } else { -1 }
}

pub(crate) fn npk_pick(mem: &mut [u8], ctx: &mut HostState, mode: i32, start_ptr: i32, start_len: i32, suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if mode != 0 && mode != 1 { return -1; }

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
    let module_cap = match capability::create_module_cap(rights, Some(600_000)) {
        Ok(id) => id,
        Err(_) => return -1,
    };

    // Wire the request as the launch argument:
    //   "<open|save>\0<start-dir>\0<suggested-name>"
    let arg = alloc::format!("{}\0{}\0{}",
        if mode == 1 { "save" } else { "open" }, start, suggest);

    // Floating + centred, like the launcher — a dialog must not
    // re-tile the workspace behind it.
    let win = match crate::shade::with_compositor(|c| {
        let id = c.create_widget_window(&module);
        c.set_overlay(id, PICKER_W, PICKER_H);
        id
    }) {
        Some(id) => id,
        None => return -1,
    };
    crate::shade::widgets::register_pick(win.0, requester, tag as u32, cap_id, mode == 1);
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
    match crate::npkfs::fs::mkdir(clean) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_delete(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
        return -1;
    }
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };
    // Apps may not delete modules or trust anchors — see
    // is_trust_critical_path.
    if is_trust_critical_path(&name) {
        kprintln!("[npk] WASM: npk_fs_delete DENIED ({} is read-only to apps)", name);
        return -1;
    }
    if !crate::wasm::private_area_allows(&name, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_delete DENIED ({} gehoert einem anderen Modul)", name);
        return -1;
    }
    match crate::npkfs::delete(&name) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_rename(mem: &mut [u8], ctx: &mut HostState, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
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
    // Neither source nor destination may be module or trust store —
    // renaming in would plant an unverified module or anchor.
    if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
        kprintln!("[npk] WASM: npk_fs_rename DENIED (module/trust store is read-only to apps)");
        return -1;
    }
    // Both sides: checking only the source would let a file be moved into
    // another module's private area, checking only the target would let
    // one be moved out.
    if !crate::wasm::private_area_allows(&old, &ctx.module_name)
        || !crate::wasm::private_area_allows(&new, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_rename DENIED (privater Bereich eines anderen Moduls)");
        return -1;
    }
    match crate::npkfs::rename(&old, &new) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_fs_copy(mem: &mut [u8], ctx: &mut HostState, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
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
    if is_trust_critical_path(&old) || is_trust_critical_path(&new) {
        kprintln!("[npk] WASM: npk_fs_copy DENIED (module/trust store is read-only to apps)");
        return -1;
    }
    // Both sides, as for rename: copying out of another module's private
    // area is the same as reading it.
    if !crate::wasm::private_area_allows(&old, &ctx.module_name)
        || !crate::wasm::private_area_allows(&new, &ctx.module_name) {
        kprintln!("[npk] WASM: npk_fs_copy DENIED (privater Bereich eines anderen Moduls)");
        return -1;
    }
    match crate::npkfs::copy(&old, &new) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub(crate) fn npk_wifi_send_cmd(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NETCTL).is_err() {
        return -1;
    }
    match read_bytes(mem, buf_ptr, len) {
        Some(msg) if crate::wifi::send_cmd(&msg) => 0,
        _ => -1,
    }
}

pub(crate) fn npk_wifi_poll_event(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::NETCTL).is_err() {
        return -1;
    }
    // The manager's own fiber is calling: remember its core so the
    // microvm keeps vCPUs off it (see wifi::note_manager_core).
    crate::wifi::note_manager_core();
    wifi_poll_into(mem, buf_ptr, max, crate::wifi::poll_event)
}

pub(crate) fn npk_wifi_poll_cmd(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    wifi_poll_into(mem, buf_ptr, max, crate::wifi::poll_cmd)
}

pub(crate) fn npk_wifi_send_event(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(msg) if crate::wifi::send_event(&msg) => 0,
        _ => -1,
    }
}

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

pub(crate) fn npk_netdev_submit_rx(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::netdev::wasm_nic_submit_rx(&frame); 0 }
        None => -1,
    }
}

pub(crate) fn npk_netdev_rx_deliver(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::net::wasm_deliver_rx(&frame); 0 }
        None => -1,
    }
}

pub(crate) fn npk_netdev_poll_tx(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if !is_netdev(ctx) { return -1; }
    let mut frame = [0u8; crate::netdev::MTU];
    let len = match crate::netdev::wasm_nic_poll_tx(&mut frame) {
        Some(n) => n,
        None => return -1,
    };
    if max < 0 || (max as usize) < len { return -1; }
    write_bytes(mem, buf_ptr, &frame[..len])
}
