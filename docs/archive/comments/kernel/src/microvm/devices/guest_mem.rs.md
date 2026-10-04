# `kernel/src/microvm/devices/guest_mem.rs` @ 5e0102684

## L1-22 · `#![allow(dead_code)]`

```
//! Guest physical memory accessors.
//!
//! `GuestMem` is the single translation point between a guest-physical
//! address and host memory. In B1 the guest RAM window is still one
//! contiguous host range, so translation is `host_phys = base + gpa`.
//! B3 turns this into a scattered, demand-paged lookup *without*
//! touching any caller — every device-side guest access already goes
//! through here. Callers therefore MUST NOT assume linearity or that a
//! multi-page buffer is contiguous in host memory; use the accessors.
//!
//! All accessors bounds-check against `len` (the advertised guest RAM
//! size) so a buggy/malicious guest descriptor can't drag us into
//! kernel memory.
//!
//! `GUEST_RAM_BYTES` is the canonical guest-RAM size. It used to be
//! duplicated in five places (this file, `guest_fetch`,
//! `vmx::ept::GUEST_WINDOW_BYTES`, svm npt window,
//! `bzimage::GUEST_RAM_TOTAL`); a stale copy from the 256 MB→1 GB bump
//! silently turned every virtio DMA / insn-fetch above the stale bound
//! into a no-op (→ SQUASHFS/EIO corruption under a memory-hungry
//! guest). Those four now reference this constant; B2 replaces it with
//! a runtime value chosen at `vm_open`.
```

## L31-36 · `static ACTIVE_GM: AtomicPtr<GuestMem> = AtomicPtr::new(ptr::null_mut());`

```
/// The active microvm's `GuestMem`, held OUTSIDE `VmShared` so the off-vCPU
/// network backend can share `&GuestMem` across cores without aliasing the
/// vCPU's `&mut VmShared`. Sound because `GuestMem` is `&self`-only + `Sync`
/// (its only interior mutability is the atomic software TLB) — a vCPU's
/// `&mut VmShared` governs only the stored reference, never the pointee. One
/// instance (one microvm at a time); a future multi-VM world keys this per VM.
```

## L39-41 · `pub fn set_active(gm: GuestMem) -> &'static GuestMem {`

```
/// Install `gm` as the active guest memory and return a `'static` handle to it.
/// The `'static` is self-managed: `clear_active()` frees it at VM close, after
/// all vCPUs + the backend fiber have stopped touching it.
```

## L45-46 · `unsafe { &*p }`

```
// SAFETY: `p` was just allocated and is non-null; it stays live until
// `clear_active()` (called only at close, after the VM threads stop).
```

## L50-51 · `pub fn active() -> Option<&'static GuestMem> {`

```
/// The active guest memory, if a microvm is running. Used by the off-vCPU
/// backend fiber (which has no `VmShared` handle).
```

## L55 · `unsafe { Some(&*p) }`

```
// SAFETY: non-null ⇒ set_active installed a live Box not yet cleared.
```

## L60-62 · `pub fn clear_active() {`

```
/// Free the active guest memory at VM close. Idempotent. Caller guarantees no
/// vCPU or backend fiber still holds a handle (teardown order: stop threads →
/// release page tables → clear_active).
```

## L66 · `unsafe { drop(Box::from_raw(p)); }`

```
// SAFETY: `p` came from `Box::into_raw` in set_active and is freed once.
```

## L71-73 · `const TLB_SIZE: usize = 1024;`

```
/// Software-TLB size (direct-mapped, power of two). 1024 slots cover a 4 MiB
/// working set of distinct guest pages — far more than the RX/TX buffer pool the
/// net hot path reuses. 8 KiB per VM.
```

## L76-85 · `pub const GUEST_RAM_BYTES: u64 = 2 * 1024 * 1024 * 1024;`

```
/// Canonical guest-RAM size: 2 GiB. Real-world browsers (Firefox/
/// LibreWolf with e10s + content sandboxing on) routinely hit 1 GiB
/// resident with a couple of moderate tabs — 1 GiB is below the
/// floor of "browser actually works". Bumped from 1 GiB once B4
/// multi-PD landed (the single-PD path capped us at 1 GiB).
///
/// Cap (host has 4 GiB QEMU / NUC bare metal has 16 GiB). With B3
/// demand-paging on, only the 256 MiB boot window is committed
/// contiguous at vm_open; the rest is faulted in 4 KiB at a time as
/// the guest touches it.
```

## L88-93 · `pub const DEMAND_ENABLED: bool = true;`

```
/// B3 demand-paging master switch. `false` → the whole guest is one
/// contiguous block (boot window = full guest, no demand PTs, no
/// scatter walk). `true` → the 256 MiB hybrid (contiguous boot
/// window + 4 KiB demand region). Re-enabled to back the 2 GiB bump:
/// a contiguous 2 GiB block on a 4 GiB host is fragile, demand-paging
/// commits only touched pages and lets the host stay responsive.
```

## L96-97 · `#[derive(Clone, Copy)]`

```
/// Which second-level paging format backs the demand region, so
/// `GuestMem` can fault a page in without pulling in `cpu::Vendor`.
```

## L104-114 · `static DEMAND_LOCK: spin::Mutex<()> = spin::Mutex::new(());`

```
/// Translates guest-physical addresses to host memory for one VM.
///
/// B3 hybrid: `[0, boot_bytes)` is one contiguous host block
/// (`boot_base`, 2-MB EPT/NPT leaves — fast, fault-free early boot);
/// `[boot_bytes, len)` is demand-paged 4 KB on first touch. The
/// page-table tree IS the gpa→host map — `page_host` walks/faults via
/// `ept`/`npt::demand_fault_in`, so a host DMA to a page the guest
/// hasn't touched yet still works (and a guest PT the insn-fetch
/// walker reads simply faults in — no recursion: fault-in is a flat
/// alloc+map). Not `Copy`. Threaded as `&GuestMem`.
/// Serialises demand fault-in (see `page_host`).
```

## L123-130 · `tlb: [AtomicU64; TLB_SIZE],`

```
/// Software TLB for the demand region: caches the page-table walk
/// (`demand_fault_in`) that `page_host` would otherwise repeat on EVERY
/// access. The demand map is STABLE once faulted (a guest page → one host
/// frame for the VM's life — no remapping until balloon, which doesn't
/// exist), so entries never need invalidation. Each slot is ONE atomic u64
/// packing `(page>>12)<<32 | (host>>12)` → lock-free, no torn reads, safe
/// for concurrent AP-vCPU access. 0 = empty. Killed the `inject=33µs`
/// per-frame walk (RX buffers are reused → near-100% hit rate).
```

## L148 · `#[inline]`

```
/// Advertised guest RAM size in bytes.
```

## L154-156 · `#[inline]`

```
/// Host phys of the 4 KB page containing `page` (must be
/// page-aligned). Boot window → linear; demand region → walk +
/// fault-in. `None` only if `page` is outside the window.
```

## L165-168 · `let pn = page >> 12;`

```
// Software TLB: the demand map is stable once faulted, so a hit skips the
// EPT/NPT walk entirely. One atomic load, no lock, no torn read (tag+host
// packed in a single u64). pn ≥ 65536 here (page ≥ 256 MiB boot window),
// so a packed entry is never 0 → 0 reliably means "empty".
```

## L175-178 · `let _g = DEMAND_LOCK.lock();`

```
// One fault-in at a time: the vCPUs (#NPF) and the net worker (DMA
// into an untouched page) can race to the same empty PTE, and two
// frames for one page lose whatever the loser wrote. The walk
// re-checks the PTE under the lock, so the second one just reads it.
```

## L188 · `self.tlb[slot].store((pn << 32) | (host >> 12), Ordering::Relaxed);`

```
// host is page-aligned (a fresh demand frame), so host>>12<<12 == host.
```

## L193-196 · `pub fn ensure(&self, gpa: u64) -> bool {`

```
/// Fault the 4 KB page containing `gpa` into the demand region
/// (no-op for the boot window). Called from the EPT-violation /
/// #NPF handler: `true` → mapped, re-enter the guest; `false` →
/// `gpa` is outside the window (a real fault → fatal dump).
```

## L201-203 · `#[inline]`

```
/// Fast path: host addr of `[gpa, gpa+n)` iff it lies wholly in
/// the physically-contiguous boot window (one span, no per-page
/// walk). The common case for early boot + low virtqueue rings.
```

## L266-269 · `pub fn read_bytes(&self, gpa: u64, dst: &mut [u8]) -> bool {`

```
/// Per-page scatter copy. The boot window is one contiguous span
/// (fast memcpy); the demand region is walked page-by-page (each
/// 4 KB may be a different scattered host frame, faulted in on
/// first touch). `false` iff `[gpa, gpa+len)` leaves the window.
```

