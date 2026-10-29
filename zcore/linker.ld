/* SystemZ ZCore link layout — provisional M1.
   ZCore is loaded by our own UEFI loader (loader/), which will place it
   at a measured address and enter at _start with the boot manifest in
   registers. Until the loader lands, this layout simply gives the
   skeleton a fixed, inspectable shape: 2 MiB base, page-aligned
   sections, boot code first. */
ENTRY(_start)

SECTIONS {
    . = 0x200000;

    .text.boot : {
        KEEP(*(.text.boot))
    }
    .text : {
        *(.text .text.*)
    }
    .rodata : {
        *(.rodata .rodata.*)
    }
    .data : {
        *(.data .data.*)
    }
    .bss : {
        *(COMMON)
        *(.bss .bss.*)
    }
}
