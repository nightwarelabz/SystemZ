/* SystemZ UEFI loader — M1 first form (SPEC.md SS4, SS19).
 *
 * What it does, honestly:
 *   1. opens the volume it was itself loaded from,
 *   2. reads \EFI\SystemZ\zcore.elf (fallback: \zcore.elf),
 *   3. validates the ELF64 header (x86-64, ET_EXEC),
 *   4. places every PT_LOAD segment at its linked physical address
 *      with AllocatePages(AllocateAddress) — ZCore is linked at a fixed
 *      base on purpose, so placement failure is an error, never a
 *      silent relocation,
 *   5. exits boot services and jumps to the ELF entry point.
 *
 * What it does NOT do yet (future PRs, in the spec's own order):
 * generation seals, TPM measurement, trial generations. The boot
 * manifest (zboot.h) IS built and handed off today — its address is
 * passed to ZCore's entry in RDI at the jump below. Manifest v2 also
 * carries a validated framebuffer value for the live boot splash
 * (boot/splash.c, the renderer ZCore continues after hand-off).
 */
#include "efi.h"
#include "zboot.h"

/* ---- ELF64 (the slice we validate and load) ------------------------ */

typedef struct {
    uint8_t ident[16];
    uint16_t type;
    uint16_t machine;
    uint32_t version;
    uint64_t entry;
    uint64_t phoff;
    uint64_t shoff;
    uint32_t flags;
    uint16_t ehsize;
    uint16_t phentsize;
    uint16_t phnum;
} elf64_header;

typedef struct {
    uint32_t type;
    uint32_t flags;
    uint64_t offset;
    uint64_t vaddr;
    uint64_t paddr;
    uint64_t filesz;
    uint64_t memsz;
    uint64_t align;
} elf64_phdr;

#define ELF_PT_LOAD 1
#define ELF_ET_EXEC 2
#define ELF_EM_X86_64 62

/* ---- console -------------------------------------------------------- */

static efi_system_table *ST;
static efi_boot_services *BS;
static sz_framebuffer splash_fb;
static uint32_t splash_step, splash_frame;
static int splash_enabled, boot_services_live = 1;

/* A short reveal is visual animation, never simulated initialization.
 * Set to zero at build time for the shortest possible boot path. */
#ifndef SZ_BOOT_REVEAL_FRAMES
#define SZ_BOOT_REVEAL_FRAMES 12
#endif

/* Harness opt-in: build with SZ_EPT_WIRE=1 (the vm-test loop's
 * --ept-wire does exactly this, through build-esp.sh) and the boot
 * manifest carries ZBOOT_HARNESS_EPT_WIRE, asking ZCore to wire
 * EPT for this boot's guest attempts. The default build writes 0:
 * no harness flag, the proven default configuration. This knob
 * changes only the manifest word — never loader behavior. */
#ifndef SZ_EPT_WIRE
#define SZ_EPT_WIRE 0
#endif

static void serial_byte(uint8_t value) {
    uint8_t ready = 0;
    for (uint32_t i = 0; i < 10000; i++) {
        __asm__ volatile ("inb %1, %0" : "=a"(ready) : "Nd"((uint16_t)0x3fd));
        if (ready & 0x20) break;
    }
    if (ready & 0x20)
        __asm__ volatile ("outb %0, %1" :: "a"(value), "Nd"((uint16_t)0x3f8));
}

static void serial_init(void) {
    const uint16_t ports[] = {0x3f9,0x3fb,0x3f8,0x3f9,0x3fb,0x3fa,0x3fc};
    const uint8_t values[] = {0,0x80,1,0,3,0xc7,0x0b};
    for (uint32_t i = 0; i < 7; i++)
        __asm__ volatile ("outb %0, %1" :: "a"(values[i]), "Nd"(ports[i]));
}

static void print(const char *ascii) {
    for (size_t j = 0; ascii[j]; j++) serial_byte((uint8_t)ascii[j]);
    if (splash_enabled || !boot_services_live || !ST->con_out) return;
    efi_char16 buf[256];
    size_t i = 0;
    for (; ascii[i] && i < 255; i++) buf[i] = (efi_char16)(uint8_t)ascii[i];
    buf[i] = 0;
    ST->con_out->output_string(ST->con_out, buf);
}

static void print_hex(uint64_t v) {
    char buf[19];
    buf[0] = '0'; buf[1] = 'x';
    for (int i = 0; i < 16; i++) {
        int nib = (int)((v >> ((15 - i) * 4)) & 0xF);
        buf[2 + i] = (char)(nib < 10 ? '0' + nib : 'a' + nib - 10);
    }
    buf[18] = 0;
    print(buf);
}

static void die(const char *what, efi_status s) {
    if (splash_enabled)
        sz_splash_draw(&splash_fb, splash_step, splash_frame, what, SZ_SPLASH_ERROR, s);
    print("loader: FAILED — ");
    print(what);
    print(" status=");
    print_hex(s);
    print("\r\nloader: halting.\r\n");
    for (;;) { __asm__ volatile ("hlt"); }
}

static void splash_poll_key(void) {
    if (!splash_enabled || !boot_services_live || !ST->con_in ||
        !ST->con_in->read_key_stroke) return;
    efi_input_key key;
    if (!EFI_ERROR(ST->con_in->read_key_stroke(ST->con_in, &key)) &&
        (key.scan_code == 0x17 || key.unicode_char == 27)) {
        splash_enabled = 0;
        if (ST->con_out && ST->con_out->clear_screen)
            ST->con_out->clear_screen(ST->con_out);
        print("SystemZ: firmware boot details (full log also on COM1)\r\n");
    }
}

static void splash_update(uint32_t completed, const char *message) {
    splash_step = completed;
    splash_poll_key();
    if (splash_enabled)
        sz_splash_draw(&splash_fb, completed, splash_frame++, message, SZ_SPLASH_LOADING, 0);
}

static void splash_init(void) {
    efi_gop *gop = NULL;
    efi_status s = BS->handle_protocol(ST->console_out_handle, &EFI_GOP_GUID, (void **)&gop);
    if (EFI_ERROR(s) || !gop)
        s = BS->locate_protocol(&EFI_GOP_GUID, NULL, (void **)&gop);
    if (EFI_ERROR(s) || !gop || !gop->mode || !gop->mode->info ||
        gop->mode->info_size < sizeof(efi_gop_info)) return;
    efi_gop_info *m = gop->mode->info;
    splash_fb.address = gop->mode->framebuffer_base;
    splash_fb.size = gop->mode->framebuffer_size;
    splash_fb.width = m->width; splash_fb.height = m->height;
    splash_fb.stride = m->pixels_per_scanline; splash_fb.format = m->pixel_format;
    splash_fb.red_mask = m->masks.red; splash_fb.green_mask = m->masks.green;
    splash_fb.blue_mask = m->masks.blue;
    splash_enabled = sz_framebuffer_valid(&splash_fb);
    if (!splash_enabled) return;
    print("splash: GOP framebuffer accepted\r\n");
    splash_update(1, "GRAPHICS READY - OPENING ZCORE");
    for (uint32_t i = 0; i < SZ_BOOT_REVEAL_FRAMES && splash_enabled; i++) {
        splash_update(1, "GRAPHICS READY - OPENING ZCORE");
        BS->stall(25000);
    }
}

/* ---- small memory helpers (freestanding) ---------------------------- */

static void zcopy(uint8_t *dst, const uint8_t *src, uint64_t n) {
    for (uint64_t i = 0; i < n; i++) dst[i] = src[i];
}
static void zzero(uint8_t *dst, uint64_t n) {
    for (uint64_t i = 0; i < n; i++) dst[i] = 0;
}

/* ---- the load -------------------------------------------------------- */

static const efi_char16 PATH_PRIMARY[] =
    { '\\','E','F','I','\\','S','y','s','t','e','m','Z','\\','z','c','o','r','e','.','e','l','f',0 };
static const efi_char16 PATH_FALLBACK[] =
    { '\\','z','c','o','r','e','.','e','l','f',0 };

static efi_file_protocol *try_volume(efi_simple_file_system_protocol *fs) {
    efi_file_protocol *root = NULL;
    if (EFI_ERROR(fs->open_volume(fs, &root))) return NULL;
    efi_file_protocol *file = NULL;
    if (!EFI_ERROR(root->open(root, &file, PATH_PRIMARY, EFI_FILE_MODE_READ, 0)))
        return file;
    if (!EFI_ERROR(root->open(root, &file, PATH_FALLBACK, EFI_FILE_MODE_READ, 0))) {
        print("loader: zcore.elf found at fallback path\r\n");
        return file;
    }
    return NULL;
}

static efi_file_protocol *open_zcore(efi_handle image) {
    efi_loaded_image *li = NULL;
    efi_status s = BS->handle_protocol(image, &EFI_LOADED_IMAGE_GUID, (void **)&li);
    if (EFI_ERROR(s)) die("LoadedImage protocol", s);

    /* First: the volume we were loaded from. Some firmware (OVMF
       included) does not answer HandleProtocol(SimpleFileSystem) on the
       loaded image's device handle, so a miss here is not fatal — the
       search below walks every SimpleFileSystem volume instead. */
    efi_simple_file_system_protocol *fs = NULL;
    if (!EFI_ERROR(BS->handle_protocol(li->device_handle, &EFI_SIMPLE_FS_GUID, (void **)&fs)) && fs) {
        efi_file_protocol *f = try_volume(fs);
        if (f) return f;
    }
    print("loader: searching all volumes for zcore.elf\r\n");

    /* LocateProtocol: the first SimpleFileSystem instance. In the M1
       world there is one FAT volume that matters — the one holding
       zcore.elf. (Volume enumeration via LocateHandleBuffer is the
       general form; on this firmware LocateHandle answered NOT_FOUND
       for a protocol the loader was itself booted through, so the
       direct route is the one we take and verify.) */
    efi_simple_file_system_protocol *lfs = NULL;
    s = BS->locate_protocol(&EFI_SIMPLE_FS_GUID, NULL, (void **)&lfs);
    if (!EFI_ERROR(s) && lfs) {
        efi_file_protocol *f = try_volume(lfs);
        if (f) return f;
    } else {
        print("loader: LocateProtocol(SimpleFileSystem) status=");
        print_hex(s);
        print("\r\n");
    }

    uint64_t buf_size = 0;
    efi_status ls = BS->locate_handle(2 /*ByProtocol*/, &EFI_SIMPLE_FS_GUID, NULL, &buf_size, NULL);
    if (buf_size == 0) die("no SimpleFileSystem volumes at all", ls);
    efi_handle *handles = NULL;
    s = BS->allocate_pool(EfiLoaderData, buf_size, (void **)&handles);
    if (EFI_ERROR(s)) die("AllocatePool handle buffer", s);
    s = BS->locate_handle(2, &EFI_SIMPLE_FS_GUID, NULL, &buf_size, handles);
    if (EFI_ERROR(s)) die("LocateHandle SimpleFileSystem", s);

    uint64_t count = buf_size / sizeof(efi_handle);
    for (uint64_t i = 0; i < count; i++) {
        efi_simple_file_system_protocol *vfs = NULL;
        if (EFI_ERROR(BS->handle_protocol(handles[i], &EFI_SIMPLE_FS_GUID, (void **)&vfs)) || !vfs)
            continue;
        efi_file_protocol *f = try_volume(vfs);
        if (f) return f;
    }
    die("zcore.elf not found on any volume", 0);
    return NULL;
}

static uint8_t *read_whole_file(efi_file_protocol *file, uint64_t *out_size) {
    uint8_t info_buf[512];
    uint64_t info_size = sizeof(info_buf);
    efi_status s = file->get_info(file, &EFI_FILE_INFO_GUID, &info_size, info_buf);
    if (EFI_ERROR(s)) die("GetInfo zcore.elf", s);
    uint64_t size = ((efi_file_info *)info_buf)->file_size;
    if (size == 0) die("zcore.elf is empty", 0);

    void *buf = NULL;
    s = BS->allocate_pool(EfiLoaderData, size, &buf);
    if (EFI_ERROR(s)) die("AllocatePool for file", s);

    uint64_t offset = 0;
    while (offset < size) {
        uint64_t got = size - offset;
        if (got > 65536) got = 65536;
        uint64_t requested = got;
        s = file->read(file, &got, (uint8_t *)buf + offset);
        if (EFI_ERROR(s) || got == 0 || got > requested) die("Read zcore.elf", s);
        offset += got;
        splash_update(2, "READING ZCORE FROM BOOT VOLUME");
    }
    *out_size = size;
    return (uint8_t *)buf;
}

static uint64_t place_segments(const uint8_t *file, uint64_t file_size) {
    if (file_size < sizeof(elf64_header)) die("file smaller than an ELF header", 0);
    const elf64_header *eh = (const elf64_header *)file;
    if (!(eh->ident[0] == 0x7F && eh->ident[1] == 'E' && eh->ident[2] == 'L' &&
          eh->ident[3] == 'F' && eh->ident[4] == 2))
        die("not an ELF64 file", 0);
    if (eh->type != ELF_ET_EXEC || eh->machine != ELF_EM_X86_64)
        die("not an x86-64 ET_EXEC", 0);

    /* Segments may share a page (ours do: .text and .rodata sit 0x30
       bytes apart), and AllocatePages works in whole pages — so first
       reserve the union page range of every PT_LOAD, then copy each
       segment into it. */
    uint64_t lo = UINT64_MAX, hi = 0;
    for (uint16_t i = 0; i < eh->phnum; i++) {
        const elf64_phdr *ph = (const elf64_phdr *)(file + eh->phoff + (uint64_t)i * eh->phentsize);
        if (ph->type != ELF_PT_LOAD) continue;
        if (ph->offset + ph->filesz > file_size) die("segment overruns file", 0);
        if (ph->paddr < lo) lo = ph->paddr;
        if (ph->paddr + ph->memsz > hi) hi = ph->paddr + ph->memsz;
    }
    if (lo == UINT64_MAX) die("no PT_LOAD segments", 0);
    {
        uint64_t base = lo & ~0xFFFULL;
        uint64_t pages = ((hi - base) + 0xFFF) / 0x1000;
        uint64_t addr = base;
        efi_status s = BS->allocate_pages(AllocateAddress, EfiLoaderData, pages, &addr);
        if (EFI_ERROR(s)) die("AllocatePages for segment union", s);
    }
    for (uint16_t i = 0; i < eh->phnum; i++) {
        const elf64_phdr *ph = (const elf64_phdr *)(file + eh->phoff + (uint64_t)i * eh->phentsize);
        if (ph->type != ELF_PT_LOAD) continue;
        zcopy((uint8_t *)ph->paddr, file + ph->offset, ph->filesz);
        if (ph->memsz > ph->filesz)
            zzero((uint8_t *)ph->paddr + ph->filesz, ph->memsz - ph->filesz);
        print("loader: segment placed at ");
        print_hex(ph->paddr);
        print("\r\n");
    }
    return eh->entry;
}

static int guid_eq(const efi_guid *a, const efi_guid *b) {
    const uint8_t *x = (const uint8_t *)a, *y = (const uint8_t *)b;
    for (int i = 0; i < 16; i++) if (x[i] != y[i]) return 0;
    return 1;
}

static uint64_t find_rsdp(void) {
    for (uint64_t i = 0; i < ST->number_of_table_entries; i++) {
        if (guid_eq(&ST->configuration_table[i].vendor_guid, &EFI_ACPI_20_TABLE_GUID))
            return (uint64_t)ST->configuration_table[i].vendor_table;
    }
    return 0;
}

static void exit_and_jump(efi_handle image, uint64_t entry) {
    /* Reserve the manifest pages first (zboot.h): page 0 is the info
       struct, the following pages receive the memory map itself.
       These pages are EfiLoaderData: after ExitBootServices they are
       ZCore's to keep.
       The manifest lives wherever firmware grants pages —
       AllocateAnyPages, never a demanded address. It used to sit at
       a fixed 0x100000 (AllocateAddress): OVMF granted that address,
       but the first physical boot (HP EliteDesk 800 G1, 2026-10-29)
       refused it with EFI_NOT_FOUND — the 1 MiB range is not free
       in HP's firmware. The address firmware returns here is the
       only address there is, and it reaches ZCore in RDI below. */
    uint64_t manifest = 0;
    {
        efi_status s = BS->allocate_pages(AllocateAnyPages, EfiLoaderData,
                                          1 + ZBOOT_MAP_PAGES, &manifest);
        if (EFI_ERROR(s)) die("AllocatePages for boot manifest", s);
    }
    print("loader: boot manifest at ");
    print_hex(manifest);
    print("\r\n");
    zboot_info *info = (zboot_info *)manifest;
    uint8_t *map = (uint8_t *)(manifest + 0x1000);
    const uint64_t map_cap = ZBOOT_MAP_PAGES * 0x1000ULL;

    uint64_t map_size = map_cap, map_key = 0, desc_size = 0;
    uint32_t desc_ver = 0;
    efi_status s = BS->get_memory_map(&map_size, map, &map_key, &desc_size, &desc_ver);
    if (EFI_ERROR(s)) die("GetMemoryMap into manifest", s);

    info->magic = ZBOOT_MAGIC;
    info->version = ZBOOT_VERSION;
    info->info_size = (uint32_t)sizeof(zboot_info);
    info->map_address = (uint64_t)map;
    info->map_size = map_size;
    info->map_desc_size = desc_size;
    info->map_desc_version = desc_ver;
    info->reserved0 = 0;
    info->rsdp_address = find_rsdp();
    info->zcore_base = 0x200000;
    info->zcore_entry = entry;
    info->framebuffer = splash_fb;
    info->splash_flags = splash_enabled ? ZBOOT_SPLASH_ENABLED : 0;
    info->splash_frame = splash_frame;
    info->harness_flags = SZ_EPT_WIRE ? ZBOOT_HARNESS_EPT_WIRE : 0;
    info->reserved1 = 0;

    /* From the final memory-map capture onward: no firmware graphics,
     * keys, timers, allocations or text output. Direct MMIO alone. */
    if (splash_enabled)
        sz_splash_draw(&splash_fb, 5, splash_frame, "MEMORY MAP READY - ENTERING ZCORE", 0, 0);
    splash_step = 5;
    boot_services_live = 0;

    s = BS->exit_boot_services(image, map_key);
    if (EFI_ERROR(s)) {
        /* One retry with a fresh map; a second failure is fatal. */
        map_size = map_cap;
        s = BS->get_memory_map(&map_size, map, &map_key, &desc_size, &desc_ver);
        if (EFI_ERROR(s)) die("GetMemoryMap retry", s);
        info->map_size = map_size;
        info->map_desc_size = desc_size;
        info->map_desc_version = desc_ver;
        s = BS->exit_boot_services(image, map_key);
        if (EFI_ERROR(s)) die("ExitBootServices", s);
    }
    /* Boot services are gone. What speaks next is ZCore, on serial.
       Entry is System V: the manifest address goes in RDI, which is
       exactly the first-argument register — declare it plainly. */
    typedef void (*entry_fn)(uint64_t);
    ((entry_fn)entry)(manifest);
    for (;;) { __asm__ volatile ("hlt"); }
}

efi_status __attribute__((ms_abi)) efi_main(efi_handle image, efi_system_table *st) {
    ST = st;
    BS = st->boot_services;
    serial_init();
    BS->watchdog_timer(0, 0, 0, NULL);
    splash_init();
    if (!splash_enabled) print("splash: text / serial fallback active\r\n");

    print("\r\nSystemZ loader — M1\r\n");

    efi_file_protocol *file = open_zcore(image);
    splash_update(2, "BOOT VOLUME OPEN - READING ZCORE");
    uint64_t size = 0;
    uint8_t *bytes = read_whole_file(file, &size);
    file->close(file);
    splash_update(3, "ZCORE READ - VALIDATING ELF SEGMENTS");
    print("loader: zcore.elf read, bytes=");
    print_hex(size);
    print("\r\n");

    uint64_t entry = place_segments(bytes, size);
    splash_update(4, "ZCORE PLACED - PREPARING HANDOFF");
    print("loader: entry=");
    print_hex(entry);
    print(" — exiting boot services\r\n");

    exit_and_jump(image, entry);
    return 0; /* unreachable */
}
