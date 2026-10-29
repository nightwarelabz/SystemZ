/* SystemZ loader — minimal UEFI definitions, our own.
 *
 * SPEC.md SS2 language doctrine: the loader is a C area. UEFI's native
 * interface is C-shaped; we define exactly the slice of it we use, from
 * the public UEFI specification, and nothing more. No gnu-efi, no EDK2,
 * no external headers: the supply chain of this file is this repo.
 *
 * All firmware entry points use the Microsoft x86-64 calling
 * convention, spelled ms_abi for GCC.
 */
#ifndef SYSTEMZ_EFI_H
#define SYSTEMZ_EFI_H

#include <stdint.h>
#include <stddef.h>

typedef void *efi_handle;
typedef uint64_t efi_status;
typedef uint16_t efi_char16;
typedef uint8_t efi_bool;

#define EFI_SUCCESS 0
#define EFI_ERROR_BIT ((uint64_t)1 << 63)
#define EFI_ERROR(s) (((s) & EFI_ERROR_BIT) != 0)

typedef struct {
    uint32_t data1;
    uint16_t data2;
    uint16_t data3;
    uint8_t data4[8];
} efi_guid;

typedef struct {
    uint64_t signature;
    uint32_t revision;
    uint32_t header_size;
    uint32_t crc32;
    uint32_t reserved;
} efi_table_header;

/* ---- Simple Text Output ------------------------------------------- */

typedef struct efi_simple_text_output efi_simple_text_output;
struct efi_simple_text_output {
    efi_status (__attribute__((ms_abi)) *reset)(efi_simple_text_output *, efi_bool);
    efi_status (__attribute__((ms_abi)) *output_string)(efi_simple_text_output *, const efi_char16 *);
    void *unused[4];
};

/* ---- File System --------------------------------------------------- */

typedef struct efi_file_protocol efi_file_protocol;

#define EFI_FILE_MODE_READ 0x1
#define EFI_FILE_DIRECTORY 0x8000000000000018ULL

struct efi_file_protocol {
    uint64_t revision;
    efi_status (__attribute__((ms_abi)) *open)(efi_file_protocol *, efi_file_protocol **,
                                                const efi_char16 *, uint64_t, uint64_t);
    efi_status (__attribute__((ms_abi)) *close)(efi_file_protocol *);
    void *delete_fn;
    efi_status (__attribute__((ms_abi)) *read)(efi_file_protocol *, uint64_t *, void *);
    efi_status (__attribute__((ms_abi)) *write)(efi_file_protocol *, uint64_t *, const void *);
    void *get_position;
    void *set_position;
    efi_status (__attribute__((ms_abi)) *get_info)(efi_file_protocol *, const efi_guid *,
                                                    uint64_t *, void *);
};

typedef struct {
    uint64_t revision;
    efi_status (__attribute__((ms_abi)) *open_volume)(void *, efi_file_protocol **);
} efi_simple_file_system_protocol;

/* File Info, from the UEFI spec (GUID below). */
typedef struct {
    uint64_t size;
    uint64_t file_size;
    uint64_t physical_size;
    uint8_t create_time[16];
    uint8_t modified_time[16];
    uint8_t accessed_time[16];
    uint64_t attribute;
    efi_char16 file_name[];
} efi_file_info;

static const efi_guid EFI_FILE_INFO_GUID =
    { 0x09576E92, 0x6D3F, 0x11D2, { 0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B } };
static const efi_guid EFI_LOADED_IMAGE_GUID =
    { 0x5B1B31A1, 0x9562, 0x11D2, { 0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B } };
static const efi_guid EFI_SIMPLE_FS_GUID =
    { 0x964E5B22, 0x6459, 0x11D2, { 0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B } };

typedef struct {
    uint32_t revision;
    efi_handle parent_handle;
    void *system_table;
    efi_handle device_handle;
    void *file_path;
    void *reserved;
    uint32_t load_options_size;
    void *load_options;
    void *image_base;
    uint64_t image_size;
    int image_code_type;
    int image_data_type;
    void *unload;
} efi_loaded_image;

/* ---- Boot Services (prefix layout; we use a named subset) ---------- */

enum efi_allocate_type { AllocateAnyPages = 0, AllocateMaxAddress = 1, AllocateAddress = 2 };
enum efi_memory_type {
    EfiReservedMemoryType = 0, EfiLoaderCode = 1, EfiLoaderData = 2,
    EfiBootServicesCode = 3, EfiBootServicesData = 4, EfiRuntimeServicesCode = 5,
    EfiRuntimeServicesData = 6, EfiConventionalMemory = 7
};

typedef struct {
    efi_table_header hdr;
    void *raise_tpl;
    void *restore_tpl;
    efi_status (__attribute__((ms_abi)) *allocate_pages)(int, int, uint64_t, uint64_t *);
    efi_status (__attribute__((ms_abi)) *free_pages)(uint64_t, uint64_t);
    efi_status (__attribute__((ms_abi)) *get_memory_map)(uint64_t *, void *, uint64_t *,
                                                          uint64_t *, uint32_t *);
    efi_status (__attribute__((ms_abi)) *allocate_pool)(int, uint64_t, void **);
    efi_status (__attribute__((ms_abi)) *free_pool)(void *);
    void *create_event, *set_timer, *wait_for_event, *signal_event;
    void *close_event, *check_event;
    void *install_protocol_interface, *reinstall_protocol_interface;
    void *uninstall_protocol_interface;
    efi_status (__attribute__((ms_abi)) *handle_protocol)(efi_handle, const efi_guid *, void **);
    void *reserved2;
    void *register_protocol_notify;
    efi_status (__attribute__((ms_abi)) *locate_handle)(int, const efi_guid *, void *,
                                                         uint64_t *, efi_handle *);
    void *locate_device_path;
    void *install_configuration_table;
    void *load_image, *start_image, *exit_fn, *unload_image;
    efi_status (__attribute__((ms_abi)) *exit_boot_services)(efi_handle, uint64_t);
    void *monotonic_counter, *watchdog_timer;
    void *connect_controller, *disconnect_controller;
    efi_status (__attribute__((ms_abi)) *open_protocol)(efi_handle, const efi_guid *,
                                                         void **, efi_handle, efi_handle,
                                                         uint32_t);
    void *close_protocol, *open_protocol_information, *protocols_per_handle;
    efi_status (__attribute__((ms_abi)) *locate_handle_buffer)(int, const efi_guid *, void *,
                                                                uint64_t *, efi_handle **);
    efi_status (__attribute__((ms_abi)) *locate_protocol)(const efi_guid *, void *, void **);
} efi_boot_services;

#define EFI_OPEN_PROTOCOL_GET_PROTOCOL 0x00000002

typedef struct {
    efi_table_header hdr;
    const efi_char16 *firmware_vendor;
    uint32_t firmware_revision;
    efi_handle console_in_handle;
    void *con_in;
    efi_handle console_out_handle;
    efi_simple_text_output *con_out;
    efi_handle standard_error_handle;
    efi_simple_text_output *std_err;
    void *runtime_services;
    efi_boot_services *boot_services;
} efi_system_table;

#endif
