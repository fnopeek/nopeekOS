//! A heap that grows, and gives back.
//!
//! Shared rather than copied because the two interesting numbers in it —
//! the doubling step and its cap — should be tuned in one place.
//!
//! Use it when the app allocates and frees repeatedly: fonts, layout trees,
//! decoded frames. An app that allocates once and lives off it (`top`,
//! `volume`) is better served by a bump heap and should not turn the
//! `heap` feature on at all.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();
//! ```

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_log_serial(ptr: i32, len: i32);
}

fn log(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    // SAFETY: pointer + length of a live `&str`; the host only reads it.
    unsafe { npk_log_serial(msg.as_ptr() as i32, msg.len() as i32) }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = msg;
}

/// Ported from talc's own `WasmGrowAndExtend` (talc 5.0.4, `src/wasm.rs`)
/// with exactly one change: the growth step.
///
/// Talc grows by just enough for the allocation that failed. That is right
/// where `memory.grow` is cheap. Under wasmi it is not — linear memory is one
/// contiguous buffer, so every grow copies all of it, and growing a page at a
/// time up to a 60 MB working set would copy tens of gigabytes. Doubling makes
/// the number of grows logarithmic and the total copying linear in the final
/// size, at the cost of holding up to twice the peak.
#[derive(Debug)]
pub struct GrowingHeap {
    /// End of the arena talc last received, so a contiguous grow can extend it
    /// instead of starting a second heap. Zero means "nothing handed over yet".
    ///
    /// An address and not a `NonNull`, which is what talc's own source stores:
    /// `NonNull` is not `Send`, and this allocator sits behind a mutex on
    /// purpose (see the note on `Allocator`) rather than behind talc's
    /// single-threaded cell.
    end: usize,
}

impl GrowingHeap {
    pub const fn new() -> Self {
        GrowingHeap { end: 0 }
    }
}

const WASM_PAGE: usize = 64 * 1024;

/// Largest growth step: 64 MB. See `acquire` — doubling without a cap
/// turns, past a few hundred MB, into a request the kernel cannot satisfy.
const GROW_CAP_PAGES: usize = 1024;

// SAFETY: `acquire` hands talc only memory that `memory.grow` just returned —
// freshly mapped pages past the previous end of linear memory, which nothing
// else can reach. It allocates nothing itself. That is talc's own
// `WasmGrowAndExtend` contract, kept.
#[cfg(target_arch = "wasm32")]
unsafe impl talc::source::Source for GrowingHeap {
    fn acquire<B: talc::base::binning::Binning>(
        talc: &mut talc::base::Talc<Self, B>,
        layout: core::alloc::Layout,
    ) -> Result<(), ()> {
        // Over-estimate deliberately: talc warns that under-sizing here loops
        // forever, handing over heaps that can never fit the allocation.
        let need = layout.size() + layout.align() + 4 * WASM_PAGE;
        let need_pages = need.div_ceil(WASM_PAGE);
        let have_pages = core::arch::wasm32::memory_size::<0>();
        // Double, but not forever. Up to 64 MB doubling is right: few
        // requests, little waste. Past that a doubled request can exceed
        // what the kernel can map, and the allocation fails although a few
        // pages would have done.
        let step = have_pages.max(1).min(GROW_CAP_PAGES);
        let delta = need_pages.max(step);

        // Ask smaller in turn instead of giving up at the first no: the
        // kernel may refuse half a gigabyte and grant the few pages the
        // caller actually needs.
        let mut delta = delta;
        let prev_end = loop {
            let got = core::arch::wasm32::memory_grow::<0>(delta);
            if got != usize::MAX { break got }
            if delta <= need_pages {
                // Now memory really is exhausted — log it with the numbers, not as
                // a bare panic line. Without allocating: we are inside the
                // allocator.
                log("[npk] Halde erschoepft — memory.grow hat nein gesagt");
                log("[npk]   Seiten bisher:");
                log(u32_str(have_pages as u32));
                log("[npk]   noch gebraucht (Seiten):");
                log(u32_str(need_pages as u32));
                return Err(());
            }
            delta = (delta / 2).max(need_pages);
        };
        let base = (prev_end * WASM_PAGE) as *mut u8;
        let size = delta * WASM_PAGE;

        let old_end = core::mem::replace(&mut talc.source.end, 0);
        if old_end == base as usize {
            // SAFETY: contiguous with the arena we handed over last time, and
            // `old_end` came from talc itself, so it is non-null.
            let new_end = unsafe {
                talc.extend(
                    core::ptr::NonNull::new_unchecked(base),
                    base.wrapping_add(size),
                )
            };
            talc.source.end = new_end.as_ptr() as usize;
            return Ok(());
        }
        // SAFETY: fresh pages, owned by nothing else.
        talc.source.end = unsafe { talc.claim(base, size) }.map_or(0, |e| e.as_ptr() as usize);
        Ok(())
    }
}

/// `TalcLock` (mutex-guarded), not talc's `WasmArenaTalc`/`TalcSyncCell`. The
/// cell variants are only sound on single-threaded WebAssembly and enforce
/// that with a target check, not the type system — so the day an app gets
/// workers, or wasmi turns on the threads proposal, they would go quietly
/// unsound. The uncontended spin lock costs a few instructions; that is the
/// cheaper mistake.
#[cfg(target_arch = "wasm32")]
pub type Allocator = talc::TalcLock<spin::Mutex<()>, GrowingHeap>;

#[cfg(target_arch = "wasm32")]
pub const fn new() -> Allocator {
    talc::TalcLock::new(GrowingHeap::new())
}

// u32 → decimal &str in a static buffer (no alloc — safe in the panic handler
// even when the panic is an allocation failure).
static mut NUMBUF: [u8; 12] = [0; 12];

/// `u32` to decimal without allocating, into a static buffer. Public
/// because a panic handler needs exactly this and must not allocate — the
/// panic may be an allocation failure.
pub fn u32_str(mut n: u32) -> &'static str {
    let b = core::ptr::addr_of_mut!(NUMBUF) as *mut u8;
    let mut i = 12usize;
    if n == 0 {
        i -= 1;
        // SAFETY: `i` is in range of the 12-byte buffer.
        unsafe { *b.add(i) = b'0' };
    }
    while n > 0 && i > 0 {
        i -= 1;
        // SAFETY: as above.
        unsafe { *b.add(i) = b'0' + (n % 10) as u8 };
        n /= 10;
    }
    // SAFETY: the bytes written are ASCII digits, and the slice is inside
    // the static buffer, which lives for the whole program.
    unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(b.add(i), 12 - i)) }
}
