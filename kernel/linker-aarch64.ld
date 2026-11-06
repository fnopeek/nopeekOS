/* nopeekOS linker script: AArch64 kernel image.
 *
 * A position-independent flat image with the arm64 Image header at its
 * first byte (arch/aarch64/boot.s), as the Pi firmware and QEMU's
 * -kernel load it: anywhere on a 2 MiB boundary, the device tree's
 * address in x0. _start applies the R_AARCH64_RELATIVE relocations
 * against the load address before any Rust runs.
 */

ENTRY(_start)

SECTIONS {
    . = 0;
    __image_base = .;

    .text : {
        KEEP(*(.text.head))
        *(.text .text.*)
    }

    .rodata ALIGN(4K) : {
        *(.rodata .rodata.*)
    }

    .data ALIGN(4K) : {
        *(.data .data.*)
    }

    .rela.dyn ALIGN(8) : {
        __rela_start = .;
        *(.rela .rela.*)
        __rela_end = .;
    }

    .dynamic ALIGN(8) : { *(.dynamic) }
    .got ALIGN(8) : { *(.got) *(.got.plt) }

    /* Everything the loader copies ends here. */
    . = ALIGN(16);
    __file_end = .;

    .bss (NOLOAD) : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __stack_bottom = .;
        . += 2M;
        __stack_top = .;
        __bss_end = .;
    }

    . = ALIGN(4K);
    __heap_start = .;
    __image_size = __heap_start - __image_base;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note.*)
        *(.dynsym)
        *(.dynstr)
        *(.hash)
        *(.gnu.hash)
    }
}
