//! Host-Funktionen, motorneutral.
//!
//! Jede Funktion hier arbeitet auf `(&mut HostState, Argumente)` und sonst
//! nichts. Was sich je Motor unterscheidet, ist nur, WIE der Zustand
//! beschafft wird: der Interpreter reicht `caller.data_mut()` durch, der
//! Compiler holt ihn aus dem vmctx. Beide fahren dieselbe Implementierung —
//! zwei Host-Schichten zu vergleichen wuerde die Host-Schichten messen.
//!
//! Hier stehen die Funktionen, die den Gastspeicher NICHT anfassen. Wer ihn
//! braucht, bekommt zusaetzlich `mem: &mut [u8]`.
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

/// Eine UTF-8-Zeichenkette aus dem Gastspeicher. Der Rumpf ist der von
/// `read_wasm_str`, nur ohne den Motor davor.
pub(crate) fn read_str(data: &[u8], ptr: i32, len: i32) -> Option<String> {
    let start = ptr as usize;
    let end = (start + len as usize).min(data.len());
    if start >= end { return None; }
    let mut buf = alloc::vec![0u8; end - start];
    buf.copy_from_slice(&data[start..end]);
    core::str::from_utf8(&buf).ok().map(String::from)
}

/// Bytes aus dem Gastspeicher, oder nichts, wenn der Bereich nicht passt.
pub(crate) fn read_bytes(data: &[u8], ptr: i32, len: i32) -> Option<alloc::vec::Vec<u8>> {
    if ptr < 0 || len <= 0 { return None; }
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    if end > data.len() { return None; }
    Some(data[start..end].to_vec())
}

/// `bytes` in den Gastspeicher an `ptr` schreiben; liefert die Laenge oder -1,
/// wenn der Bereich nicht passt.
pub(crate) fn write_bytes(data: &mut [u8], ptr: i32, bytes: &[u8]) -> i32 {
    if ptr < 0 { return -1; }
    let start = ptr as usize;
    let end = match start.checked_add(bytes.len()) {
        Some(e) => e,
        None => return -1,
    };
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
}

/// Gemeinsamer Rumpf der beiden poll-Funktionen: eine Nachricht aus `dequeue`
/// in den Gastpuffer holen (durch `max` begrenzt), Laenge oder -1.
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

pub(crate) fn npk_theme_token(ctx: &mut HostState, token_id: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return 0;
    }
    // One table, in palette.rs — the local copy here stopped at
    // Danger and silently answered 0 for Page/AccentRing/… long
    // after those tokens existed.
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
        // here. The promote-or-create branch above already focuses,
        // but if the app calls set_overlay a second time (or after
        // some other host fn shifted focus elsewhere) we need to
        // re-claim it so keys don't end up routed at a stale window.
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 { return -1; }

    let mut wid = ctx.widget_window_id;
    if wid == 0 {
        // Promote the spawning terminal to a widget window if there
        // is one; otherwise create a fresh widget window. Unlike
        // the overlay path we do NOT focus it — the dock is a
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if w <= 0 || h <= 0 || edge < 0 || behavior < 0 { return -1; }

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

pub(crate) fn npk_audio_open(_ctx: &mut HostState) -> i32 {
 crate::audio::open() 
}

pub(crate) fn npk_audio_close(_ctx: &mut HostState, slot: i32) -> i32 {
    if slot >= 0 { crate::audio::close(slot as usize); }
    0
}

pub(crate) fn npk_audio_set_volume(_ctx: &mut HostState, pct: i32) -> i32 {
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
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
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

pub(crate) fn npk_sys_info(_ctx: &mut HostState, key: i32) -> i64 {
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
            crate::smp::per_core::core_freq_mhz(core) as i64
        },
        13 => crate::smp::per_core::max_turbo_mhz() as i64,
        14 => crate::smp::per_core::min_eff_mhz() as i64,
        15 => {
            let core = (key >> 8) as usize;
            crate::smp::per_core::core_usage(core) as i64
        },
        // CPUID 0x15 raw values for diagnostics
        16 => { let (eax, _, _) = crate::interrupts::cpuid15(); eax as i64 },
        17 => { let (_, ebx, _) = crate::interrupts::cpuid15(); ebx as i64 },
        18 => { let (_, _, ecx) = crate::interrupts::cpuid15(); ecx as i64 },

        // 19 → raw TSC reading (monotonic, high-resolution).
        // Combine with key=10 (tsc_mhz) to convert ticks → time.
        // 64-bit TSC fits in i64 (sign bit unused for ~150 years
        // at 2 GHz), so the cast is safe.
        19 => unsafe { core::arch::x86_64::_rdtsc() as i64 },

        // ── Process tracking (keys 20-29) → process table ──
        // 20: count, 21: pid_at_index, 22-29: query by PID
        20..=29 => crate::process::sys_info(key),

        // ── Bench probes (keys 30-34) → cached on first call ──
        // 30: BLAKE3 MB/s, 31: AES-GCM enc MB/s,
        // 32: AES-GCM dec(in-place) MB/s,
        // 33: raw blkdev write MB/s, 34: raw blkdev read MB/s.
        // First call across any of these triggers ~100 ms of
        // measurement; results live in BENCH_CACHE until reboot.
        30..=34 => bench_sys_info(key),

        // ── fsck self-check (key 40) → read-only integrity scan ──
        // Runs on every call (NOT cached), logs a full report to
        // serial, and returns the total problem count (0 = clean,
        // -1 = scan error). testdisk calls this at the END of its run
        // so corruption surfaces in-flight — a reboot would brick the
        // mount before we could ever see it.
        40 => fsck_sys_info(),

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
    // one-shot wasm on Core 0) → HLT-idle until the deadline. NO
    // core-stealing helper (that was the nesting hazard).
    let freq = crate::interrupts::tsc_freq();
    let target = crate::interrupts::rdtsc() + (ms as u64) * (freq / 1000);
    let cid = crate::smp::per_core::current_core_id();
    while crate::interrupts::rdtsc() < target {
        let rflags: u64;
        unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags); }
        let t0 = crate::interrupts::rdtsc();
        if rflags & (1 << 9) != 0 {
            // SAFETY: IF=1 already → HLT wakes on the timer IRQ.
            unsafe { core::arch::asm!("hlt"); }
        } else {
            // SAFETY: enable for the HLT, then restore IF=0.
            unsafe { core::arch::asm!("sti; hlt; cli"); }
        }
        crate::smp::per_core::record_halt(
            cid, crate::interrupts::rdtsc().saturating_sub(t0));
        crate::smp::per_core::record_wake(
            cid, crate::smp::per_core::WAKE_NPK_SLEEP);
    }

    0
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

    let result = loop {
        if let Some(k) = pop_app_key(term_idx) {
            break k as i32;
        }
        if crate::interrupts::rdtsc() >= deadline {
            break -1;
        }
        // Don't busy-spin, and don't run the old next_task helper
        // here: stealing a fiber task and calling its func directly
        // would bypass the fiber scheduler and re-introduce the
        // nesting freeze (docs/plan/SCHEDULER_FIBERS.md). Just HLT until the
        // next interrupt; the per-core 100 Hz timer wakes us to
        // re-check the key buffer + deadline (≤10 ms latency).
        // IF-preserving. (Only `wifi` uses npk_input_wait today; the
        // panels use npk_event_poll + npk_sleep, which yields.)
        let rflags: u64;
        unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags); }
        let t0 = crate::interrupts::rdtsc();
        if rflags & (1 << 9) != 0 {
            // SAFETY: IF=1 already → HLT wakes on the timer IRQ.
            unsafe { core::arch::asm!("hlt"); }
        } else {
            // SAFETY: enable for the HLT, then restore IF=0.
            unsafe { core::arch::asm!("sti; hlt; cli"); }
        }
        crate::smp::per_core::record_halt(
            core_id, crate::interrupts::rdtsc().saturating_sub(t0));
        crate::smp::per_core::record_wake(
            core_id, crate::smp::per_core::WAKE_NPK_SLEEP);
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

pub(crate) fn npk_stream_open(_ctx: &mut HostState, idx: i32) -> i32 {
    // -1 = the everything-sink: every write, whichever terminal it was
    // routed to. A remote console bound to one index goes silent as soon
    // as output is redirected elsewhere.
    if idx < 0 {
        return if crate::shade::terminal::stream_open_global() { 0 } else { -1 };
    }
    if crate::shade::terminal::stream_open(idx as usize) { 0 } else { -1 }
}

pub(crate) fn npk_stream_close(_ctx: &mut HostState, idx: i32) -> i32 {
    if idx >= 0 { crate::shade::terminal::stream_close(idx as usize); }
    else { crate::shade::terminal::stream_close_global(); }
    0
}

pub(crate) fn npk_key_inject(_ctx: &mut HostState, byte: i32) -> i32 {
    crate::keyboard::inject_byte((byte & 0xFF) as u8);
    0
}

pub(crate) fn npk_tcp_connect(_ctx: &mut HostState, ip_packed: i32, port: i32) -> i32 {
    let ip = [
        ((ip_packed >> 24) & 0xFF) as u8,
        ((ip_packed >> 16) & 0xFF) as u8,
        ((ip_packed >> 8) & 0xFF) as u8,
        (ip_packed & 0xFF) as u8,
    ];
    if port <= 0 || port > 65535 { return -1; }
    match crate::net::tcp::connect_start(ip, port as u16) {
        Ok(h) => h as i32,
        Err(_) => -1,
    }
}

pub(crate) fn npk_tcp_status(_ctx: &mut HostState, handle: i32) -> i32 {
    if handle < 0 { return -1; }
    crate::net::tcp::connect_status(handle as usize)
}

pub(crate) fn npk_tcp_close(_ctx: &mut HostState, handle: i32) -> i32 {
    if handle >= 0 { let _ = crate::net::tcp::close(handle as usize); }
    0
}

pub(crate) fn npk_debug_target_ip(_ctx: &mut HostState) -> i32 {
    get_debug_target().0 as i32
}

pub(crate) fn npk_debug_target_port(_ctx: &mut HostState) -> i32 {
    get_debug_target().1 as i32
}

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
        && capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
        kprintln!("[npk] WASM: npk_pci_bind DENIED {:04x}:{:04x}", vid, did);
        return -2;
    }
    ctx.hw = Some(HwDriverState {
        pci_addr: dev.addr,
        vendor_id: vid,
        device_id: did,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
    });
    kprintln!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, vid, did);
    0
}

pub(crate) fn npk_pci_bind_class(ctx: &mut HostState, class: i32, subclass: i32) -> i32 {
    let cls = class as u8;
    let sub = subclass as u8;
    let dev = match pci::find_by_class(cls, sub) {
        Some(d) => d,
        None => return -1,
    };
    let cap_id = ctx.cap_id;
    let a = dev.addr;
    if capability::check_pci_device(&cap_id, capability::Rights::EXECUTE, a.bus, a.device, a.function).is_err()
        && capability::check_global(&cap_id, capability::Rights::EXECUTE).is_err() {
        kprintln!("[npk] WASM: npk_pci_bind_class DENIED {:02x}:{:02x}", cls, sub);
        return -2;
    }
    kprintln!("[npk] WASM driver bound to {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        a.bus, a.device, a.function, dev.vendor_id, dev.device_id);
    ctx.hw = Some(HwDriverState {
        pci_addr: dev.addr,
        vendor_id: dev.vendor_id,
        device_id: dev.device_id,
        mmio_maps: Vec::new(),
        dma_allocs: Vec::new(),
        bus_master_enabled: false,
        registered_as_netdev: false,
    });
    0
}

pub(crate) fn npk_pci_read_config(ctx: &mut HostState, offset: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    if offset < 0 || offset > 255 { return -1; }
    pci::read32(hw.pci_addr, offset as u8) as i32
}

pub(crate) fn npk_pci_write_config(ctx: &mut HostState, offset: i32, value: i32) -> i32 {
    let hw = match ctx.hw.as_ref() {
        Some(h) => h,
        None => return -1,
    };
    if offset < 0 || offset > 255 { return -1; }
    pci::write32(hw.pci_addr, offset as u8, value as u32);
    0
}

pub(crate) fn npk_pci_enable_bus_master(ctx: &mut HostState) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    pci::enable_bus_master(hw.pci_addr);
    // Also enable memory space
    let cmd = pci::read32(hw.pci_addr, 0x04);
    pci::write32(hw.pci_addr, 0x04, cmd | 0x06);
    hw.bus_master_enabled = true;
    0
}

pub(crate) fn npk_irq_register(ctx: &mut HostState, entry: i32) -> i32 {
    let hw = match ctx.hw.as_ref() { Some(h) => h, None => return -1 };
    if !(0..2048).contains(&entry) { return -1; }
    match crate::irq::register(hw.pci_addr, entry as u16) {
        Some(v) => v as i32,
        None => -1,
    }
}

pub(crate) fn npk_irq_arm(_ctx: &mut HostState, vector: i32) -> i64 {
    let base = crate::interrupts::DEVICE_IRQ_VEC_BASE as i32;
    let count = crate::interrupts::DEVICE_IRQ_VEC_COUNT as i32;
    if vector < base || vector >= base + count { return -1; }
    crate::irq::arm(vector as u8) as i64
}

pub(crate) fn npk_irq_wait(_ctx: &mut HostState, vector: i32, since: i64, timeout_ms: i32) -> i32 {
    let base = crate::interrupts::DEVICE_IRQ_VEC_BASE as i32;
    let count = crate::interrupts::DEVICE_IRQ_VEC_COUNT as i32;
    if vector < base || vector >= base + count || since < 0 { return -1; }
    let t = if timeout_ms <= 0 { 1000 } else { timeout_ms as u64 };
    if crate::irq::wait(vector as u8, since as u64, t) { 1 } else { 0 }
}

pub(crate) fn npk_mmio_map_bar(ctx: &mut HostState, bar_idx: i32, pages: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
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
    kprintln!("[npk] WASM driver: MMIO BAR{} mapped at {:#x} — BAR size {:#x}, requested {} pages, mapped {} pages",
        bar_idx, bar_base, bar_size, requested, page_count);
    handle as i32
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
    if off + 4 > pages * 4096 { return -1; }
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
    if off + 4 > pages * 4096 { return -1; }
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
    if off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
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
    if off + 2 > pages * 4096 || off & 0x1 != 0 { return -1; }
    // SAFETY: validated MMIO region within mapped BAR, 2-byte aligned
    unsafe { core::ptr::write_volatile((base + off as u64) as *mut u16, value as u16) }
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
    if off + 8 > pages * 4096 { return -1; }
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
    if off + 8 > pages * 4096 { return -1; }
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

    // DMA buffers MUST be below 4GB — PCIe TX BD has 32-bit address field
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
    if off + 4 > pages * 4096 { return -1; }
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
    if off + 4 > pages * 4096 { return -1; }
    // SAFETY: writing to validated DMA buffer
    unsafe { core::ptr::write_volatile((phys + off as u64) as *mut u32, value as u32) }
    0
}

pub(crate) fn npk_memory_fence(_ctx: &mut HostState) -> i32 {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    0
}

pub(crate) fn npk_netdev_set_link(ctx: &mut HostState, up: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
    crate::netdev::set_wasm_nic_link(up != 0);
    0
}

pub(crate) fn npk_netdev_set_link_state(ctx: &mut HostState, carrier: i32, dormant: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
    crate::netdev::set_wasm_nic_link_state(carrier != 0, dormant != 0);
    0
}

pub(crate) fn npk_fetch(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if let Err(e) = capability::check_global(&cap_id, capability::Rights::READ) {
        kprintln!("[npk] WASM: npk_fetch DENIED (cap_id={:08x}, {:?})",
            capability::short_id(&cap_id), e);
        return -1;
    }

    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };

    let (content, _) = match crate::npkfs::fetch(&name) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    let write_len = content.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let result = if start + write_len <= data.len() {
        data[start..start + write_len].copy_from_slice(&content[..write_len]);
        write_len as i32
    } else {
        -1
    };

    result
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
    let n = hdrs.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    // checked_add: a wrapping `start + n` would panic the KERNEL on
    // the slice index — a guest-triggerable halt.
    match start.checked_add(n) {
        Some(end) if end <= data.len() => {
            data[start..end].copy_from_slice(&hdrs.as_bytes()[..n]);
            n as i32
        }
        _ => -1,
    }
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
    let n = url.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    // checked_add: a wrapping `start + n` would produce start > end and
    // panic the KERNEL on the slice index — a guest-triggerable halt.
    match start.checked_add(n) {
        Some(end) if end <= data.len() => {
            data[start..end].copy_from_slice(&url.as_bytes()[..n]);
            n as i32
        }
        _ => -1,
    }
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
    let n = ct.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    // checked_add: a wrapping `start + n` would produce start > end and
    // panic the KERNEL on the slice index — a guest-triggerable halt.
    match start.checked_add(n) {
        Some(end) if end <= data.len() => {
            data[start..end].copy_from_slice(&ct.as_bytes()[..n]);
            n as i32
        }
        _ => -1,
    }
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
    let n = err.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    // checked_add: a wrapping `start + n` would produce start > end and
    // panic the KERNEL on the slice index — a guest-triggerable halt.
    match start.checked_add(n) {
        Some(end) if end <= data.len() => {
            data[start..end].copy_from_slice(&err.as_bytes()[..n]);
            n as i32
        }
        _ => -1,
    }
}

pub(crate) fn npk_store(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    let name = match read_str(mem, name_ptr, name_len) {
        Some(s) => s,
        None => return -1,
    };

    // Apps may not write the module store or the trust store — see
    // is_trust_critical_path.
    // Checked BEFORE the grant path so a per-file grant can never
    // become a way in there.
    if is_trust_critical_path(&name) {
        kprintln!("[npk] WASM: npk_store DENIED ({} is read-only to apps)", name);
        return -1;
    }

    // Three ways to be allowed to write, narrowest last:
    //   1. blanket WRITE from `.npk.caps`
    //   2. a grant for exactly this file — what the user handed over
    //      by picking the path in a trusted dialog
    //   3. the app's OWN settings file, `sys/config/<module>`. An
    //      app that keeps preferences shouldn't need write access to
    //      the whole store for it, and the name is the kernel's to
    //      derive — a module can't claim someone else's.
    let own_config = alloc::format!("sys/config/{}", ctx.module_name);
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err()
        && !capability::check_path_grant(&cap_id, &name, capability::Rights::WRITE)
        && name != own_config
    {
        kprintln!("[npk] WASM: npk_store DENIED (no WRITE, no grant for {})", name);
        return -1;
    }

    let data = &*mem;
    let start = data_ptr as usize;
    let end = (start + data_len as usize).min(data.len());
    if start >= end { return -1; }

    // Insert-or-replace: apps with state to persist (panel
    // configs, etc.) re-write the same key on every change. The
    // strict-create `store` would fail on the second write and
    // leave the app's state diverging from disk.
    match crate::npkfs::upsert(&name, &data[start..end], cap_id) {
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
    if bytes.len() > buf_max as usize { return -1; }
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + bytes.len();
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
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
    if bytes.len() > buf_max as usize { return -1; }
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + bytes.len();
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(bytes);
    bytes.len() as i32
}

pub(crate) fn npk_clipboard_set(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if !app_is_focused(ctx) { return -1; }
    let data = &*mem;
    let start = ptr as usize;
    let end = (start + len.max(0) as usize).min(data.len());
    if start > end { return -1; }
    let slice = &data[start..end];
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
    let data = &mut *mem;
    let start = ptr as usize;
    let end = start + n;
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&text[..n]);
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

    let (bytes_start, bytes_end) = {
        let data = &*mem;
        let start = ptr as usize;
        let end = start.saturating_add(len as usize).min(data.len());
        if start >= end { return -1; }
        (start, end)
    };

    // Heap copy, 200–600 bytes for a typical tree. The borrow checker
    // used to force it; with `mem` a parameter it no longer does.
    let payload: alloc::vec::Vec<u8> = mem[bytes_start..bytes_end].to_vec();

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
    let pixel_bytes = (width as usize) * (height as usize) * 4;
    if len as usize != pixel_bytes { return -1; }
    let data = &*mem;
    let start = ptr as usize;
    let end = start + pixel_bytes;
    if end > data.len() { return -1; }
    let px = data[start..end].to_vec();
    if !crate::shade::widgets::canvas::commit(
        wid, canvas_id as u32, width as u32, height as u32, px) {
        return -1;
    }
    crate::shade::widgets::rerender_window(wid);
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
    let data = &mut *mem;
    let start = out_ptr as usize;
    if start + 16 > data.len() {
        return -1;
    }
    data[start..start + 4].copy_from_slice(&x.to_le_bytes());
    data[start + 4..start + 8].copy_from_slice(&y.to_le_bytes());
    data[start + 8..start + 12].copy_from_slice(&(w as i32).to_le_bytes());
    data[start + 12..start + 16].copy_from_slice(&(h as i32).to_le_bytes());
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
    // read it, yielding a stale background-only frame — the
    // reason an earlier shadow capture missed all the windows.
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
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + need;
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&tmp);
    need as i32
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

    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + encoded.len();
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&encoded);
    encoded.len() as i32
}

pub(crate) fn npk_list_modules(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, buf_max: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }

    // v2: `sys/wasm` is a real directory. List immediate children
    // directly instead of scanning + prefix-filtering the whole tree.
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

    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + out.len();
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&out);
    out.len() as i32
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

    let write_len = meta.len().min(buf_max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    if start + write_len > data.len() { return -1; }
    data[start..start + write_len].copy_from_slice(&meta[..write_len]);
    write_len as i32
}

pub(crate) fn npk_spawn_module(mem: &mut [u8], ctx: &mut HostState, name_ptr: i32, name_len: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::RENDER).is_err() {
        return -1;
    }
    if name_len <= 0 || name_len > 64 { return -1; }

    let name = {
        let data = &*mem;
        let start = name_ptr as usize;
        let end = start + name_len as usize;
        if end > data.len() { return -1; }
        match core::str::from_utf8(&data[start..end]) {
            Ok(s) => alloc::string::String::from(s),
            Err(_) => return -1,
        }
    };

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
    let verb = {
        let data = &*mem;
        let start = verb_ptr as usize;
        let end = start + verb_len as usize;
        if end > data.len() { return -1; }
        match core::str::from_utf8(&data[start..end]) {
            Ok(s) => alloc::string::String::from(s),
            Err(_) => return -1,
        }
    };
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
    let write_len = bytes.len().min(max as usize);
    let data = &mut *mem;
    let start = buf_ptr as usize;
    if start + write_len <= data.len() {
        data[start..start + write_len].copy_from_slice(&bytes[..write_len]);
        write_len as i32
    } else {
        -1
    }
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
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start + len;
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(src);
    len as i32
}

pub(crate) fn npk_audio_submit(mem: &mut [u8], _ctx: &mut HostState, slot: i32, ptr: i32, len: i32) -> i32 {
    if slot < 0 || ptr < 0 || len < 0 { return -1; }
    let data = &*mem;
    let (start, end) = (ptr as usize, ptr as usize + len as usize);
    if end > data.len() { return -1; }
    crate::audio::submit(slot as usize, &data[start..end]) as i32
}

pub(crate) fn npk_audio_poll_mix(mem: &mut [u8], _ctx: &mut HostState, ptr: i32, max: i32) -> i32 {
    if ptr < 0 || max < 0 { return -1; }
    let data = &mut *mem;
    let (start, end) = (ptr as usize, ptr as usize + max as usize);
    if end > data.len() { return -1; }
    crate::audio::poll_mix(&mut data[start..end]) as i32
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

    // v2: directories are real Tree objects, listings come straight
    // from them — no scan + prefix-filter pass.
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let prefix_for_list = prefix.trim_matches('/');

    if recursive == 0 {
        // Non-recursive: one directory's immediate children.
        let entries = match crate::npkfs::fs::list(prefix_for_list) {
            Ok(Some(v)) => v,
            Ok(None) => alloc::vec::Vec::new(),
            Err(_) => return -1,
        };
        for e in &entries {
            let is_dir = matches!(e.kind, crate::npkfs::object::EntryKind::Dir);
            append_entry(&mut out, &e.name, e.size, is_dir, e.mtime);
        }
    } else {
        // Recursive: DFS the subtree, emit relative paths.
        fn dfs(
            base: &str, rel: alloc::string::String,
            out: &mut alloc::vec::Vec<u8>,
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
                match e.kind {
                    crate::npkfs::object::EntryKind::File => {
                        append_entry(out, &child_rel, e.size, false, e.mtime);
                    }
                    crate::npkfs::object::EntryKind::Dir => {
                        append_entry(out, &child_rel, 0, true, e.mtime);
                        dfs(base, child_rel, out)?;
                    }
                }
            }
            Ok(())
        }
        if dfs(prefix_for_list, alloc::string::String::new(), &mut out).is_err() {
            return -1;
        }
    }

    if out.len() > out_cap as usize { return -1; }

    let data = &mut *mem;
    let start = out_ptr as usize;
    let end = start + out.len();
    if end > data.len() { return -1; }
    data[start..end].copy_from_slice(&out);
    out.len() as i32
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
    let data = &mut *mem;
    let start = out_ptr as usize;
    if start + 17 > data.len() { return -1; }
    data[start..start + 17].copy_from_slice(&buf);
    17
}

pub(crate) fn npk_set_wallpaper(mem: &mut [u8], ctx: &mut HostState, ptr: i32, len: i32, width: i32, height: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
        kprintln!("[npk] WASM: npk_set_wallpaper DENIED (no WRITE)");
        return -1;
    }

    let data = &*mem;
    let start = ptr as usize;
    let pixel_bytes = (width as usize) * (height as usize) * 4;
    let end = start + pixel_bytes;
    if end > data.len() || end > len as usize + start { return -1; }

    let info = crate::framebuffer::get_info();
    crate::gui::background::set_wallpaper(
        &data[start..end], width as u32, height as u32, &info);

    // Force compositor full redraw
    crate::shade::force_redraw();
    kprintln!("[npk] Wallpaper set ({}x{}, theme extracted)", width, height);
    0
}

pub(crate) fn npk_set_theme(mem: &mut [u8], ctx: &mut HostState, ptr: i32) -> i32 {
    let cap_id = ctx.cap_id;
    if capability::check_global(&cap_id, capability::Rights::WRITE).is_err() {
        return -1;
    }

    let data = &*mem;
    let start = ptr as usize;
    if start + 64 > data.len() { return -1; }

    let mut colors = [0u32; 16];
    for i in 0..16 {
        let off = start + i * 4;
        colors[i] = u32::from_le_bytes([
            data[off], data[off + 1], data[off + 2], data[off + 3],
        ]);
    }
    crate::theme::set_palette(&colors);
    crate::shade::force_redraw();
    0
}

pub(crate) fn npk_stream_read(mem: &mut [u8], _ctx: &mut HostState, idx: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    if buf_len <= 0 { return 0; }
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start.saturating_add(buf_len as usize);
    if end > data.len() { return -1; }
    if idx < 0 {
        crate::shade::terminal::stream_read_global(&mut data[start..end]) as i32
    } else {
        crate::shade::terminal::stream_read(idx as usize, &mut data[start..end]) as i32
    }
}

pub(crate) fn npk_tcp_send(mem: &mut [u8], _ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    if handle < 0 || buf_len <= 0 { return -1; }
    let data = &*mem;
    let start = buf_ptr as usize;
    let end = start.saturating_add(buf_len as usize);
    if end > data.len() { return -1; }
    match crate::net::tcp::send(handle as usize, &data[start..end]) {
        Ok(_) => 0,
        // Backpressure, not a failure: too much is still unacked.
        // A module that treats this as fatal drops a live connection.
        Err(crate::net::tcp::TcpError::WouldBlock) => -2,
        Err(_) => -1,
    }
}

pub(crate) fn npk_tcp_recv(mem: &mut [u8], _ctx: &mut HostState, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    if handle < 0 || buf_max <= 0 { return -1; }
    let data = &mut *mem;
    let start = buf_ptr as usize;
    let end = start.saturating_add(buf_max as usize);
    if end > data.len() { return -1; }
    match crate::net::tcp::recv(handle as usize, &mut data[start..end]) {
        Ok(n) => n as i32,
        Err(_) => -1,
    }
}

pub(crate) fn npk_dma_read(mem: &mut [u8], ctx: &mut HostState, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    let (phys, pages) = {
        let hw = match ctx.hw.as_ref() {
            Some(h) => h,
            None => return -1,
        };
        let h = handle as usize;
        if h >= hw.dma_allocs.len() { return -1; }
        hw.dma_allocs[h]
    };
    let off = dma_off as usize;
    let length = len as usize;
    if off + length > pages * 4096 { return -1; }

    let data = &mut *mem;
    let dst = wasm_ptr as usize;
    if dst + length > data.len() { return -1; }
    // SAFETY: copying from validated DMA buffer to WASM linear memory
    unsafe {
        core::ptr::copy_nonoverlapping(
            (phys + off as u64) as *const u8,
            data[dst..].as_mut_ptr(),
            length,
        );
    }
    0
}

pub(crate) fn npk_dma_write(mem: &mut [u8], ctx: &mut HostState, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    let (phys, pages) = {
        let hw = match ctx.hw.as_ref() {
            Some(h) => h,
            None => return -1,
        };
        let h = handle as usize;
        if h >= hw.dma_allocs.len() { return -1; }
        hw.dma_allocs[h]
    };
    let off = dma_off as usize;
    let length = len as usize;
    if off + length > pages * 4096 { return -1; }

    let data = &*mem;
    let src = wasm_ptr as usize;
    if src + length > data.len() { return -1; }
    // SAFETY: copying from WASM linear memory to validated DMA buffer
    unsafe {
        core::ptr::copy_nonoverlapping(
            data[src..].as_ptr(),
            (phys + off as u64) as *mut u8,
            length,
        );
    }
    0
}

pub(crate) fn npk_netdev_register(mem: &mut [u8], ctx: &mut HostState, mac_ptr: i32) -> i32 {
    let hw = match ctx.hw.as_mut() {
        Some(h) => h,
        None => return -1,
    };
    if hw.registered_as_netdev { return -1; } // already registered

    let data = &*mem;
    let start = mac_ptr as usize;
    if start + 6 > data.len() { return -1; }
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&data[start..start + 6]);

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

pub(crate) fn npk_log(mem: &mut [u8], _ctx: &mut HostState, ptr: i32, len: i32) {
    if let Some(s) = read_str(mem, ptr, len) {
        kprintln!("{}", s);
    }
}

pub(crate) fn npk_log_serial(mem: &mut [u8], _ctx: &mut HostState, ptr: i32, len: i32) {
    if let Some(s) = read_str(mem, ptr, len) {
        {
            let serial = crate::drivers::serial::SERIAL.lock();
            for byte in s.bytes() {
                if byte == b'\n' { serial.write_byte(b'\r'); }
                serial.write_byte(byte);
            }
            serial.write_byte(b'\r');
            serial.write_byte(b'\n');
        }
        // ...and to the remote mirror, which was blind to every app
        // that logs this way. Outside the SERIAL lock: the sink takes
        // its own, and holding two is how a deadlock is built.
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
            // The document itself: 4,1x-9,9x fewer bytes on the wire,
            // measured (docs/plan/JS_SCOPE_CONTENT_WEB.md §8).
            accept_gzip: tls,
            // And over HTTP/2: this is the request Wikimedia
            // throttles, four of them per page load (BROWSER.md §8.1).
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
    // wrapping `start + len` would panic the KERNEL on the slice index.
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
    // everything after it is read as a SECOND one. This is the check
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
        // Alles, was ueber die WASM-Grenze kommt, ist Seitencode — auch
        // wenn beak es weiterreicht. Die Reichweite steht am Kontext, nicht
        // an der Anfrage, und der Kernel hat sie selbst ausgerechnet.
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

/// Start one request. Returns a handle (>= 1), or -1 with the reason in
/// `npk_http_last_error`. Same validation as `npk_http_send`: it is the same
/// request, only nobody waits for it here.
/// Den Reichweiten-Kontext dieses Moduls setzen.
///
/// **Der Kernel glaubt dem Modul den Namen, aber nicht die Klasse.** beak
/// reicht die Adresse des Dokuments ein; welcher Netzbereich das ist,
/// rechnet diese Funktion selbst aus — sonst waere die Grenze eine, die das
/// Modul im Sandkasten selbst zieht, und das ist keine.
///
/// Eine Adresse ohne Herkunft (`beak:selftest`, `about:blank`) und alles,
/// was sich nicht aufloesen laesst, faellt auf `Public` zurueck: die
/// strengste Klasse, nicht die bequemste.
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
pub(crate) fn npk_http_begin_many(
    mem: &mut [u8], ctx: &mut HostState,
    urls_ptr: i32, urls_len: i32, out_max: i32,
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
    match crate::intent::fetch::begin_many(ctx.pid, ctx.core_id, urls, out_max as usize,
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
    // Asked BEFORE taking: `take` destroys the job, so a length table too
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
    // Refused, not truncated. The length table describes the WHOLE blob, so a
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
    let bodies = crate::intent::http::https_get_many(&urls, total_cap, Some(ctx.net_reach));

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
            if let Some(cap) = crate::shade::widgets::window_cap(id.0) {
                capability::grant_path(cap, &arg_str,
                    capability::Rights::READ | capability::Rights::WRITE);
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
    // Launching an app ON a file is the user pointing at it — grant
    // that one path so the app can save it back without holding
    // WRITE over the whole store.
    if let Some(a) = arg.as_deref() {
        capability::grant_path(module_cap, a,
            capability::Rights::READ | capability::Rights::WRITE);
    }
    // Create the widget window NOW (synchronously, titled with the
    // module name) instead of lazily on first scene_commit. The
    // app spawns asynchronously, so without this a rapid second
    // open (e.g. a double-click = two opens) would see no window
    // yet and spawn a DUPLICATE instance. Pre-creating lets the
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
    if ctx.hw.is_none() { return -1; }
    wifi_poll_into(mem, buf_ptr, max, crate::wifi::poll_cmd)
}

pub(crate) fn npk_wifi_send_event(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
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
    if ctx.hw.is_none() { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::netdev::wasm_nic_submit_rx(&frame); 0 }
        None => -1,
    }
}

pub(crate) fn npk_netdev_rx_deliver(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, len: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
    match read_bytes(mem, buf_ptr, len) {
        Some(frame) => { crate::net::wasm_deliver_rx(&frame); 0 }
        None => -1,
    }
}

pub(crate) fn npk_netdev_poll_tx(mem: &mut [u8], ctx: &mut HostState, buf_ptr: i32, max: i32) -> i32 {
    if ctx.hw.is_none() { return -1; }
    let mut frame = [0u8; crate::netdev::MTU];
    let len = match crate::netdev::wasm_nic_poll_tx(&mut frame) {
        Some(n) => n,
        None => return -1,
    };
    if max < 0 || (max as usize) < len { return -1; }
    write_bytes(mem, buf_ptr, &frame[..len])
}
