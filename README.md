# SystemZ

**NIGHTWARE LABZ**

SystemZ is a hypervisor-based operating system for x86_64 computers, built
entirely from scratch on our own concepts — no borrowed hypervisor, no
borrowed kernel, no borrowed bedrock.

Primarily a **developer's operating system**: a machine made for building
all types of development projects. Its minimal core is system
administration and networking. Everything else is a brick — versioned,
signed, swappable — in a system that describes itself, updates itself from
inside itself, and isolates everything by default.

Insanely fast. Insanely moddable. Insanely powerful. Insanely secure.

**Status:** founding specification — see [SPEC.md](SPEC.md). No code yet;
every state claim in this repo is proven or labelled unproven.

## The shape

- **ZCore** — a microhypervisor in Rust (`no_std`, zero external
  dependencies): vCPU scheduling, EPT/NPT memory isolation, interrupt
  routing, inter-domain messaging. Four jobs, nothing else.
- **Domains** — admin, network, firewall, update, and developer project
  domains, each isolated, each talking over the ZBus, each replaceable.
- **Bricks** — every component is a signed, content-hashed, versioned
  unit stored in a self-healing replication tree. Swap any piece; the
  system re-solves and boots a new generation.
- **Generations** — the whole machine is one declarative description.
  Updates are atomic new generations with trial boot and automatic
  rollback.

## First target

x86_64 with VT-x/EPT (or AMD-V/NPT) and an IOMMU (VT-d / AMD-Vi).
First witness machine class: HP EliteDesk 800 G1.
