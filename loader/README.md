# loader/

Our own UEFI boot loader (SPEC.md §§4, 10, 19), written in
freestanding C against our own minimal UEFI definitions (`efi.h`) —
no gnu-efi, no EDK2. Per the §2 language doctrine, the loader is a C
area: firmware speaks C, so the boot path meets the machine in its
native tongue.

## State — M1 first form, QEMU-proven

`loader.c` today: opens the volume it was loaded from (with a
LocateProtocol / LocateHandle volume search behind it), reads
`\EFI\SystemZ\zcore.elf`, validates the ELF64 header (x86-64, ET_EXEC),
reserves the union page range of its PT_LOAD segments at their linked
addresses, copies the segments, exits boot services, and jumps to the
ELF entry.

**Proven** (2026-10-28, `proofs/qemu-serial.log`): the full chain runs
under QEMU/OVMF — loader → ZCore ELF → ZCore's serial banner and CPUID
verdict. That is an *architecture* proof (SPEC.md §18).

The loader also builds the **boot manifest** (`zboot.h` — the C ABI
header ZCore mirrors): on pages firmware grants it (AllocateAnyPages
— there is no fixed address) it leaves the UEFI memory map as it
stood at ExitBootServices, the ACPI RSDP address found in the
firmware's configuration table, and ZCore's placement facts, and
passes the manifest address to ZCore's entry in RDI. The RDI pointer
is the only address contract. ZCore verifies the magic before
trusting a byte of it.

**First physical boot (2026-10-29).** The ESP image was written to a
USB stick and booted on an HP EliteDesk 800 G1. The loader ran on
the real firmware: it read `zcore.elf` (0x8e80 bytes) and placed all
four segments at their linked addresses (0x200000, 0x205048,
0x206990, 0x207000). It then failed at the boot manifest, which at
the time was demanded at a fixed 0x100000 with AllocateAddress:
HP's firmware answered `EFI_NOT_FOUND` (status
0x800000000000000e) — the 1 MiB range is not free there — and the
loader halted. **ZCore has still never entered on physical
hardware**, and M1's exit proof is still owed. The fix is what this
section now describes (anywhere-allocation + RDI hand-off); the
QEMU harness passes with it, OVMF granting the manifest at a high
address which ZCore consumes from the pointer. A hardware re-test
is the next proof owed.

**Not yet true, stated plainly:** no generation seals, no TPM
measurement, no trial generations. Secure Boot signing with
operator keys is future work — OVMF in the proof harness runs
unsigned.

## Build

```sh
make            # produces BOOTX64.EFI (PE32+ EFI application)
```

GCC compiles freestanding objects; GNU ld's `i386pep` emulation links
the EFI application directly (subsystem 10).
