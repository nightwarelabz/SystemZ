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

**Language doctrine — Rust and C, each where it is strongest**
(Chase's amendment, 2026-10-28: *C for key areas, for maximum capability
and universality*):

- **Rust** (`no_std`, zero external dependencies — the phase1 discipline)
  is the default for ZCore's logic, the domain kernels, the solver, and
  everything where memory safety is the security boundary.
- **C** (freestanding, our own headers, no libc in the trusted core) is
  the language of the **key areas where universality is the point**:
  1. **The loader and firmware interop** — UEFI speaks C; the boot path
     meets the machine in its native tongue, with no translation layer.
  2. **The brick ABI** — every brick interface (ZBus channels, driver
     classes, capability calls) is defined by a C header. A brick may be
     written in any language that can speak the C ABI — which is every
     language — so the Lego system is never fenced to one toolchain.
  3. **CPU plumbing** — the thinnest VMX/SVM setup, page-table, and
     interrupt-entry code where direct control of the machine text is
     the capability itself, kept small enough to audit line by line.
  4. **Driver class interfaces** — defined in C headers so a class
     driver (§17) is portable across every domain that hosts it.
- **Assembly** exists only inside those C/Rust modules, at the exact
  instructions the architecture demands, never as free-standing files
  nobody can audit.
- The boundary rule: C areas are kept *small, flat, and audited*; Rust
  carries the complexity. Zero external dependencies binds both —
  in the trusted core, the whole supply chain is this repo, in either
  language. Convenience tooling may relax dependency discipline only
  above the admin line, never inside the trusted core.

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
│  ZCore — microhypervisor (Rust core, C at the seams, §2)   │
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


## 4. ZCore — the microhypervisor

**Scope.** ZCore is the only software that runs at the highest privilege.
It is deliberately tiny: a small, auditable Rust `no_std` core. If a
feature can live above ZCore, it must.

- **Boot.** Our own UEFI loader measures each stage into the TPM (when
  present) and hands ZCore a sealed manifest of the generation it was
  asked to boot (§10, §11).
- **Virtual CPUs.** VMX (Intel) and SVM (AMD) backends behind one internal
  interface. vCPU scheduling is work-conserving with hard caps per domain;
  a domain can be given guaranteed cores, burstable cores, or both.
  Pinning is a configuration, not a patch.
- **Memory isolation.** EPT/NPT second-level translation built and owned
  by ZCore. No domain maps another domain's memory except through an
  explicit, revocable, ZAdmin-granted shared window (the ZBus fast path).
  IOMMU (VT-d / AMD-Vi) confines every DMA-capable device to its owner's
  memory — a passed-through device cannot escape its domain.
- **Interrupts.** Physical interrupts route to owning domains; virtual
  interrupts are delivered by ZCore. No polling loops in the core.
- **Timers.** Per-domain virtual timers over the hardware TSC/APIC timer;
  domains may not read another domain's time state. (Real time is locked —
  the house law from Simulacrum/Comigen: the system clock is the real
  clock, and test levers exist only in proof harnesses, never in shipped
  hosts.)
- **Exits budget.** Every VM-exit class carries a cost-ledger budget.
  Emulation is a failure of design: hot paths are paravirtual (ZBus) or
  passthrough, never emulated.

**Non-jobs.** ZCore has no scheduler policy beyond caps and guarantees,
no drivers, no allocator policy visible to domains, no logging beyond a
fixed flight-recorder ring that ZAdmin can read after the fact.

## 5. Domains — the isolation model

A **domain** is an isolated machine inside the machine: its own memory,
its own vCPUs, its own devices (real, passed through, or virtual over
ZBus), its own kernel or bare-metal payload, its own failure boundary.
A domain crash is a local event. ZCore and sibling domains keep running;
ZAdmin restarts the corpse per its profile.

**Standing domains (the minimal core system):**

| Domain | Job | Network? |
|---|---|---|
| **ZAdmin** | Owns the system description, generations, brick store, capability grants, operator console. The control plane. | No — never |
| **sys-net** | Owns the physical NIC(s). Runs our network stack. The only domain that touches wire hardware. | Hardware only |
| **sys-firewall** | All inter-domain and external traffic policy passes here. Default deny, both directions. | Via sys-net |
| **sys-update** | The only domain allowed to fetch new bricks from outside. Downloads, verifies, stages generations; cannot activate them — activation is ZAdmin's act on the operator's word. | Via sys-firewall, on demand only |
| **sys-store** | Owns the physical storage devices and the brick store (§9). Serves verified blocks; knows nothing of policy. | No |

**Developer domains** are created per project or per class of work:

- **Project domain** — one per development project: its toolchain bricks,
  its sources (a snapshot-able filesystem), its build/test services.
  Clone it, snapshot it, throw it away, ship it to another SystemZ host
  as a brick-set.
- **Build domain** — disposable, created for one build/test run from a
  project snapshot, destroyed after. Builds never run in the project
  domain itself: a poisoned build script gets a room that burns down.
- **Lab domain** — for running untrusted or experimental code, including
  other operating systems later (§14). No network unless its profile
  grants it; no persistence unless granted.
- **Vault domain** — permanently offline, no ZBus channels except a
  narrow operator-mediated transfer door. Holds keys and secrets. EMCON
  Alpha made into a place.
- **Disposable domain** — one-shot browsing/testing/unknown binaries;
  destroyed on close, state never written back.

**Profiles** bind domains into a working machine: *Bare* (standing
domains only), *Admin* (+ operator tooling), *Fortress* (+ vault,
disposables, strict EMCON defaults), *Lab* (+ lab domains, passthrough),
*Studio* (the full developer arrangement, §13). A profile is itself a
brick: editable, forkable, shareable.

## 6. Bricks — the Lego system

A **brick** is the unit of everything: code, drivers, services, tools,
configuration, profiles, even whole domains definitions.

**Manifest (every brick carries one):**

- `id`, `name`, `version`, content hash of every payload file
- `kind`: core / driver / service / tool / config / profile / domain /
  app / data
- `provides` / `requires`: capabilities and interfaces, by name and
  version range — the studs and sockets
- `capabilities`: the exact powers the brick needs (device, network
  class, filesystem scope, ZBus channels). A brick runs with exactly
  these and no more; unlisted power is unreachable, not merely unused.
- `config schema`: every configuration knob, typed, with defaults and
  documentation — configuration is part of the brick's interface, so
  "oozing with configurations" is structural, not aspirational
- `ledger`: measured cost profile (size, memory, exit/CPU cost class)
- `signature`: NIGHTWARE LABZ release signature, or a local operator
  signature for home-built bricks — both are first-class; trust is a
  policy in ZAdmin, not a gate kept by us

**Brick rules:**

1. Bricks are immutable once sealed. A change is a new version, never an
   edit in place. The running system is therefore always describable.
2. Bricks declare; they do not reach. All wiring is done by the solver
   (§7) from manifests. A brick cannot discover or attach to anything it
   did not declare.
3. Any brick can be forked by the operator, modified, and substituted —
   including ours. The system does not distinguish "vendor" bricks from
   operator bricks except by signature trust policy.
4. A brick must fail honestly: if its requirements cannot be met it
   refuses with a named reason, in the house style — nothing is listed
   that does not run.

## 7. Recursive dynamism — descriptions, the solver, generations

**The system description** is a single declarative document (our own
format; human-writable, machine-diffable) naming: hardware capabilities
(as discovered at runtime, §17),
domains and their bricks, capability grants, network policy, EMCON
defaults, performance budgets, and operator preferences. It is the
machine, written down.

**The solver** takes description + brick store and produces a
**generation**: a complete, sealed, bootable arrangement — ZCore config,
domain images, wiring, policy tables — plus the generation's seal (the
hash set the integrity system verifies against). Solving is deterministic:
same description + same store = same generation, on any host.

**Recursion, three ways:**

1. The description can describe *itself being changed*: profiles and
   config bricks are inputs to the solver like any other, so the
   configuration system is configured by the same system.
2. A domain can run a full SystemZ (ZCore nested or emulated-in-domain
   for development), so SystemZ is its own best test lab.
3. Bricks can generate bricks: build domains output sealed bricks into a
   local store; a developer's whole toolchain is the loop
   *describe → solve → run → build new bricks → re-describe*.

**The four dimensions** of a running SystemZ, each independently
changeable: **(1) topology** — which domains exist and how they are
wired; **(2) composition** — which brick versions fill each slot;
**(3) state** — each domain's filesystem snapshots and data;
**(4) time** — the generation lineage: every past generation remains
bootable, diffable, and restorable. Chess-like depth comes from the
product of the four, not from any single complicated part.

**Generations and rollback.** Generations are numbered, sealed, and kept
(operator policy sets retention). Boot failure, integrity failure, or an
operator's bad feeling → reboot into the previous generation. Nothing is
ever updated "in place"; there is only the next generation.

## 8. ZBus — inter-domain communication

Domains do not share; they **message**. ZBus is our paravirtual transport:

- **Channels** are created by ZAdmin from the description (never by
  domains themselves), typed by the interface the two bricks declared.
- **Fast path:** shared-memory rings with zero-copy payloads and
  doorbell notifications; data moves at memory speed, not device speed.
- **Slow path:** copied messages for small/control traffic, fully
  mediated and auditable.
- **Every channel is a capability**: unidirectional or bidirectional,
  rate-capped, and revocable at runtime by ZAdmin (revocation is how
  EMCON falls: cut channels, don't beg processes to stop).
- **No ambient authority:** a domain can address only its granted
  channels, by handle. There is no global namespace to scan, no network
  to discover inside the machine.


## 9. Storage — the self-healing brick store and filesystems

**Brick store.** All bricks live in a content-addressed store built on
Chase's authenticated 3-ary self-healing replication tree: each block is
stored as `[x, tag, x⊕tag]` under a keyed HMAC-SHA256 tag (128-bit),
expanded 4 levels to 81 blocks. Reads reconstruct bottom-up by majority
vote of MAC-verified candidates; losing any whole 27-block branch is
survivable. **Freshness:** presence flags are the freshness mechanism —
a stale-but-validly-tagged block is rejected, which closes the rollback
attack against the store itself. Integrity is therefore not a scan we
run; it is a property of every read.

**Domain filesystems.** Each domain gets its own filesystem instance,
descended from the phase1/Blacksite VFS conceptions: in-house format,
snapshot-native (copy-on-write), with snapshots as cheap, named,
first-class objects. A snapshot is the unit of: project states, build
inputs, pre-update safety, domain cloning, and time travel along
dimension 4 (§7). Cross-domain file movement happens only through
ZAdmin-mediated transfer or an explicitly granted shared folder brick —
never through a global shared filesystem by default.

**Encryption.** Vault and (optionally) any domain filesystem encrypt at
rest with keys held by the vault domain / TPM-sealed keys. The store's
replication tree provides integrity and availability, not
confidentiality — confidentiality is encryption's job, and the spec
keeps the two separate, as the original design does.

## 10. Updates — the system updates itself

The update loop is a closed circuit inside the machine:

1. **Fetch.** sys-update (and only sys-update) is granted a temporary
   network channel by ZAdmin, through sys-firewall, under the current
   EMCON condition. It fetches new brick versions and release manifests.
2. **Verify.** Every brick is hash- and signature-checked *before* it
   enters the store; the store's own tree verification runs on ingest.
   A brick that fails never exists as far as the solver is concerned.
3. **Solve.** ZAdmin solves a candidate next generation from the
   operator's (possibly unchanged) description plus the new store.
   The operator sees a full diff: bricks changed, wiring changed,
   policy changed, cost-ledger deltas.
4. **Trial boot.** The candidate generation boots once in trial mode.
   Standing domains must pass their health contracts; the integrity
   seal must verify; the flight recorder must be clean.
5. **Commit or roll back.** Success promotes the generation. Any failure
   reboots the previous sealed generation automatically — the machine
   is never left half-updated, because there is no "half": generations
   are whole.
6. **The updater updates itself** the same way: a new sys-update is just
   a brick in a later generation. The one component that can never be
   brick-swapped at runtime is ZCore — and ZCore changes arrive as a new
   generation whose trial boot is supervised by the *previous*
   generation's loader chain, with automatic fallback.

**Offline updates.** A generation can be exported as a sealed update
bundle (USB, local network) and ingested by sys-store with the same
verification path — no network grant required, no verification skipped.

**Channels and cadence** are operator policy bricks: stable / edge /
pinned-per-brick. Pinning any brick to any version is a one-line
description change, forever.

## 11. Security model

**Threat model, stated plainly.** SystemZ assumes: hostile networks,
hostile files and web content, hostile or buggy applications, malicious
build scripts and dependencies, and a physically present attacker
without the operator's keys. It does not claim to survive an attacker
with sustained physical possession *and* unlimited time — no honest
system does — it makes that attack slow, loud, and evidentiary.

**Chain of trust.** UEFI Secure Boot (operator-owned keys) → our loader
(measures every stage) → ZCore (verifies the generation seal before
starting any domain) → ZAdmin (verifies bricks on solve and on load) →
domains (receive only verified, capability-scoped bricks). TPM measured
boot records the chain; the operator can inspect the measurements and
seal secrets to them.

**Compartmentalization (the Qubes lesson, rebuilt).** Damage is bounded
by domain boundaries and capability grants. The admin domain has no
network. The network domain has no files except its own. The update
domain can fetch but cannot activate. The store can serve but cannot
decide. No single domain's compromise is a system compromise; the
attacker's prize for each breach is one room, with the doors they were
explicitly given.

**Hardening (the Kicksecure lesson, rebuilt in our own hand).** Kernel
and domain hardening by default: W^X everywhere, stack/heap protections
in our allocator and compiler flags, ASLR for domain payloads, strict
syscall surfaces per domain (a domain's kernel exposes only the calls
its bricks declared), default-deny firewall both directions, no
listening services in the minimal core, and attack-surface accounting:
every open channel, device, and port appears in the operator's exposure
report with a number that must sum honestly — the EMCON inventory
doctrine.

**AEGIS / Ivory Tower.** A small set of defended registers — generation
seal, capability table roots, firewall policy root, update trust roots —
is continuously notarized by an observer that lives outside the domains
it watches. An unaccounted write to a defended register triggers the
sentence: record the accusation to the flight recorder and audit ledger,
restore armed defaults, reboot into the last known-good generation.
The sentence never destroys evidence and never deletes files.

**Audit ledger.** Security-relevant events (grants, generation changes,
channel creation/revocation, tripwire events, update verdicts) append to
a hash-chained, append-only ledger the operator can export. Tampering
with history breaks the chain visibly.

**Cryptography.** Our own implementations of standard primitives,
following the house pattern (verified against published test vectors;
the orange project exists precisely to specify and verify crypto, and
SystemZ bricks are natural orange subjects as that tooling matures).
Keys are generated and held in the vault domain or TPM; no key material
ever transits ZBus in the clear between domains that do not both hold a
grant for it.

**EMCON.** Graded network posture (Open / Charlie guarded / Bravo
receive-only / Alpha silent) as a system-wide condition, enforced by
channel revocation and firewall policy — a mechanism, not a request
processes may ignore.

## 12. Networking

Networking is a core duty of the minimal system, and it is ours end to
end above the wire:

- **Drivers.** From-scratch drivers by NIC device class (§17),
  owned by sys-net, DMA-confined by the IOMMU.
- **Stack.** Our own implementation, staged: Ethernet/ARP, IPv4 then
  IPv6, ICMP, UDP, TCP, DHCP, DNS — each protocol a separately testable
  brick inside sys-net, each proven against independent implementations
  on a wire we control before it is trusted.
- **Firewall.** sys-firewall holds all policy: inter-domain, external,
  per-profile and per-EMCON tables. Policy is part of the system
  description — diffable, generational, revertible like everything else.
- **Names and trust.** DNS resolution is its own service brick with its
  policy (including encrypted transports) explicit in the description.
  No silent fallbacks: a resolver that cannot honour its policy says so.
- **Services.** The minimal core *serves nothing* by default. Any
  listening service (SSH-class remote admin, git hosting, file transfer)
  is a brick the operator adds, in a domain the operator chooses, with
  exposure counted in the EMCON inventory.

## 13. The developer's OS — development as the first-class workload

SystemZ exists to build software. The developer arrangement is not an
app installed on top; it is the profile the machine is designed around.

- **Toolchains as bricks.** Compilers and language runtimes (Rust first —
  the house language — then C, Python, and the rest) are bricks with
  declared capabilities. A project pins its exact toolchain versions in
  its own description; ten projects, ten different toolchains, zero
  interference.
- **Project domains.** Each project lives in its own domain (§5) with its
  sources, toolchain, services, and network policy. Opening a project is
  resuming a machine that was built for exactly that project.
- **Build isolation.** Builds and tests run in disposable build domains
  spawned from project snapshots. Reproducible by construction: the
  build's inputs are a snapshot + a brick set + a description, and the
  same inputs rebuild the same outputs — or the ledger says why not.
- **Snapshots as workflow.** Snapshot before the scary refactor. Branch
  a domain to try the crazy idea. The cost of an experiment is a copy-on-
  write snapshot, not a weekend.
- **Git and remotes.** Git tooling is a brick set; remotes are reached
  through sys-firewall with per-project grants. Signing keys never leave
  the vault domain: signing is a service the vault performs through a
  narrow channel, so a compromised project domain cannot exfiltrate the
  key it uses.
- **Editors and shell.** The house lineage: phase1's shell and avim-class
  editor conceptions, grown up — a fast native shell where every system
  object (domains, bricks, generations, channels) is addressable, and
  scripting the OS is scripting the description.
- **Debugging across domains.** ZAdmin can inspect, pause, and trace any
  domain it owns; the flight recorder and per-domain logs are one query
  surface. Performance work uses the cost-ledger tooling: every subsystem
  reports its budget and its spend.
- **Any project type.** Systems code (SystemZ develops itself, hosted in
  project domains, tested in nested SystemZ instances), applications,
  web work (serve a project domain's output to a lab domain's browser),
  embedded targets (cross toolchains as bricks), and long builds (build
  domains with guaranteed cores while the operator keeps working).


## 14. Guests and full operating systems

SystemZ domains natively run SystemZ kernels and bare-metal brick
payloads — the fast path. Full foreign operating systems are a later,
explicit capability:

- A **guest domain** may run a complete OS on ZCore's virtual hardware
  (virtual ZBus devices first; selective device passthrough via IOMMU
  when a guest needs real hardware).
- Guests are for development and lab work: testing software on other
  systems, running the one tool that exists only elsewhere, studying a
  foreign system in a room with no doors.
- Guests never join the standing trust structure: a guest is a lab
  animal, however friendly. Its channels are granted like any brick's,
  and its compromise is assumed in its profile from the start.

## 15. Operator surface — admin, shell, simplicity

- **`zadm`** is the one administrative instrument: domains, bricks,
  generations, channels, grants, updates, snapshots, ledger, exposure —
  every system act is a `zadm` act, scriptable and logged.
- **The shell** descends from the phase1/Blacksite shells: fast, native,
  honest (a thing that cannot run says why, in its own voice).
- **First boot** asks almost nothing: pick a profile (Studio default),
  name the machine, set the operator key. Everything else has a sane,
  stated default that the operator can open and change — depth is
  available, never mandatory.
- **The exposure report** is the security conscience, always one command
  away: what can this machine be reached through, right now, and which
  profile line is responsible for each opening.

## 16. Graphics and the C35 question

Graphics are **not** part of the minimal core. The console is text-first
by design — the fastest, smallest, most auditable surface.

When SystemZ needs a graphical deployment — a composited desktop, game
or engine workloads, GPU-accelerated development — that need is met by
**Comigen (C35)**, our own graphics engine, entering as the sys-gpu /
compositor brick-set. Per Chase's ruling of 2026-10-28, **C35 is on hold
until SystemZ needs it for deployment**; it will be built then, to
SystemZ's requirements, as a brick — not bolted on. Until that day, no
graphics work blocks the core, and no core decision may assume a
compositor that does not yet exist. GPU passthrough to lab/dev domains
(for compute and foreign desktops) is a ZCore/IOMMU matter and does not
wait for C35.

**Designation (Chase, 2026-10-29):** Comigen (C35) is SystemZ's **main
graphics engine** — the designation settles *which* engine; the
milestone settles *when*. The hold above stands unchanged: C35 enters
as the sys-gpu / compositor brick-set when the deployment need arrives
(M8), built to SystemZ's requirements. The full house composition —
the house's other utilities, and where each belongs — is censused in
INTEGRATION.md.

**The browser system is likewise designated (Chase, 2026-10-29):
orc**, our own browser, with its bundled Tractor search. Like the
graphics engine it is no part of the minimal core: it arrives as an
app brick-set hosted in user-facing domains (disposable, lab, project)
in the same deployment era, and no core decision may assume a browser
that does not yet exist either.

## 17. Hardware — one x86_64 logic, every machine

SystemZ is built **from the x86_64 architecture itself**: the same logic
runs on all machines. There are no per-model targets. A specific machine
(Chase's EliteDesk 800 G1 included) is only ever a *witness* — a place a
universal claim gets proven — never a profile the system is built for.

- **Required by the architecture:** a 64-bit x86 CPU with hardware
  virtualization (Intel VT-x with EPT, or AMD-V with NPT) and an IOMMU
  (VT-d / AMD-Vi) for device confinement and passthrough. TPM 2.0
  strongly preferred; measured boot degrades honestly (and says so)
  without one.
- **Discovery, not assumption.** Everything machine-specific is read
  from the machine at runtime, through the architecture's own doors:
  CPUID leaves and MSRs for the processor, ACPI tables for the board's
  wiring, PCI/PCIe enumeration for devices, UEFI for firmware services.
  Code asks the architecture; it never carries a table of models.
- **Drivers are by device class, never by machine.** Storage speaks
  AHCI or NVMe, USB speaks xHCI, network speaks its controller class —
  the published interface standards, implemented once, valid wherever
  that class appears. A device whose class SystemZ has no driver brick
  for is reported honestly as unsupported; the remedy is a new class
  driver brick (§6), not a machine profile.
- **One honest gate.** A machine that fails the architectural
  requirements (no EPT/NPT, no IOMMU) cannot host SystemZ, and the
  loader says so, naming the missing capability — on any machine, by
  the same check, with the same words.

## 18. How SystemZ is built — development doctrine

- **PR-first** (house law since 2026-10-23): branch → PR → checks green
  → merge. A red PR is repaired on its branch, whole error census in one
  pass — never one-screenshot-one-fix.
- **Read the diff before merging.** A clean merge is not proof of scope.
- **Proof lives on the artifact:** a claim about boot, isolation, update,
  or speed is proven on real hardware (or explicitly labelled with the
  harness that proved it and what the harness cannot show). Software-
  renderer and emulator numbers are architecture proof, never hardware
  verdicts — the Comigen lesson, generalized.
- **Cost ledgers gate merges:** performance regressions against a
  subsystem's budget are defects with numbers attached.
- **The spec is versioned with the code.** This document changes by PR
  like everything else; the shipped system and its spec may never
  quietly disagree.

## 19. Boot sequence (normative sketch)

1. Firmware (Secure Boot, operator keys) verifies our UEFI loader.
2. Loader reads the boot policy: which generation is *current*, which
   (if any) is on *trial*.
3. Loader measures ZCore + generation seal into the TPM, starts ZCore.
4. ZCore verifies the seal, builds EPT/NPT structures, starts ZAdmin
   alone.
5. ZAdmin verifies the brick store (tree verification on the paths it
   will use), starts sys-store, sys-net, sys-firewall, sys-update in
   dependency order, checking each health contract.
6. Profile domains start. The operator console opens on ZAdmin.
7. Trial generations report health; ZAdmin commits or schedules
   rollback before the console declares the boot good.
8. The flight recorder for the boot is sealed into the audit ledger.

Target: the minimal core (through step 6, Bare profile) boots in
**single-digit seconds** on reference hardware — a budget to be
measured and then defended, not a promise made here.

## 20. Repository layout (proposed)

```
SystemZ/
  SPEC.md            this document
  README.md
  zcore/             the microhypervisor (Rust core, C seams)
  loader/            our UEFI boot loader
  zadmin/            admin domain: solver, generations, zadm
  domains/           standing domains (sys-net, sys-firewall,
                     sys-update, sys-store) and domain kernels
  bricks/            brick manifests + house brick sources
  net/               our network stack bricks
  store/             self-healing brick store + filesystems
  tools/             host-side build/proof tooling
  profiles/          house profiles (Bare, Admin, Fortress, Lab, Studio)
  proofs/            hardware proof reports, cost ledgers, censuses
```

## 21. Milestones — each with an exit proof

- **M0 — Ground truth.** Prove the architectural gate (§17) on real
  hardware: census a physical x86_64 machine (CPU, EPT/NPT, IOMMU, TPM)
  with the census tool; toolchain pinned; repo seeded with this spec.
  Any qualifying machine serves — the census proves the *architecture's*
  requirements, not a model's. *Exit: a written hardware census, signed
  off as read from the machine itself.*
- **M1 — First boot.** Our loader → ZCore → one domain running our code,
  on real x86_64 hardware, from a cold power-on. *Exit: flight-recorder
  log + a witness run, repeated.*
- **M2 — Two rooms and a wall.** Two domains, isolated memory, ZBus
  channel between them, admin shell killing/restarting either. *Exit:
  an isolation proof — one domain's hostile payload fails to read the
  other, on hardware.*
- **M3 — Bricks and generations.** Brick store live (self-healing tree),
  solver producing generations, rollback demonstrated by breaking one
  on purpose. *Exit: break-and-heal proof on hardware.*
- **M4 — Wire.** sys-net with our stack reaches the real network
  through sys-firewall under EMCON control. *Exit: independent-host
  interoperability proof (our TCP talking to foreign stacks).*
- **M5 — Self-update.** A new brick version fetched by sys-update,
  solved into a trial generation, committed — and a sabotaged update
  rolled back automatically. *Exit: both proofs, on hardware, logged.*
- **M6 — Developer's machine.** Project + build domains, Rust toolchain
  brick, SystemZ building its own next brick inside itself. *Exit:
  a SystemZ component built by SystemZ, sealed and booted.*
- **M7 — Studio.** The full developer profile: snapshots as workflow,
  vault signing, exposure report, audit ledger export. *Exit: a real
  project (a house repo) developed end-to-end on SystemZ.*
- **M8 — Guests and graphics (when called).** Guest-OS domain; C35
  compositor bricks when the deployment need arrives, per §16.

No milestone is "done" on the strength of its code existing — only on
its exit proof, recorded in `proofs/`.

## 22. Non-goals and honest limits

- Not a general-purpose consumer OS; the operator is a developer.
- Not Windows/macOS application compatible except inside guest domains.
- Not a server product claim: it can serve, but its design centre is a
  developer's own machine.
- From-scratch drivers mean the device-*class* coverage list starts
  short (AHCI/NVMe, xHCI, the first NIC classes). Any machine carrying
  those classes is in scope; coverage grows class by class. That is the
  price of owning the stack, and it is paid openly.
- A from-scratch hypervisor is a multi-year-class undertaking in the
  large. These milestones are ordered so that every stage is a real,
  bootable, provable machine — the spectrum is full, the path is
  honest about being walked one proof at a time.

## 23. Open decisions (Chase's calls, none blocking the spec)

1. **Licence** for the SystemZ repo (house precedent: AGPL-3.0 for
   snoot/C35; unrecorded for SystemZ at spec time).
2. **Description format** — evolve phase1's registry/config idioms, or a
   fresh minimal format designed for diffing.
3. **Device-class order** — after the first storage/USB/NIC classes,
   which device class earns a driver brick next (§17: classes, never
   machine models).
4. **Guest priority** — whether M8 guests matter before or after wider
   device-class coverage.

---

*End of founding spec v0.1. Amendments land by PR, with this document
and the code moving together.*
