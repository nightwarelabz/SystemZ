# proofs/

Every milestone in SPEC.md §21 exits on a proof recorded here — a
hardware census, a flight-recorder log, a break-and-heal run — never on
the existence of code alone.

- `M0-hardware-census.md` — owed: a physical x86_64 machine's census,
  produced by `tools/m0-census/m0-census.sh` on real hardware. Any
  qualifying machine serves (SPEC.md §17): the census proves the
  architecture's requirements, not a machine model.
- `qemu-serial.log` — the full boot chain under QEMU/OVMF (our
  loader → ZCore ELF → banner, verdict, boot-manifest report, NPT
  build + self-check census, SVM attempt, VMX readiness block),
  produced by `tools/qemu-proof/run.sh`. The harness FAILs unless
  the NPT self-check prints PASS and the VMX readiness block
  appears. Architecture proof only — M1's exit proof remains a
  cold boot on physical hardware. The SVM guest birth is
  explicitly NOT claimed in this log: QEMU TCG's VMRUN does not
  enter, and its emulated CPU offers no VMX at all (see
  zcore/README.md). On hardware, this log's successors are where
  the NPT wiring and both births get proven.
- *First physical boot attempt (2026-10-29, HP EliteDesk 800 G1;
  the record is a photograph of the machine's console):* the USB
  boot worked — the loader ran on HP's real firmware, read
  zcore.elf, placed all four segments at their linked addresses,
  and halted on `AllocatePages for boot manifest
  status=0x800000000000000e` (EFI_NOT_FOUND): the manifest had been
  demanded at a fixed 0x100000, which HP firmware does not grant.
  Fixed in the loader — AllocateAnyPages, the granted address
  printed on the console, and the manifest address reaching ZCore
  in RDI as the only contract (see `loader/zboot.h`). No hardware
  serial log exists yet, and ZCore entry on hardware remains
  unproven; the re-test is owed.
