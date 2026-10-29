# SystemZ — Full-Spectrum Specification

**Status:** Founding spec, v0.1 — 2026-10-28. Concept and architecture only; no code exists yet.
**Home:** `nightwarelabz/SystemZ`
**Author of record:** Chase Bryan / NIGHTWARE LABZ. Drafted by Skelli (Muse) at his direction.

---

## 0. What SystemZ is

SystemZ is a hypervisor-based operating system for **x86_64 computers**, built
**entirely from scratch, on our own concepts** — no borrowed hypervisor, no
borrowed kernel, no borrowed bedrock.

It is **primarily a developer's operating system**: a machine made for building
all types of development projects. Its minimal core ships system
administration and networking only; everything else arrives as swappable
pieces.

Four words govern it: **insanely fast, insanely moddable, insanely powerful,
insanely secure.** A fifth governs how it is reported: **proven, not claimed.**

## 1. The Laws

1. **From scratch.** The hypervisor, kernel(s), drivers, network stack,
   filesystem, shell, and toolchain glue are ours. Outside specifications
   (the x86_64 manuals, UEFI, wire protocols) are read and implemented;
   outside *code* is not taken. Where a standard exists we implement the
   standard, in our own hand.
2. **Everything is a brick.** Every component above — and eventually
   including — the hypervisor core is a versioned, signed, swappable unit.
   Any piece can be fully modified, updated, replaced, or removed without
   rebuilding the whole.
3. **The system describes itself.** One declarative system description
   resolves into a running machine. Change the description, re-solve, boot
   the new generation. Every generation is reproducible and reversible.
4. **The system updates itself, from inside itself.** No external installer
   is ever required after first boot. Updates are atomic, verified,
   self-healing, and roll back on failure — including updates to the
   updater itself.
5. **Isolation is the default shape.** Like Qubes, work lives in separated
   domains with explicit, narrow channels between them. Like Kicksecure,
   the platform assumes a hostile world and hardens by default. SystemZ
   takes both more seriously and makes the whole arrangement configurable
   to a depth Arch and Gentoo do not attempt: they configure packages and
   compile flags; SystemZ reconfigures *implementations, isolation
   topology, and trust relationships*.
6. **Simple surface, chess depth.** A new operator gets a working,
   beautiful machine from a handful of choices. Underneath, every choice
   opens into full depth. There is no ceiling placed on the operator.
7. **Speed is a ledger, not a vibe.** Every subsystem carries a measured
   cost budget, in the Comigen manner: costs are counted, published in the
   system's own reports, and regressions are defects.
8. **Honesty of state.** At all times SystemZ can state exactly what it is:
   which generation, which bricks, which hashes, which capabilities, which
   domains, what changed. A component that cannot run says so, plainly,
   in its own voice — the Blacksite honesty architecture, at OS scale.

## 2. Our own concepts — the parts bin

SystemZ is assembled from conceptions already proven across NIGHTWARE LABZ
projects. It reuses *designs and, where the language fits, our own code* —
all of it ours:

| Conception | Origin | SystemZ role |
|---|---|---|
| Rust kernel + scheduler, zero external dependencies | phase1 | Kernel lineage for domains; proof a house kernel can be dependency-free |
| House kernel: process table, VFS, quotas, syscall gate | nightjar Blacksite console; Blacksite OS (TypeScript) | Domain kernel semantics: processes, files, resource quotas, one syscall door |
| Authenticated 3-ary self-healing replication tree (`[x, tag, x⊕tag]`, HMAC-SHA256 tags, majority-vote reconstruction, presence-flag freshness) | Chase's crypto design (orange work) | The brick store: storage and updates that verify, heal single-branch loss, and refuse stale-but-valid rollback |
| Cost ledger + quality governor | Comigen (C35) | Performance budgets per subsystem; the system sheds cost in ranked order instead of stalling |
| EMCON graded conditions | nightjar | Network/radio posture as a system-wide graded state (Open / guarded / receive-only / silent) with per-domain exposure accounting |
| AEGIS + Ivory Tower tripwire | Blacksite OS | Defended system registers; tamper with one and the accusation is recorded, armed defaults restore, the machine reboots into a known generation |
| Verify-integrity seal | Blacksite OS | Running bytes hashed against the generation seal; a mismatch is reported as tamper, never silently absorbed |
| Recursion as a native idea (the OS inside the OS) | Blacksite OS / orc | SystemZ domains can host SystemZ: a full SystemZ instance is a legal brick inside a domain |
| PR-first, whole-census repair, verify-in-the-served-artifact | House workflow doctrine | How SystemZ itself is developed (§18) |

**Language ruling:** the hypervisor core and domain kernels are **Rust,
`no_std`, zero external dependencies** — the phase1 discipline. Tooling
that must be fast follows the same rule. Convenience tooling may relax it
only above the admin line, never inside the trusted core.

## 3. Architecture at a glance

```
┌────────────────────────────────────────────────────────────┐
│  OPERATOR SURFACE   shell · admin console · profiles       │
├────────────────────────────────────────────────────────────┤
│  DEVELOPER DOMAINS  project domains · build domains · lab  │
├────────────────────────────────────────────────────────────┤
│  SERVICE DOMAINS    sys-net · sys-firewall · sys-update ·  │
│                     sys-store · sys-gpu (later, C35)       │
├────────────────────────────────────────────────────────────┤
│  ADMIN DOMAIN (ZAdmin)  system description · generations · │
│                         brick store · capability grants    │
├────────────────────────────────────────────────────────────┤
│  ZCore — microhypervisor (Rust, no_std)                    │
│  vCPU scheduling · EPT/NPT isolation · interrupt routing · │
│  ZBus inter-domain messaging · timers · IOMMU control      │
├────────────────────────────────────────────────────────────┤
│  BOOT  our UEFI loader · measured chain · generation seal  │
├────────────────────────────────────────────────────────────┤
│  x86_64 HARDWARE   VT-x/EPT or AMD-V/NPT · VT-d / AMD-Vi   │
└────────────────────────────────────────────────────────────┘
```

ZCore does **four jobs only**: run vCPUs, isolate memory, route
interrupts, and carry messages. It contains no drivers, no filesystem, no
network stack, and no policy. Policy lives in ZAdmin; mechanism lives in
ZCore. Everything else is a brick in a domain.

