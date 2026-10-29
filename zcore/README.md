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
- Builds for `x86_64-unknown-none` with `linker.ld`.
- **Boots through our UEFI loader under QEMU/OVMF** — proven
  2026-10-28, see `proofs/qemu-serial.log` (architecture proof, not a
  hardware verdict). Physical-hardware boot is still owed. VMXON, EPT,
  scheduling, and ZBus do not exist yet and are not claimed.

## Build

```sh
rustup target add x86_64-unknown-none
cargo build --release
```
