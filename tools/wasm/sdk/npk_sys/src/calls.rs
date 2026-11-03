//! The imports and their safe forms. Signatures follow the `host_imports!`
//! list in the kernel (`kernel/src/wasm/forge_glue.rs`); the release gate
//! rejects a module that declares one differently.

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);
    fn npk_log(ptr: i32, len: i32);
    fn npk_log_serial(ptr: i32, len: i32);
    fn npk_sleep(ms: i32) -> i32;
    fn npk_wait(mask: i32, timeout_ms: i32) -> i32;
    fn npk_ticks() -> i64;
    fn npk_now_us() -> i64;
    fn npk_unix_time() -> i64;
    fn npk_sys_info(key: i32) -> i64;
    fn npk_input_wait(timeout_ms: i32) -> i32;
    fn npk_input_poll() -> i32;
    fn npk_clear();
    fn npk_random_bytes(buf_ptr: i32, len: i32) -> i32;

    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_store(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_file_open(name_ptr: i32, name_len: i32) -> i32;
    fn npk_file_read(handle: i32, offset: i64, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_file_size(handle: i32) -> i64;
    fn npk_file_create(name_ptr: i32, name_len: i32) -> i32;
    fn npk_file_write(handle: i32, data_ptr: i32, len: i32) -> i32;
    fn npk_file_close(handle: i32) -> i32;
    fn npk_file_abort(handle: i32) -> i32;

    // Keyvault — usable only by the signed `vault` GUI module.
    fn npk_vault_status() -> i32;
    fn npk_vault_init() -> i32;
    fn npk_vault_set_passphrase(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;
    fn npk_vault_unlock(pass_ptr: i32, pass_len: i32) -> i32;
    fn npk_vault_lock() -> i32;
    fn npk_vault_user_set(name_ptr: i32, name_len: i32, val_ptr: i32, val_len: i32) -> i32;
    fn npk_vault_user_get(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_vault_user_list(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_vault_system_set(name_ptr: i32, name_len: i32, val_ptr: i32, val_len: i32) -> i32;
    fn npk_vault_system_list(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_vault_user_delete(name_ptr: i32, name_len: i32) -> i32;
    fn npk_vault_system_delete(name_ptr: i32, name_len: i32) -> i32;

    // Clipboard — focus-gated (only the focused widget app).
    fn npk_clipboard_set(ptr: i32, len: i32) -> i32;
    fn npk_clipboard_get(ptr: i32, max: i32) -> i32;
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
    fn npk_config_get(key_ptr: i32, key_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_fs_delete(name_ptr: i32, name_len: i32) -> i32;
    fn npk_fs_mkdir(name_ptr: i32, name_len: i32) -> i32;
    fn npk_fs_rename(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;
    fn npk_fs_copy(old_ptr: i32, old_len: i32, new_ptr: i32, new_len: i32) -> i32;
    fn npk_home_dir(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_launch_arg(buf_ptr: i32, buf_max: i32) -> i32;

    fn npk_scene_commit(ptr: i32, len: i32) -> i32;
    fn npk_event_poll(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_close_widget() -> i32;
    fn npk_spawn_module(ptr: i32, len: i32) -> i32;
    fn npk_run_intent(verb_ptr: i32, verb_len: i32) -> i32;
    fn npk_window_set_modal(modal: i32) -> i32;
    fn npk_window_set_overlay(w: i32, h: i32) -> i32;
    fn npk_window_titles(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_screen_size() -> i32;
    fn npk_get_fb_size() -> i64;
    fn npk_audio_get_volume() -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_audio_format(slot: i32, rate: i32, encoding: i32, channels: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_frame(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32, format: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
    fn npk_canvas_drop(canvas_id: i32) -> i32;
    fn npk_drag_start(ptr: i32, len: i32) -> i32;
    fn npk_pick(mode: i32, start_ptr: i32, start_len: i32,
                suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32;

    fn npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn npk_pci_bind_class_n(class: i32, subclass: i32, index: i32) -> i32;
    fn npk_pci_read_config(offset: i32) -> i32;
    fn npk_pci_read_config_at(bdf: i32, offset: i32) -> i32;
    fn npk_pci_write_config16(offset: i32, value: i32) -> i32;
    fn npk_vga_msr_touch() -> i32;
    fn npk_pci_bar_size(bar: i32) -> i64;
    fn npk_pci_write_config(offset: i32, value: i32) -> i32;
    fn npk_pci_enable_bus_master() -> i32;
    fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;
    fn npk_mmio_map_phys(hi: i32, lo: i32, pages: i32) -> i32;
    fn npk_mmio_read8(handle: i32, offset: i32) -> i32;
    fn npk_mmio_read16(handle: i32, offset: i32) -> i32;
    fn npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn npk_mmio_read64(handle: i32, offset: i32) -> i64;
    fn npk_mmio_write8(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_write64(handle: i32, offset: i32, value: i64) -> i32;
    fn npk_mmio_readq(handle: i32, offset: i32) -> i64;
    fn npk_mmio_writeq(handle: i32, offset: i32, value: i64) -> i32;
    fn npk_memory_fence() -> i32;
    fn npk_dma_alloc(pages: i32) -> i32;
    fn npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;
    fn npk_dma_set_mask(bits: i32) -> i32;
    fn npk_dma_free(handle: i32) -> i32;
    fn npk_dma_phys_addr(handle: i32) -> i64;
    fn npk_dma_read32(handle: i32, offset: i32) -> i32;
    fn npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_dma_read(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_dma_write(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_irq_register(entry: i32) -> i32;
    fn npk_irq_register_gsi(gsi: i32, flags: i32) -> i32;
    fn npk_irq_arm(vector: i32) -> i64;
    fn npk_irq_wait(vector: i32, since: i64, timeout_ms: i32) -> i32;
    fn npk_driver_report(buf_ptr: i32, len: i32) -> i32;

    fn npk_netdev_register(mac_ptr: i32) -> i32;
    fn npk_gpudev_register(version: i32) -> i32;
    fn npk_gpudev_bo_info(id: i32, out_ptr: i32) -> i32;
    fn npk_gpudev_bo_pages(id: i32, first: i32, out_ptr: i32, max: i32) -> i32;
    fn npk_gpudev_job_data(seq: i32, offset: i32, out_ptr: i32, len: i32) -> i32;
    fn npk_gpudev_bo_bound(id: i32, gpu_addr: i64) -> i32;
    fn npk_gpudev_bo_gpu_only(id: i32) -> i32;
    fn npk_gpudev_ready(ok: i32) -> i32;
    fn npk_gpudev_poll(buf_ptr: i32, max: i32) -> i32;
    fn npk_gpudev_complete(seq: i32, status: i32) -> i32;
    fn npk_gpudev_reply(seq: i32, ptr: i32, len: i32) -> i32;
    fn npk_gpudev_capset(ptr: i32, len: i32) -> i32;
    fn npk_gpudev_tt_alloc(client: i32, pages: i32) -> i32;
    fn npk_gpudev_tt_buffer(tt: i32) -> i32;
    fn npk_gpudev_tt_read(tt: i32, offset: i64, out_ptr: i32, len: i32) -> i32;
    fn npk_gpudev_tt_free(tt: i32) -> i32;
    fn npk_gpudev_ring(client: i32, ring: i32, submitted: i64, completed: i64) -> i32;
    fn npk_gpudev_vram(base: i64, size: i64) -> i32;
    fn npk_gpudev_released(id: i32) -> i32;
    fn npk_gpudev_stats(ptr: i32, len: i32) -> i32;
    fn npk_netdev_submit_rx(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_rx_deliver(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_poll_tx(buf_ptr: i32, max: i32) -> i32;
    fn npk_netdev_set_link(up: i32) -> i32;
    fn npk_netdev_set_link_state(carrier: i32, dormant: i32) -> i32;
    fn npk_wifi_send_cmd(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_cmd(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_send_event(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_event(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_known(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_new_network() -> i32;
    fn npk_wifi_poll_request(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_publish(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_status(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_request(buf_ptr: i32, len: i32) -> i32;
    fn npk_net_info(buf_ptr: i32, max: i32) -> i32;
    fn npk_status_set(key_ptr: i32, key_len: i32, icon: i32, value: i32, flags: i32,
        text_ptr: i32, text_len: i32, opens_ptr: i32, opens_len: i32) -> i32;
    fn npk_status_clear(key_ptr: i32, key_len: i32) -> i32;

    fn npk_acpi_dsdt(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_acpi_table(sig: i32, index: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_acpi_mem_read(hi: i32, lo: i32) -> i32;
    fn npk_sci_arm(gpe: i32) -> i32;
    fn npk_sci_service() -> i32;
    fn npk_acpi_video_notify(event: i32) -> i32;
    fn npk_acpi_proc_caps() -> i32;
    fn npk_acpi_cst(ptr: i32, count: i32) -> i32;
    fn npk_ec_read(addr: i32) -> i32;
    fn npk_ec_write(addr: i32, val: i32) -> i32;
    fn npk_ec_query() -> i32;
    fn npk_battery_report(packed: i32);
    fn npk_battery_detail(rate: i32, remaining: i32, full: i32, voltage_mv: i32, unit: i32);
    fn npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;
    fn npk_pointer_abs(x: i32, y: i32, buttons: i32) -> i32;
    fn npk_audio_poll_mix(ptr: i32, max: i32) -> i32;
    fn npk_audio_device_format(rate: i32, encoding: i32, channels: i32) -> i32;
    fn npk_display_audio_set(port: i32, ptr: i32, len: i32) -> i32;
    fn npk_display_audio_get(port: i32, ptr: i32, max: i32) -> i32;
    fn npk_display_audio_generation() -> i32;
    fn npk_display_audio_serve(done: i32) -> i32;
    fn npk_display_audio_request(bits: i32) -> i32;
    fn npk_display_audio_done() -> i32;
    fn npk_locale(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_list_modules(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_app_meta(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
}

/// Calls that take no pointer: the safe form is the call itself.
macro_rules! plain {
    ($( $(#[$m:meta])* fn $name:ident = $imp:ident ( $($a:ident : $t:ty),* ) $(-> $r:ty)? ; )*) => {
        $(
            $(#[$m])*
            #[inline]
            pub fn $name($($a: $t),*) $(-> $r)? {
                // SAFETY: FFI without pointers.
                unsafe { $imp($($a),*) }
            }
        )*
    };
}

plain! {
    /// 0 until the display driver reported its sinks once, then a count
    /// that moves with every report.
    fn display_audio_generation = npk_display_audio_generation() -> i32;
    /// Display driver: request `done` is carried out; returns
    /// `(number << 2) | bits` of what the audio driver wants now.
    fn display_audio_serve = npk_display_audio_serve(done: i32) -> i32;
    /// Audio driver: ask for the display's audio power (bit 0) and the
    /// codec wake signal (bit 1). The request's number; -2 while no display
    /// driver is bound, -3 when none will be.
    fn display_audio_request = npk_display_audio_request(bits: i32) -> i32;
    /// Audio driver: the last request carried out, -2 while none is bound.
    fn display_audio_done = npk_display_audio_done() -> i32;
    fn sleep = npk_sleep(ms: i32) -> i32;
    fn wait = npk_wait(mask: i32, timeout_ms: i32) -> i32;
    /// Milliseconds since boot.
    fn ticks = npk_ticks() -> i64;
    fn now_us = npk_now_us() -> i64;
    fn unix_time = npk_unix_time() -> i64;
    fn sys_info = npk_sys_info(key: i32) -> i64;
    fn input_wait = npk_input_wait(timeout_ms: i32) -> i32;
    fn input_poll = npk_input_poll() -> i32;
    fn clear = npk_clear();

    fn close_widget = npk_close_widget() -> i32;
    fn window_set_modal = npk_window_set_modal(modal: i32) -> i32;
    fn window_set_overlay = npk_window_set_overlay(w: i32, h: i32) -> i32;
    /// `w << 16 | h`.
    fn screen_size = npk_screen_size() -> i32;
    /// Physical framebuffer size, `w << 32 | h`.
    fn fb_size = npk_get_fb_size() -> i64;
    fn audio_get_volume = npk_audio_get_volume() -> i32;
    fn audio_set_volume = npk_audio_set_volume(pct: i32) -> i32;
    /// The format of one's own stream `slot`: `encoding` 0 = S16LE,
    /// 1 = S32LE, 2 = F32LE, 1 or 2 channels, any rate 8..384 kHz.
    fn audio_format = npk_audio_format(slot: i32, rate: i32, encoding: i32, channels: i32) -> i32;
    /// The device format the mixer produces (driver side): `encoding`
    /// 0 = S16LE, 1 = S32LE, 2 = F32LE.
    fn audio_device_format = npk_audio_device_format(rate: i32, encoding: i32, channels: i32) -> i32;

    fn pci_bind = npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn pci_bind_class = npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn pci_bind_class_n = npk_pci_bind_class_n(class: i32, subclass: i32, index: i32) -> i32;
    fn pci_read_config = npk_pci_read_config(offset: i32) -> i32;
    /// A config dword of any function, read-only (needs HARDWARE).
    /// `bdf` = bus << 8 | device << 3 | function.
    fn pci_read_config_at = npk_pci_read_config_at(bdf: i32, offset: i32) -> i32;
    /// One 16-bit config write to the bound device (Linux pci_write_config_word).
    fn pci_write_config16 = npk_pci_write_config16(offset: i32, value: i32) -> i32;
    /// Rewrites the VGA MSR with its own value (Linux intel_vga_reset_io_mem).
    fn vga_msr_touch = npk_vga_msr_touch() -> i32;
    /// Size of the bound device's memory BAR `bar` (boot scan), 0 = none.
    fn pci_bar_size = npk_pci_bar_size(bar: i32) -> i64;
    fn pci_write_config = npk_pci_write_config(offset: i32, value: i32) -> i32;
    fn pci_enable_bus_master = npk_pci_enable_bus_master() -> i32;
    fn mmio_map_bar = npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;
    fn mmio_map_phys = npk_mmio_map_phys(hi: i32, lo: i32, pages: i32) -> i32;
    fn mmio_read8 = npk_mmio_read8(handle: i32, offset: i32) -> i32;
    fn mmio_read16 = npk_mmio_read16(handle: i32, offset: i32) -> i32;
    fn mmio_read32 = npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn mmio_read64 = npk_mmio_read64(handle: i32, offset: i32) -> i64;
    fn mmio_write8 = npk_mmio_write8(handle: i32, offset: i32, value: i32) -> i32;
    fn mmio_write16 = npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;
    fn mmio_write32 = npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn mmio_write64 = npk_mmio_write64(handle: i32, offset: i32, value: i64) -> i32;
    /// One 64-bit read (Linux `readq`); `mmio_read64` is two 32-bit reads.
    fn mmio_readq = npk_mmio_readq(handle: i32, offset: i32) -> i64;
    /// One 64-bit write (Linux `writeq`); `mmio_write64` is two 32-bit writes.
    fn mmio_writeq = npk_mmio_writeq(handle: i32, offset: i32, value: i64) -> i32;
    fn memory_fence = npk_memory_fence() -> i32;
    fn dma_alloc = npk_dma_alloc(pages: i32) -> i32;
    fn dma_alloc_below = npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;
    /// `dma_set_mask(DMA_BIT_MASK(bits))`: later DMA buffers lie below `1 << bits`.
    fn dma_set_mask = npk_dma_set_mask(bits: i32) -> i32;
    /// `dma_free_coherent`: the buffer goes back; its handle is empty after.
    fn dma_free = npk_dma_free(handle: i32) -> i32;
    fn dma_phys_addr = npk_dma_phys_addr(handle: i32) -> i64;
    fn dma_read32 = npk_dma_read32(handle: i32, offset: i32) -> i32;
    fn dma_write32 = npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn irq_register = npk_irq_register(entry: i32) -> i32;
    fn irq_register_gsi = npk_irq_register_gsi(gsi: i32, flags: i32) -> i32;
    fn irq_arm = npk_irq_arm(vector: i32) -> i64;
    fn irq_wait = npk_irq_wait(vector: i32, since: i64, timeout_ms: i32) -> i32;
    fn gpudev_register = npk_gpudev_register(version: i32) -> i32;
    fn gpudev_bo_bound = npk_gpudev_bo_bound(id: i32, gpu_addr: i64) -> i32;
    /// Buffer `id`'s pages hold a picture only the card reads (tiled).
    fn gpudev_bo_gpu_only = npk_gpudev_bo_gpu_only(id: i32) -> i32;
    fn gpudev_ready = npk_gpudev_ready(ok: i32) -> i32;
    fn gpudev_complete = npk_gpudev_complete(seq: i32, status: i32) -> i32;
    /// Fence counters of `ring` of native-context client `client`.
    fn gpudev_ring = npk_gpudev_ring(client: i32, ring: i32, submitted: i64, completed: i64) -> i32;
    fn gpudev_released = npk_gpudev_released(id: i32) -> i32;
    /// The CPU's window onto all of the card's VRAM, before `gpudev_ready`.
    fn gpudev_vram = npk_gpudev_vram(base: i64, size: i64) -> i32;
    fn gpudev_tt_alloc = npk_gpudev_tt_alloc(client: i32, pages: i32) -> i32;
    fn gpudev_tt_buffer = npk_gpudev_tt_buffer(tt: i32) -> i32;
    fn gpudev_tt_free = npk_gpudev_tt_free(tt: i32) -> i32;

    fn netdev_set_link = npk_netdev_set_link(up: i32) -> i32;
    /// The WLAN joined another network: a new DHCP lease (`wifid` only).
    fn wifi_new_network = npk_wifi_new_network() -> i32;
    fn netdev_set_link_state = npk_netdev_set_link_state(carrier: i32, dormant: i32) -> i32;

    fn acpi_mem_read = npk_acpi_mem_read(hi: i32, lo: i32) -> i32;
    fn sci_arm = npk_sci_arm(gpe: i32) -> i32;
    fn sci_service = npk_sci_service() -> i32;
    /// An ACPI video output's event (0x85..0x89), for the kernel to act on
    /// as Linux' `acpi_video` input keys (`acpi` only).
    fn acpi_video_notify = npk_acpi_video_notify(event: i32) -> i32;
    /// The capabilities for the processors' `_OSC`/`_PDC` (`acpi` only).
    fn acpi_proc_caps = npk_acpi_proc_caps() -> i32;
    fn ec_read = npk_ec_read(addr: i32) -> i32;
    fn ec_write = npk_ec_write(addr: i32, val: i32) -> i32;
    fn ec_query = npk_ec_query() -> i32;
    fn battery_report = npk_battery_report(packed: i32);
    fn battery_detail = npk_battery_detail(rate: i32, remaining: i32, full: i32, voltage_mv: i32, unit: i32);
    fn pointer_inject = npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;
    /// The pointer at `x`, `y` on 0..=65535 of the screen (touch screen, pen).
    fn pointer_abs = npk_pointer_abs(x: i32, y: i32, buttons: i32) -> i32;
}

/// Calls that read one slice of this module's memory.
macro_rules! reads {
    ($( $(#[$m:meta])* fn $name:ident = $imp:ident $(-> $r:ty)? ; )*) => {
        $(
            $(#[$m])*
            #[inline]
            pub fn $name(data: &[u8]) $(-> $r)? {
                // SAFETY: FFI; `data` is borrowed for the call and the kernel
                // checks the range against this instance's memory.
                unsafe { $imp(data.as_ptr() as i32, data.len() as i32) }
            }
        )*
    };
}

/// Calls that fill one slice of this module's memory.
macro_rules! fills {
    ($( $(#[$m:meta])* fn $name:ident = $imp:ident -> $r:ty ; )*) => {
        $(
            $(#[$m])*
            #[inline]
            pub fn $name(buf: &mut [u8]) -> $r {
                // SAFETY: FFI; the kernel writes at most `buf.len()` bytes
                // into `buf`, which is borrowed for the call.
                unsafe { $imp(buf.as_mut_ptr() as i32, buf.len() as i32) }
            }
        )*
    };
}

reads! {
    /// To the terminal the module runs in (or the boot log).
    fn print = npk_print;
    /// To the boot log.
    fn log = npk_log;
    /// To the serial port.
    fn log_serial = npk_log_serial;
    fn scene_commit = npk_scene_commit -> i32;
    fn spawn_module = npk_spawn_module -> i32;
    fn run_intent = npk_run_intent -> i32;
    fn fs_delete = npk_fs_delete -> i32;
    fn fs_mkdir = npk_fs_mkdir -> i32;
    fn driver_report = npk_driver_report -> i32;
    fn netdev_submit_rx = npk_netdev_submit_rx -> i32;
    fn netdev_rx_deliver = npk_netdev_rx_deliver -> i32;
    fn wifi_send_cmd = npk_wifi_send_cmd -> i32;
    fn wifi_send_event = npk_wifi_send_event -> i32;
    /// The manager's state text (`wifid` only).
    fn wifi_publish = npk_wifi_publish -> i32;
    /// A user request to the manager (`link` only).
    fn wifi_request = npk_wifi_request -> i32;
}

fills! {
    fn random_bytes = npk_random_bytes -> i32;
    fn home_dir = npk_home_dir -> i32;
    fn launch_arg = npk_launch_arg -> i32;
    fn event_poll = npk_event_poll -> i32;
    fn window_titles = npk_window_titles -> i32;
    fn netdev_poll_tx = npk_netdev_poll_tx -> i32;
    /// Jobs for the GPU service, 32 bytes each (`gpudev_poll`).
    fn gpudev_poll = npk_gpudev_poll -> i32;
    /// The next command (driver side); 0 when there is none.
    fn wifi_poll_cmd = npk_wifi_poll_cmd -> i32;
    /// The next driver event (`wifid` only); 0 when there is none.
    fn wifi_poll_event = npk_wifi_poll_event -> i32;
    /// Known networks with their records (`wifid` only).
    fn wifi_known = npk_wifi_known -> i32;
    /// The next user request (`wifid` only); 0 when there is none.
    fn wifi_poll_request = npk_wifi_poll_request -> i32;
    /// The manager's state text (`link` only).
    fn wifi_status = npk_wifi_status -> i32;
    /// Interfaces + IPv4 as `key value` lines (RENDER).
    fn net_info = npk_net_info -> i32;
    fn acpi_dsdt = npk_acpi_dsdt -> i32;
    fn audio_poll_mix = npk_audio_poll_mix -> i32;
    /// The UI language code (`en`, `de`).
    fn locale = npk_locale -> i32;
    /// Installed module names, NUL-separated (RENDER).
    fn list_modules = npk_list_modules -> i32;
}

/// The `.npk.app_meta` section of module `name` (RENDER).
pub fn app_meta(name: &[u8], buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_app_meta(name.as_ptr() as i32, name.len() as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// The display driver reports the ELD of `port`'s sink; empty for none.
pub fn display_audio_set(port: i32, eld: &[u8]) -> i32 {
    // SAFETY: FFI; `eld` is borrowed for the call.
    unsafe { npk_display_audio_set(port, eld.as_ptr() as i32, eld.len() as i32) }
}

/// The audio driver reads the ELD of `port`; its length, 0 for no sink.
pub fn display_audio_get(port: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`,
    // which is borrowed for the call.
    unsafe { npk_display_audio_get(port, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Read file `name` into `buf`.
pub fn fetch(name: &[u8], buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_fetch(name.as_ptr() as i32, name.len() as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Write `data` as file `name`.
pub fn store(name: &[u8], data: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_store(name.as_ptr() as i32, name.len() as i32, data.as_ptr() as i32, data.len() as i32) }
}

/// Open file `name` for ranged reads (gates of `fetch`); a handle or -1.
pub fn file_open(name: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_file_open(name.as_ptr() as i32, name.len() as i32) }
}

/// Bytes of the file at `offset` into `buf`: the count, 0 at the end, -1 on
/// error. A short count is no error. Offsets past `i64::MAX` are refused.
pub fn file_read(handle: i32, offset: u64, buf: &mut [u8]) -> i32 {
    let Ok(offset) = i64::try_from(offset) else { return -1 };
    let len = buf.len().min(i32::MAX as usize) as i32;
    // SAFETY: FFI; the kernel writes at most `len` bytes into `buf`, which
    // is borrowed for the call.
    unsafe { npk_file_read(handle, offset, buf.as_mut_ptr() as i32, len) }
}

/// Length of an open file (a writer: bytes so far), or -1.
pub fn file_size(handle: i32) -> i64 {
    // SAFETY: FFI without pointers.
    unsafe { npk_file_size(handle) }
}

/// Start writing file `name` (gates of `store`); a handle or -1. The file
/// appears at [`file_close`].
pub fn file_create(name: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_file_create(name.as_ptr() as i32, name.len() as i32) }
}

/// Append `data` (at most `i32::MAX` bytes per call); its length or -1.
pub fn file_write(handle: i32, data: &[u8]) -> i32 {
    let len = data.len().min(i32::MAX as usize) as i32;
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_file_write(handle, data.as_ptr() as i32, len) }
}

/// Close a handle; a writer's file is published. 0 or -1.
pub fn file_close(handle: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_file_close(handle) }
}

/// Close a handle without publishing a writer's file. 0 or -1.
pub fn file_abort(handle: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_file_abort(handle) }
}

// ── Keyvault — only the `vault` module may call ──

/// Bits: 1 exists, 2 user tier present, 4 user tier unlocked; -1 if denied.
pub fn vault_status() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_vault_status() }
}
pub fn vault_init() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_vault_init() }
}
/// Publish status item `key`: `flags` = state
/// | place << 8; a click on it opens module `opens`.
pub fn status_set(key: &[u8], icon: i32, value: i32, flags: i32, text: &[u8], opens: &[u8]) -> i32 {
    // SAFETY: FFI; the three ranges are borrowed for the call.
    unsafe {
        npk_status_set(key.as_ptr() as i32, key.len() as i32, icon, value, flags,
            text.as_ptr() as i32, text.len() as i32, opens.as_ptr() as i32, opens.len() as i32)
    }
}
pub fn status_clear(key: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_status_clear(key.as_ptr() as i32, key.len() as i32) }
}
pub fn vault_set_passphrase(old: &[u8], new: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_vault_set_passphrase(old.as_ptr() as i32, old.len() as i32, new.as_ptr() as i32, new.len() as i32) }
}
pub fn vault_unlock(pass: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_vault_unlock(pass.as_ptr() as i32, pass.len() as i32) }
}
pub fn vault_lock() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_vault_lock() }
}
pub fn vault_user_set(name: &[u8], val: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_vault_user_set(name.as_ptr() as i32, name.len() as i32, val.as_ptr() as i32, val.len() as i32) }
}
pub fn vault_user_get(name: &[u8], buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_vault_user_get(name.as_ptr() as i32, name.len() as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}
pub fn vault_user_list(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_vault_user_list(buf.as_mut_ptr() as i32, buf.len() as i32) }
}
pub fn vault_system_set(name: &[u8], val: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_vault_system_set(name.as_ptr() as i32, name.len() as i32, val.as_ptr() as i32, val.len() as i32) }
}
pub fn vault_system_list(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_vault_system_list(buf.as_mut_ptr() as i32, buf.len() as i32) }
}
pub fn vault_user_delete(name: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_vault_user_delete(name.as_ptr() as i32, name.len() as i32) }
}
pub fn vault_system_delete(name: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_vault_system_delete(name.as_ptr() as i32, name.len() as i32) }
}

/// Put `text` on the clipboard. Bytes stored, or -1 (not focused / denied).
pub fn clipboard_set(text: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_clipboard_set(text.as_ptr() as i32, text.len() as i32) }
}
/// Copy the clipboard text into `buf`. Its length, 0 if empty, or -1 (not
/// focused / denied).
pub fn clipboard_get(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_clipboard_get(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

pub fn fs_list(prefix: &[u8], out: &mut [u8], recursive: bool) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe {
        npk_fs_list(prefix.as_ptr() as i32, prefix.len() as i32,
            out.as_mut_ptr() as i32, out.len() as i32, recursive as i32)
    }
}

/// Move `old` to `new` (files and whole directories; `new` must not exist).
pub fn fs_rename(old: &[u8], new: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_fs_rename(old.as_ptr() as i32, old.len() as i32, new.as_ptr() as i32, new.len() as i32) }
}

/// Copy `old` to `new` (`new` must not exist).
pub fn fs_copy(old: &[u8], new: &[u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_fs_copy(old.as_ptr() as i32, old.len() as i32, new.as_ptr() as i32, new.len() as i32) }
}

/// A `set` value of this module's own namespace (`<module>.<name>`) into
/// `buf`: its length, 0 when unset, -1 for another namespace.
pub fn config_get(key: &[u8], buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_config_get(key.as_ptr() as i32, key.len() as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Size (8 bytes LE), directory flag (1), mtime (8 bytes LE) into `out`.
pub fn fs_stat(name: &[u8], out: &mut [u8; 17]) -> i32 {
    // SAFETY: FFI; `out` is the 17 bytes the kernel writes.
    unsafe { npk_fs_stat(name.as_ptr() as i32, name.len() as i32, out.as_mut_ptr() as i32) }
}

pub fn canvas_commit(canvas_id: i32, px: &[u8], w: i32, h: i32) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_canvas_commit(canvas_id, px.as_ptr() as i32, px.len() as i32, w, h) }
}

/// A frame for canvas `canvas_id`, shown as a scene layer over the window
/// and scaled on the GPU. `format` 0 = BGRA32. -1 when refused or the
/// format is not served (then `canvas_commit` is the way).
pub fn canvas_frame(canvas_id: i32, px: &[u8], w: i32, h: i32, format: i32) -> i32 {
    // SAFETY: FFI; the kernel reads `px.len()` bytes from `px`.
    unsafe { npk_canvas_frame(canvas_id, px.as_ptr() as i32, px.len() as i32, w, h, format) }
}

/// `x, y, w, h` as four i32 LE into `out`.
pub fn canvas_rect(canvas_id: i32, out: &mut [u8; 16]) -> i32 {
    // SAFETY: FFI; `out` is the 16 bytes the kernel writes.
    unsafe { npk_canvas_rect(canvas_id, out.as_mut_ptr() as i32) }
}

/// Free canvas `canvas_id` of this window. 0, or -1 if there was none.
pub fn canvas_drop(canvas_id: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_canvas_drop(canvas_id) }
}

/// Start dragging `paths` (NUL-separated) from this window. 0, or -1 when
/// refused (not focused, no button held, a path the caller may not read).
pub fn drag_start(paths: &[u8]) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_drag_start(paths.as_ptr() as i32, paths.len() as i32) }
}

pub fn pick(mode: i32, start: &[u8], suggest: &[u8], tag: i32) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe {
        npk_pick(mode, start.as_ptr() as i32, start.len() as i32,
            suggest.as_ptr() as i32, suggest.len() as i32, tag)
    }
}

/// Copy `buf.len()` bytes from DMA buffer `handle` at `off` into `buf`.
pub fn dma_read(handle: i32, off: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; `buf` is borrowed for the call.
    unsafe { npk_dma_read(handle, off, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Copy `data` into DMA buffer `handle` at `off`.
pub fn dma_write(handle: i32, off: i32, data: &[u8]) -> i32 {
    // SAFETY: FFI; `data` is borrowed for the call.
    unsafe { npk_dma_write(handle, off, data.as_ptr() as i32, data.len() as i32) }
}

pub fn netdev_register(mac: &[u8; 6]) -> i32 {
    // SAFETY: FFI; the kernel reads the 6 bytes.
    unsafe { npk_netdev_register(mac.as_ptr() as i32) }
}

/// ACPI table `sig` (four ASCII bytes), the `index`-th of that name.
pub fn acpi_table(sig: [u8; 4], index: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; `buf` is borrowed for the call.
    unsafe { npk_acpi_table(i32::from_le_bytes(sig), index, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// A processor's `_CST` for MWAIT idle, 24 bytes per entry (`acpi` only);
/// the number of states idle may use.
pub fn acpi_cst(entries: &[u8]) -> i32 {
    // SAFETY: FFI; `entries` is borrowed for the call.
    unsafe { npk_acpi_cst(entries.as_ptr() as i32, (entries.len() / 24) as i32) }
}

/// A mapped register window (a handle from `mmio_map_bar`/`mmio_map_phys`).
/// Reads return the register; the kernel's -1 for a refused access is not
/// told apart from an all-ones register.
#[derive(Clone, Copy, Debug)]
pub struct Mmio(pub i32);

impl Mmio {
    /// BAR `bar` of the bound device, `pages` pages.
    pub fn map_bar(bar: i32, pages: i32) -> Option<Mmio> {
        let h = mmio_map_bar(bar, pages);
        (h >= 0).then_some(Mmio(h))
    }

    /// `pages` pages of physical MMIO at `phys`.
    pub fn map_phys(phys: u64, pages: i32) -> Option<Mmio> {
        let h = mmio_map_phys((phys >> 32) as i32, phys as u32 as i32, pages);
        (h >= 0).then_some(Mmio(h))
    }

    pub fn r8(self, off: u32) -> u8 { mmio_read8(self.0, off as i32) as u8 }
    pub fn r16(self, off: u32) -> u16 { mmio_read16(self.0, off as i32) as u16 }
    pub fn r32(self, off: u32) -> u32 { mmio_read32(self.0, off as i32) as u32 }
    pub fn r64(self, off: u32) -> u64 { mmio_read64(self.0, off as i32) as u64 }
    pub fn readq(self, off: u32) -> u64 { mmio_readq(self.0, off as i32) as u64 }
    pub fn w8(self, off: u32, v: u8) { mmio_write8(self.0, off as i32, v as i32); }
    pub fn w16(self, off: u32, v: u16) { mmio_write16(self.0, off as i32, v as i32); }
    pub fn w32(self, off: u32, v: u32) { mmio_write32(self.0, off as i32, v as i32); }
    pub fn w64(self, off: u32, v: u64) { mmio_write64(self.0, off as i32, v as i64); }
    pub fn writeq(self, off: u32, v: u64) { mmio_writeq(self.0, off as i32, v as i64); }
}

/// A DMA buffer of the bound device: its handle and bus address.
#[derive(Clone, Copy, Debug)]
pub struct Dma {
    pub handle: i32,
    pub phys: u64,
}

impl Dma {
    /// `pages` zeroed pages below 4 GiB.
    pub fn alloc(pages: i32) -> Option<Dma> {
        Self::from_handle(dma_alloc(pages))
    }

    /// `pages` zeroed pages below `limit_mb` MiB (0: 4 GiB).
    /// Give the buffer back (`dma_free_coherent`). The device must no longer
    /// reach it; the handle is empty afterwards.
    pub fn free(self) {
        dma_free(self.handle);
    }

    pub fn alloc_below(pages: i32, limit_mb: i32) -> Option<Dma> {
        Self::from_handle(dma_alloc_below(pages, limit_mb))
    }

    fn from_handle(handle: i32) -> Option<Dma> {
        if handle < 0 { return None; }
        let phys = dma_phys_addr(handle);
        (phys > 0).then_some(Dma { handle, phys: phys as u64 })
    }

    pub fn r32(self, off: u32) -> u32 { dma_read32(self.handle, off as i32) as u32 }
    pub fn w32(self, off: u32, v: u32) { dma_write32(self.handle, off as i32, v as i32); }

    /// Copy `data` in at `off`. False if the kernel refused (range or handle).
    #[must_use]
    pub fn write(self, off: u32, data: &[u8]) -> bool {
        dma_write(self.handle, off as i32, data) == 0
    }

    /// Copy out at `off`. False if the kernel refused.
    #[must_use]
    pub fn read(self, off: u32, buf: &mut [u8]) -> bool {
        dma_read(self.handle, off as i32, buf) == 0
    }
}

/// The kernel's description of GPU buffer `id` (`npk_gpudev_bo_info`):
/// `{phys: u64, size: u64, pitch, width, height, format, offset, pages: u32}`.
pub fn gpudev_bo_info(id: i32, out: &mut [u8; 40]) -> i32 {
    // SAFETY: FFI; the kernel writes 40 bytes into `out`, borrowed for the call.
    unsafe { npk_gpudev_bo_info(id, out.as_mut_ptr() as i32) }
}

/// Physical addresses of the pages of GPU buffer `id` from page `first` on
/// (`npk_gpudev_bo_pages`); the number written, or -1.
pub fn gpudev_bo_pages(id: i32, first: i32, out: &mut [u64]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `out.len()` u64s into `out`,
    // borrowed for the call.
    unsafe { npk_gpudev_bo_pages(id, first, out.as_mut_ptr() as i32, out.len() as i32) }
}

/// The module's cumulative GPU counters for `cores` (`npk_gpudev_stats`);
/// 0, or -1 if the kernel refused them.
pub fn gpudev_stats(words: &[u64]) -> i32 {
    // SAFETY: FFI; `words` is borrowed for the call.
    unsafe { npk_gpudev_stats(words.as_ptr() as i32, (words.len() * 8) as i32) }
}

/// The response of job `seq` (`npk_gpudev_reply`), before it completes;
/// 0, or -1 if the kernel refused it.
pub fn gpudev_reply(seq: i32, data: &[u8]) -> i32 {
    // SAFETY: FFI; `data` is borrowed for the call.
    unsafe { npk_gpudev_reply(seq, data.as_ptr() as i32, data.len() as i32) }
}

/// The card's DRM capset for guest native contexts
/// (`npk_gpudev_capset`), before `gpudev_ready`; 0, or -1.
pub fn gpudev_capset(data: &[u8]) -> i32 {
    // SAFETY: FFI; `data` is borrowed for the call.
    unsafe { npk_gpudev_capset(data.as_ptr() as i32, data.len() as i32) }
}

/// Bytes of GTT pages `tt` this module holds, from byte `offset` on
/// (`npk_gpudev_tt_read`); the number copied, or -1.
pub fn gpudev_tt_read(tt: i32, offset: u64, out: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `out.len()` bytes into `out`,
    // borrowed for the call.
    unsafe { npk_gpudev_tt_read(tt, offset as i64, out.as_mut_ptr() as i32, out.len() as i32) }
}

/// Bytes of job `seq`'s payload from `offset` on (`npk_gpudev_job_data`);
/// the number copied, or -1.
pub fn gpudev_job_data(seq: i32, offset: i32, out: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `out.len()` bytes into `out`,
    // borrowed for the call.
    unsafe { npk_gpudev_job_data(seq, offset, out.as_mut_ptr() as i32, out.len() as i32) }
}

// External volumes: the block handle and request channel of an `extfs`
// instance. Every other module gets -1.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_blk_info(out_ptr: i32) -> i32;
    fn npk_blk_read(lba: i64, len: i32, buf_ptr: i32) -> i32;
    fn npk_blk_write(lba: i64, len: i32, buf_ptr: i32) -> i32;
    fn npk_blk_flush() -> i32;
    fn npk_ext_parts(ptr: i32, len: i32) -> i32;
    fn npk_ext_mount(label_ptr: i32, label_len: i32, total: i64, free: i64) -> i32;
    fn npk_ext_next(buf_ptr: i32, buf_max: i32, timeout_ms: i32) -> i32;
    fn npk_ext_reply(seq: i32, status: i32, ptr: i32, len: i32) -> i32;
    fn npk_ext_volumes(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_ext_eject(label_ptr: i32, label_len: i32) -> i32;
}

/// The mounted external volumes (needs EXTERNAL): per volume label length
/// u16, read-only u8, size u64, free u64, label. The `fill` protocol.
pub fn ext_volumes(buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`,
    // borrowed for the call.
    unsafe { npk_ext_volumes(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Eject the drive of volume `label`: 0 once it can be removed, or -1.
pub fn ext_eject(label: &[u8]) -> i32 {
    // SAFETY: FFI; `label` is borrowed for the call.
    unsafe { npk_ext_eject(label.as_ptr() as i32, label.len() as i32) }
}

/// The block handle: role (1 probe, 2 volume), block size, block count,
/// read-only. `None` without a handle.
pub fn blk_info() -> Option<(u32, u32, u64, bool)> {
    let mut b = [0u8; 20];
    // SAFETY: FFI; the kernel writes 20 bytes into `b`, borrowed for the call.
    if unsafe { npk_blk_info(b.as_mut_ptr() as i32) } != 20 { return None; }
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut blocks = [0u8; 8];
    blocks.copy_from_slice(&b[8..16]);
    Some((u32_at(0), u32_at(4), u64::from_le_bytes(blocks), u32_at(16) != 0))
}

/// Read whole blocks at `lba` into `buf` (at most 1 MiB): 0, -1 range,
/// -2 gone, -3 I/O.
pub fn blk_read(lba: u64, buf: &mut [u8]) -> i32 {
    let Ok(lba) = i64::try_from(lba) else { return -1 };
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`,
    // borrowed for the call.
    unsafe { npk_blk_read(lba, buf.len() as i32, buf.as_mut_ptr() as i32) }
}

/// Write whole blocks at `lba`: the codes of `blk_read`, -4 read-only.
pub fn blk_write(lba: u64, buf: &[u8]) -> i32 {
    let Ok(lba) = i64::try_from(lba) else { return -1 };
    // SAFETY: FFI; `buf` is borrowed for the call.
    unsafe { npk_blk_write(lba, buf.len() as i32, buf.as_ptr() as i32) }
}

/// Flush the device's write cache.
pub fn blk_flush() -> i32 {
    // SAFETY: FFI without arguments.
    unsafe { npk_blk_flush() }
}

/// A probe's partitions, 16 bytes each (start u64, count u64): how many
/// volumes started, or -1.
pub fn ext_parts(parts: &[u8]) -> i32 {
    // SAFETY: FFI; `parts` is borrowed for the call.
    unsafe { npk_ext_parts(parts.as_ptr() as i32, parts.len() as i32) }
}

/// The volume is mounted: its label, size and free bytes.
pub fn ext_mount(label: &[u8], total: u64, free: u64) -> i32 {
    // SAFETY: FFI; `label` is borrowed for the call.
    unsafe { npk_ext_mount(label.as_ptr() as i32, label.len() as i32, total as i64, free as i64) }
}

/// The next request into `buf`: its length, 0 when none came within
/// `timeout_ms`, -2 when the volume is gone.
pub fn ext_next(buf: &mut [u8], timeout_ms: i32) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`,
    // borrowed for the call.
    unsafe { npk_ext_next(buf.as_mut_ptr() as i32, buf.len() as i32, timeout_ms) }
}

/// The answer to request `seq`.
pub fn ext_reply(seq: u32, status: i32, data: &[u8]) -> i32 {
    // SAFETY: FFI; `data` is borrowed for the call.
    unsafe { npk_ext_reply(seq as i32, status, data.as_ptr() as i32, data.len() as i32) }
}

// USB interfaces: class video needs CAMERA, class audio MIC. Errors: -1
// gone, -2 none free, -3 denied, -4 device refused, -5 bandwidth, -6
// memory, -7 bad argument.
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_usb_claim(class: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_usb_control(handle: i32, setup_ptr: i32, data_ptr: i32, data_len: i32) -> i32;
    fn npk_usb_set_alt(handle: i32, ifnum: i32, alt: i32) -> i32;
    fn npk_usb_stream_open(handle: i32, ep: i32, td_len: i32, count: i32) -> i32;
    fn npk_usb_stream_read(handle: i32, sid: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_usb_stream_close(handle: i32, sid: i32) -> i32;
    fn npk_usb_release(handle: i32) -> i32;
}

/// Claim the interfaces of `class` on the first free device: the handle,
/// with the device descriptor (18 bytes) and the configuration descriptor
/// written to `buf`.
pub fn usb_claim(class: u8, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`.
    unsafe { npk_usb_claim(class as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// A control request on a claimed interface or its endpoints. `data`
/// holds the OUT bytes or receives the IN bytes (wLength of `setup`).
pub fn usb_control(handle: i32, setup: &[u8; 8], data: &mut [u8]) -> i32 {
    // SAFETY: FFI; both ranges are borrowed for the call.
    unsafe { npk_usb_control(handle, setup.as_ptr() as i32, data.as_mut_ptr() as i32, data.len() as i32) }
}

pub fn usb_set_alt(handle: i32, ifnum: u8, alt: u8) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_usb_set_alt(handle, ifnum as i32, alt as i32) }
}

/// An IN stream: `count` TDs of `td_len` bytes kept queued; `td_len` 0 is
/// one service interval of an isochronous endpoint.
pub fn usb_stream_open(handle: i32, ep: u8, td_len: u32, count: u32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_usb_stream_open(handle, ep as i32, td_len as i32, count as i32) }
}

/// What came back: records of `len: u16, status: u8, 0`, then the bytes.
pub fn usb_stream_read(handle: i32, sid: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`.
    unsafe { npk_usb_stream_read(handle, sid, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

pub fn usb_stream_close(handle: i32, sid: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_usb_stream_close(handle, sid) }
}

pub fn usb_release(handle: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_usb_release(handle) }
}

// ── Device register (docs/spec/DEVICES.md) ───────────────────────────

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_device_register(kind: i32, rank: i32, id_ptr: i32, id_len: i32, name_ptr: i32, name_len: i32) -> i32;
    fn npk_device_remove(handle: i32) -> i32;
    fn npk_device_active(handle: i32) -> i32;
    fn npk_device_list(kind: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_device_select(handle: i32) -> i32;
}

pub const DEVICE_AUDIO_OUT: i32 = 0;
pub const DEVICE_AUDIO_IN: i32 = 1;
pub const DEVICE_CAMERA: i32 = 2;

/// Ranks for the default rule: the highest present wins.
pub const RANK_DISPLAY: i32 = 0;
pub const RANK_BUILTIN: i32 = 1;
pub const RANK_USB: i32 = 2;
pub const RANK_JACK: i32 = 3;

/// An audio driver enters one of its devices: the handle, or -1.
pub fn device_register(kind: i32, rank: i32, id: &str, name: &str) -> i32 {
    // SAFETY: FFI; both strings are borrowed for the call.
    unsafe { npk_device_register(kind, rank, id.as_ptr() as i32, id.len() as i32, name.as_ptr() as i32, name.len() as i32) }
}

pub fn device_remove(handle: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_device_remove(handle) }
}

/// Whether the caller's entry `handle` is the active one of its kind.
pub fn device_active(handle: i32) -> bool {
    // SAFETY: FFI without pointers.
    unsafe { npk_device_active(handle) == 1 }
}

/// The records of `kind` into `buf`: bytes, or -(needed + 2), or -1.
pub fn device_list(kind: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`.
    unsafe { npk_device_list(kind, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

pub fn device_select(handle: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_device_select(handle) }
}

// ── Audio capture (docs/spec/AUDIO.md) ───────────────────────────────

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_audio_capture_device_format(rate: i32, encoding: i32, channels: i32) -> i32;
    fn npk_audio_capture_wanted() -> i32;
    fn npk_audio_capture_push(ptr: i32, len: i32) -> i32;
    fn npk_audio_capture_open() -> i32;
    fn npk_audio_capture_format(stream: i32, rate: i32, encoding: i32, channels: i32) -> i32;
    fn npk_audio_capture_read(stream: i32, ptr: i32, max: i32) -> i32;
    fn npk_audio_capture_close(stream: i32) -> i32;
}

/// Driver side: what the driver records (encoding 0 S16LE, 1 S32LE, 2 F32LE).
pub fn audio_capture_device_format(rate: i32, encoding: i32, channels: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_capture_device_format(rate, encoding, channels) }
}

/// Driver side: streams listening to this driver's input; 0 means the
/// hardware may stop.
pub fn audio_capture_wanted() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_capture_wanted() }
}

/// Driver side: recorded PCM in the driver's format; the bytes taken.
pub fn audio_capture_push(pcm: &[u8]) -> i32 {
    // SAFETY: FFI; `pcm` is borrowed for the call.
    unsafe { npk_audio_capture_push(pcm.as_ptr() as i32, pcm.len() as i32) }
}

/// A capture stream from the active input (MIC right), or -1.
pub fn audio_capture_open() -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_capture_open() }
}

pub fn audio_capture_format(stream: i32, rate: i32, encoding: i32, channels: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_capture_format(stream, rate, encoding, channels) }
}

/// What the stream has, whole frames, into `buf`: bytes, or -1.
pub fn audio_capture_read(stream: i32, buf: &mut [u8]) -> i32 {
    // SAFETY: FFI; the kernel writes at most `buf.len()` bytes into `buf`.
    unsafe { npk_audio_capture_read(stream, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

pub fn audio_capture_close(stream: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_audio_capture_close(stream) }
}

// ── Camera feed (docs/spec/CAMERA.md) ────────────────────────────────

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_camera_wanted() -> i64;
    fn npk_camera_push(ptr: i32, len: i32, width: i32, height: i32, format: i32) -> i32;
    fn npk_camera_open(width: i32, height: i32) -> i32;
    fn npk_camera_read(reader: i32, buf_ptr: i32, buf_max: i32, info_ptr: i32) -> i32;
    fn npk_camera_close(reader: i32) -> i32;
}

/// Feed formats, as `npk_canvas_frame` numbers them.
pub const CAMERA_YUYV: i32 = 1;
pub const CAMERA_MJPEG: i32 = 3;

/// Producer side: (readers, width, height) the largest reader wants, or
/// None when refused.
pub fn camera_wanted() -> Option<(u32, u16, u16)> {
    // SAFETY: FFI without pointers.
    let v = unsafe { npk_camera_wanted() };
    (v >= 0).then(|| ((v & 0xFFFF) as u32, (v >> 16) as u16, (v >> 32) as u16))
}

/// Producer side: a frame as the camera sent it.
pub fn camera_push(frame: &[u8], width: u16, height: u16, format: i32) -> bool {
    // SAFETY: FFI; `frame` is borrowed for the call.
    unsafe { npk_camera_push(frame.as_ptr() as i32, frame.len() as i32, width as i32, height as i32, format) == 0 }
}

/// A reader of the active camera wanting `width` x `height`, or -1.
pub fn camera_open(width: u16, height: u16) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_camera_open(width as i32, height as i32) }
}

/// One feed frame's description.
#[derive(Clone, Copy, Default, Debug)]
pub struct CameraFrame {
    pub width: u16,
    pub height: u16,
    pub format: i32,
    pub seq: u64,
}

/// The newest frame not seen yet into `buf`: Ok(Some(len, info)), Ok(None)
/// for none, Err(needed) when `buf` is too small, or Err(0) when refused.
pub fn camera_read(reader: i32, buf: &mut [u8]) -> Result<Option<(usize, CameraFrame)>, usize> {
    let mut info = [0u8; 16];
    // SAFETY: FFI; both buffers are borrowed for the call.
    let n = unsafe { npk_camera_read(reader, buf.as_mut_ptr() as i32, buf.len() as i32, info.as_mut_ptr() as i32) };
    if n == 0 { return Ok(None); }
    if n == -1 { return Err(0); }
    if n < -1 { return Err((-n - 2) as usize); }
    let f = CameraFrame {
        width: u16::from_le_bytes([info[0], info[1]]),
        height: u16::from_le_bytes([info[2], info[3]]),
        format: info[4] as i32,
        seq: u64::from_le_bytes([info[8], info[9], info[10], info[11], info[12], info[13], info[14], info[15]]),
    };
    Ok(Some((n as usize, f)))
}

pub fn camera_close(reader: i32) -> i32 {
    // SAFETY: FFI without pointers.
    unsafe { npk_camera_close(reader) }
}
