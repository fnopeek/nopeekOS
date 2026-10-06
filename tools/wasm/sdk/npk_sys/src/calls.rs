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
    fn npk_fs_list(prefix_ptr: i32, prefix_len: i32, out_ptr: i32, out_cap: i32, recursive: i32) -> i32;
    fn npk_fs_stat(name_ptr: i32, name_len: i32, out_ptr: i32) -> i32;
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
    fn npk_audio_get_volume() -> i32;
    fn npk_audio_set_volume(pct: i32) -> i32;
    fn npk_canvas_commit(canvas_id: i32, ptr: i32, len: i32, w: i32, h: i32) -> i32;
    fn npk_canvas_rect(canvas_id: i32, out_ptr: i32) -> i32;
    fn npk_pick(mode: i32, start_ptr: i32, start_len: i32,
                suggest_ptr: i32, suggest_len: i32, tag: i32) -> i32;

    fn npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn npk_pci_bind_class_n(class: i32, subclass: i32, index: i32) -> i32;
    fn npk_pci_read_config(offset: i32) -> i32;
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
    fn npk_memory_fence() -> i32;
    fn npk_dma_alloc(pages: i32) -> i32;
    fn npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;
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
    fn npk_netdev_submit_rx(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_rx_deliver(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_poll_tx(buf_ptr: i32, max: i32) -> i32;
    fn npk_netdev_set_link(up: i32) -> i32;
    fn npk_netdev_set_link_state(carrier: i32, dormant: i32) -> i32;
    fn npk_wifi_send_cmd(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_cmd(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_send_event(buf_ptr: i32, len: i32) -> i32;
    fn npk_wifi_poll_event(buf_ptr: i32, max: i32) -> i32;

    fn npk_acpi_dsdt(buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_acpi_table(sig: i32, index: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_acpi_mem_read(hi: i32, lo: i32) -> i32;
    fn npk_sci_arm(gpe: i32) -> i32;
    fn npk_sci_service() -> i32;
    fn npk_ec_read(addr: i32) -> i32;
    fn npk_ec_write(addr: i32, val: i32) -> i32;
    fn npk_ec_query() -> i32;
    fn npk_battery_report(packed: i32);
    fn npk_battery_detail(rate: i32, remaining: i32, full: i32, voltage_mv: i32, unit: i32);
    fn npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;
    fn npk_audio_poll_mix(ptr: i32, max: i32) -> i32;
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
    fn audio_get_volume = npk_audio_get_volume() -> i32;
    fn audio_set_volume = npk_audio_set_volume(pct: i32) -> i32;

    fn pci_bind = npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn pci_bind_class = npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn pci_bind_class_n = npk_pci_bind_class_n(class: i32, subclass: i32, index: i32) -> i32;
    fn pci_read_config = npk_pci_read_config(offset: i32) -> i32;
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
    fn memory_fence = npk_memory_fence() -> i32;
    fn dma_alloc = npk_dma_alloc(pages: i32) -> i32;
    fn dma_alloc_below = npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;
    fn dma_phys_addr = npk_dma_phys_addr(handle: i32) -> i64;
    fn dma_read32 = npk_dma_read32(handle: i32, offset: i32) -> i32;
    fn dma_write32 = npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn irq_register = npk_irq_register(entry: i32) -> i32;
    fn irq_register_gsi = npk_irq_register_gsi(gsi: i32, flags: i32) -> i32;
    fn irq_arm = npk_irq_arm(vector: i32) -> i64;
    fn irq_wait = npk_irq_wait(vector: i32, since: i64, timeout_ms: i32) -> i32;

    fn netdev_set_link = npk_netdev_set_link(up: i32) -> i32;
    fn netdev_set_link_state = npk_netdev_set_link_state(carrier: i32, dormant: i32) -> i32;

    fn acpi_mem_read = npk_acpi_mem_read(hi: i32, lo: i32) -> i32;
    fn sci_arm = npk_sci_arm(gpe: i32) -> i32;
    fn sci_service = npk_sci_service() -> i32;
    fn ec_read = npk_ec_read(addr: i32) -> i32;
    fn ec_write = npk_ec_write(addr: i32, val: i32) -> i32;
    fn ec_query = npk_ec_query() -> i32;
    fn battery_report = npk_battery_report(packed: i32);
    fn battery_detail = npk_battery_detail(rate: i32, remaining: i32, full: i32, voltage_mv: i32, unit: i32);
    fn pointer_inject = npk_pointer_inject(dx: i32, dy: i32, buttons: i32, scroll: i32, hscroll: i32) -> i32;
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
}

fills! {
    fn random_bytes = npk_random_bytes -> i32;
    fn home_dir = npk_home_dir -> i32;
    fn launch_arg = npk_launch_arg -> i32;
    fn event_poll = npk_event_poll -> i32;
    fn window_titles = npk_window_titles -> i32;
    fn netdev_poll_tx = npk_netdev_poll_tx -> i32;
    fn wifi_poll_cmd = npk_wifi_poll_cmd -> i32;
    fn wifi_poll_event = npk_wifi_poll_event -> i32;
    fn acpi_dsdt = npk_acpi_dsdt -> i32;
    fn audio_poll_mix = npk_audio_poll_mix -> i32;
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

/// Size (8 bytes LE), directory flag (1), mtime (8 bytes LE) into `out`.
pub fn fs_stat(name: &[u8], out: &mut [u8; 17]) -> i32 {
    // SAFETY: FFI; `out` is the 17 bytes the kernel writes.
    unsafe { npk_fs_stat(name.as_ptr() as i32, name.len() as i32, out.as_mut_ptr() as i32) }
}

pub fn canvas_commit(canvas_id: i32, px: &[u8], w: i32, h: i32) -> i32 {
    // SAFETY: FFI; the range is borrowed for the call.
    unsafe { npk_canvas_commit(canvas_id, px.as_ptr() as i32, px.len() as i32, w, h) }
}

/// `x, y, w, h` as four i32 LE into `out`.
pub fn canvas_rect(canvas_id: i32, out: &mut [u8; 16]) -> i32 {
    // SAFETY: FFI; `out` is the 16 bytes the kernel writes.
    unsafe { npk_canvas_rect(canvas_id, out.as_mut_ptr() as i32) }
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
    pub fn w8(self, off: u32, v: u8) { mmio_write8(self.0, off as i32, v as i32); }
    pub fn w16(self, off: u32, v: u16) { mmio_write16(self.0, off as i32, v as i32); }
    pub fn w32(self, off: u32, v: u32) { mmio_write32(self.0, off as i32, v as i32); }
    pub fn w64(self, off: u32, v: u64) { mmio_write64(self.0, off as i32, v as i64); }
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
