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
- `vm-serial-2026-10-29-run3.log` — that run (same harness, same
  machine, the operand-fixed build). The instrumentation now
  reads as designed: the control read-back is exact (pin / proc /
  exit / entry all read back precisely as written), the
  post-mortem guest-RIP read-back holds 0x205f80 as written —
  reported successes are real — and exactly **1 of 87** field
  writes failed: field 0xC0E, named "host TR selector", value
  0x48, **VM-instruction error 12** (VMREAD/VMWRITE to an
  unsupported VMCS component). The abort-before-launch discipline
  held, teardown was clean, and guest birth is still NOT proven.
  The conviction: 0xC0E is not a field that exists. SDM Vol. 3C
  Appendix B, Table B-3 enumerates **seven** 16-bit host-state
  fields — §24.5 states in words that "there is no field in the
  host-state area for the LDTR selector" — and the host TR
  selector is index 6, encoding **0xC0C**. The field table had
  been built by mirroring the guest selector group at +0x400,
  which is correct for ES…GS and wrong only for TR: the guest
  group has eight members (guest LDTR at 0x80C pushes guest TR
  to 0x80E), the host group seven. Fixed — `H_TR_SEL` is now
  0x0C0C, and because the constant keys both the write and the
  `vf_name` entry, one correction repairs both. The whole
  host-state group (16-bit, 64-bit, 32-bit, natural-width) was
  then audited encoding-by-encoding against Appendix B (Tables
  B-3, B-7, B-15 and the 32-bit host table), including the
  bridge's asm-written host RSP/RIP (0x6C14/0x6C16 — correct);
  no sibling error was found, and the guest TR selector stays
  0x80E, correct in its own group. The next run is the witness
  that decides whether all 87 writes land and the first launch
  attempt is reached.
- `vm-serial-2026-10-29-run4.log` — that run (same harness, same
  machine, the host-TR-fixed build). **All 87 field writes land**,
  the control read-back is exact, VMLAUNCH is attempted — and the
  first exit is **reason 33** (VM-entry failure due to invalid
  guest state), with guest RIP still at the entry RIP 0x205f70
  and guest RAX still holding the 0x5eed5eed5eed5eed sentinel:
  zero guest instructions executed. The entry consistency checks
  (SDM §26.3.1) refused the guest state itself, and the static
  audit convicted the segment access-rights assembly in
  `seg_ar`: it emitted the **SVM VMCB attribute layout** (access
  byte + flags nibble at bits 11:8) into VMCS access-rights
  fields, whose layout is access byte at 7:0, **bits 11:8
  reserved — §26.3.1.2 requires them all 0 for CS and for every
  usable segment** — and the flags at 15:12 (AVL/L/D/B/G). As
  written, all six usable segments carried nonzero reserved bits
  (CS 0xA, the data segments 0xC), CS and the data segments had
  G = 0 against a 0xFFFF_FFFF limit (§26.3.1.2 requires G = 1
  when limit bits 31:20 are set), and the CS had L = 0 for a
  guest entered as IA-32e mode. TR (0x8B) and the unusable LDTR
  (0x10000) were unaffected — their flags nibbles are 0, so the
  mis-assembly was a no-op for them. Fixed in `seg_ar` (access
  byte from LAR bits 15:8, flags nibble from LAR bits 23:20 into
  bits 15:12, LAR's limit nibble dropped — the VMCS format does
  not carry it): the host-mirror values now assemble to CS
  0xA09B and data 0xC093. The rest of the §26.3.1 checklist was
  audited field by field against this log's values and cleared
  (see the stage report). Guest birth still NOT proven; the next
  run — the first with a guest state that passes the audit
  end-to-end — is the witness.
- `vm-serial-2026-10-29-run5.log` — that run (same harness, same
  machine, the seg_ar-fixed build). The fixed build is what ran:
  guest RIP moved to 0x205f40 (run 4: 0x205f70), exactly the
  seg_ar fix's code-size delta — the corrected assembly removed
  16 instructions upstream of the guest page. All 87 field writes
  land, the control read-back is exact, VMLAUNCH is attempted —
  and the first exit is **reason 33 again**, guest RIP unmoved,
  guest RAX still holding the sentinel: zero guest instructions
  executed. So the audit's three convictions were real but not
  sufficient: at least one more check in the invalid-guest-state
  class refuses the state, and a static audit — which judges the
  values the code INTENDS to write — has no further purchase on
  which. The next build therefore changes evidence class: before
  any launch, ZCore reads the filled VMCS back and prints the
  state AS STORED (every guest field, the control block, the
  MSR counts; each segment's access rights decoded bit by bit;
  guest CS judged against the canonical 0xA09B itself, so a wrong
  intent faithfully stored cannot pass in silence), and after a
  failed entry it prints the exit qualification and interruption
  information while the exit record still exists. If the mirror
  is refused again, the same boot bisects: teardown, VMX
  re-entry, and a second launch of the same 7-byte program on a
  hand-built canonical guest state (ZCore's own freshly built
  GDT and TSS, a constant segment block, the shared CR/EFER/
  control block untouched), with a stored-vs-stored diff of the
  two dumps printed between them — a canonical entry convicts
  the mirror field the diff names; a second refusal confines the
  hunt to the block both states share. Guest birth still NOT
  proven; the rerun is the witness.
