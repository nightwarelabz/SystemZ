#!/usr/bin/env bash
# SystemZ ESP image pipeline — THE one place ZCore + the loader are
# built and assembled into tools/qemu-proof/esp.img.
#
# Shared by both boot harnesses so the image they boot can never
# drift apart:
#   tools/qemu-proof/run.sh   QEMU/TCG architecture proof (sandbox)
#   tools/vm-test/run-vm.sh   QEMU/KVM dev loop on a real host
#
# If the build or the image layout ever needs to change, change it
# here and both harnesses inherit it. Do not copy this into a caller.
set -eu
cd "$(dirname "$0")/../.."
ROOT="$(pwd)"

export PATH="$HOME/.cargo/bin:$PATH"

echo "== build zcore =="
(cd zcore && cargo build --release)
ZCORE_ELF="$ROOT/zcore/target/x86_64-unknown-none/release/zcore"

echo "== build loader =="
make -C loader clean >/dev/null
if [ "${SYSTEMZ_EPT_WIRE:-0}" = "1" ]; then
  # Harness opt-in (run-vm.sh --ept-wire sets this): the loader is
  # built with its SZ_EPT_WIRE knob on, so the boot manifest it
  # writes carries the EPT-wiring harness flag and ZCore flies
  # that boot's guest attempts with EPT wired. The default build
  # carries no flag — the proven default configuration.
  echo "loader knob: SZ_EPT_WIRE=1 (manifest will carry the EPT-wiring harness flag)"
  make -C loader EPT_WIRE=1
else
  make -C loader
fi

echo "== assemble ESP =="
ESP="$ROOT/tools/qemu-proof/esp.img"
rm -f "$ESP"
dd if=/dev/zero of="$ESP" bs=1M count=64 status=none
mkfs.fat -F 32 "$ESP" >/dev/null
mmd -i "$ESP" ::/EFI
mmd -i "$ESP" ::/EFI/BOOT
mmd -i "$ESP" ::/EFI/SystemZ
mcopy -i "$ESP" loader/BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP" "$ZCORE_ELF" ::/EFI/SystemZ/zcore.elf
echo "ESP image: $ESP"
