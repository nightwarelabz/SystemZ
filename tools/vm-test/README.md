# tools/vm-test/

The dev loop for SystemZ bring-up: boot ZCore under QEMU **with
KVM** on a host that has real hardware virtualization, and read the
verdict off the serial log. One command — rebuild, relaunch,
report — instead of powering a physical machine off and on for
every test.

```sh
tools/vm-test/run-vm.sh [--mem MB] [--timeout SECONDS] [--no-build] [--ept-wire]
```

## Why KVM changes the witness class

The older harness (`tools/qemu-proof/`) runs under QEMU/TCG —
pure emulation. That environment is *convicted*, not merely weak:
a control experiment proved its VMRUN is a no-op (a guest that is a
pure infinite loop, with every intercept cleared, still "returns"
instantly without executing), and its emulated CPU offers no VMX
at all. Under TCG, a guest birth is unprovable in principle.

On a KVM host the physics are different. KVM executes the guest on
the host's real silicon, and `-cpu host` passes the host's real
feature bits — VMX and EPT included — into the guest. ZCore boots
inside the VM exactly as it would on bare metal, reaches its Intel
twin path, and attempts a real VMLAUNCH: nested VMX, one level
down. The entry either happens or it doesn't, against real
hardware virtualization either way. That is what this loop buys:
the birth proof (and every future ZCore change) becomes testable
at the speed of a rebuild.

Both harnesses build the ESP through the one shared pipeline,
`tools/qemu-proof/build-esp.sh` — the image cannot drift between
them.

## What the run prints

Preflight first (see below), then the boot, then a verdict read
**from the serial log itself** (`tools/vm-test/serial.log`, the
previous run kept as `serial.prev.log`): loader ran, ZCore banner,
the manifest tally, the NPT self-check line, the EPT self-check
line and whether EPT was wired this run, the VMX readiness
line, whether VMLAUNCH was attempted, and the tail line —

```
guest birth: PROVEN in VM — …
guest birth: NOT proven — <what the log actually says>
```

`PROVEN` prints only when ZCore's own `vmx: GUEST BORN` line is in
the log. Everything else quotes the log: a VMfail with its
instruction-error number, a birth ZCore judges incomplete, or its
no-entry detection. The script never upgrades a result. Its exit
status says only whether the loop ran and captured a ZCore boot;
the birth verdict is the printed line, not the code.

Note the normal ending: ZCore halts in place ("skeleton ends here,
on purpose") and never powers the VM off, so QEMU is ended by the
`--timeout` (default 60s) or by Ctrl-C. The log is complete either
way.

## The EPT-wired run (`--ept-wire`)

`--ept-wire` selects the EPT experiment rung for one run. The flag
travels the way every boot fact travels — through the manifest:
the shared pipeline builds the loader with its `SZ_EPT_WIRE` knob
on, the loader writes the harness EPT flag into the v3 boot
manifest, and ZCore — seeing the flag and a verified EPT build —
applies the wiring to every guest attempt that boot as a patch
after the VMCS fill (EPTP written, secondary controls armed), each
attempt announcing itself with an `ept patch:` line. Nothing about
the default run changes: without the flag the manifest carries no
request, no patch line prints, and the attempts fly the exact
configuration every birth so far was proven on.

File a wired run as an **EPT-wired attempt** only when its log
shows all of: `ept: self-check PASS`, at least one
`vmx: ept patch:` line (the mirror attempt's, first), and the
usual birth evidence. A birth on the mirror in such a log is a
guest born *under EPT*; an EPT violation (exit reason 48) or
misconfiguration (49) in one is filed as the tables' verdict, with
the exit qualification as the address-level evidence. A run
requested with `--ept-wire` whose log has no patch line flew
unwired — the verdict says so — and is filed as a default run.

## First run on the EliteDesk 800 G1 (Ubuntu)

1. Get this tree onto the machine (clone or pull `SystemZ`).
2. Install the host packages:

   ```sh
   sudo apt update
   sudo apt install -y qemu-system-x86 ovmf mtools dosfstools build-essential
   ```

   Plus Rust via rustup if it isn't there, and the ZCore target:

   ```sh
   rustup target add x86_64-unknown-none
   ```

3. Give yourself KVM access (group membership applies at login —
   log out and back in after this):

   ```sh
   sudo usermod -aG kvm $USER
   ```

4. Confirm nested virtualization is on — this is what lets the
   *guest* see VMX:

   ```sh
   cat /sys/module/kvm_intel/parameters/nested   # want: Y (or 1)
   ```

   If it says `N`: `echo 'options kvm_intel nested=1' | sudo tee
   /etc/modprobe.d/kvm-intel-nested.conf`, then reboot the host
   once (or reload `kvm_intel` with no VMs running).

5. Run it:

   ```sh
   tools/vm-test/run-vm.sh
   ```

6. Send back `tools/vm-test/serial.log`. The log is the witness;
   it gets filed into `proofs/` deliberately, by hand, when a run
   earns it — the script never files anything itself.

## Troubleshooting ladder

- **Preflight: no /dev/kvm** — VT-x is off in the BIOS/UEFI setup,
  or the `kvm_intel` module isn't loaded (`sudo modprobe
  kvm_intel`). If the host is itself a VM, the outer hypervisor
  must grant nested virtualization; a sandbox without that grant
  cannot run this loop at all, and the preflight says so instead of
  failing cryptically later.
- **Preflight: /dev/kvm not writable** — you're not in the `kvm`
  group yet, or you haven't logged in again since being added.
  `ls -l /dev/kvm` shows the owner/group.
- **Guest log: "CPUID leaf 1 reports NO VMX"** — KVM ran, but
  nested VMX is off, so the guest CPU has no VMX to find. Step 4
  above; the preflight also warns about this before launching.
- **Guest log: "firmware has VMX disabled"**
  (IA32_FEATURE_CONTROL) — inside the VM this reflects how KVM
  presents the MSR; on the host it means virtualization is locked
  off in firmware setup.
- **Guest log: ZCore's no-entry detection fires under KVM** —
  VMLAUNCH returned without the guest executing. Under TCG that
  was the emulator's confession; under KVM it is **real evidence
  against our machinery or its assumptions**. File the serial log
  as a proof record. Do not tune around it silently, and do not
  let anyone re-label it an environment problem without a control
  experiment as strong as the one that convicted TCG.
- **VMXON / VMLAUNCH FAILED with an instruction-error number** —
  the number is read back from the VMCS, not guessed; quote it in
  the report exactly.

## Honest limits

- A birth proven here is proven on **Intel virtualization, nested
  one level down**. It is strong evidence the twin path's
  machinery is right; it is not the bare-metal birth. SPEC.md
  §21's M2 exit still wants the guest born on physical hardware.
- M1's exit proof — a cold boot on a physical machine — remains
  its own witness; this loop doesn't discharge it.
- The **physical M0 census** (`proofs/M0-hardware-census.md`,
  produced by `tools/m0-census/m0-census.sh`) is likewise still
  owed from a real machine's own OS, not from inside this VM: the
  VM would census QEMU's presented hardware, not the machine's.
- The AMD/SVM twin gets nothing from this host: the G1 is Intel.
  SVM's birth awaits AMD hardware or a KVM host on AMD (where the
  same script's verdict understands the `svm: GUEST BORN` line).
