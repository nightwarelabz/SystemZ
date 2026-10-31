/* SystemZ boot manifest — the loader -> ZCore hand-off ABI.
 *
 * Per SPEC.md SS2, brick and boot ABIs are defined in C headers: this
 * file is the canonical definition. zcore/src/zboot.rs mirrors it
 * field for field; the two must never disagree (the magic + size
 * fields let ZCore refuse a manifest it does not understand).
 *
 * The loader allocates the manifest wherever firmware grants pages
 * (AllocateAnyPages): page 0 holds zboot_info, the following pages
 * hold the UEFI memory map exactly as GetMemoryMap returned it at
 * ExitBootServices time. The address of zboot_info is passed to
 * ZCore's entry in RDI (System V first argument) — that pointer is
 * the only address contract; there is no fixed address.
 *
 * History: the manifest originally lived at a fixed 0x100000,
 * demanded with AllocateAddress. That worked under QEMU/OVMF, but
 * the first physical boot (HP EliteDesk 800 G1, 2026-10-29) failed
 * there with EFI_NOT_FOUND (status 0x800000000000000e) — the 1 MiB
 * range is not free in HP's firmware. ZBOOT_ADDRESS is gone on
 * purpose: any code that wants the manifest uses the RDI pointer.
 *
 * Version 2 appends the boot-graphics hand-off after the complete
 * v1 prefix: a framebuffer VALUE (boot/splash.h — never a firmware
 * protocol pointer), whether the live splash is enabled, and its
 * animation frame at hand-off. The allocation story above is
 * unchanged. ZCore accepts v1 and v2; v2 is what the loader writes.
 */
#ifndef SYSTEMZ_ZBOOT_H
#define SYSTEMZ_ZBOOT_H

#include <stdint.h>
#include <stddef.h>
#include "../boot/splash.h"

#define ZBOOT_MAGIC 0x313030544F4F425AULL /* "ZBOOT001" little-endian */
#define ZBOOT_MAP_PAGES 16 /* 64 KiB of memory-map space after page 0 */
#define ZBOOT_VERSION 2u
#define ZBOOT_V1_SIZE 72u
#define ZBOOT_SPLASH_ENABLED 1u

typedef struct {
    uint64_t magic;          /* ZBOOT_MAGIC */
    uint32_t version;        /* v2 appends graphics; v1 prefix unchanged */
    uint32_t info_size;      /* sizeof(zboot_info) as the loader wrote it */
    uint64_t map_address;    /* physical address of the memory map copy */
    uint64_t map_size;       /* bytes of map that are valid */
    uint64_t map_desc_size;  /* stride of one descriptor */
    uint32_t map_desc_version;
    uint32_t reserved0;
    uint64_t rsdp_address;   /* ACPI RSDP physical address, 0 if absent */
    uint64_t zcore_base;     /* where ZCore was placed */
    uint64_t zcore_entry;    /* the entry that is about to be called */
    sz_framebuffer framebuffer;
    uint32_t splash_flags;
    uint32_t splash_frame;
} zboot_info;

_Static_assert(offsetof(zboot_info, framebuffer) == ZBOOT_V1_SIZE, "v1 prefix retained");
_Static_assert(sizeof(zboot_info) == 128, "boot manifest v2 ABI");

#endif
