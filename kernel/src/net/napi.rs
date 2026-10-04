//! NAPI: the host NIC is drained by its own fiber, woken by the card's RX
//! interrupt — Linux `napi_schedule` → `net_rx_action` → `napi_poll`.
//!
//! Core 0 is tickless, so draining from its shell loop would cap a card at
//! one ring per loop pass; the interrupt must wake a fiber that drains.
//!
//! The drain itself is `poll_rx_only` (NIC → `eth::handle_frame`, TCP timers,
//! TX flush); the driver re-enables its RX interrupt when it finds the ring
//! empty (`virtio_net::recv`), which is `napi_complete_done`.

use core::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// True while the NAPI fiber owns the active card's RX interrupt. Core 0 then
/// no longer needs its 10 ms cadence for the NIC (`net::needs_tick`).
pub fn active() -> bool {
    RUNNING.load(Ordering::Acquire) && crate::netdev::rx_wake_vector().is_some()
}

/// Start the fiber on a worker core. No-op without workers: the loop never
/// returns, and `spawn_fiber` would run it inline on the boot core.
pub fn start() {
    if crate::smp::scheduler::worker_count() == 0 { return; }
    if RUNNING.swap(true, Ordering::AcqRel) { return; }
    crate::smp::scheduler::spawn_fiber(napi_fiber, 0);
}

/// Park bound while TCP timers are pending (delayed ACK, RTO): the timers run
/// on the 10 ms `ticks()` grid, so waking more often buys nothing.
const TIMER_PARK_MS: u64 = 10;
/// Park bound otherwise — only a card switch (cable pulled, WiFi took over)
/// needs a re-check without an interrupt.
const IDLE_PARK_MS: u64 = 1000;

fn napi_fiber(_arg: u64) {
    loop {
        let Some(vector) = crate::netdev::rx_wake_vector() else {
            // The active card raises no RX interrupt (polled or WASM driver):
            // Core 0 / the driver's own fiber drain it. Look again later.
            crate::smp::fiber::yield_sleep(IDLE_PARK_MS);
            continue;
        };
        // Snapshot before draining (and route the vector to this core): an
        // interrupt that lands during the drain advances the count past the
        // snapshot, so the wait below returns at once — no lost wakeup.
        let since = crate::irq::arm(vector);
        crate::net::poll_rx_only();
        let park = if super::tcp::has_timers() { TIMER_PARK_MS } else { IDLE_PARK_MS };
        crate::irq::wait(vector, since, park);
    }
}
