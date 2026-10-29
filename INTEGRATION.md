# SystemZ — House Integration Map

**Status:** Map, not code — 2026-10-29. Records designations and the
homes the architecture already has for the house's own work. **Nothing
in this document is integrated yet.** SystemZ has not booted a guest on
hardware; integration happens per-domain, as milestones (§21 of the
SPEC) bring domains online, and each piece below names the earliest
milestone that could honestly absorb it.

**The ruling (Chase, 2026-10-29):** *"Plug all of our utilities into
it. Nightwire — everything we already have made that will be useful."*
Together with the same night's designations — **C35/Comigen is
SystemZ's main graphics engine** and **orc is SystemZ's browser
system** — the house's existing work is SystemZ's composition. This
map is the census of that composition: what exists, where each piece
belongs in the architecture the SPEC already describes (bricks,
domains, ZBus, generations — this map invents no second architecture),
what form the integration takes, and what is true about it today.

## How to read the map

- **SystemZ home** — the domain, brick family, or layer the SPEC's
  architecture assigns the piece to.
- **Integration form** — *native port* (rewritten in-house, in the
  house languages, for the target domain), *brick wrap* (the piece
  arrives as a sealed brick / brick-set, §6), *protocol peer* (the
  piece's wire discipline becomes a pattern ZBus mirrors for
  off-machine peers), or *design-pattern-only* (what transfers is the
  conception, not the code).
- **Earliest milestone** — the first SPEC milestone whose exit proof
  creates the surface the piece needs. Earlier is not claimed.
- **Status today** — the honest state, in the house style: designated
  is not integrated, live elsewhere is not live here.

## The census

| Piece | What it is | Repo | SystemZ home | Form | Earliest milestone | Status today |
|---|---|---|---|---|---|---|
| **C35 / Comigen** | Procedural games graphics engine (Rust + wgpu): reality-driven worlds (real clock and latitude drive sun and seasons), every pass cost-ledgered | `nightwarelabz/C35` | **sys-gpu domain — the graphics / compositor layer** (SPEC §16, §3) | Brick wrap — the engine arrives as the sys-gpu / compositor brick-set; it renders into domain surfaces, never into ZCore | M8 | **Designated main graphics engine (Chase, 2026-10-29).** M1–M3 shipped in its own repo, M4 in build. Not integrated — no SystemZ domain exists to host it yet |
| **Comigen cost ledger + quality governor** | Every rendering pass priced; a governor sheds cost in ranked order instead of stalling | part of `nightwarelabz/C35` | System-wide doctrine: SPEC Law 7, §2 parts bin, §4 exit budgets, §13 ledger tooling | Design-pattern-only (already adopted) | In force from M1 | **Already plugged by design** — written into the SPEC's laws before this map existed |
| **orc** | The house browser: LibreWolf-based, rebranded and configured in-house, nosuchmachine.net as its home | `nightwarelabz/orc` | **Browser system** for user-facing domains (disposable, lab, project) | Brick wrap — an app-kind brick-set hosted inside a domain; the trusted core never contains it | M8 | **Designated browser system (Chase, 2026-10-29).** v0.1.10 ships standalone today. Not integrated |
| **Tractor** | orc's bundled search engine — a local service (127.0.0.1:45873), wired as orc's default search | ships with `nightwarelabz/orc` | orc's search brick, inside whichever domain hosts the browser | Brick wrap — rides with orc | M8 | Ships today as orc's default search (Tractor 0.5.1), the two wired as one system. Not integrated |
| **nightwire** | WebRTC gateway: real UDP from the browser to the house server, no relay queue — one static binary (Go + pion), bounded signaling, supervised sessions, counted backpressure | `nightwarelabz/nightwire` | sys-net-adjacent **realtime transport brick**; the pattern for ZBus-over-network peers — a ZBus channel whose far end is off-machine | Protocol peer / native port — its session discipline (bounded signaling, session lifecycle, counted drops) is the transferable design | M4 | Live today, serving the house game rooms from the house server host. Not integrated into SystemZ |
| **phonehome** | WebRTC transmitter / reception room inside Blacksite OS: peer-to-peer, signed signaling, sealed text channel, tap-to-talk voice | `nightwarelabz/nosuchmachine.net` | Realtime comms brick for operator / user domains — nightwire's sibling | Brick wrap, over nightwire-class transport | M7 | Built and serving on the website today; its own open proof (a real two-device call over a real network) is still owed there. Not integrated |
| **nightjar** | BLE listening post — passive advertisement logging, presence registry, tracker alerts — plus the iOS companion's send/serve payload suite | `nightwarelabz/nightjar` | **Peripheral / RF sensing brick family**, hosted in a sensing or lab domain; its EMCON conception is already in SPEC §2 | Native port (sensing core) / brick wrap (tooling above the admin line) | M7 | Ships standalone today (v12 published). Not integrated. Its posture is the family's charter — see notes |
| **Blacksite toolchest** | The web OS's command roster: 645 verbs in one declarative roster every surface reads | `nightwarelabz/nosuchmachine.net` | **ZAdmin's tooling heritage** — the roster discipline behind `zadm` (SPEC §15) | Design-pattern-only — the one-roster discipline transfers, not the TypeScript | Pattern from M2 | Live in production on the site. Heritage only; nothing ported |
| **SIGINT bench** | Receive-only RF bench: waterfall, demodulators, CW copy, graded intercept registry — EMCON-gated, no transmission path | `nightwarelabz/nosuchmachine.net` | **Diagnostics bricks for sys-net / ZAdmin** — spectrum diagnostics beside nightjar's sensing family | Design-pattern + brick wrap (its graded-registry discipline) | Pattern M4; bricks later | Live in production on the site. Not integrated |
| **Forensics bench + FXBAG** | Evidence workbench: hash-at-intake, append-only chain of custody, and FXBAG sealed evidence bags (magic `FXBAG001`, PBKDF2-SHA-256, AES-256-GCM) | `nightwarelabz/nosuchmachine.net` | **sys-store's evidence handling**, with the workbench as ZAdmin heritage; the custody chain is kin to §11's audit ledger | Native port (the FXBAG format) / design-pattern (custody discipline) | Format: M3-era candidate; workbench: M7 | Live in production; the format is proven in served bytes. Not integrated |
| **Relay + GuardianAngel** | Community IRC with a cross-device room memory; GuardianAngel is its always-on, silent automated moderation — no toggle, no announcements, it simply works | `nightwarelabz/nosuchmachine.net` | Comms service brick for user domains; GuardianAngel as a **channel-policy pattern** (moderation as silent policy, not prose) | Brick wrap / design-pattern | M7 | Live in production on the site. Not integrated |
| **phantom (phase1)** | The house Rust kernel — kernel + scheduler, zero external dependencies — the living original of the phase1 line; runs today as a guest-machine console inside Blacksite | `Bryforge/phase1` | **The pattern for developer-domain kernels**; SPEC §2 / §9 / §13 lineage (VFS, shell, editor conceptions) | Design-pattern / native lineage | M2 | **Already plugged by design**: ZCore's zero-dependency Rust discipline is its lineage, active today. No phase1 code sits in this repo |
| **Magic Box** | Keyless AES-256-GCM file container — the key wrapped by an RSW repeated-squaring time-lock (3072-bit modulus), so opening is a matter of time, not possession of a key | `chasebryan/magicbox` | **Vault-domain crypto brick heritage** — sealed containers; time-delayed opening as a vault mechanism | Native port (the container format as a vault / store brick) | M7 | Public repo; v1 stands — and was opened by the house's own attack tradition, which is the standing arrangement for its successor. v2 (passkey + time-destruction) designed, unbuilt. Not integrated |
| **orange** | A Rust language for specifying, implementing, and verifying cryptography | `chasebryan/orange` | **Developer-domain toolchain brick**: the verify half of a verify → build pipeline for crypto bricks; SPEC §11 already names it | Brick wrap (toolchain brick in project / build domains) | M6 | Exists (public repo); named in SPEC §11 as the crypto-verification path. Not integrated |
| **snoot** | PQC crypto-inventory scanner: finds classical crypto (RSA/ECDSA/DH), maps NIST PQC replacements, emits a CBOM (CycloneDX) | `chasebryan/snoot` | **Brick-store manifest attestation** — a brick's crypto inventory scanned and attested at ingest (SPEC §10, Verify); also a developer-domain tool brick | Native port / brick wrap | Attestation pattern: M3; tool brick: M6 | Exists (v0.1.0, AGPL-3.0; has run real scans with CBOM output). New to the SPEC by way of this map. Not integrated |
| **Authenticated 3-ary self-healing replication tree** | Blocks stored as `[x, tag, x⊕tag]` under keyed HMAC-SHA256 tags; majority-vote reconstruction heals whole-branch loss; presence flags refuse stale-but-valid rollback | Chase's crypto design (the orange work) | **The brick store itself** (SPEC §9; §2 parts bin) | Already native by design — it *is* the store's structure | M3 | **Already plugged by design.** The store it defines is M3 work; no store code exists yet |
| **Simulacrum's laws** | Real time locked — shipped hosts carry no time levers; the world seed rolled once and kept forever | House creative work (Simulacrum) | System laws: real-time is already SPEC §4 (timers); seed-keeping maps to §7's deterministic generations and to world / lab domain state | Design-pattern-only (already adopted) | In force now, as doctrine | **Already plugged by design** — §4 states the real-time law in Simulacrum and Comigen's name. This map adds only the provenance |
| **800g1-setup** | Post-install provisioning for the house development machine: one idempotent script, `--dry-run`, safe to re-run, signs in to nothing | `nightwarelabz/800g1-setup` | **sys-update / first-boot provisioning pattern** (SPEC §15 first boot; profile bricks) | Design-pattern-only — the script targets Ubuntu; what transfers is declarative, idempotent, re-runnable provisioning | Pattern now; provisioning bricks M6 | Ships today as a standalone script. Not integrated; SystemZ's first boot is its own (§19) |

## Notes the table can't hold

**The Blacksite benches, sorted.** The web OS's benches divide three
ways, and the map keeps them divided: the **toolchest** is *admin
tooling* heritage (it shapes `zadm`); the **SIGINT bench** is
*diagnostics* (it belongs beside sys-net, with nightjar's sensing
family); **Forensics / FXBAG** is *evidence handling* (it belongs to
sys-store). No Blacksite bench maps to sys-firewall policy itself —
firewall doctrine in the house line is EMCON plus default-deny
(SPEC §11, §12), and the map does not force a bench into a slot it
doesn't fit.

**nightjar's charter carries.** The sensing brick family's posture is
the one nightjar itself holds: passive listening, the operator's own
gear and environment — no stitching identities across time, no
location-history profiling of passersby. A SystemZ sensing domain
inherits that line as its charter, not as a feature to revisit.

**nightwire and phonehome are one family at two heights.** nightwire
is the transport (the gateway, the UDP, the session discipline);
phonehome is a room built on that class of transport. In SystemZ the
transport pattern arrives first (M4, with sys-net) and rooms arrive
with user domains (M7) — the same order the house built them in.

**orc's lineage is exactly why it is an app brick.** orc's engine
descends from upstream (LibreWolf / Firefox). Law 1 takes no outside
code into the trusted core — so orc can only ever enter as an app
brick-set inside a domain, which is what the map says. The same
reasoning keeps every bench's TypeScript out of the core: what
transfers from Blacksite is formats (FXBAG), disciplines (the roster,
the custody chain, silent moderation), and proof — not the web stack.

**Magic Box, honestly.** Its time-lock is a *delay* mechanism: the
house's own PQC review of Magic Box notes the RSW modulus is
classical RSA — Shor's algorithm collapses the delay, and there is no
drop-in NIST replacement for the time-lock itself. As a vault brick
its honest job is time-delayed opening as one mechanism among the
vault's (§5, §9), never as post-quantum confidentiality.

**Already in the SPEC by design.** Four census entries needed no
plugging in — they were written into the founding spec before this
map: the 3-ary tree (§9), the cost ledger + governor (Law 7, §2),
phantom's kernel lineage (§2), and Simulacrum's real-time law (§4).
The map cross-references them so the census is complete in one place;
their status is *designed-in*, which is a different thing from
*built* — the milestones still owe the building.

## Sequencing — what plugs in, when

Nothing plugs in now. The order below is the order the milestones
create the surfaces:

- **M2 — first domains.** Patterns become real: phase1's lineage
  becomes the first domain kernels; the toolchest's roster discipline
  shapes the first admin shell.
- **M3 — bricks and generations.** The store arrives, and with it the
  pieces the store can absorb: the 3-ary tree (already its design),
  snoot's CBOM attestation at ingest, FXBAG as the evidence-sealing
  format candidate.
- **M4 — wire.** The sys-net era: nightwire's realtime-transport
  patterns become ZBus-over-network peers; the SIGINT bench's
  diagnostics heritage lands beside sys-net. EMCON is already law.
- **M6 — developer's machine.** Toolchain bricks: orange's verify →
  build pipeline in project and build domains; 800g1-setup's
  provisioning pattern becomes provisioning bricks.
- **M7 — Studio.** The profile-era brick families: nightjar sensing,
  Magic Box's vault heritage, Relay comms, phonehome, the Forensics
  workbench.
- **M8 — guests and graphics, when called.** Comigen enters as the
  main graphics engine (sys-gpu / compositor brick-set); orc +
  Tractor arrive as the browser system in user-facing domains.

**Explicitly not being plugged in now:** everything on this page.
And one permanent exclusion, at any milestone: nothing on this map
enters ZCore. The core keeps its four jobs (SPEC §3, §4) — engines,
browsers, benches, and gateways live in domains, or they do not live
here at all. Blacksite OS the web suite is likewise not being ported
wholesale; what transfers is named, piece by piece, above.

---

*This map changes by PR like everything else in this repo (§18). A
row's status moves only when a milestone's exit proof moves it.*
