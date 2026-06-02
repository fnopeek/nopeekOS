/* nopeekOS Linker Script – x86_64 UEFI image
 *
 * Build produces a PIE ELF (relocation-model=pic + -pie); build.sh
 * post-processes with `objcopy -O pei-x86-64 --subsystem=efi-app` to
 * produce kernel.efi (PE32+ UEFI Application). The image is position-
 * independent: the firmware loads it at any address and boot.s's
 * _start applies the R_X86_64_RELATIVE relocations from .rela.dyn to
 * the actual base before any Rust runs.
 */

ENTRY(_start)

SECTIONS {
    /* Link-time image base 0x10000000 (256 MB) — MUST match build.sh's
     * objcopy --image-base flag and the literal in boot.s's relocation
     * loop. It is only a preference now: DYNAMIC_BASE is set in the PE
     * and RELOCS_STRIPPED is cleared, so firmware that can't place us
     * here relocates us (and we self-relocate) instead of refusing —
     * which fixed the cross-hardware bootloop. Keep linker VMA and
     * objcopy --image-base equal so SizeOfImage stays correct.
     *
     * .text starts at ImageBase + 0x1000 (RVA 0x1000) so the PE
     * headers (mapped at ImageBase + 0x0..0x400) don't collide
     * with our code section.
     *
     * Anchor symbol at the link-time base — boot.s computes the load
     * delta as `lea __image_base(%rip) - 0x10000000`. */
    . = 0x10000000;
    __image_base = .;

    . = 0x10001000;

    .text ALIGN(4K) : {
        *(.text .text.*)
    }

    .rodata ALIGN(4K) : {
        *(.rodata .rodata.*)
    }

    .data ALIGN(4K) : {
        *(.data .data.*)
    }

    /* PIE dynamic relocations. boot.s's _start walks this table and
     * applies every R_X86_64_RELATIVE entry to the actual load address
     * before the first Rust instruction. Bracketed with symbols so the
     * loop can find it RIP-relative. Read-only (we only read it). */
    .rela.dyn ALIGN(8) : {
        __rela_start = .;
        *(.rela .rela.*)
        __rela_end = .;
    }

    /* .dynamic + .got hold relocated pointers — keep them writable
     * (their WA flags carry into the PE) so the reloc loop can patch
     * them even when firmware enforces section protections. */
    .dynamic ALIGN(8) : { *(.dynamic) }
    .got ALIGN(8) : { *(.got) *(.got.plt) }

    /* Dummy PE base-relocation block (boot.s). Named .nreloc because
     * lld drops a section literally called `.reloc`. KEEP so
     * --gc-sections doesn't collect it (nothing references it);
     * pe-fixup.py wires it into the PE Base Relocation data directory. */
    .nreloc ALIGN(4) : { KEEP(*(.nreloc)) }

    /* SMP trampoline: copied to 0x8000 at runtime for AP boot.
     * Sized data — UEFI maps it RX/RO with our section flags. */
    .smp_trampoline ALIGN(16) : {
        KEEP(*(.smp_trampoline))
    }

    /* .bss covers both Rust statics AND the kernel stack.
     *
     * The stack MUST live inside a declared section so UEFI's PE
     * loader actually allocates its pages. With it floating
     * between sections (the old `. += 256K` pattern), UEFI mapped
     * only the explicit sections and our first push went to
     * unallocated memory → silent #PF → triple-fault reset.
     * Symptoms were non-deterministic across runs because exactly
     * which addresses are/aren't mapped varies with firmware
     * memory-map quirks.
     */
    .bss ALIGN(4K) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __stack_bottom = .;
        /* 2 MB stack — bumped from 256 KB. Rust frames in efi_main
         * are LARGE (xmm6-15 saves + 0x228 byte locals = ~700 bytes
         * per call frame); 256 KB ran out somewhere down the call
         * chain → silent overflow into other .bss data → random
         * hang locations across runs. */
        . += 2M;
        __stack_top = .;
        __bss_end = .;
    }

    /* Heap start (for buddy allocator) */
    . = ALIGN(4K);
    __heap_start = .;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note.*)
    }
}
