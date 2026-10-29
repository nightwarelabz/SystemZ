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
- `vm-serial-2026-10-29-run6.log` — that run (same harness, same
  machine, the read-back + bisect build). The new evidence class
  performs exactly as designed: both attempts' dumps read back
  **47 fields, 0 mismatched** — the VMCS holds precisely what the
  code intended to write, so the crossed-writer class of defect
  is closed for good — and guest CS sits in the VMCS as 0xA09B,
  decoded [l 1 d/b 0 g 1]. The mirror is refused at entry
  (**reason 33**, guest RIP unmoved, RAX sentinel intact, exit
  qualification 0, exit interruption-information 0); the
  canonical bisect — differing from the mirror in exactly **10
  stored fields**, all of them in the segment/GDTR/TR block — is
  refused **identically**. The segment block is therefore
  exonerated as a class, and the refusal is confined to the
  block both attempts share: the four control words (pin 0x16,
  proc 0x40061F2, exit 0x336FFB, entry 0x13FB), guest CR0
  0x80010033 / CR3 0x3FC01000 / CR4 0x668, guest IA32_EFER
  0x900, PAT, the link pointer, the zeroed non-register trio,
  the zeroed SYSENTER trio, GDTR/IDTR (IDTR was mirrored in both
  attempts), RFLAGS/DR7/RSP/RIP. Guest birth still NOT proven.
  The next build bisects that shared block itself: a ladder of
  launches in one boot, one variable per rung against the
  refused mirror — guest EFER taken from the host's live MSR
  (the stored 0x900 carries LMA clear against an IA-32e-mode
  entry), the SYSENTER trio re-sourced from the host MSRs, the
  entry controls' load-IA32_EFER bit (SDM bit 15) set
  explicitly, and a guarded probe of the entry controls'
  unnamed bit 12 — stopping at the first rung that enters.
- `vm-serial-2026-10-29-run7.log` — the ladder run (same harness,
  same machine, the shared-block ladder build). The ladder
  climbed exactly as designed and every rung was refused:
  **R0** (the mirror, unchanged), **R1** (guest IA32_EFER :=
  the host's live MSR — the dump proves the field as stored
  became **0xD00**, LMA as the host holds it, and the entry
  still refused, so the EFER/LMA hypothesis is empirically
  dead, not merely argued against), **R2** (the guest SYSENTER
  trio re-sourced from the host's live MSRs, which the log
  reads on the record as **0/0/0** — the stored zeros are
  certified at their source, and the refusal is unchanged),
  **R3** (the entry itself loading IA32_EFER, entry controls
  0x93FB, on top of R1+R2 — still refused), each with a
  **47-field read-back dump, 0 mismatched**, exit qualification
  0 and interruption-information 0 throughout. **R4 did not
  launch**: entry-controls bit 12 is must-be-1 per the live
  capability MSR (0xF3FF000011FB, must-be-1 dword 0x11FB), so
  clearing it is not a legal experiment on this machine. The
  guest block, the EFER field under both treatments, the
  SYSENTER zeros, and the entry load bit stand exonerated
  variable by variable. What seven runs have still never
  examined: the **host state** — written into every VMCS,
  never once read back or audited — and the **control words**
  themselves, byte-identical across all seven runs. The next
  build dumps the host state with the rest (selectors decoded
  index/TI/RPL on the line) and varies exactly those two
  blocks: a second ladder whose rungs depart from the mirror
  singly — host data selectors nulled, each of the proc /
  exit / entry control words reduced to its capability floor
  — plus one combined rung, stopping at the first that
  enters. Guest birth still NOT proven.
- `vm-serial-2026-10-29-run8.log` — the host-state run
  (same harness, same machine, the host-dump + ladder-2
  build). The dumps grew to **70 fields** — the host state
  read back for the first time (CRs, the SYSENTER trio,
  EFER, the MSR bases, the GDTR/IDTR/TR bases, all seven
  selectors decoded index/TI/RPL on the line, and
  HOST_RSP/RIP read in the entry forensics as flown) —
  and every attempt stored **70/70 as written**. The
  KVM-source audit (Linux master nested.c, read in full)
  rendered its routing verdict: host-state and controls
  check failures return VMfailValid (error 8), a shape no
  run has ever shown, so reason 33 routes only through
  the guest-state check, prepare_vmcs02, or the hardware
  checks on vmcs02. Ladder-2 then varied what seven runs
  never had: **H1** (host data selectors nulled —
  refused), **H3** (exit controls at their capability
  floor, the EFER save/load pair off — refused), **H5**
  (the two combined — refused); **H2 and H4 did not
  launch** — the flown proc and entry words already equal
  their capability floors on this machine, leaving no
  variable to change. The host state and the control
  words now stand varied or exonerated beside everything
  else. Two environment facts close the ring from the
  other side (the parent's manual variant runs, recorded
  here so the record is complete): the refusal is
  **invariant across CPU models and chipsets** — qemu64
  with +vmx, Haswell with +vmx, and -machine pc all exit
  reason 33 on the same code — and **KVM nested entry
  works on this stack**: kvm-unit-tests' x86/vmx.flat
  suite, run on this EliteDesk under this QEMU/KVM
  (kernel 7.3.0-10), passes its nested L2 launches,
  including the MSR-load and activity-state tests. A
  working reference therefore exists, and its VMCS
  recipe (x86/vmx.c: init_vmcs_ctrl / init_vmcs_host /
  init_vmcs_guest) diffs against this log's dumps on
  exactly seven axes — guest and host SYSENTER_CS, guest
  DR7, the guest LDTR form, the entry load-EFER request,
  the exit load-PAT request beside a written HOST_PAT,
  the pin-control request bits, and the GS-base
  sourcing. The next build flies that reference state
  whole, as one rung (X1) between the mirror and the
  ladders. Guest birth still NOT proven.
