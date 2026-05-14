/* nopeekOS – UEFI Boot Entry
 *
 * UEFI hands off in long mode with paging on, using its own GDT, IDT
 * and page tables:
 *   rcx = EFI_HANDLE         (image handle)
 *   rdx = EFI_SYSTEM_TABLE*  (system table pointer)
 *
 * Calling convention at entry is Microsoft x64 (UEFI spec). Our Rust
 * efi_main is declared `extern "efiapi"` so Rust handles that ABI.
 *
 * We deliberately do NOT change CS/SS/DS or load our own GDT here.
 * Boot Services (especially ExitBootServices) run code inside the
 * firmware that assumes UEFI's own GDT layout — replacing selectors
 * mid-flight hangs the firmware. The GDT swap is deferred to
 * boot_uefi::install_kernel_gdt(), called AFTER ExitBootServices.
 *
 * Steps here:
 *   1. Save the UEFI args in callee-saved regs across CPU-setup.
 *   2. Enable OSXSAVE + XCR0 bits for x87/SSE/AVX — Rust codegen with
 *      `+avx2` emits VEX-encoded instructions starting from the first
 *      stack frame.
 *   3. Switch to our own (2 MB) stack — UEFI's is too small for Rust
 *      frames.
 *   4. Reserve MS-x64 shadow space and call efi_main(handle, table).
 *
 * efi_main does the UEFI service work, calls ExitBootServices, then
 * installs our own GDT, IDT-stub-stack-state and jumps to kernel_main
 * with a synthesized BootInfo. It never returns.
 */

.section .text
.global _start
.code64

_start:
    /* Do NOT cli — UEFI Boot Services internally rely on the
     * firmware's timer IRQ. efi_main does cli AFTER ExitBootServices. */
    mov %rcx, %r12
    mov %rdx, %r13

    /* Enable AVX state save (CR4.OSXSAVE = bit 18). UEFI's default
     * CR4 has SSE bits on but not OSXSAVE — and Rust codegen with
     * +avx2 (set in .cargo/config.toml) emits VEX-encoded AVX
     * instructions, so without this the first push into a Rust
     * function #UDs. CR4 is already valid (UEFI set it for itself);
     * OR-in just the new bit so UEFI's other CR4 state is preserved. */
    mov %cr4, %rax
    or $0x40000, %rax       /* bit 18 = OSXSAVE */
    mov %rax, %cr4

    /* XSETBV: enable x87 (bit 0) | SSE/XMM (bit 1) | AVX/YMM (bit 2)
     * in XCR0. Required by AVX2 codegen (and the blake3 SIMD path
     * later in the kernel). XSETBV needs OSXSAVE set above. */
    xor %ecx, %ecx
    xor %edx, %edx
    mov $7, %eax
    xsetbv

    /* Zero BSS. lea (%rip)-relative because UEFI may load the image
     * at any address — but we set RELOCS_STRIPPED in the PE, so UEFI
     * must honor ImageBase exactly; absolute symbol addresses also
     * work. RIP-relative is safer regardless. */
    lea __bss_start(%rip), %rdi
    lea __bss_end(%rip), %rcx
    sub %rdi, %rcx
    shr $3, %rcx
    xor %rax, %rax
    rep stosq

    /* Our own stack — 2 MB reserved in linker.ld, beyond what UEFI's
     * boot-services pool gave us. Aligned to 16 by linker.ld. */
    lea __stack_top(%rip), %rsp

    /* MS x64 ABI requires 32-byte "shadow space" on stack for
     * register-arg spills. RSP must be 16-byte aligned at call site. */
    sub $32, %rsp

    /* Restore args and call efi_main(image_handle, system_table) */
    mov %r12, %rcx
    mov %r13, %rdx
    call efi_main

    /* efi_main is `-> !` but if it ever returns, park here. */
.hang:
    cli
    hlt
    jmp .hang

/* ============================================================
 * Kernel GDT — installed by install_kernel_gdt() in boot_uefi.rs
 * AFTER ExitBootServices completes. Until then, UEFI's GDT stays.
 *
 * Slot 1 = code (0x08), slot 2 = data (0x10). AR.access (bit 0) is
 * set so VMX host-state validation accepts the descriptors without
 * re-loading them.
 * ============================================================ */
.section .rodata
.align 16
.global gdt64
.global gdt64_end

gdt64:
    .quad 0
gdt64_code:
    .quad 0x00AF9B000000FFFF      /* 64-bit code, accessed=1 */
gdt64_data:
    .quad 0x00CF93000000FFFF      /* 32-bit data, accessed=1 */
gdt64_end:
