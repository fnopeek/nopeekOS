/* nopeekOS Linker Script – x86_64 UEFI image
 *
 * Build produces an ELF; build.sh post-processes with `objcopy
 * -O pei-x86-64 --subsystem=efi-app` to produce kernel.efi
 * (PE32+ UEFI Application). UEFI firmware loads the .efi image
 * at the address it chooses; we use position-independent code
 * where it matters (boot stub uses RIP-relative lea).
 *
 * Image base 0x100000 (1 MB) is conventional but UEFI may relocate.
 */

ENTRY(_start)

SECTIONS {
    /* Image base 0x10000000 (256 MB) — MUST match build.sh's objcopy
     * --image-base flag. Picked deep in conventional RAM so a 22 MB
     * installer image has plenty of free space ahead of it on a
     * 1 GB QEMU VM (256 MB..(256+22)=278 MB is well clear of OVMF's
     * own runtime services area at ~1015 MB).
     *
     * SizeOfImage = (highest section VMA + size) - ImageBase, so a
     * mismatch between linker.ld VMA and objcopy --image-base
     * inflates SizeOfImage and OVMF then refuses to allocate that
     * much contiguous memory at ImageBase (silently relocates
     * → all `mov $imm64, %reg` absolute pointers in the binary
     * become wrong → string literals point at garbage). We use
     * static relocation model, so RELOCS_STRIPPED is set in the
     * PE: UEFI MUST load us at ImageBase exactly or fail loudly.
     *
     * .text starts at ImageBase + 0x1000 (RVA 0x1000) so the PE
     * headers (mapped at ImageBase + 0x0..0x400) don't collide
     * with our code section. */
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
