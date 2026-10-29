# zcore/

The SystemZ microhypervisor (SPEC.md §4): vCPU scheduling, EPT/NPT
isolation, interrupt routing, ZBus messaging. Rust, `no_std`, **zero
external dependencies — by law, not preference.**

## State — M1 skeleton

- Freestanding entry (`_start`), COM1 serial output, CPU capability
  probe (vendor, VMX, SVM, hypervisor-present) read from CPUID.
- Builds for `x86_64-unknown-none` with `linker.ld`.
- **Not yet bootable standalone**: our UEFI loader (../loader/) is what
  will load and enter it. VMXON, EPT, scheduling, and ZBus do not exist
  yet and are not claimed.

## Build

```sh
rustup target add x86_64-unknown-none
cargo build --release
```
