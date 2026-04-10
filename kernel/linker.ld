/* nopeekOS Linker Script – x86_64 Multiboot2 */

ENTRY(_start)

SECTIONS {
    /* Multiboot2 header muss in den ersten 32KB liegen */
    . = 1M;

    .multiboot2 ALIGN(8) : {
        KEEP(*(.multiboot2))
    }

    .text ALIGN(4K) : {
        *(.text .text.*)
    }

    .rodata ALIGN(4K) : {
        *(.rodata .rodata.*)
    }

    .data ALIGN(4K) : {
        *(.data .data.*)
    }

    .bss ALIGN(4K) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* SMP trampoline: copied to 0x8000 at runtime for AP boot */
    .smp_trampoline ALIGN(16) : {
        KEEP(*(.smp_trampoline))
    }

    /* Stack: 256KB, 16-byte aligned (Debug-Mode braucht mehr wegen fehlender Optimierung) */
    . = ALIGN(16);
    __stack_bottom = .;
    . += 256K;
    __stack_top = .;

    /* Heap start (für buddy allocator) */
    . = ALIGN(4K);
    __heap_start = .;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note.*)
    }
}
