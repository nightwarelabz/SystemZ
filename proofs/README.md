# proofs/

Every milestone in SPEC.md §21 exits on a proof recorded here — a
hardware census, a flight-recorder log, a break-and-heal run — never on
the existence of code alone.

- `M0-hardware-census.md` — owed: a physical x86_64 machine's census,
  produced by `tools/m0-census/m0-census.sh` on real hardware. Any
  qualifying machine serves (SPEC.md §17): the census proves the
  architecture's requirements, not a machine model.
- `qemu-serial.log` — the full boot chain under QEMU/OVMF (our
  loader → ZCore ELF → banner, verdict, boot-manifest report, SVM
  attempt), produced by `tools/qemu-proof/run.sh`. Architecture proof
  only — M1's exit proof remains a cold boot on physical hardware.
  The SVM guest birth is explicitly NOT claimed in this log: QEMU
  TCG's VMRUN does not enter (see zcore/README.md).
