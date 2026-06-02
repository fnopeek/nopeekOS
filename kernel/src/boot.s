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

    /* --- PIE self-relocation -------------------------------------------
     * We are a position-independent image (relocation-model=pic + -pie).
     * The firmware may load us at ANY address (DYNAMIC_BASE set in the
     * PE). Apply every R_X86_64_RELATIVE entry in .rela.dyn to the actual
     * load address NOW, before any code dereferences relocated data
     * (statics holding &str / fn-ptrs, the .got). Without this those
     * pointers hold link-time addresses → triple-fault before any output
     * (the old static-model bootloop on firmware that didn't keep
     * ImageBase free). All reloc targets live in the writable
     * .data/.dynamic/.got window, so this is safe even if firmware maps
     * .text/.rodata read-only.
     *
     * delta = runtime(__image_base) - link-time base. `lea sym(%rip)` is
     * PC-relative and relocation-invariant → yields the true runtime
     * address; the 0x10000000 literal is the link-time base (MUST match
     * __image_base in linker.ld and objcopy --image-base in build.sh).
     * Each Elf64_Rela is [r_offset, r_info, r_addend] = 24 bytes; for
     * type R_X86_64_RELATIVE(8): *(r_offset+delta) = r_addend+delta.
     * Clobbers rax/rcx/rdx/rsi/rdi/r8/r9 — args are saved in r12/r13. */
    lea __image_base(%rip), %r8
    movabs $0x10000000, %r9
    sub %r9, %r8                    /* r8 = load delta */
    lea __rela_start(%rip), %rsi
    lea __rela_end(%rip), %rdi
1:
    cmp %rdi, %rsi
    jae 2f
    movl 8(%rsi), %eax             /* r_info low 32 bits = reloc type */
    cmp $8, %eax                    /* R_X86_64_RELATIVE? */
    jne 3f                          /* other types: skip (build asserts none) */
    mov 0(%rsi), %rcx              /* r_offset (link VMA) */
    mov 16(%rsi), %rdx             /* r_addend (link VMA) */
    add %r8, %rcx
    add %r8, %rdx
    mov %rdx, (%rcx)              /* *(r_offset+delta) = r_addend+delta */
3:
    add $24, %rsi
    jmp 1b
2:
    /* relocation complete — relocated data is now safe to touch */

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

    /* Zero BSS. lea (%rip)-relative — position-independent, works at
     * whatever address the firmware loaded us at (see self-relocation
     * above). .bss is NOBITS so it carries no relocations. */
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

/* ============================================================
 * Dummy PE base-relocation block.
 *
 * We self-relocate in _start, so we don't need the firmware to apply
 * relocations — but some UEFI loaders (observed: OVMF) REFUSE to load
 * an image whose Base Relocation data directory is empty when they
 * can't honour ImageBase exactly. A single no-op block makes the
 * directory non-empty so the loader happily relocates us; the lone
 * IMAGE_REL_BASED_ABSOLUTE (type 0) entry applies nothing, leaving the
 * real fixups to _start. tools/pe-fixup.py points DataDirectory[5] at
 * this section. Format (PE spec §6.6): per 4 KiB page a block of
 * u32 PageRVA, u32 BlockSize, then u16 entries (type<<12 | offset).
 *
 * Named .nreloc, not .reloc: lld's ELF linker reserves the name
 * `.reloc` and silently drops it. objcopy carries .nreloc into the PE
 * and pe-fixup.py points DataDirectory[5] at it — the loader keys off
 * the data directory, not the section name.
 * ============================================================ */
.section .nreloc, "a"
.global __reloc_dummy
.global __reloc_dummy_end
__reloc_dummy:
    .long 0                       /* Page RVA */
    .long 12                      /* BlockSize = 8 + 2 entries * 2 bytes */
    .word 0                       /* IMAGE_REL_BASED_ABSOLUTE (no-op) */
    .word 0                       /* second no-op → pad to 4-byte align */
__reloc_dummy_end:
