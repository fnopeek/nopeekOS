# Cheap cross-vCPU interrupts: host mitigations, APICv, AVIC

Why: a TLB shootdown (or any IPI) aimed at a vCPU that is running in the
guest costs ~5 µs on the N100 (`vmbench`, `npkbench_vm`), native ~1 µs.
Every multi-threaded guest program that frees or re-protects memory pays it;
the browser's allocator made an upload stall for 1.5 s per round
(`npkscudo`, worked around in the guest). Measured on the N100:

| per shootdown to a running vCPU | IBPB on exit | IBPB off |
|---|---|---|
| unmap | +6.4 µs | +4.0 µs |
| mprotect | +5.1 µs | +3.9 µs |

The rest is the target's VM exit + re-entry (no posted interrupts), the
sender's hypercall exit, and 4× XSAVE/XRSTOR per exit.

Source of truth: Linux 6.18.55 (`~/.cache/nopeekos/linux-src/linux-6.18.55`).
Paths below: `bugs.c` = arch/x86/kernel/cpu/bugs.c, `K/` = arch/x86/kvm/.

## Stage 1 — host mitigations as Linux chooses them

Our host has no retpolines, no return thunks, and never set IBRS: IBPB on
every exit was the only guest→host branch-predictor protection. Linux
without thunks (bugs.c):

- spectre_v2: eIBRS if present (2339-2343): Intel `SPEC_CTRL.IBRS`
  (ARCH_CAPABILITIES.IBRS_ALL), AMD `EFER.AUTOIBRS` (CPUID 8000_0021 EAX[8])
  (2426-2431). Without eIBRS and without retpolines: NONE (2104-2106).
- BHI (Intel eIBRS parts): `BHI_DIS_S` (SPEC_CTRL bit 10) if CPUID 7.2
  EDX[4], else `clear_bhb_loop` on VM exit (2251-2278).
- PBRSB (eIBRS without PBRSB_NO): one CALL + LFENCE after VM exit
  (2159-2165). We fill all 32 entries, a superset.
- retbleed on AMD 0x15-0x17 without UNRET: IBPB, i.e. IBPB_ON_VMEXIT
  (1407-1411, 1499-1517).
- SRSO (AMD 0x17/0x19/0x1A unless SRSO_NO, 8000_0021 EAX[29]): safe-RET
  needs thunks; with SRSO_USER_KERNEL_NO (EAX[30]) → IBPB on VM exit; with
  SRSO_BP_SPEC_REDUCE (EAX[31]) → BP_CFG (0xC001102E) bit 4 instead
  (3140-3149, K/svm/svm.c:1349-1353).
- KVM restores the host SPEC_CTRL in asm before the first RET after the
  exit (K/vmx/vmenter.S:258-277, K/svm/vmenter.S:211-224), and swaps to the
  guest value on entry whenever guest ≠ host.

Choice per CPU (where Linux would leave a hole because thunks are missing,
we keep IBPB on exit):

| CPU | boot, every core | after each exit |
|---|---|---|
| Intel eIBRS + BHI_CTRL (N100 expected) | SPEC_CTRL = IBRS \| BHI_DIS_S | restore host SPEC_CTRL, RSB fill; no IBPB |
| Intel eIBRS without BHI_CTRL | SPEC_CTRL = IBRS | + IBPB (stands in for clear_bhb_loop) |
| Intel without eIBRS | — | IBPB (as before) |
| AMD Zen 2 / Zen 3 (no AutoIBRS) | — | IBPB (as before) |
| AMD AutoIBRS, not SRSO-affected | EFER.AUTOIBRS | no IBPB |
| AMD AutoIBRS, SRSO + BP_SPEC_REDUCE (Zen 5 expected) | EFER.AUTOIBRS, BP_CFG bit 4 | no IBPB |
| AMD AutoIBRS, SRSO otherwise (Zen 4) | EFER.AUTOIBRS | IBPB |

Not covered (named): STIBP on SMT AMD, VERW before entry (RFDS/MMIO/TSA),
IBPB on vCPU switch (vCPUs never share a core with each other).

## Stage 2 — posted interrupts

### VMX APICv (N100)

KVM: K/vmx/vmx.c, posted_intr.c, common.h, lapic.c.
- Controls: pin POSTED_INTR; primary TPR_SHADOW (clear CR8 exiting);
  secondary APIC_REGISTER_VIRT, VIRTUAL_INTR_DELIVERY,
  VIRTUALIZE_X2APIC_MODE (when the guest sets EXTD); exit control
  ACK_INTR_ON_EXIT (required with posted interrupts; the external-interrupt
  exit then dispatches the acked host vector by hand,
  vmx.c:7016-7049). Optional tertiary IPI_VIRT (MSR 0x492).
- Fields: POSTED_INTR_NV 0x0002 (own host vector, not the kick vector),
  POSTED_INTR_DESC_ADDR 0x2016, VIRTUAL_APIC_PAGE_ADDR 0x2012,
  GUEST_INTR_STATUS 0x0810 (RVI = max IRR on every entry, SVI on ISR
  change), EOI_EXIT_BITMAP0-3 = 0 (our IOAPIC is edge only), TPR_THRESHOLD 0.
- PI descriptor: 64-byte aligned, PIR bits 0-255, ON bit 256, SN 257,
  NV byte 34, NDST u32 at 36 (xAPIC host: id<<8).
- Post: set PIR bit, set ON; on the edge, if the target is in guest mode
  send NV to its core, else wake (common.h:111-176).
- Entry: after IN_GUEST is set, `sync_pir_to_irr`: clear ON, OR PIR into the
  page IRR, RVI = max IRR (vmx.c:6924-6968).
- x2APIC MSR bitmap: reads 0x800-0x83F pass through except TMCCT; writes
  intercepted except TPR, EOI, SELF_IPI (vmx.c:4043-4098). Exits 45
  EOI_INDUCED, 56 APIC_WRITE.
- Injection: LAPIC vectors are never injected or windowed; only ExtINT
  (irq.c:96-104). PV-EOI is never armed (lapic.c:3428-3446).

### SVM AVIC (Zen 2 hybrid, Zen 5 x2AVIC)

KVM: K/svm/avic.c, svm.c.
- Detect CPUID 8000_000A EDX[13] AVIC, [18] x2AVIC. KVM default: on only
  with x2AVIC and Zen 4+ (avic.c:1195-1197); Zen 2 needs avic=1 and
  erratum 1235 forces IsRunning never set (avic.c:1253-1254, 991-997).
- x2APIC guest without x2AVIC: hybrid — all x2APIC MSRs intercepted, only
  doorbell delivery to running vCPUs (avic.c:121-122, 197-202).
- VMCB: avic_vapic_bar 0x098 = 0xFEE00000, backing page 0xE0, logical table
  0xF0, physical table 0xF8 (bits 8:0 max index); int_ctl AVIC_ENABLE
  bit 31, X2APIC_MODE bit 30. Physical entry: host APIC ID 11:0, backing PA
  51:12, IsRunning 62, Valid 63.
- Host delivery: set IRR in the backing page, fence, if in guest
  `wrmsr 0xC001011B = host APIC ID` (doorbell), else wake (svm.c:3750-3797).
- Exits 0x401 INCOMPLETE_IPI (avic.c:571-618), 0x402 UNACCELERATED_ACCESS.
- ExtINT: deactivate AVIC and use VINTR (IRQWIN inhibit, svm.c:3939-3956).

### Shared prerequisite

`LocalApic.regs` becomes a 4 KiB page in the architectural layout (u32 at
byte offset `off`), at a stable physical address, with IRR as atomics.
x2APIC ICR stored as one u64 at 0x300; LDR stored, not computed. The same
page serves as VMX virtual-APIC page and SVM backing page.

## Stage 3 — FPU swap

Keep the swap (the host is built with SSE/AVX) but use XSAVEOPT/XSAVES as
Linux does, which skip unmodified components.

## Exit criterion

`npkbench_vm`: "others running" within ~1.5× of "alone"; upload fast with
`npkscudo=off` → remove the switch.

## Stage 4 — the remaining Linux mitigations

`cpu_vulns.rs` ports `cpu_set_bug_bits`; its vendor tables are generated
from Linux (`tools/gen_cpu_vulns.py <linux-src>`, rerun on every Linux
bump). `spec_mitigations::report_bugs` decides VERW (MDS, TAA, RFDS with
`RFDS_CLEAR`, MMIO with MDS/TAA; TSA with `VERW_CLEAR` on AMD), the L1D
flush (L1TF, KVM `always`), legacy IBRS (retbleed Intel without eIBRS) and
STIBP (SMT AMD with IBPB for retbleed). The boot line `[npk] cpu bugs:`
names every affected bug with what covers it, or `NOT MITIGATED`. Known
open by design: ITS on parts that are not `ITS_NATIVE_ONLY` (Linux needs
thunks there), and any bug whose microcode is missing.

## Verified hardware

What a machine reports, and whether it was measured. A CPU that is not here
has only the generated tables to go on — read its two boot lines.

| Machine | CPU | `guest isolation` | `cpu bugs` | APICv/AVIC | Measured |
|---|---|---|---|---|---|
| NUC | Tiger Lake (6, 0x8C) | eIBRS + BHB clearing, one RSB entry | expected: BHI, PBRSB, GDS microcode, ITS host only | APICv on | 0.591–0.592: vmbench, upload |
| IdeaPad | Ryzen 5 5500U (0x17) | IBPB on every exit, full RSB fill | expected: retbleed IBPB, retbleed SMT STIBP, SRSO IBPB, SMT_RSB | software (AVIC open) | 0.432 boot + LibreWolf |
| Dev box (QEMU) | Ryzen 5 9600X (0x1A), nested | AutoIBRS, IBPB on exit for SRSO (BP_CFG is not written under a hypervisor) | expected: SRSO IBPB | software | 0.583 upload |
| N100 | Gracemont (6, 0xBE) | — | expected: RFDS VERW if `RFDS_CLEAR`, BHI | — | not run |
