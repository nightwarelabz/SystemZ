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
- `vm-serial-2026-10-29.log` — the first QEMU/KVM run
  (`tools/vm-test/run-vm.sh` on the HP EliteDesk 800 G1, `-cpu
  host`, nested VMX exposed): ZCore booted in the VM, consumed the
  manifest at the firmware-granted address (0x3da6b000 — the
  AllocateAnyPages fix working outside QEMU/TCG), found real VMX,
  built the VMCS, computed its controls from the capability MSRs —
  and was stopped by the VMCS itself: 57 field writes failed,
  launch never attempted, teardown clean. Guest birth explicitly
  NOT proven. What the log could not say — which fields failed, and
  with what VM-instruction error — is exactly what the follow-up
  run exists to answer: the write path is now instrumented
  (per-field failure lines, first-failure error capture, control
  read-back). A code audit at the time claimed to exonerate the
  write helper's operand order against the emitted bytes — that
  audit was WRONG (it misread the ModRM register field), as the
  rerun's evidence convicted; see the run-2 entry. The rerun's
  log succeeds this one as the witness.
- `vm-serial-2026-10-29-run2.log` — the instrumented rerun (same
  harness, same machine). VMXON ok, VMCLEAR + VMPTRLD ok, controls
  computed (pin 0x16, proc 0x40061f2, exit 0x336ffb, entry
  0x13fb) — and every one of the 57 failing writes fails from the
  very first; the captured VM-instruction error reads 0 (no valid
  class was ever recorded — the capture read was itself among the
  crossed instructions, see below), and the control read-backs are
  inconsistent (pin controls read back 0x0 "successfully", three
  sibling control reads fail outright, the post-mortem guest-RIP
  read returns 0x0). Guest birth still NOT proven; launch never
  attempted; teardown clean. What this witness convicted, in
  combination with a corrected byte-level audit: the failure split
  is exactly value-shaped — all 30 writes carrying value 0
  "succeeded", all 57 nonzero writes failed — because the
  VMWRITE/VMREAD helpers and the launch bridge emitted their
  operands crossed (the field register lands in the value slot
  and vice versa), so each write executed against field := the
  intended value (0 → VPID, valid; nonzero → no such field) and
  the read-backs executed against stale register contents. The
  crossed templates and bridge lines are fixed and every emitted
  site re-verified in the rebuilt binary (field register in
  ModRM.reg at 15/15 VMWRITE/VMREAD sites); the next run on the
  fixed build is the witness that decides whether the setup now
  lands and the first launch is reached.
