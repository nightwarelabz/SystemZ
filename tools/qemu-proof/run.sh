#!/usr/bin/env bash
# SystemZ QEMU proof harness — builds ZCore + the loader, assembles an
# ESP image, boots it under QEMU/OVMF, and captures the serial log.
#
# What a PASS proves: our UEFI loader ran under real UEFI firmware,
# loaded our ZCore ELF, exited boot services, and ZCore executed and
# reported its CPU probe on the serial line.
# What it does NOT prove: anything about physical hardware. QEMU is an
# architecture harness (SPEC.md SS18) — hardware verdicts come only from
# hardware. The serial log is written to proofs/qemu-serial.log.
set -eu
cd "$(dirname "$0")/../.."
ROOT="$(pwd)"

export PATH="$HOME/.cargo/bin:$PATH"

echo "== build zcore =="
(cd zcore && cargo build --release)
ZCORE_ELF="$ROOT/zcore/target/x86_64-unknown-none/release/zcore"

echo "== build loader =="
make -C loader clean >/dev/null
make -C loader

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

echo "== boot under QEMU/OVMF =="
if [ -z "${OVMF_CODE:-}" ]; then
  for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$f" ] && OVMF_CODE="$f" && break
  done
fi
if [ -z "${OVMF_VARS:-}" ]; then
  for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do
    [ -f "$f" ] && OVMF_VARS="$f" && break
  done
fi
WORK="$(mktemp -d)"
cp "$OVMF_VARS" "$WORK/vars.fd"
SERIAL_LOG="$ROOT/proofs/qemu-serial.log"
timeout 25 qemu-system-x86_64 \
  -machine q35 -m 256 -cpu max \
  -drive if=pflash,format=raw,unit=0,readonly=on,file="$OVMF_CODE" \
  -drive if=pflash,format=raw,unit=1,file="$WORK/vars.fd" \
  -drive format=raw,file="$ESP" \
  -serial "file:$SERIAL_LOG" \
  -display none -monitor none -no-reboot || true
rm -rf "$WORK"

echo "== serial log =="
cat "$SERIAL_LOG" || true
# The guest-birth line accepts either outcome, honestly reported:
# "GUEST BORN" on a capable host, or ZCore's own plain statement that
# this environment's VMRUN did not enter (QEMU TCG's is a no-op —
# proven by control experiment, see zcore/src/main.rs). What the
# harness must never do is claim a birth that did not happen.
# The NPT groundwork must self-verify (its census is only worth
# printing if the software walk agrees with the builder), and the
# VMX readiness block must appear — present or honestly absent.
if grep -q "SystemZ ZCore" "$SERIAL_LOG" && grep -q "verdict:" "$SERIAL_LOG" \
   && grep -q "zboot: memory map:" "$SERIAL_LOG" \
   && grep -Eq "GUEST BORN|guest birth NOT CLAIMED" "$SERIAL_LOG" \
   && grep -q "npt: self-check PASS" "$SERIAL_LOG" \
   && grep -q "vmx: readiness" "$SERIAL_LOG"; then
  echo "QEMU PROOF: PASS — boot chain, manifest, NPT self-check, VMX readiness, SVM attempt honestly reported"
else
  echo "QEMU PROOF: FAIL — expected ZCore evidence not seen on serial" >&2
  exit 1
fi
