//! Die forge-Seite der Host-Bruecke.
//!
//! Ein Adapter je Host-Funktion, und jeder tut genau zwei Dinge: `mem` und
//! `ctx` aus dem vmctx holen und `host_core` rufen. Der wasmi-Adapter in
//! `wasm.rs` holt dieselben zwei anders. Alles darunter ist EINE
//! Implementierung — zwei Host-Schichten zu vergleichen wuerde die
//! Host-Schichten messen, nicht die Compiler.
//!
//! Die Aufrufform gibt der Generator vor: `rdi` = vmctx, ganzzahlige
//! Argumente ab `rsi`, Rueckgabe in `rax`. Das ist SysV, also passt
//! `extern "C"` ohne Zutun — auch fuer die drei mit mehr als fuenf
//! Argumenten, die dadurch ueber den Stapel gehen.
//!
//! DIESE DATEI IST ERZEUGT. Sie folgt den Signaturen in `host_core.rs`.

use super::host_core;
use super::HostState;
use forge_core::vmctx;

/// Der Zustand, den diese Instanz gehoert. Der Zeiger steht im vmctx, nicht in
/// einem `static` — zwei Module auf zwei Kernen haetten sich einen `static`
/// geteilt.
///
/// # Safety
/// `vm` muss der vmctx einer Instanz sein, die ueber `NpkHost` gebaut wurde;
/// nur dann ist `HOST_CTX` ein gueltiger `HostState`.
unsafe fn ctx_of<'a>(vm: *const u64) -> &'a mut HostState {
    unsafe { &mut *(*vm.add(vmctx::HOST_CTX as usize / 8) as *mut HostState) }
}

/// Gastspeicher und Zustand. Beide werden bei JEDEM Aufruf neu gelesen: die
/// Basis bewegt sich nie, aber `memory.grow` verschiebt das Ende.
///
/// # Safety
/// Wie `ctx_of`, und `MEM_BASE`/`MEM_SIZE` muessen die Reservierung dieser
/// Instanz beschreiben.
pub(crate) unsafe fn parts<'a>(vm: *const u64) -> (&'a mut [u8], &'a mut HostState) {
    unsafe {
        let base = *vm.add(vmctx::MEM_BASE as usize / 8);
        let size = *vm.add(vmctx::MEM_SIZE as usize / 8) as usize;
        // Ein Modul ohne Speicher bekommt eine LEERE Scheibe, keine mit
        // Nullzeiger: `read_str` und die anderen antworten darauf schon mit
        // "ausserhalb", also braucht kein Adapter einen Sonderfall.
        let mem = if base == 0 {
            core::slice::from_raw_parts_mut(core::ptr::NonNull::<u8>::dangling().as_ptr(), 0)
        } else {
            core::slice::from_raw_parts_mut(base as *mut u8, size)
        };
        (mem, ctx_of(vm))
    }
}

/// Was `forge_rt` fragen muss, um die Tabelle zu fuellen.
///
/// Steht bereit, faehrt aber noch niemand: den Ausfuehrungspfad gibt es erst,
/// wenn `install` uebersetzt und den Codeblob ablegt.
#[allow(dead_code)]
pub(crate) struct NpkHost(pub(crate) *mut HostState);

impl crate::forge_rt::HostImports for NpkHost {
    fn ctx_ptr(&self) -> u64 {
        self.0 as u64
    }
    fn resolve(&self, module: &str, name: &str) -> Option<u64> {
        // Ein Modul kann beide ABIs importieren — beak nur `env`, python nur
        // wasi. Der Host beantwortet deshalb beide, statt dass der Aufrufer
        // sich einen aussuchen muesste.
        resolve(module, name).or_else(|| crate::wasi::forge_glue::resolve(module, name))
    }
}

extern "C" fn f_npk_http_status(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_http_status(ctx)
}

extern "C" fn f_npk_fs_usage(vm: *const u64) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_fs_usage(ctx)
}

extern "C" fn f_npk_clipboard_len(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_clipboard_len(ctx)
}

extern "C" fn f_npk_window_set_close_guard(vm: *const u64, on: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_close_guard(ctx, on)
}

extern "C" fn f_npk_screen_size(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_screen_size(ctx)
}

extern "C" fn f_npk_ticks(vm: *const u64) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_ticks(ctx)
}

extern "C" fn f_npk_now_us(vm: *const u64) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_now_us(ctx)
}

extern "C" fn f_npk_unix_time(vm: *const u64) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_unix_time(ctx)
}

extern "C" fn f_npk_theme_token(vm: *const u64, token_id: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_theme_token(ctx, token_id)
}

extern "C" fn f_npk_cursor_pos(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_cursor_pos(ctx)
}

extern "C" fn f_npk_screen_flash(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_screen_flash(ctx)
}

extern "C" fn f_npk_window_set_overlay(vm: *const u64, w: i32, h: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_overlay(ctx, w, h)
}

extern "C" fn f_npk_window_set_modal(vm: *const u64, modal: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_modal(ctx, modal)
}

extern "C" fn f_npk_window_set_overlay_at(vm: *const u64, x: i32, y: i32, w: i32, h: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_overlay_at(ctx, x, y, w, h)
}

extern "C" fn f_npk_window_set_light_dismiss(vm: *const u64, on: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_light_dismiss(ctx, on)
}

extern "C" fn f_npk_window_set_clipboard_sink(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_clipboard_sink(ctx)
}

extern "C" fn f_npk_window_set_dock(vm: *const u64, w: i32, h: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_dock(ctx, w, h)
}

extern "C" fn f_npk_window_set_panel(vm: *const u64, edge: i32, behavior: i32, w: i32, h: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_window_set_panel(ctx, edge, behavior, w, h)
}

extern "C" fn f_npk_battery(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_battery(ctx)
}

extern "C" fn f_npk_ec_read(vm: *const u64, addr: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_ec_read(ctx, addr)
}

extern "C" fn f_npk_ec_write(vm: *const u64, addr: i32, val: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_ec_write(ctx, addr, val)
}

extern "C" fn f_npk_battery_report(vm: *const u64, packed: i32) {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_battery_report(ctx, packed)
}

extern "C" fn f_npk_audio_open(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_audio_open(ctx)
}

extern "C" fn f_npk_audio_close(vm: *const u64, slot: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_audio_close(ctx, slot)
}

extern "C" fn f_npk_audio_set_volume(vm: *const u64, pct: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_audio_set_volume(ctx, pct)
}

extern "C" fn f_npk_audio_get_volume(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_audio_get_volume(ctx)
}

extern "C" fn f_npk_workspace_switch(vm: *const u64, n: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_workspace_switch(ctx, n)
}

extern "C" fn f_npk_power(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_power(ctx)
}

extern "C" fn f_npk_close_widget(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_close_widget(ctx)
}

extern "C" fn f_npk_get_fb_size(vm: *const u64) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_get_fb_size(ctx)
}

extern "C" fn f_npk_sys_info(vm: *const u64, key: i32) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_sys_info(ctx, key)
}

extern "C" fn f_npk_sleep(vm: *const u64, ms: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_sleep(ctx, ms)
}

extern "C" fn f_npk_input_poll(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_input_poll(ctx)
}

extern "C" fn f_npk_input_wait(vm: *const u64, timeout_ms: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_input_wait(ctx, timeout_ms)
}

extern "C" fn f_npk_clear(vm: *const u64) {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_clear(ctx)
}

extern "C" fn f_npk_self_terminal(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_self_terminal(ctx)
}

extern "C" fn f_npk_stream_open(vm: *const u64, idx: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_stream_open(ctx, idx)
}

extern "C" fn f_npk_stream_close(vm: *const u64, idx: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_stream_close(ctx, idx)
}

extern "C" fn f_npk_key_inject(vm: *const u64, byte: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_key_inject(ctx, byte)
}

extern "C" fn f_npk_tcp_connect(vm: *const u64, ip_packed: i32, port: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_tcp_connect(ctx, ip_packed, port)
}

extern "C" fn f_npk_tcp_status(vm: *const u64, handle: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_tcp_status(ctx, handle)
}

extern "C" fn f_npk_tcp_close(vm: *const u64, handle: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_tcp_close(ctx, handle)
}

extern "C" fn f_npk_debug_target_ip(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_debug_target_ip(ctx)
}

extern "C" fn f_npk_debug_target_port(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_debug_target_port(ctx)
}

extern "C" fn f_npk_pci_bind(vm: *const u64, vendor: i32, device: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_pci_bind(ctx, vendor, device)
}

extern "C" fn f_npk_pci_bind_class(vm: *const u64, class: i32, subclass: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_pci_bind_class(ctx, class, subclass)
}

extern "C" fn f_npk_pci_read_config(vm: *const u64, offset: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_pci_read_config(ctx, offset)
}

extern "C" fn f_npk_pci_write_config(vm: *const u64, offset: i32, value: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_pci_write_config(ctx, offset, value)
}

extern "C" fn f_npk_pci_enable_bus_master(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_pci_enable_bus_master(ctx)
}

extern "C" fn f_npk_irq_register(vm: *const u64, entry: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_irq_register(ctx, entry)
}

extern "C" fn f_npk_irq_arm(vm: *const u64, vector: i32) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_irq_arm(ctx, vector)
}

extern "C" fn f_npk_irq_wait(vm: *const u64, vector: i32, since: i64, timeout_ms: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_irq_wait(ctx, vector, since, timeout_ms)
}

extern "C" fn f_npk_mmio_map_bar(vm: *const u64, bar_idx: i32, pages: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_map_bar(ctx, bar_idx, pages)
}

extern "C" fn f_npk_mmio_read32(vm: *const u64, handle: i32, offset: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_read32(ctx, handle, offset)
}

extern "C" fn f_npk_mmio_write32(vm: *const u64, handle: i32, offset: i32, value: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_write32(ctx, handle, offset, value)
}

extern "C" fn f_npk_mmio_read16(vm: *const u64, handle: i32, offset: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_read16(ctx, handle, offset)
}

extern "C" fn f_npk_mmio_write16(vm: *const u64, handle: i32, offset: i32, value: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_write16(ctx, handle, offset, value)
}

extern "C" fn f_npk_mmio_read64(vm: *const u64, handle: i32, offset: i32) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_read64(ctx, handle, offset)
}

extern "C" fn f_npk_mmio_write64(vm: *const u64, handle: i32, offset: i32, value: i64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_mmio_write64(ctx, handle, offset, value)
}

extern "C" fn f_npk_dma_alloc(vm: *const u64, pages: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_dma_alloc(ctx, pages)
}

extern "C" fn f_npk_dma_phys_addr(vm: *const u64, handle: i32) -> i64 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_dma_phys_addr(ctx, handle)
}

extern "C" fn f_npk_dma_read32(vm: *const u64, handle: i32, offset: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_dma_read32(ctx, handle, offset)
}

extern "C" fn f_npk_dma_write32(vm: *const u64, handle: i32, offset: i32, value: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_dma_write32(ctx, handle, offset, value)
}

extern "C" fn f_npk_memory_fence(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_memory_fence(ctx)
}

extern "C" fn f_npk_netdev_set_link(vm: *const u64, up: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_netdev_set_link(ctx, up)
}

extern "C" fn f_npk_netdev_set_link_state(vm: *const u64, carrier: i32, dormant: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_netdev_set_link_state(ctx, carrier, dormant)
}

extern "C" fn f_npk_fetch(vm: *const u64, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fetch(mem, ctx, name_ptr, name_len, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_response_headers(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_response_headers(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_final_url(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_final_url(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_content_type(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_content_type(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_last_error(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_last_error(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_store(vm: *const u64, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_store(mem, ctx, name_ptr, name_len, data_ptr, data_len)
}

extern "C" fn f_npk_home_dir(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_home_dir(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_locale(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_locale(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_launch_arg(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_launch_arg(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_clipboard_set(vm: *const u64, ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_clipboard_set(mem, ctx, ptr, len)
}

extern "C" fn f_npk_clipboard_get(vm: *const u64, ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_clipboard_get(mem, ctx, ptr, max)
}

extern "C" fn f_npk_scene_commit(vm: *const u64, ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_scene_commit(mem, ctx, ptr, len)
}

extern "C" fn f_npk_canvas_commit(vm: *const u64, canvas_id: i32, ptr: i32, len: i32, width: i32, height: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_canvas_commit(mem, ctx, canvas_id, ptr, len, width, height)
}

extern "C" fn f_npk_canvas_rect(vm: *const u64, canvas_id: i32, out_ptr: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_canvas_rect(mem, ctx, canvas_id, out_ptr)
}

extern "C" fn f_npk_capture_screen(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_capture_screen(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_event_poll(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_event_poll(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_list_modules(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_list_modules(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_app_meta(vm: *const u64, name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_app_meta(mem, ctx, name_ptr, name_len, buf_ptr, buf_max)
}

extern "C" fn f_npk_spawn_module(vm: *const u64, name_ptr: i32, name_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_spawn_module(mem, ctx, name_ptr, name_len)
}

extern "C" fn f_npk_run_intent(vm: *const u64, verb_ptr: i32, verb_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_run_intent(mem, ctx, verb_ptr, verb_len)
}

extern "C" fn f_npk_bar_state(vm: *const u64, buf_ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_bar_state(mem, ctx, buf_ptr, max)
}

extern "C" fn f_npk_window_titles(vm: *const u64, buf_ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_window_titles(mem, ctx, buf_ptr, max)
}

extern "C" fn f_npk_acpi_dsdt(vm: *const u64, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_acpi_dsdt(mem, ctx, buf_ptr, buf_max)
}

extern "C" fn f_npk_audio_submit(vm: *const u64, slot: i32, ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_audio_submit(mem, ctx, slot, ptr, len)
}

extern "C" fn f_npk_audio_poll_mix(vm: *const u64, ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_audio_poll_mix(mem, ctx, ptr, max)
}

extern "C" fn f_npk_fs_list(vm: *const u64, prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fs_list(mem, ctx, prefix_ptr, prefix_len, out_ptr, out_cap, recursive)
}

extern "C" fn f_npk_fs_stat(vm: *const u64, name_ptr: i32, name_len: i32, out_ptr: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fs_stat(mem, ctx, name_ptr, name_len, out_ptr)
}

extern "C" fn f_npk_set_wallpaper(vm: *const u64, ptr: i32, len: i32, width: i32, height: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_set_wallpaper(mem, ctx, ptr, len, width, height)
}

extern "C" fn f_npk_set_theme(vm: *const u64, ptr: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_set_theme(mem, ctx, ptr)
}

extern "C" fn f_npk_stream_read(vm: *const u64, idx: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_stream_read(mem, ctx, idx, buf_ptr, buf_len)
}

extern "C" fn f_npk_tcp_send(vm: *const u64, handle: i32, buf_ptr: i32, buf_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_tcp_send(mem, ctx, handle, buf_ptr, buf_len)
}

extern "C" fn f_npk_tcp_recv(vm: *const u64, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_tcp_recv(mem, ctx, handle, buf_ptr, buf_max)
}

extern "C" fn f_npk_dma_read(vm: *const u64, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_dma_read(mem, ctx, handle, dma_off, wasm_ptr, len)
}

extern "C" fn f_npk_dma_write(vm: *const u64, handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_dma_write(mem, ctx, handle, dma_off, wasm_ptr, len)
}

extern "C" fn f_npk_netdev_register(vm: *const u64, mac_ptr: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_netdev_register(mem, ctx, mac_ptr)
}

extern "C" fn f_npk_print(vm: *const u64, ptr: i32, len: i32) {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_print(mem, ctx, ptr, len)
}

extern "C" fn f_npk_log(vm: *const u64, ptr: i32, len: i32) {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_log(mem, ctx, ptr, len)
}

extern "C" fn f_npk_log_serial(vm: *const u64, ptr: i32, len: i32) {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_log_serial(mem, ctx, ptr, len)
}

extern "C" fn f_npk_http_request(vm: *const u64, url_ptr: i32, url_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_request(mem, ctx, url_ptr, url_len, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_send(vm: *const u64, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_send(mem, ctx, method_ptr, method_len, url_ptr, url_len, hdrs_ptr, hdrs_len, body_ptr, body_len, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_begin(vm: *const u64, method_ptr: i32, method_len: i32, url_ptr: i32, url_len: i32, hdrs_ptr: i32, hdrs_len: i32, body_ptr: i32, body_len: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_begin(mem, ctx, method_ptr, method_len, url_ptr, url_len, hdrs_ptr, hdrs_len, body_ptr, body_len, buf_max)
}

extern "C" fn f_npk_net_context(vm: *const u64, url_ptr: i32, url_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_net_context(mem, ctx, url_ptr, url_len)
}

extern "C" fn f_npk_http_begin_many(vm: *const u64, urls_ptr: i32, urls_len: i32, out_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_begin_many(mem, ctx, urls_ptr, urls_len, out_max)
}

extern "C" fn f_npk_http_poll(vm: *const u64, handle: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_http_poll(ctx, handle)
}

extern "C" fn f_npk_http_take(vm: *const u64, handle: i32, buf_ptr: i32, buf_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_take(mem, ctx, handle, buf_ptr, buf_max)
}

extern "C" fn f_npk_http_take_many(vm: *const u64, handle: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_take_many(mem, ctx, handle, out_ptr, out_max, lens_ptr, lens_max)
}

extern "C" fn f_npk_http_cancel(vm: *const u64, handle: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let ctx = unsafe { ctx_of(vm) };
    host_core::npk_http_cancel(ctx, handle)
}

extern "C" fn f_npk_http_request_many(vm: *const u64, urls_ptr: i32, urls_len: i32, out_ptr: i32, out_max: i32, lens_ptr: i32, lens_max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_http_request_many(mem, ctx, urls_ptr, urls_len, out_ptr, out_max, lens_ptr, lens_max)
}

extern "C" fn f_npk_open(vm: *const u64, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_open(mem, ctx, app_ptr, app_len, arg_ptr, arg_len)
}

extern "C" fn f_npk_launch(vm: *const u64, app_ptr: i32, app_len: i32, arg_ptr: i32, arg_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_launch(mem, ctx, app_ptr, app_len, arg_ptr, arg_len)
}

extern "C" fn f_npk_pick(vm: *const u64, mode: i32, start_ptr: i32, start_len: i32, suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_pick(mem, ctx, mode, start_ptr, start_len, suggest_ptr, suggest_len, tag)
}

extern "C" fn f_npk_pick_result(vm: *const u64, path_ptr: i32, path_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_pick_result(mem, ctx, path_ptr, path_len)
}

extern "C" fn f_npk_pick_mkdir(vm: *const u64, path_ptr: i32, path_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_pick_mkdir(mem, ctx, path_ptr, path_len)
}

extern "C" fn f_npk_fs_delete(vm: *const u64, name_ptr: i32, name_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fs_delete(mem, ctx, name_ptr, name_len)
}

extern "C" fn f_npk_fs_rename(vm: *const u64, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fs_rename(mem, ctx, old_ptr, old_len, new_ptr, new_len)
}

extern "C" fn f_npk_fs_copy(vm: *const u64, old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_fs_copy(mem, ctx, old_ptr, old_len, new_ptr, new_len)
}

extern "C" fn f_npk_wifi_send_cmd(vm: *const u64, buf_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_wifi_send_cmd(mem, ctx, buf_ptr, len)
}

extern "C" fn f_npk_wifi_poll_event(vm: *const u64, buf_ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_wifi_poll_event(mem, ctx, buf_ptr, max)
}

extern "C" fn f_npk_wifi_poll_cmd(vm: *const u64, buf_ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_wifi_poll_cmd(mem, ctx, buf_ptr, max)
}

extern "C" fn f_npk_wifi_send_event(vm: *const u64, buf_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_wifi_send_event(mem, ctx, buf_ptr, len)
}

extern "C" fn f_npk_driver_report(vm: *const u64, buf_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_driver_report(mem, ctx, buf_ptr, len)
}

extern "C" fn f_npk_netdev_submit_rx(vm: *const u64, buf_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_netdev_submit_rx(mem, ctx, buf_ptr, len)
}

extern "C" fn f_npk_netdev_rx_deliver(vm: *const u64, buf_ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_netdev_rx_deliver(mem, ctx, buf_ptr, len)
}

extern "C" fn f_npk_netdev_poll_tx(vm: *const u64, buf_ptr: i32, max: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, ctx) = unsafe { parts(vm) };
    host_core::npk_netdev_poll_tx(mem, ctx, buf_ptr, max)
}

/// Adresse der Routine fuer einen Import, oder nichts — dann behaelt der
/// Schlitz den Trap-Stumpf und das Modul sagt beim ersten Aufruf Bescheid.
pub(crate) fn resolve(module: &str, name: &str) -> Option<u64> {
    if module != "env" {
        return None;
    }
    Some(match name {
        "npk_http_status" => f_npk_http_status as *const () as u64,
        "npk_fs_usage" => f_npk_fs_usage as *const () as u64,
        "npk_clipboard_len" => f_npk_clipboard_len as *const () as u64,
        "npk_window_set_close_guard" => f_npk_window_set_close_guard as *const () as u64,
        "npk_screen_size" => f_npk_screen_size as *const () as u64,
        "npk_ticks" => f_npk_ticks as *const () as u64,
        "npk_now_us" => f_npk_now_us as *const () as u64,
        "npk_unix_time" => f_npk_unix_time as *const () as u64,
        "npk_theme_token" => f_npk_theme_token as *const () as u64,
        "npk_cursor_pos" => f_npk_cursor_pos as *const () as u64,
        "npk_screen_flash" => f_npk_screen_flash as *const () as u64,
        "npk_window_set_overlay" => f_npk_window_set_overlay as *const () as u64,
        "npk_window_set_modal" => f_npk_window_set_modal as *const () as u64,
        "npk_window_set_overlay_at" => f_npk_window_set_overlay_at as *const () as u64,
        "npk_window_set_light_dismiss" => f_npk_window_set_light_dismiss as *const () as u64,
        "npk_window_set_clipboard_sink" => f_npk_window_set_clipboard_sink as *const () as u64,
        "npk_window_set_dock" => f_npk_window_set_dock as *const () as u64,
        "npk_window_set_panel" => f_npk_window_set_panel as *const () as u64,
        "npk_battery" => f_npk_battery as *const () as u64,
        "npk_ec_read" => f_npk_ec_read as *const () as u64,
        "npk_ec_write" => f_npk_ec_write as *const () as u64,
        "npk_battery_report" => f_npk_battery_report as *const () as u64,
        "npk_audio_open" => f_npk_audio_open as *const () as u64,
        "npk_audio_close" => f_npk_audio_close as *const () as u64,
        "npk_audio_set_volume" => f_npk_audio_set_volume as *const () as u64,
        "npk_audio_get_volume" => f_npk_audio_get_volume as *const () as u64,
        "npk_workspace_switch" => f_npk_workspace_switch as *const () as u64,
        "npk_power" => f_npk_power as *const () as u64,
        "npk_close_widget" => f_npk_close_widget as *const () as u64,
        "npk_get_fb_size" => f_npk_get_fb_size as *const () as u64,
        "npk_sys_info" => f_npk_sys_info as *const () as u64,
        "npk_sleep" => f_npk_sleep as *const () as u64,
        "npk_input_poll" => f_npk_input_poll as *const () as u64,
        "npk_input_wait" => f_npk_input_wait as *const () as u64,
        "npk_clear" => f_npk_clear as *const () as u64,
        "npk_self_terminal" => f_npk_self_terminal as *const () as u64,
        "npk_stream_open" => f_npk_stream_open as *const () as u64,
        "npk_stream_close" => f_npk_stream_close as *const () as u64,
        "npk_key_inject" => f_npk_key_inject as *const () as u64,
        "npk_tcp_connect" => f_npk_tcp_connect as *const () as u64,
        "npk_tcp_status" => f_npk_tcp_status as *const () as u64,
        "npk_tcp_close" => f_npk_tcp_close as *const () as u64,
        "npk_debug_target_ip" => f_npk_debug_target_ip as *const () as u64,
        "npk_debug_target_port" => f_npk_debug_target_port as *const () as u64,
        "npk_pci_bind" => f_npk_pci_bind as *const () as u64,
        "npk_pci_bind_class" => f_npk_pci_bind_class as *const () as u64,
        "npk_pci_read_config" => f_npk_pci_read_config as *const () as u64,
        "npk_pci_write_config" => f_npk_pci_write_config as *const () as u64,
        "npk_pci_enable_bus_master" => f_npk_pci_enable_bus_master as *const () as u64,
        "npk_irq_register" => f_npk_irq_register as *const () as u64,
        "npk_irq_arm" => f_npk_irq_arm as *const () as u64,
        "npk_irq_wait" => f_npk_irq_wait as *const () as u64,
        "npk_mmio_map_bar" => f_npk_mmio_map_bar as *const () as u64,
        "npk_mmio_read32" => f_npk_mmio_read32 as *const () as u64,
        "npk_mmio_write32" => f_npk_mmio_write32 as *const () as u64,
        "npk_mmio_read16" => f_npk_mmio_read16 as *const () as u64,
        "npk_mmio_write16" => f_npk_mmio_write16 as *const () as u64,
        "npk_mmio_read64" => f_npk_mmio_read64 as *const () as u64,
        "npk_mmio_write64" => f_npk_mmio_write64 as *const () as u64,
        "npk_dma_alloc" => f_npk_dma_alloc as *const () as u64,
        "npk_dma_phys_addr" => f_npk_dma_phys_addr as *const () as u64,
        "npk_dma_read32" => f_npk_dma_read32 as *const () as u64,
        "npk_dma_write32" => f_npk_dma_write32 as *const () as u64,
        "npk_memory_fence" => f_npk_memory_fence as *const () as u64,
        "npk_netdev_set_link" => f_npk_netdev_set_link as *const () as u64,
        "npk_netdev_set_link_state" => f_npk_netdev_set_link_state as *const () as u64,
        "npk_fetch" => f_npk_fetch as *const () as u64,
        "npk_http_response_headers" => f_npk_http_response_headers as *const () as u64,
        "npk_http_final_url" => f_npk_http_final_url as *const () as u64,
        "npk_http_content_type" => f_npk_http_content_type as *const () as u64,
        "npk_http_last_error" => f_npk_http_last_error as *const () as u64,
        "npk_store" => f_npk_store as *const () as u64,
        "npk_home_dir" => f_npk_home_dir as *const () as u64,
        "npk_locale" => f_npk_locale as *const () as u64,
        "npk_launch_arg" => f_npk_launch_arg as *const () as u64,
        "npk_clipboard_set" => f_npk_clipboard_set as *const () as u64,
        "npk_clipboard_get" => f_npk_clipboard_get as *const () as u64,
        "npk_scene_commit" => f_npk_scene_commit as *const () as u64,
        "npk_canvas_commit" => f_npk_canvas_commit as *const () as u64,
        "npk_canvas_rect" => f_npk_canvas_rect as *const () as u64,
        "npk_capture_screen" => f_npk_capture_screen as *const () as u64,
        "npk_event_poll" => f_npk_event_poll as *const () as u64,
        "npk_list_modules" => f_npk_list_modules as *const () as u64,
        "npk_app_meta" => f_npk_app_meta as *const () as u64,
        "npk_spawn_module" => f_npk_spawn_module as *const () as u64,
        "npk_run_intent" => f_npk_run_intent as *const () as u64,
        "npk_bar_state" => f_npk_bar_state as *const () as u64,
        "npk_window_titles" => f_npk_window_titles as *const () as u64,
        "npk_acpi_dsdt" => f_npk_acpi_dsdt as *const () as u64,
        "npk_audio_submit" => f_npk_audio_submit as *const () as u64,
        "npk_audio_poll_mix" => f_npk_audio_poll_mix as *const () as u64,
        "npk_fs_list" => f_npk_fs_list as *const () as u64,
        "npk_fs_stat" => f_npk_fs_stat as *const () as u64,
        "npk_set_wallpaper" => f_npk_set_wallpaper as *const () as u64,
        "npk_set_theme" => f_npk_set_theme as *const () as u64,
        "npk_stream_read" => f_npk_stream_read as *const () as u64,
        "npk_tcp_send" => f_npk_tcp_send as *const () as u64,
        "npk_tcp_recv" => f_npk_tcp_recv as *const () as u64,
        "npk_dma_read" => f_npk_dma_read as *const () as u64,
        "npk_dma_write" => f_npk_dma_write as *const () as u64,
        "npk_netdev_register" => f_npk_netdev_register as *const () as u64,
        "npk_print" => f_npk_print as *const () as u64,
        "npk_log" => f_npk_log as *const () as u64,
        "npk_log_serial" => f_npk_log_serial as *const () as u64,
        "npk_http_request" => f_npk_http_request as *const () as u64,
        "npk_http_send" => f_npk_http_send as *const () as u64,
        "npk_http_request_many" => f_npk_http_request_many as *const () as u64,
        "npk_http_begin" => f_npk_http_begin as *const () as u64,
        "npk_net_context" => f_npk_net_context as *const () as u64,
        "npk_http_begin_many" => f_npk_http_begin_many as *const () as u64,
        "npk_http_poll" => f_npk_http_poll as *const () as u64,
        "npk_http_take" => f_npk_http_take as *const () as u64,
        "npk_http_take_many" => f_npk_http_take_many as *const () as u64,
        "npk_http_cancel" => f_npk_http_cancel as *const () as u64,
        "npk_open" => f_npk_open as *const () as u64,
        "npk_launch" => f_npk_launch as *const () as u64,
        "npk_pick" => f_npk_pick as *const () as u64,
        "npk_pick_result" => f_npk_pick_result as *const () as u64,
        "npk_pick_mkdir" => f_npk_pick_mkdir as *const () as u64,
        "npk_fs_delete" => f_npk_fs_delete as *const () as u64,
        "npk_fs_rename" => f_npk_fs_rename as *const () as u64,
        "npk_fs_copy" => f_npk_fs_copy as *const () as u64,
        "npk_wifi_send_cmd" => f_npk_wifi_send_cmd as *const () as u64,
        "npk_wifi_poll_event" => f_npk_wifi_poll_event as *const () as u64,
        "npk_wifi_poll_cmd" => f_npk_wifi_poll_cmd as *const () as u64,
        "npk_wifi_send_event" => f_npk_wifi_send_event as *const () as u64,
        "npk_driver_report" => f_npk_driver_report as *const () as u64,
        "npk_netdev_submit_rx" => f_npk_netdev_submit_rx as *const () as u64,
        "npk_netdev_rx_deliver" => f_npk_netdev_rx_deliver as *const () as u64,
        "npk_netdev_poll_tx" => f_npk_netdev_poll_tx as *const () as u64,
        _ => return None,
    })
}
