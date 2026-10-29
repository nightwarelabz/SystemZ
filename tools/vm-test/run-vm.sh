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
  # Cargo's presence does not prove zcore's build target is
  # installed — the first G1 run (2026-10-29) passed the tools check
  # and then died in the build with E0463 (can't find crate for
  # `core`). Check the target itself, here, where the fix is cheap.
  if command -v rustup >/dev/null 2>&1; then
    if rustup target list --installed 2>/dev/null | grep -qx 'x86_64-unknown-none'; then
      ok "rustup target x86_64-unknown-none installed (zcore's build target)"
    else
      problem "rustup target x86_64-unknown-none not installed — zcore cannot build without it" \
        "rustup target add x86_64-unknown-none"
    fi
  else
    warn "rustup not found — the x86_64-unknown-none target cannot be verified here; if the build fails with E0463, that target is what is missing" \
      "install Rust via rustup (https://rustup.rs), then: rustup target add x86_64-unknown-none"
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
  # Via bash, never direct exec: the git tree does not preserve the
  # exec bit on these scripts (first G1 run, 2026-10-29: Permission
  # denied after an otherwise-green preflight).
  bash "$ROOT/tools/qemu-proof/build-esp.sh"
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
NPT_NA_LINE="$(line 'npt: not applicable')"
if [ -n "$NPT_LINE" ]; then
  echo "npt:         $NPT_LINE"
elif [ -n "$NPT_NA_LINE" ]; then
  # Intel path, pre-Y1 logs: the NPT block was AMD-only then, and
  # ZCore said so in the log itself. Absence of the self-check
  # there is by design, never a failure — but only when this line
  # stands in its place. Current logs build the tables on the
  # Intel path too (for rung Y1's guest CR3) and take the
  # self-check branch above.
  echo "npt:         not applicable on this path — $NPT_NA_LINE"
elif [ -n "$(line 'npt: skipped')" ]; then
  # The tables were NOT built this boot (no manifest, or its
  # magic refused): rung Y1 will have reported NOT APPLICABLE
  # and not flown. Say so rather than calling the block missing.
  echo "npt:         tables not built — $(line 'npt: skipped')"
else
  echo "npt:         MISSING — no self-check line and no not-applicable line in the log"
fi
VMX_READY_LINE="$(line 'vmx: readiness:')"
if [ -n "$VMX_READY_LINE" ]; then echo "vmx ready:   $VMX_READY_LINE"; else echo "vmx ready:   readiness line NOT SEEN"; fi
[ -n "$(line 'vmx: VMLAUNCH #1')" ] && echo "vmx entry:   VMLAUNCH was attempted"

# The line the whole loop exists for. Priority: a birth counts only
# on ZCore's own GUEST BORN; every other VMX outcome is quoted.
BIRTH=""
SHOW_TAIL=0
if grep -q 'vmx: GUEST BORN' "$LOG"; then
  BIRTH="PROVEN in VM"
  if grep -q 'born on the mirror state' "$LOG"; then
    # Born on the mirror itself — the production path, no rung
    # involved. Since the round-11 fix, the mirror's guest CR4
    # is sourced from the live CR4 read (VMXE standing — run
    # 10's Y2 conviction carried into the construction site),
    # so the state ZCore flies FIRST is the born state. This
    # is the headline case and must precede every rung branch;
    # the rung branches below remain for the record, for any
    # future log where the mirror is refused and a rung enters.
    BIRTH_WHY="VMX path: $(line 'vmx: born on the mirror state') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on ladder rung X1' "$LOG"; then
    # Born on the reference-state rung: the state that entered
    # differs from the refused mirror by exactly the seven
    # deltas to kvm-unit-tests' VMCS recipe (its vmx suite
    # passes on this stack). The rung line names the recipe;
    # the x1 patch lines in the log carry each delta's field
    # and value. Quote the rung line whole.
    BIRTH_WHY="VMX path: $(line 'vmx: born on ladder rung X1') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on ladder rung Y1' "$LOG"; then
    # Born on the first structure rung: the guest entered under
    # ZCore's OWN page tables — its CR3 named the groundwork
    # root, audited in the log (code and stack pages identity-
    # mapped, the NX/EFER ruling printed) — where every refused
    # state translated through the firmware's tables. The seat
    # of the refusal was whose tables CR3 names. These greps
    # must precede the generic 'born on ladder rung' branch:
    # the Y lines carry that substring too.
    BIRTH_WHY="VMX path: $(line 'vmx: born on ladder rung Y1') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on ladder rung Y2' "$LOG"; then
    # Born on the second structure rung: the guest entered on
    # the host's CRs read live, post-VMXON, unedited — VMXE
    # standing in its CR4 — where every refused state flew the
    # mirror's constructed CRs. The seat of the refusal was
    # the CRs' provenance. The y2 line in the log carries the
    # raw reads.
    BIRTH_WHY="VMX path: $(line 'vmx: born on ladder rung Y2') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on ladder-2 rung' "$LOG"; then
    # Born on a ladder-2 rung: the variable was in the HOST
    # state or a CONTROL word — the two blocks that had never
    # varied. The rung line names the departure; quote it whole.
    BIRTH_WHY="VMX path: $(line 'vmx: born on ladder-2 rung') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on ladder rung' "$LOG"; then
    # Born on a ladder rung: the guest program ran (a birth), and
    # the rung line names the single shared-block variable whose
    # change admitted it. Quote the rung line whole — it carries
    # the rung number and the delta that convicted.
    BIRTH_WHY="VMX path: $(line 'vmx: born on ladder rung') ($(line 'vmx: GUEST BORN'))"
  elif grep -q 'born on the canonical bisect state' "$LOG"; then
    # Born — but on the bisect's hand-built canonical state, not
    # on the mirror ZCore would actually fly. The guest program
    # ran, which IS a birth; the mirror state is still refused,
    # which is the standing defect. The verdict carries both
    # facts, distinctly, and neither upgrades the other.
    BIRTH_WHY="born via canonical bisect state; mirror state still refused — the bisect diff in the log names the fields where the two stored states differ. ($(line 'vmx: GUEST BORN'))"
  else
    BIRTH_WHY="VMX path: $(line 'vmx: GUEST BORN')"
  fi
elif grep -q 'svm: GUEST BORN' "$LOG"; then
  BIRTH="PROVEN in VM"
  BIRTH_WHY="SVM path: $(line 'svm: GUEST BORN')"
elif grep -q 'vmx: guest birth NOT CLAIMED' "$LOG"; then
  BIRTH="NOT proven"
  BIRTH_WHY="ZCore's no-entry detection fired under KVM: VMLAUNCH returned without the guest executing. That is REAL evidence, not an environment excuse — file the log, do not tune around it silently. ($(line 'vmx: guest birth NOT CLAIMED'))"
elif [ -n "$(line 'VMCS field writes FAILED')" ]; then
  BIRTH="NOT proven"; BIRTH_WHY="VMCS setup was rejected before any launch: $(line 'VMCS field writes FAILED') — every failing field is named in the log above that line, with the first failure's VM-instruction error; file the log, do not guess."
elif [ -n "$(line 'vmx: guest birth FAILED')" ]; then
  BIRTH="NOT proven"
  # Name the exit reason instead of calling it "wrong": the reason
  # IS the diagnosis class. 33/34 are VM-entry failures — the entry
  # was refused before any guest instruction executed (guest state
  # vs. the MSR-load area). 10 (CPUID) is the birth path itself,
  # reached but not completed; 12 (HLT) means the guest ran but
  # halted before its CPUID. Neither upgrades anything: PROVEN is
  # still judged only by the GUEST BORN grep above.
  EXIT1_LINE="$(line 'vmx: exit #1 reason')"
  REASON_N="$(printf '%s' "$EXIT1_LINE" | sed -n 's/.*reason \([0-9][0-9]*\).*/\1/p')"
  case "$REASON_N" in
    33) REASON_NAME="VM-entry failure due to invalid guest state — the guest-state consistency checks (SDM SS26.3.1) refused the state ZCore loaded; zero guest instructions executed" ;;
    34) REASON_NAME="VM-entry failure due to MSR loading — the VM-entry MSR-load area was refused" ;;
    10) REASON_NAME="CPUID exit — the birth path, but the run did not complete it; see the log" ;;
    12) REASON_NAME="HLT exit — the guest ran but halted before its CPUID; not the birth sequence" ;;
    *)  REASON_NAME="not classified here — the log line itself is the evidence" ;;
  esac
  if grep -q 'vmx: y1 exit #1 reason' "$LOG"; then
    # The structure rungs ran (they fly ahead of X1, so an X1
    # line is also present below — this branch must come
    # first): neither entered. Whose tables the guest CR3
    # names (Y1 — ZCore's own, audited in the log) and the
    # guest CRs' provenance (Y2 — the host's live CRs, read
    # post-VMXON, unedited) now stand varied beside every
    # field value. The last unvaried family is the entry
    # mechanics themselves — the launch bridge and the
    # VMCS-region handling — and the dumps in the log are
    # the complete evidence.
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME); the structure rungs ran and neither entered: $(line 'vmx: y1 exit #1 reason') / $(line 'vmx: y2 exit #1 reason') — whose tables the guest CR3 names and the guest CRs' provenance now stand varied beside every field value; the last unvaried family is the entry mechanics themselves (the launch bridge, the VMCS-region handling), and the dumps in the log are the evidence: $(line 'vmx: guest birth FAILED')"
  elif grep -q 'vmx: x1 exit #1 reason' "$LOG"; then
    # The reference state was refused too: a state matching —
    # on every axis the extraction found — the VMCS recipe of
    # kvm-unit-tests' vmx suite, which enters on this stack.
    # With no L1-visible field value left to separate the two,
    # the remaining delta is structural (the entry mechanics),
    # and the dumps in the log are the complete evidence. This
    # check precedes the ladder branches: an X1 log also
    # contains both ladders' lines.
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME); the reference state (rung X1 — kvm-unit-tests' VMCS recipe, all seven deltas applied at once) was also refused: $(line 'vmx: x1 exit #1 reason') — no L1-visible field value separates ZCore's state from a recipe that enters on this stack; the remaining delta is structural (entry mechanics), and the dumps in the log are the evidence: $(line 'vmx: guest birth FAILED')"
  elif grep -q 'vmx: bisect exit #1 reason' "$LOG"; then
    # The mirror's refusal (reason 33) armed the canonical bisect
    # and the bisect was refused too: both stored states failed
    # entry, so the refusal lives in the block the two attempts
    # share. Say that, and point at the dumps.
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME); the canonical bisect was also refused: $(line 'vmx: bisect exit #1 reason') — both states refused at entry, so the refusal lives in the block they share (controls, CRs, EFER, non-register state); the read-back dumps in the log are the evidence: $(line 'vmx: guest birth FAILED')"
  elif grep -q 'vmx: ladder2' "$LOG"; then
    # Ladder-2 climbed too — this check must precede the ladder
    # branch below, because 'vmx: ladder' is a substring of the
    # ladder-2 lines' prefix family and a ladder-2 log also
    # contains ladder-1's summary. Host state is dumped in the
    # log and the control words stand varied or exonerated;
    # quote ZCore's own closing line for the census.
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME); the host-state/controls ladder (ladder-2) ran and no rung entered: $(line 'vmx: ladder2: no rung entered') — the per-rung dumps in the log (host state read back for the first time) are the evidence: $(line 'vmx: guest birth FAILED')"
  elif grep -q 'vmx: ladder' "$LOG"; then
    # The shared-block ladder climbed and no rung entered: each
    # rung changed exactly one variable against the refused
    # mirror, so the refusals exonerate that block variable by
    # variable. Quote ZCore's own closing line for the census.
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME); the shared-block ladder ran and no rung entered: $(line 'vmx: ladder: no rung entered') — the per-rung read-back dumps in the log are the evidence: $(line 'vmx: guest birth FAILED')"
  else
    BIRTH_WHY="first VMX exit: reason ${REASON_N:-?} ($REASON_NAME): $(line 'vmx: guest birth FAILED')"
  fi
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
