#!/usr/bin/env bash
# SystemZ M0 hardware census — read-only.
# Collects exactly the facts SPEC.md M0 requires, from the machine itself:
# CPU + virtualization, EPT/NPT, IOMMU, TPM, firmware, RAM, storage, NICs.
# Writes a Markdown report to ./m0-census-<hostname>.md and prints it.
# Nothing is modified; no package is installed; no network is touched.
set -u

out="m0-census-$(hostname -s 2>/dev/null || echo unknown).md"
cpu_model="$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')"
flags="$(grep -m1 '^flags' /proc/cpuinfo | cut -d: -f2- || true)"

has_flag() { case " $flags " in *" $1 "*) echo yes ;; *) echo no ;; esac; }

vmx="$(has_flag vmx)"
svm="$(has_flag svm)"
ept="$(has_flag ept)"
npt="$(has_flag npt)"
vmode="none"
[ "$vmx" = yes ] && vmode="Intel VMX"
[ "$svm" = yes ] && vmode="AMD-V"

iommu_groups="0"
[ -d /sys/kernel/iommu_groups ] && iommu_groups="$(find /sys/kernel/iommu_groups -maxdepth 1 -mindepth 1 -type d | wc -l | tr -d ' ')"
dmar="no"
{ dmesg 2>/dev/null || sudo -n dmesg 2>/dev/null; } | grep -qi 'DMAR\|IOMMU enabled' && dmar="yes (kernel log)"

tpm="absent"
if [ -e /dev/tpm0 ] || [ -e /dev/tpmrm0 ]; then
  tpm="present"
  for f in /sys/class/tpm/tpm0/tpm_version_major /sys/class/tpm/tpm0/pcr-sha256/*; do
    [ -f "$f" ] && tpm="$tpm; $f = $(cat "$f" 2>/dev/null)"
  done
fi

firmware="BIOS/legacy"
[ -d /sys/firmware/efi ] && firmware="UEFI"
secureboot="unknown"
if command -v mokutil >/dev/null 2>&1; then
  secureboot="$(mokutil --sb-state 2>/dev/null | tr '\n' ' ')"
elif [ -f /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c ]; then
  secureboot="efivar present (see report notes)"
fi

{
  echo "# SystemZ M0 hardware census"
  echo
  echo "- Host: $(hostname)"
  echo "- Date: $(date -u '+%Y-%m-%d %H:%M UTC')"
  echo "- Kernel: $(uname -srm)"
  echo "- Machine: $(cat /sys/devices/virtual/dmi/id/product_name 2>/dev/null || echo unknown) / $(cat /sys/devices/virtual/dmi/id/sys_vendor 2>/dev/null || echo unknown)"
  echo "- Board: $(cat /sys/devices/virtual/dmi/id/board_name 2>/dev/null || echo unknown), BIOS $(cat /sys/devices/virtual/dmi/id/bios_version 2>/dev/null || echo unknown) ($(cat /sys/devices/virtual/dmi/id/bios_date 2>/dev/null || echo unknown))"
  echo
  echo "## Virtualization (the M0 gate)"
  echo "- CPU: $cpu_model"
  echo "- Virtualization mode: $vmode (vmx=$vmx, svm=$svm)"
  echo "- Second-level translation: EPT=$ept, NPT=$npt"
  echo "- IOMMU groups: $iommu_groups; DMAR/IOMMU in kernel log: $dmar"
  echo
  echo "## Firmware and trust"
  echo "- Firmware mode: $firmware"
  echo "- Secure Boot: $secureboot"
  echo "- TPM: $tpm"
  echo
  echo "## Memory and storage"
  echo "- RAM total: $(free -h | awk 'NR==2{print $2}')"
  echo "- Storage:"
  lsblk -d -o NAME,MODEL,SIZE,TRAN,TYPE 2>/dev/null | sed 's/^/  /'
  echo
  echo "## Network interfaces"
  lspci 2>/dev/null | grep -i 'ethernet\|network' | sed 's/^/  /' || echo "  (lspci unavailable)"
  ip -br link 2>/dev/null | sed 's/^/  /'
  echo
  echo "## M0 verdict inputs"
  echo "- VT-x/EPT or AMD-V/NPT present: $([ "$ept" = yes ] || [ "$npt" = yes ] && echo yes || echo NO)"
  echo "- IOMMU groups > 0: $([ "$iommu_groups" -gt 0 ] && echo yes || echo NO)"
} > "$out"

cat "$out"
echo
echo "Report written to: $out"
