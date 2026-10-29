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

**Not yet true, stated plainly:** no boot on physical hardware yet
(that is M1's exit proof and it is still owed); no generation seals,
no TPM measurement, no trial generations, no boot manifest hand-off.
Secure Boot signing with operator keys is future work — OVMF in the
proof harness runs unsigned.

## Build

```sh
make            # produces BOOTX64.EFI (PE32+ EFI application)
```

GCC compiles freestanding objects; GNU ld's `i386pep` emulation links
the EFI application directly (subsystem 10).
