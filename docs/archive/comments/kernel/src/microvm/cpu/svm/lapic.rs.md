# `kernel/src/microvm/cpu/svm/lapic.rs` @ 5e0102684

## L1-14 · `use crate::interrupts::rdtsc;`

```
//! Local APIC (xAPIC, MMIO @ 0xFEE00000), per vCPU — ported from Linux
//! `arch/x86/kvm/lapic.c`. Shared by the SVM and VMX backends.
//!
//! IRR / ISR / TMR live in the register page at their architectural offsets
//! (as in KVM's regs page), so the guest reads them like hardware. Priority
//! is TPR/PPR (`__apic_update_ppr`); `has_interrupt` / `ack_interrupt` are
//! `kvm_apic_has_interrupt` / `kvm_apic_ack_interrupt`; EOI clears the
//! highest in-service vector (`apic_set_eoi`). The timer expiring sets its
//! vector in IRR — it is an interrupt source like any other, delivered by the
//! same `inject_pending_event` and taken in the guest's own priority order.
//!
//! Interrupts from other vCPUs are posted: `post` sets a bit in the target's
//! lock-free descriptor and the target folds it into IRR before each entry
//! (`sync_pir_to_irr`, KVM's posted-interrupt model).
```

## L22 · `pub const APIC_BASE_MSR_VALUE: u64 = LAPIC_BASE | (1 << 11) | (1 << 8);`

```
/// IA32_APIC_BASE MSR value: base + global enable (bit 11) + BSP (bit 8).
```

## L28 · `const MSR_X2APIC_FIRST: u32 = 0x800;`

```
/// x2APIC register MSRs: 0x800 + (xAPIC offset >> 4).
```

## L32 · `const MSR_TSC_DEADLINE: u32 = 0x6E0;`

```
/// IA32_TSC_DEADLINE (armed when LVTT is in TSC-deadline mode).
```

## L34 · `const MSR_KVM_PV_EOI_EN: u32 = 0x4b56_4d04;`

```
/// `MSR_KVM_PV_EOI_EN` (kvm_para.h).
```

## L38 · `const APIC_ID: u32 = 0x20;`

```
// Register offsets (apicdef.h). Index into `regs` is `off >> 4`.
```

## L66 · `const LVT_TIMER_MODE_MASK: u32 = 0x3 << 17;`

```
/// LVTT timer mode, bits 18:17: 00 one-shot, 01 periodic, 10 TSC-deadline.
```

## L80-83 · `const MAX_PENDING: u32 = 8;`

```
/// Owed periodic ticks we carry (the PIT's reinject bound, see `pit8253`).
/// `calibrate_APIC_clock` counts LAPIC ticks against PIT jiffies one for
/// one; a tick that could not be taken yet is owed, not dropped. Repaid one
/// per acknowledgement — the guest paces the repayment, not a rate cap.
```

## L86 · `#[derive(Clone, Copy)]`

```
/// Decoded ICR write — routed by the backend (it knows the other vCPUs).
```

## L90 · `pub shorthand: u32,`

```
/// Destination shorthand (bits 19:18): 0 dest, 1 self, 2 all, 3 all-but-self.
```

## L96 · `const LVR_VALUE: u32 = 0x14 | (5 << 16);`

```
/// LVR: integrated xAPIC 0x14, MAXLVT 5 (KVM `kvm_apic_set_version`, no CMCI).
```

## L102 · `pub const MAX_VCPUS: usize = 8;`

```
// ── Posted interrupts (other vCPUs → this one) ──
```

## L106-107 · `static PIR: [[AtomicU64; 4]; MAX_VCPUS] =`

```
/// `struct pi_desc`'s PIR: one 256-bit request bitmap per vCPU. Any vCPU may
/// set bits; only the owner clears them (in `sync_posted`).
```

## L111-115 · `static PIR_ON: [AtomicBool; MAX_VCPUS] = [const { AtomicBool::new(false) }; MAX_VCPUS];`

```
/// `pi_desc.ON`: set by the poster AFTER its PIR bit, cleared by the owner
/// BEFORE it drains PIR. Whoever flips it false→true kicks, so a post that
/// lands after a drain always kicks — deriving the edge from the PIR words
/// (load, then or) lost it when the owner drained in between, and the vector
/// then sat in PIR while the target ran in guest with no exit to fold it in.
```

## L118-119 · `pub fn post(apic_id: u8, vector: u8) -> bool {`

```
/// Post `vector` to vCPU `apic_id`. True when the caller must kick the
/// target (`pi_test_and_set_on`), once per burst.
```

## L127 · `pub fn posted_pending(apic_id: u8) -> bool {`

```
/// Anything posted to `apic_id` and not yet folded into its IRR?
```

## L133-134 · `static VCPU_HOST_CORE: [AtomicUsize; MAX_VCPUS] =`

```
/// Host core each vCPU runs on (indexed by apic_id), so a sender can kick it
/// out of guest mode; `usize::MAX` = not running yet.
```

## L137-138 · `static VCPU_LAST_ACTIVE: [AtomicU64; MAX_VCPUS] = [const { AtomicU64::new(0) }; MAX_VCPUS];`

```
/// TSC of each vCPU's last exit — a running vCPU exits ≥1 kHz, a blocked one
/// goes quiet (the PV-TLB "preempted" estimate in `rip_sample`).
```

## L141-143 · `pub const ACCESS_BUCKETS: usize = 8;`

```
/// Guest LAPIC accesses by register (`cores`): each one is a trapped MMIO
/// exit with an instruction fetch + decode, so which register dominates
/// decides the cure (EOI / ICR → x2APIC or PV-EOI, TMICT → TSC deadline).
```

## L167-168 · `pub const PH_INJECT: u8 = 1;`

```
/// Where each vCPU's host thread is right now (`cores`): a hang shows as a
/// phase that stopped moving. Written at the run-loop landmarks only.
```

## L191 · `pub fn phase_snapshot(i: usize) -> Option<(u8, u64, usize)> {`

```
/// (phase, TSC it was entered, host core) for vCPU `i`.
```

## L199-200 · `pub fn vcpu_on_this_core() -> u8 {`

```
/// The vCPU running on the calling host core, or 0xFF (a device worker, the
/// shell). A sender that is a vCPU never kicks its own core.
```

## L220 · `fn kick(target: u8, self_core: usize) {`

```
/// `kvm_vcpu_kick`: get vCPU `target` out of guest mode / out of its park.
```

## L235-237 · `pub fn deliver_ipi(sender: u8, icr: &IcrWrite) {`

```
/// `kvm_irq_delivery_to_apic` for an ICR write: FIXED/LOWEST are posted to
/// the target(s) and the target is kicked on the empty→pending edge; STARTUP
/// spawns the AP; INIT/NMI/SMI are not modelled cross-vCPU.
```

## L262-264 · `pub fn pv_send_ipi(sender: u8, low: u64, high: u64, min: u64, icr: u64) -> i64 {`

```
/// `kvm_pv_send_ipi` (KVM_HC_SEND_IPI): a bitmap of physical APIC IDs from
/// `min` (64 per word in long mode), vector + delivery mode in `icr`. Returns
/// the number of vCPUs reached, or a negative KVM error.
```

## L287-290 · `pub fn deliver_msg(dest: u8, logical: bool, mode: u8, vector: u8, from: u8) {`

```
/// Deliver a message-signalled interrupt (I/O APIC redirection entry or MSI)
/// to its destination LAPIC(s) — `kvm_irq_delivery_to_apic`. `dest` is the
/// physical APIC ID (0xFF = broadcast) or, `logical`, a flat-model bitmask;
/// lowest priority goes to the first vCPU named.
```

## L295 · `_ => return, // SMI / NMI / INIT / ExtINT: no such source here`

```
// SMI / NMI / INIT / ExtINT: no such source here
```

## L311 · `pub fn reset_posted() {`

```
/// Clear every descriptor (VM start/teardown).
```

## L322-323 · `regs: [u32; 256],`

```
/// The register page, indexed by `offset >> 4` (whole 4 KiB page, so any
/// in-page offset is a safe store).
```

## L326 · `timer_start_tsc: u64,`

```
/// Host TSC the current timer count started from.
```

## L328 · `timer_fired: bool,`

```
/// One-shot: fired once for the current TMICT write.
```

## L330 · `timer_owed: u32,`

```
/// Periodic ticks owed (IRR already held the vector when they fell due).
```

## L332 · `x2apic: bool,`

```
/// IA32_APIC_BASE.EXTD: registers are MSRs 0x800+, ICR is one 64-bit write.
```

## L334 · `tsc_deadline: u64,`

```
/// IA32_TSC_DEADLINE in guest TSC = host TSC (no TSC offset); 0 = disarmed.
```

## L336 · `pv_eoi_gpa: u64,`

```
/// Guest address of its PV-EOI flag (`MSR_KVM_PV_EOI_EN`), 0 = off.
```

## L338 · `pv_eoi_pending: bool,`

```
/// `KVM_APIC_PV_EOI_PENDING`: we set the guest's flag before this entry.
```

## L343-344 · `pub fn new(apic_id: u8) -> Self {`

```
/// `kvm_lapic_reset`: LVTs masked, except the BSP's LVT0 in ExtINT
/// (KVM_X86_QUIRK_LINT0_REENABLED) — the virtual wire the PIC uses.
```

## L365 · `fn set_bit(&mut self, base: u32, v: u8) {`

```
// ── IRR / ISR bitmaps (8 × 32-bit words at 16-byte stride) ──
```

## L376 · `fn find_highest(&self, base: u32) -> Option<u8> {`

```
/// `apic_search_irr` / `apic_find_highest_isr`.
```

## L387 · `fn update_ppr(&mut self) -> u32 {`

```
/// `__apic_update_ppr`.
```

## L396 · `pub fn accept_irq(&mut self, vector: u8) {`

```
/// `__apic_accept_irq` for a fixed, edge-triggered vector.
```

## L403 · `fn sync_posted(&mut self) {`

```
/// `sync_pir_to_irr`: fold posted vectors into IRR.
```

## L417-418 · `pub fn has_interrupt(&mut self) -> Option<u8> {`

```
/// `kvm_apic_has_interrupt`: highest IRR vector above PPR, after folding
/// posted interrupts and an expired timer into IRR.
```

## L429 · `pub fn ack_interrupt(&mut self, vector: u8) {`

```
/// `kvm_apic_ack_interrupt`: IRR → ISR for `vector`.
```

## L436-437 · `pub fn accept_pic_intr(&self) -> bool {`

```
/// `kvm_apic_accept_pic_intr`: does this vCPU take the PIC's INTR (LINT0
/// in ExtINT, the virtual wire)?
```

## L443 · `fn divide_count(&self) -> u64 {`

```
// ── Timer (KVM `start_sw_timer` / `apic_timer_expired`) ──
```

## L445 · `fn divide_count(&self) -> u64 {`

```
/// TDCR → divide count (`update_divide_count`).
```

## L452 · `fn period(&self) -> Option<u64> {`

```
/// Period of one count in host TSC cycles, if the timer is armed.
```

## L471-472 · `fn tscdeadline_poll(&mut self) {`

```
/// TSC-deadline expiry (`start_sw_tscdeadline` → `apic_timer_expired`):
/// the deadline is consumed; a masked LVTT delivers nothing.
```

## L483-485 · `fn timer_poll(&mut self) {`

```
/// Expire the timer: set LVTT's vector in IRR. A periodic tick that falls
/// due while the previous one still sits in IRR is owed (bounded) and
/// posted once IRR is free again.
```

## L515 · `pub fn next_timer_deadline_tsc(&self) -> Option<u64> {`

```
/// Host TSC of the next timer expiry (park and host one-shot), or None.
```

## L521-522 · `if !self.periodic() && self.timer_fired { return None; }`

```
// An owed tick is not a deadline: it is delivered on the exit the
// guest's EOI of the previous one causes.
```

## L527 · `fn tmcct(&self) -> u32 {`

```
/// APIC_TMCCT (`apic_get_tmcct`).
```

## L542 · `fn set_eoi(&mut self) {`

```
/// `apic_set_eoi`: clear the highest in-service vector.
```

## L550-556 · `pub fn pv_eoi_sync_to(&mut self, mem: &crate::microvm::devices::guest_mem::GuestMem) {`

```
// ── PV EOI (KVM `apic_sync_pv_eoi_to_guest` / `_from_guest`) ──
//
// With the flag set, the guest's EOI is a bit clear in its own memory
// instead of a trapped register write; we complete it at the next exit.
// Offered only when the EOI can have no side effect: nothing else pending
// in IRR and exactly one vector in service (edge-triggered — every vector
// this LAPIC accepts is).
```

## L558 · `pub fn pv_eoi_sync_to(&mut self, mem: &crate::microvm::devices::guest_mem::GuestMem) {`

```
/// Before entry: arm the guest's flag if its next EOI may be lazy.
```

## L567-568 · `pub fn pv_eoi_sync_from(&mut self, mem: &crate::microvm::devices::guest_mem::GuestMem) {`

```
/// After exit: flag cleared by the guest = it did EOI → complete it here;
/// still set = no EOI yet → take the flag back (its EOI then traps).
```

## L582 · `pub fn msr(&mut self, msr: u32, write: Option<u64>) -> Option<Result<(u64, Option<IcrWrite>), ()>> {`

```
// ── MSR interface: IA32_APIC_BASE, x2APIC, PV-EOI enable ──
```

## L584-585 · `pub fn msr(&mut self, msr: u32, write: Option<u64>) -> Option<Result<(u64, Option<IcrWrite>), ()>> {`

```
/// `Some(result)` if `msr` belongs to the LAPIC, `None` otherwise.
/// A write may yield an IPI for the caller to route.
```

## L595-596 · `if v & APIC_BASE_X2APIC != 0 && v & APIC_BASE_ENABLE != 0 && !self.x2apic {`

```
// xAPIC → x2APIC needs the global enable; x2APIC → xAPIC is
// not a legal transition (SDM 10.12.5) and is ignored.
```

## L603 · `(MSR_TSC_DEADLINE, None) => {`

```
// `kvm_set_lapic_tscdeadline_msr`: only in TSC-deadline mode.
```

## L615 · `if v & 0x2 != 0 { return Some(Err(())); } // reserved, 4-byte aligned`

```
// reserved, 4-byte aligned
```

## L628 · `fn x2apic_access(&mut self, off: u32, write: Option<u64>) -> Result<(u64, Option<IcrWrite>), ()> {`

```
/// `kvm_x2apic_msr_read` / `kvm_x2apic_msr_write`.
```

## L631 · `(APIC_DFR, _) | (APIC_ICR2, _) | (X2APIC_SELF_IPI, None) => Err(()),`

```
// No DFR, no ICR2 in x2APIC mode; SELF_IPI is write-only.
```

## L638 · `let id = self.apic_id as u32;`

```
// Cluster = id[31:4], bit id[3:0] (x2APIC logical ID, read-only).
```

## L661 · `(APIC_ID | APIC_LDR, Some(_)) => Err(()),`

```
// Read-only in x2APIC mode.
```

## L672 · `pub fn read(&mut self, off: u32) -> u32 {`

```
// ── MMIO (`kvm_lapic_reg_read` / `kvm_lapic_reg_write`) ──
```

## L683 · `#[must_use]`

```
/// Returns the decoded IPI for an ICR-low write; the backend routes it.
```

## L696 · `if val & SPIV_APIC_ENABLED == 0 {`

```
// Software-disabling masks every LVT (KVM `kvm_lapic_reg_write`).
```

## L704 · `APIC_TMICT if self.tscdeadline_mode() => {}`

```
// Ignored in TSC-deadline mode (KVM `kvm_lapic_reg_write`).
```

## L716 · `if off & 0xFF0 == APIC_LVTT`

```
// A timer-mode change stops the timer (`apic_update_lvtt`).
```

## L737 · `APIC_LVR | APIC_PROCPRI => {}`

```
// Read-only: LVR, PPR, ISR, TMR, IRR.
```

