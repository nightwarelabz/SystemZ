#!/usr/bin/env bash
# SystemZ VM dev loop — boot ZCore under QEMU *with KVM* on a host
# with real hardware virtualization, then read the verdict off the
# serial log. One command: rebuild the ESP, relaunch, report.
#
# Why this exists: the sandbox harness (tools/qemu-proof/) runs under
# QEMU/TCG, where VMRUN is a proven no-op and the emulated CPU offers
# no VMX at all — that environment can never prove a guest birth.
# Here, KVM runs the guest on the host's real CPU and `-cpu host`
# passes the real VMX + EPT into the guest, so ZCore's Intel twin
# path attempts a REAL nested-VMX entry. The witness is the serial
# log (tools/vm-test/serial.log), and the verdict at the end is read
# from that log — this script reports what the log says and never
# upgrades a result.
#
# Bare metal is still the final witness class (SPEC.md SS18/SS21):
# a birth proven here is proven on Intel virtualization, one nesting
# level down. The physical boot and the M0 census remain owed.
#
# Usage: run-vm.sh [--mem MB] [--timeout SECONDS] [--no-build]
#   --mem      guest RAM in MiB (default 1024; the NPT census window
#              follows the RAM the manifest reports)
#   --timeout  kill QEMU after SECONDS (default 60). ZCore halts in
#              place at the end of its run; it never powers the VM
#              off, so the timeout expiring is the NORMAL ending.
#   --no-build skip the rebuild and boot the existing ESP image.
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VM_DIR="$ROOT/tools/vm-test"
LOG="$VM_DIR/serial.log"
PREV_LOG="$VM_DIR/serial.prev.log"
ESP="$ROOT/tools/qemu-proof/esp.img"
export PATH="$HOME/.cargo/bin:$PATH"

MEM=1024
TIMEOUT=60
BUILD=1

usage() {
  echo "usage: run-vm.sh [--mem MB] [--timeout SECONDS] [--no-build]" >&2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --mem)     [ $# -ge 2 ] || { usage; exit 2; }; MEM="$2"; shift 2 ;;
    --timeout) [ $# -ge 2 ] || { usage; exit 2; }; TIMEOUT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "run-vm: unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

# ------------------------------------------------------------------
# Preflight. Every check prints what it found and, when it fails,
# exactly what to fix. Nothing is launched unless all hard checks
# pass; a warning (nested off) still runs, because ZCore's own log
# will convict that case honestly.
# ------------------------------------------------------------------
FAILURES=0

problem() { # problem <what> <fix>
  echo "preflight: FAIL — $1"
  echo "           fix: $2"
  FAILURES=$((FAILURES + 1))
}
ok() { echo "preflight: ok — $1"; }
warn() {
  echo "preflight: WARNING — $1"
  echo "           fix: $2"
}

echo "== preflight =="

# --- /dev/kvm ------------------------------------------------------
if [ ! -e /dev/kvm ]; then
  problem "no /dev/kvm on this host — KVM is not available to run anything" \
    "enable Intel VT-x in the BIOS/UEFI setup, then 'sudo modprobe kvm_intel'. If this machine is itself a VM, the outer hypervisor must grant nested virtualization — a plain sandbox cannot do this at all."
elif [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  problem "/dev/kvm exists but $USER cannot open it read-write" \
    "sudo usermod -aG kvm $USER, then log out and back in (group membership applies at login). Check with: ls -l /dev/kvm"
else
  ok "/dev/kvm present and writable"
fi

# --- the host CPU itself -------------------------------------------
if grep -m1 '^flags' /proc/cpuinfo | grep -qw vmx; then
  ok "host CPU exposes VMX (VT-x)"
else
  problem "the host CPU does not expose VMX to Linux (no 'vmx' flag in /proc/cpuinfo)" \
    "enable Intel VT-x / virtualization in the BIOS/UEFI setup. If it is enabled there, this kernel or machine cannot offer it — this host can't run the loop."
fi

# --- nested virtualization (what lets the guest see VMX too) -------
NESTED_FILE=/sys/module/kvm_intel/parameters/nested
if [ -r "$NESTED_FILE" ]; then
  NESTED_VAL="$(cat "$NESTED_FILE")"
  case "$NESTED_VAL" in
    Y|1) ok "kvm_intel nested=$NESTED_VAL — the guest can see VMX" ;;
    *) warn "kvm_intel nested=$NESTED_VAL — the guest will NOT see VMX, so ZCore will honestly report no VMX and no entry will be attempted" \
         "echo 'options kvm_intel nested=1' | sudo tee /etc/modprobe.d/kvm-intel-nested.conf, then reboot (or reload kvm_intel with no VMs running). The run continues anyway; the log will show the consequence." ;;
  esac
elif [ -d /sys/module/kvm_amd ]; then
  warn "kvm_amd is loaded, not kvm_intel — this loop targets the Intel twin path (the 800 G1)" \
    "on an AMD host check /sys/module/kvm_amd/parameters/nested instead; ZCore's SVM path is what would be exercised there."
else
  warn "cannot read $NESTED_FILE — nested state unknown" \
    "load the module first: sudo modprobe kvm_intel. If the guest later reports no VMX, this is the first place to look."
fi

# --- QEMU -----------------------------------------------------------
if command -v qemu-system-x86_64 >/dev/null 2>&1; then
  ok "qemu-system-x86_64 present"
else
  problem "qemu-system-x86_64 not found" \
    "sudo apt install qemu-system-x86"
fi

# --- OVMF firmware (same search spirit as the proof harness) -------
OVMF_MODE=""
if [ -n "${OVMF_CODE:-}" ] && [ -n "${OVMF_VARS:-}" ]; then
  OVMF_MODE="split"
elif [ -z "${OVMF_CODE:-}" ]; then
  for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd /usr/share/qemu/OVMF_CODE.fd; do
    [ -f "$f" ] && OVMF_CODE="$f" && break
  done
  for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd /usr/share/qemu/OVMF_VARS.fd; do
    [ -f "$f" ] && OVMF_VARS="$f" && break
  done
  if [ -n "${OVMF_CODE:-}" ] && [ -n "${OVMF_VARS:-}" ]; then
    OVMF_MODE="split"
  elif [ -f /usr/share/qemu/OVMF.fd ]; then
    OVMF_MODE="bios"
    OVMF_UNIFIED=/usr/share/qemu/OVMF.fd
  fi
fi
case "$OVMF_MODE" in
  split) ok "OVMF firmware: $OVMF_CODE (+ writable copy of $OVMF_VARS)" ;;
  bios)  ok "OVMF firmware: $OVMF_UNIFIED (unified image, -bios)" ;;
  *) problem "no OVMF firmware found in the standard Ubuntu paths" \
       "sudo apt install ovmf — or point OVMF_CODE and OVMF_VARS at your firmware files." ;;
esac

# --- build tools (only when building) -------------------------------
if [ "$BUILD" -eq 1 ]; then
  MISSING=""
  for tool in cargo make mkfs.fat mmd mcopy; do
    command -v "$tool" >/dev/null 2>&1 || MISSING="$MISSING $tool"
  done
  if [ -n "$MISSING" ]; then
    problem "build tools missing:$MISSING" \
      "cargo comes from rustup (plus: rustup target add x86_64-unknown-none); make from build-essential; mkfs.fat from dosfstools; mmd/mcopy from mtools — sudo apt install build-essential dosfstools mtools"
  else
    ok "build tools present (cargo, make, mkfs.fat, mtools)"
  fi
else
  if [ -f "$ESP" ]; then
    ok "--no-build: reusing existing ESP image $ESP"
  else
    problem "--no-build given but no ESP image exists at $ESP" \
      "run once without --no-build so the shared pipeline builds it."
  fi
fi

if [ "$FAILURES" -ne 0 ]; then
  echo "run-vm: preflight failed ($FAILURES problem(s)) — nothing was built or launched."
  exit 1
fi

# ------------------------------------------------------------------
# Build (the shared pipeline — the same image the TCG proof boots)
# ------------------------------------------------------------------
if [ "$BUILD" -eq 1 ]; then
  "$ROOT/tools/qemu-proof/build-esp.sh"
fi

# ------------------------------------------------------------------
# Launch. -machine q35 matches the proof harness, so the ONLY
# differences from the TCG run are the accelerator and the CPU:
# KVM executes guest code on this host's real silicon, and
# `-cpu host` passes the host's real feature bits — VMX and EPT
# included — into the guest. A named CPU model would mask exactly
# the bits this loop exists to exercise.
# ------------------------------------------------------------------
[ -f "$LOG" ] && mv "$LOG" "$PREV_LOG"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FW_ARGS=()
if [ "$OVMF_MODE" = "split" ]; then
  cp "$OVMF_VARS" "$WORK/vars.fd"
  FW_ARGS=(-drive "if=pflash,format=raw,unit=0,readonly=on,file=$OVMF_CODE"
           -drive "if=pflash,format=raw,unit=1,file=$WORK/vars.fd")
else
  FW_ARGS=(-bios "$OVMF_UNIFIED")
fi

echo "== boot under QEMU/KVM (-cpu host, ${MEM} MiB RAM, timeout ${TIMEOUT}s) =="
echo "serial log: $LOG"
timeout --foreground "$TIMEOUT" qemu-system-x86_64 \
  -machine q35 -accel kvm -cpu host -m "$MEM" \
  "${FW_ARGS[@]}" \
  -drive format=raw,file="$ESP" \
  -serial "file:$LOG" \
  -display none -monitor none -no-reboot
QEMU_RC=$?
if [ "$QEMU_RC" -eq 124 ]; then
  echo "(timeout reached — the normal ending: ZCore halts in place and never powers the VM off)"
elif [ "$QEMU_RC" -ne 0 ]; then
  echo "run-vm: QEMU exited with status $QEMU_RC — its own messages above are the evidence."
fi

# ------------------------------------------------------------------
# Verdict — read from the serial log, quoting it. This section never
# upgrades a result: the tail line says PROVEN only when ZCore's own
# GUEST BORN line is in the log.
# ------------------------------------------------------------------
echo
echo "== verdict (from the serial log) =="
if [ ! -s "$LOG" ]; then
  echo "no serial output was captured — the guest never spoke."
  echo "guest birth: NOT proven — no log to judge from (QEMU status $QEMU_RC)"
  exit 1
fi

line() { grep -m1 "$1" "$LOG" 2>/dev/null || true; }

if [ -n "$(line 'SystemZ loader')" ]; then echo "loader:      ran (UEFI -> zcore.elf)"; else echo "loader:      NOT SEEN in the log"; fi
if [ -n "$(line 'SystemZ ZCore')" ]; then echo "zcore:       banner seen — ZCore executed in the VM"; else echo "zcore:       banner NOT SEEN"; fi
MANIFEST_LINE="$(line 'zboot: memory map:')"
[ -n "$MANIFEST_LINE" ] && echo "manifest:    $MANIFEST_LINE"
NPT_LINE="$(line 'npt: self-check')"
if [ -n "$NPT_LINE" ]; then echo "npt:         $NPT_LINE"; else echo "npt:         self-check line NOT SEEN"; fi
VMX_READY_LINE="$(line 'vmx: readiness:')"
if [ -n "$VMX_READY_LINE" ]; then echo "vmx ready:   $VMX_READY_LINE"; else echo "vmx ready:   readiness line NOT SEEN"; fi
[ -n "$(line 'vmx: VMLAUNCH #1')" ] && echo "vmx entry:   VMLAUNCH was attempted"

# The line the whole loop exists for. Priority: a birth counts only
# on ZCore's own GUEST BORN; every other VMX outcome is quoted.
BIRTH=""
SHOW_TAIL=0
if grep -q 'vmx: GUEST BORN' "$LOG"; then
  BIRTH="PROVEN in VM"
  BIRTH_WHY="VMX path: $(line 'vmx: GUEST BORN')"
elif grep -q 'svm: GUEST BORN' "$LOG"; then
  BIRTH="PROVEN in VM"
  BIRTH_WHY="SVM path: $(line 'svm: GUEST BORN')"
elif grep -q 'vmx: guest birth NOT CLAIMED' "$LOG"; then
  BIRTH="NOT proven"
  BIRTH_WHY="ZCore's no-entry detection fired under KVM: VMLAUNCH returned without the guest executing. That is REAL evidence, not an environment excuse — file the log, do not tune around it silently. ($(line 'vmx: guest birth NOT CLAIMED'))"
elif [ -n "$(line 'vmx: guest birth FAILED')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="first VMX exit was wrong: $(line 'vmx: guest birth FAILED')"
elif [ -n "$(line 'vmx: guest birth incomplete')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="$(line 'vmx: guest birth incomplete')"
elif [ -n "$(line 'vmx: could not advance')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="$(line 'vmx: could not advance')"
elif [ -n "$(line 'vmx: VMLAUNCH FAILED')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="$(line 'vmx: VMLAUNCH FAILED')"
elif [ -n "$(line 'vmx: VMRESUME FAILED')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="$(line 'vmx: VMRESUME FAILED')"
elif [ -n "$(line 'vmx: VMXON FAILED')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="$(line 'vmx: VMXON FAILED')"
elif grep -q 'vmx: readiness: firmware has VMX disabled' "$LOG"; then
  BIRTH="NOT proven"
  BIRTH_WHY="firmware has VMX locked off on this host (IA32_FEATURE_CONTROL) — enable virtualization in the BIOS/UEFI setup. ($(line 'vmx: readiness:'))"
elif grep -q 'vmx: readiness: CPUID leaf 1 reports NO VMX' "$LOG"; then
  BIRTH="NOT proven"
  BIRTH_WHY="the guest CPU shows no VMX — KVM is not exposing nested VMX. Check the preflight nested warning: /sys/module/kvm_intel/parameters/nested must be Y/1."
elif [ -n "$(line 'SystemZ ZCore')" ]; then
  BIRTH="NOT proven"
  BIRTH_WHY="ZCore ran but no VMX outcome line appears — the tail of the log is:"
  SHOW_TAIL=1
else
  BIRTH="NOT proven"
  BIRTH_WHY="ZCore itself never appeared in the log — the boot chain broke earlier; read the log from the top."
fi

echo "guest birth: $BIRTH — $BIRTH_WHY"
if [ "$SHOW_TAIL" -eq 1 ]; then tail -n 5 "$LOG"; fi
echo
echo "full log: $LOG (previous run kept at $PREV_LOG)"
echo "send serial.log back for the record — it is the witness, not this summary."

# Exit status means only: did the loop run and capture a ZCore boot?
# (The birth verdict lives in the printed tail line, not the code.)
if [ -n "$(line 'SystemZ ZCore')" ]; then exit 0; else exit 1; fi
