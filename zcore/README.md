# zcore/

The SystemZ microhypervisor (SPEC.md §4): vCPU scheduling, EPT/NPT
isolation, interrupt routing, ZBus messaging. Rust, `no_std`, **zero
external dependencies — by law, not preference.** Per the SPEC.md §2
language doctrine, the CPU-plumbing layer (VMX/SVM setup, page tables,
interrupt entry) will arrive as small, audited C modules at the seam,
and the ZBus/brick ABI is defined in C headers — this skeleton is the
Rust core those seams attach to.

## State — M1 skeleton

- Freestanding entry (`_start`), COM1 serial output, CPU capability
  probe (vendor, VMX, SVM, hypervisor-present) read from CPUID, plus a
  deep probe (VMX MSRs on Intel; SVM revision/ASIDs/VM_CR on AMD).
- Consumes the loader's boot manifest (mirror of `loader/zboot.h`):
  verifies its magic, then reports the UEFI memory map — descriptor
  count, conventional (free) memory — and the ACPI RSDP address.
- First SVM guest machinery is complete: hand-built VMCB, host save
  area, a `svm_launch` register bridge, CPUID/HLT intercepts with
  CPUID emulation. **Its birth is NOT yet proven**: on QEMU 8.2.2 TCG,
  VMRUN returns instantly without the guest executing (proven by a
  control run: zero intercepts + an infinite-loop guest still
  "returns" at once). ZCore detects this and says so on serial rather
  than claiming a birth. The proof belongs on physical hardware or a
  KVM-capable host — M2's first owed witness.
- Nested page tables for the first guest are built from the boot
  manifest — an identity map of the RAM window (2 MiB large pages
  where coverage and NX class are uniform, 4 KiB pages elsewhere,
  firmware code executable and data NX, holes left not-present) —
  and verified by a software walk on every boot; the serial census
  prints the table/map counts and the self-check verdict. They are
  **not wired into the VMCB** (NP_ENABLE stays 0, nCR3 never
  written): the wiring belongs to the birth-proof step on hardware.
- The VMX twin of the guest machinery is built to the same pattern:
  VMXON and VMCS regions carrying revision IDs from IA32_VMX_BASIC,
  control fields computed from the IA32_VMX_* capability MSRs
  (allowed-0/allowed-1 masks, never hard-coded), a `vmx_launch`
  register bridge (disassembly-verified, like `svm_launch`) that
  writes HOST_RSP/HOST_RIP itself, the mirrored CPUID/HLT intercept
  policy, VMfail readback through the VMCS instruction-error field,
  and the same no-entry detection — a launch that returns with the
  guest's first instruction demonstrably unexecuted is reported,
  not claimed. Under QEMU TCG the emulated CPU offers no VMX at
  all (measured), so the boot prints the readiness block stating
  that absence and entry is not attempted. EPT is deliberately not
  built yet — secondary controls stay off; it is the next
  groundwork step, the twin of the NPT work above.
- Builds for `x86_64-unknown-none` with `linker.ld`.
- **Boots through our UEFI loader under QEMU/OVMF** — proven
  2026-10-28, see `proofs/qemu-serial.log` (architecture proof, not a
  hardware verdict). Physical-hardware boot is still owed. EPT,
  scheduling, and ZBus do not exist yet and are not claimed.

## Build

```sh
rustup target add x86_64-unknown-none
cargo build --release
```
