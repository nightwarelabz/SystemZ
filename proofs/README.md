# proofs/

The live boot splash has separate [QEMU/OVMF boot, error and fallback
evidence](boot-splash/README.md), including actual framebuffer captures.

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

