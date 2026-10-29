//! ZCore — the SystemZ microhypervisor (SPEC.md §4).
//!
//! M1 skeleton. What exists today, honestly:
//! - a freestanding entry point (`_start`) for our UEFI loader to enter,
//! - a COM1 serial writer (the flight recorder's first voice),
//! - hardware capability detection read straight from the CPU:
//!   vendor, VMX (Intel) / SVM (AMD), and the hypervisor-present bit,
//! - then a halt loop. Nothing else is claimed.
//!
//! - the first SVM guest machinery, complete: VMCB built by hand,
//!   host save area armed, VMRUN executed, CPUID-exit emulation and
//!   HLT-exit handling written. Its BIRTH is not yet proven: QEMU
//!   TCG's VMRUN is a no-op (control experiment in the svm section),
//!   so the birth proof awaits hardware or a KVM host.
//!
//! - nested page tables for the first guest, built from the boot
//!   manifest and verified by a software walk — but NOT wired into
//!   the VMCB: the wiring belongs to the birth proof on hardware.
//! - the VMX twin of the guest machinery: VMXON/VMCS regions, controls
//!   computed from the capability MSRs, a launch bridge mirroring
//!   svm_launch. Its entry is likewise unproven here (QEMU TCG does
//!   not offer VMX at all — the readiness block says so as measured).
//!
//! What does NOT exist yet: EPT, vCPU scheduling, ZBus. This file
//! will say so until they land.

#![no_std]
#![no_main]

use core::arch::asm;
use core::fmt::{self, Write};
use core::panic::PanicInfo;

// ---------------------------------------------------------------- serial

const COM1: u16 = 0x3F8;

unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nostack, preserves_flags));
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", out("al") value, in("dx") port, options(nostack, preserves_flags));
    value
}

struct Serial;

impl Serial {
    unsafe fn init() {
        outb(COM1 + 1, 0x00); // interrupts off
        outb(COM1 + 3, 0x80); // DLAB: set baud divisor
        outb(COM1 + 0, 0x01); // 115200
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1
        outb(COM1 + 2, 0xC7); // FIFO on, cleared
        outb(COM1 + 4, 0x0B); // DTR + RTS + OUT2
    }
}

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        unsafe {
            for byte in s.bytes() {
                while inb(COM1 + 5) & 0x20 == 0 {}
                outb(COM1, byte);
            }
        }
        Ok(())
    }
}

macro_rules! sprint {
    ($($arg:tt)*) => {
        {
            let mut serial = Serial;
            let _ = write!(serial, $($arg)*);
        }
    };
}

// ---------------------------------------------------------------- cpuid

struct Capabilities {
    vendor: [u8; 12],
    vmx: bool,
    svm: bool,
    hypervisor_present: bool,
    max_basic_leaf: u32,
}

fn probe() -> Capabilities {
    {
        let vendor_res = core::arch::x86_64::__cpuid(0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&vendor_res.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&vendor_res.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&vendor_res.ecx.to_le_bytes());

        let features = core::arch::x86_64::__cpuid(1);
        let vmx = features.ecx & (1 << 5) != 0;
        let hypervisor_present = features.ecx & (1 << 31) != 0;

        // SVM lives behind the AMD extended leaf; present only if the
        // leaf exists, so a bare Intel box never reads garbage as SVM.
        let ext_max = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        let svm = if ext_max >= 0x8000_0001 {
            core::arch::x86_64::__cpuid(0x8000_0001).ecx & (1 << 2) != 0
        } else {
            false
        };

        Capabilities {
            vendor,
            vmx,
            svm,
            hypervisor_present,
            max_basic_leaf: vendor_res.eax,
        }
    }
}

// ---------------------------------------------------------------- zboot
// Mirror of loader/zboot.h — the boot-manifest ABI. Field for field;
// if the two ever disagree, the magic check below is what catches it.

const ZBOOT_MAGIC: u64 = 0x31303054_4F4F425A; // "ZBOOT001" little-endian, per loader/zboot.h

#[repr(C)]
struct ZBootInfo {
    magic: u64,
    version: u32,
    info_size: u32,
    map_address: u64,
    map_size: u64,
    map_desc_size: u64,
    map_desc_version: u32,
    reserved0: u32,
    rsdp_address: u64,
    zcore_base: u64,
    zcore_entry: u64,
}

fn report_memory(info: &ZBootInfo) {
    sprint!("zboot: manifest v{} ok — map at {:#x}, {} bytes, desc {} bytes\r\n",
            info.version, info.map_address, info.map_size, info.map_desc_size);
    sprint!("zboot: acpi rsdp: {:#x}\r\n", info.rsdp_address);

    let mut sys_pages: u64 = 0;   // memory types: RAM and firmware regions
    let mut mmio_pages: u64 = 0;  // types 11/12: mapped IO apertures, not RAM
    let mut conventional_pages: u64 = 0;
    let mut descriptors: u64 = 0;
    if info.map_desc_size >= 40 && info.map_size > 0 {
        let base = info.map_address as *const u8;
        let count = info.map_size / info.map_desc_size;
        for i in 0..count {
            unsafe {
                let d = base.add((i * info.map_desc_size) as usize);
                let ty = core::ptr::read_unaligned(d as *const u32);
                let pages = core::ptr::read_unaligned(d.add(24) as *const u64);
                if ty == 11 || ty == 12 { mmio_pages += pages; } else { sys_pages += pages; }
                if ty == 7 { conventional_pages += pages; } // EfiConventionalMemory
            }
            descriptors += 1;
        }
    }
    // The page sums describe ADDRESS SPACE, not installed RAM: reserved
    // descriptors span device apertures far beyond physical memory.
    // The honest RAM headline at this stage is the conventional total.
    sprint!("zboot: memory map: {} descriptors, conventional (free) {} MiB, described span {} MiB of address space\r\n",
            descriptors,
            conventional_pages * 4096 / (1024 * 1024),
            (sys_pages + mmio_pages) * 4096 / (1024 * 1024));
}

// ---------------------------------------------------------------- rdmsr

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nostack));
    }
    ((hi as u64) << 32) | lo as u64
}

fn deep_probe(caps: &Capabilities) {
    if caps.vmx {
        // Safe to read: CPUID said VMX exists.
        sprint!("vmx: IA32_FEATURE_CONTROL = {:#x}\r\n", rdmsr(0x3A));
        let basic = rdmsr(0x480);
        sprint!("vmx: IA32_VMX_BASIC = {:#x} (rev {})\r\n", basic, basic & 0x7FFF_FFFF);
        sprint!("vmx: IA32_VMX_EPT_VPID_CAP = {:#x}\r\n", rdmsr(0x48C));
    }
    if caps.svm {
        let rev = core::arch::x86_64::__cpuid(0x8000_000A);
        // EBX carries the ASID count; EDX is feature bits.
        sprint!("svm: revision {:#x}, asids {}\r\n", rev.eax, rev.ebx);
        let vm_cr = rdmsr(0xC001_0114);
        sprint!("svm: VM_CR = {:#x} (svmdis {})\r\n", vm_cr, (vm_cr >> 4) & 1);
    }
}

// ---------------------------------------------------------------- svm
// The first guest. Deliberately tiny: a VMCB describing a long-mode
// guest whose whole program is XOR EAX,EAX / CPUID / HLT / spin.
// CPUID and HLT are intercepted. The hypervisor emulates the CPUID
// (writes the result registers itself, advances RIP past it) and
// re-enters; the guest then halts into a second exit. Two controlled
// exits = a guest was born, ran our code, and came back both times.
//
// VMCB field offsets follow the AMD architecture manuals, Vol. 3,
// Appendix B. Guest general-purpose registers other than RAX/RSP/RIP
// ride in the physical registers across VMRUN — svm_launch saves the
// host's, loads the guest's, and writes the guest's back on exit.

use core::arch::global_asm;

global_asm!(
    ".global svm_launch",
    "svm_launch:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "push rsi",             // regs pointer, kept on the stack
    "mov rax, rdi",         // RAX = VMCB physical address (VMRUN's operand)
    "mov rbx, [rsi]",
    "mov rcx, [rsi + 8]",
    "mov rdx, [rsi + 16]",
    "mov rdi, [rsi + 32]",
    "mov rbp, [rsi + 40]",
    "mov r8,  [rsi + 48]",
    "mov r9,  [rsi + 56]",
    "mov r10, [rsi + 64]",
    "mov r11, [rsi + 72]",
    "mov r12, [rsi + 80]",
    "mov r13, [rsi + 88]",
    "mov r14, [rsi + 96]",
    "mov r15, [rsi + 104]",
    "mov rsi, [rsi + 24]",  // guest RSI goes last; the pointer is spent
    "clgi",
    "vmrun",
    "stgi",
    "mov rax, [rsp]",       // recover the regs pointer
    "mov [rax], rbx",
    "mov [rax + 8], rcx",
    "mov [rax + 16], rdx",
    "mov [rax + 24], rsi",
    "mov [rax + 32], rdi",
    "mov [rax + 40], rbp",
    "mov [rax + 48], r8",
    "mov [rax + 56], r9",
    "mov [rax + 64], r10",
    "mov [rax + 72], r11",
    "mov [rax + 80], r12",
    "mov [rax + 88], r13",
    "mov [rax + 96], r14",
    "mov [rax + 104], r15",
    "pop rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
);

extern "C" {
    fn svm_launch(vmcb_pa: u64, regs: *mut GuestRegs);
}

#[repr(C)]
#[derive(Default)]
struct GuestRegs {
    rbx: u64, rcx: u64, rdx: u64, rsi: u64, rdi: u64, rbp: u64,
    r8: u64, r9: u64, r10: u64, r11: u64, r12: u64, r13: u64,
    r14: u64, r15: u64,
}

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct AlignedPage([u8; 4096]);

static mut VMCB: AlignedPage = AlignedPage([0; 4096]);
static mut HSAVE: AlignedPage = AlignedPage([0; 4096]);
static mut GUEST_STACK: AlignedPage = AlignedPage([0; 4096]);

// xor eax,eax ; cpuid ; hlt ; jmp $
static GUEST_CODE: [u8; 7] = [0x31, 0xC0, 0x0F, 0xA2, 0xF4, 0xEB, 0xFE];

// VMCB offsets used below.
mod voff {
    pub const INTERCEPT_MISC1: usize = 0x00C;
    pub const GUEST_ASID: usize = 0x030;
    pub const EXIT_CODE: usize = 0x048;
    pub const NP_ENABLE: usize = 0x068;
    pub const CLEAN: usize = 0x098;
    pub const SEG_ES: usize = 0x400;
    pub const SEG_CS: usize = 0x410;
    pub const SEG_SS: usize = 0x420;
    pub const SEG_DS: usize = 0x430;
    pub const SEG_GDTR: usize = 0x460;
    pub const SEG_IDTR: usize = 0x480;
    pub const EFER: usize = 0x4D0;
    pub const CR4: usize = 0x4E0;
    pub const CR3: usize = 0x4E8;
    pub const CR0: usize = 0x4F0;
    pub const RFLAGS: usize = 0x508;
    pub const RIP: usize = 0x510;
    pub const RSP: usize = 0x548;
    pub const RAX: usize = 0x5D8;
}

fn wrmsr(msr: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
             options(nostack));
    }
}

fn vmcb_w64(off: usize, value: u64) {
    unsafe {
        let base = core::ptr::addr_of_mut!(VMCB) as *mut u8;
        core::ptr::write_unaligned(base.add(off) as *mut u64, value);
    }
}
fn vmcb_w32(off: usize, value: u32) {
    unsafe {
        let base = core::ptr::addr_of_mut!(VMCB) as *mut u8;
        core::ptr::write_unaligned(base.add(off) as *mut u32, value);
    }
}
fn vmcb_r64(off: usize) -> u64 {
    unsafe {
        let base = core::ptr::addr_of!(VMCB) as *const u8;
        core::ptr::read_unaligned(base.add(off) as *const u64)
    }
}
fn vmcb_seg(off: usize, selector: u16, attrib: u16, limit: u32, base: u64) {
    unsafe {
        let p = (core::ptr::addr_of_mut!(VMCB) as *mut u8).add(off);
        core::ptr::write_unaligned(p as *mut u16, selector);
        core::ptr::write_unaligned(p.add(2) as *mut u16, attrib);
        core::ptr::write_unaligned(p.add(4) as *mut u32, limit);
        core::ptr::write_unaligned(p.add(8) as *mut u64, base);
    }
}

const EXIT_CPUID: u64 = 0x72;
const EXIT_HLT: u64 = 0x78;

fn svm_first_guest(manifest_addr: u64) {
    build_npt(manifest_addr);

    sprint!("svm: enabling SVM (EFER.SVME + host save area)...\r\n");
    let efer = rdmsr(0xC000_0080);
    let hsave_pa = core::ptr::addr_of!(HSAVE) as u64;
    wrmsr(0xC001_0117, hsave_pa); // VM_HSAVE_PA
    wrmsr(0xC000_0080, efer | (1 << 12)); // EFER.SVME

    // Host control state the guest inherits as its starting world:
    // the machine as the firmware left it — long mode, paging on,
    // identity-mapped where we live. The guest mirrors it.
    let (cr0, cr3, cr4): (u64, u64, u64);
    unsafe {
        let v0: u64; let v3: u64; let v4: u64;
        asm!("mov {}, cr0", out(reg) v0, options(nostack));
        asm!("mov {}, cr3", out(reg) v3, options(nostack));
        asm!("mov {}, cr4", out(reg) v4, options(nostack));
        cr0 = v0; cr3 = v3; cr4 = v4;
    }
    let code_pa = GUEST_CODE.as_ptr() as u64;

    // Intercepts: CPUID (misc1 bit 18) and HLT (misc1 bit 24).
    vmcb_w32(voff::INTERCEPT_MISC1, (1 << 18) | (1 << 24));
    vmcb_w32(voff::GUEST_ASID, 1);
    vmcb_w64(voff::NP_ENABLE, 0); // no nested paging yet: guest phys = host phys
    // Mirror the host's own world as the guest's first world: long
    // mode, the firmware's page tables, the host's descriptor tables,
    // the host's EFER (SVME included, as nested-virt test rigs do).
    let (gdt_base, gdt_limit, idt_base, idt_limit): (u64, u32, u64, u32);
    let (tr_sel, ldt_sel): (u16, u16);
    unsafe {
        let mut gdtr = [0u8; 10];
        let mut idtr = [0u8; 10];
        asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
        asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack));
        let trv: u16; let ldtv: u16;
        asm!("str {0:x}", out(reg) trv, options(nostack));
        asm!("sldt {0:x}", out(reg) ldtv, options(nostack));
        gdt_limit = u16::from_le_bytes(gdtr[0..2].try_into().unwrap()) as u32;
        gdt_base = u64::from_le_bytes(gdtr[2..10].try_into().unwrap());
        idt_limit = u16::from_le_bytes(idtr[0..2].try_into().unwrap()) as u32;
        idt_base = u64::from_le_bytes(idtr[2..10].try_into().unwrap());
        tr_sel = trv; ldt_sel = ldtv;
    }
    vmcb_seg(voff::SEG_CS, 0x08, 0x209B, 0xFFFF_FFFF, 0); // long-mode code
    vmcb_seg(voff::SEG_SS, 0x10, 0x0093, 0xFFFF_FFFF, 0);
    vmcb_seg(voff::SEG_DS, 0x10, 0x0093, 0xFFFF_FFFF, 0);
    vmcb_seg(voff::SEG_ES, 0x10, 0x0093, 0xFFFF_FFFF, 0);
    vmcb_seg(voff::SEG_GDTR, 0, 0, gdt_limit, gdt_base);
    vmcb_seg(voff::SEG_IDTR, 0, 0, idt_limit, idt_base);
    vmcb_seg(0x490, tr_sel, 0x008B, 0xFFFF, 0); // TR (attrib: busy TSS)
    vmcb_seg(0x470, ldt_sel, 0x0082, 0xFFFF, 0); // LDTR
    let _ = (tr_sel, ldt_sel);
    vmcb_w64(voff::EFER, efer | (1 << 12));
    vmcb_w64(voff::CR0, cr0);
    vmcb_w64(voff::CR3, cr3);
    vmcb_w64(voff::CR4, cr4);
    vmcb_w64(voff::RFLAGS, 0x2);
    vmcb_w64(voff::RIP, code_pa);
    vmcb_w64(voff::RSP, core::ptr::addr_of!(GUEST_STACK) as u64 + 4096);
    vmcb_w64(voff::RAX, 0);
    vmcb_w32(voff::CLEAN, 0);

    let vmcb_pa = core::ptr::addr_of!(VMCB) as u64;
    let mut regs = GuestRegs::default();
    sprint!("svm: vmcb {:#x} hsave {:#x} intercepts readback {:#x} host efer {:#x}\r\n",
            vmcb_pa, hsave_pa, vmcb_r64(voff::INTERCEPT_MISC1) & 0xFFFF_FFFF,
            rdmsr(0xC000_0080));
    sprint!("svm: VMRUN #1 — guest enters at {:#x}\r\n", code_pa);
    unsafe { svm_launch(vmcb_pa, &mut regs) };
    let exit1 = vmcb_r64(voff::EXIT_CODE);
    sprint!("svm: exit #1 code {:#x} (cpuid=0x72, hlt=0x78) info1 {:#x} info2 {:#x} guest rip {:#x}\r\n",
            exit1, vmcb_r64(0x50), vmcb_r64(0x58), vmcb_r64(voff::RIP));
    if exit1 == 0 && vmcb_r64(voff::RIP) == code_pa {
        // VMRUN returned without the guest executing a single
        // instruction. Proven on QEMU 8.2.2 TCG by control experiment:
        // with ALL intercepts cleared and a guest that is a pure
        // infinite loop, VMRUN still "returns" instantly, exit 0,
        // RIP untouched — its VMRUN is a no-op. Say so plainly; the
        // birth proof belongs on hardware or a KVM host (SPEC SS18).
        sprint!("svm: guest birth NOT CLAIMED here — VMRUN did not enter in this environment (emulated VMRUN is a no-op); the machinery is complete and awaits a capable host\r\n");
        return;
    }
    if exit1 != EXIT_CPUID {
        sprint!("svm: guest birth FAILED at first exit\r\n");
        return;
    }

    // Emulate CPUID leaf 0 the way an AMD part answers it, ourselves:
    // max leaf in RAX, "AuthenticAMD" across RBX/RDX/RCX.
    vmcb_w64(voff::RAX, 0xD);
    regs.rbx = 0x6874_7541; // "Auth"
    regs.rdx = 0x6974_6E65; // "enti"
    regs.rcx = 0x444D_4163; // "cAMD"
    vmcb_w64(voff::RIP, vmcb_r64(voff::RIP) + 2); // step over CPUID
    vmcb_w32(voff::CLEAN, 0);

    sprint!("svm: cpuID emulated (leaf 0 -> AuthenticAMD), VMRUN #2\r\n");
    unsafe { svm_launch(vmcb_pa, &mut regs) };
    let exit2 = vmcb_r64(voff::EXIT_CODE);
    sprint!("svm: exit #2 code {:#x}\r\n", exit2);
    if exit2 == EXIT_HLT {
        sprint!("svm: GUEST BORN — ran, took our emulated cpuid (rax={:#x} rbx={:#x}), halted back to ZCore\r\n",
                vmcb_r64(voff::RAX), regs.rbx);
    } else {
        sprint!("svm: guest birth incomplete — second exit was not HLT\r\n");
    }
}

// ---------------------------------------------------------------- npt
// Nested page tables for the first guest — groundwork only.
//
// The tables are BUILT from the boot manifest, VERIFIED by walking
// them in software, and their census is printed. They are NOT wired
// into the VMCB: NP_ENABLE stays 0 and nCR3 (VMCB offset 0x088) is
// deliberately never written this round. Wiring a guest-visible
// translation change nobody has witnessed a guest survive is how
// hardware bites; the wiring belongs to the birth-proof step on real
// hardware, beside the VMRUN that will first consume it.
//
// The mapping rule, stated once and used by both the builder and the
// verifier, so the census can be checked against it:
//   * The window is [0, W) where W is the highest end address among
//     RAM-typed manifest descriptors (UEFI types 1-7, 9, 10: loader /
//     boot-services / runtime code+data, conventional, ACPI
//     reclaim/NVS), rounded up to 2 MiB. MMIO apertures (types 11/12),
//     reserved (type 0) and everything else are NOT RAM: holes inside
//     the window stay not-present, so a guest touch there faults.
//   * A 4 KiB page is mapped identity (GPA == HPA) iff some RAM
//     descriptor covers it. Code descriptors (types 1/3/5) map
//     executable (NX clear); data descriptors (the rest) map NX.
//   * A 2 MiB large page replaces its 512 small pages iff every one
//     of them is covered AND all covering descriptors share one NX
//     class — otherwise the block falls back to a page table of
//     4 KiB entries. Presence, R/W and U/S are set at every level:
//     second-level translation is permissive here; the guest's own
//     tables enforce its privilege rules.
// Table budget: the window is capped at 64 GiB (1 PML4 page, 1 PDPT
// page, up to 64 PD pages) plus a 128-page PT pool. A machine whose
// map needs more prints that plainly instead of under-mapping
// silently. The tables describe themselves too: ZCore's own image
// sits in loader RAM, so the pool pages are inside the map they
// build — as with every hypervisor's EPT/NPT.

#[derive(Clone, Copy)]
struct RamRegion {
    start: u64,
    end: u64,
    code: bool, // covering descriptor is a firmware code type (NX clear)
}

const EMPTY_REGION: RamRegion = RamRegion { start: 0, end: 0, code: false };
const MAX_RAM_REGIONS: usize = 512;

static mut RAM_REGIONS: [RamRegion; MAX_RAM_REGIONS] = [EMPTY_REGION; MAX_RAM_REGIONS];

const ZERO_PAGE: AlignedPage = AlignedPage([0; 4096]);
static mut NPT_PML4: AlignedPage = AlignedPage([0; 4096]);
static mut NPT_PDPT: AlignedPage = AlignedPage([0; 4096]);
static mut NPT_PD: [AlignedPage; 64] = [ZERO_PAGE; 64];
const NPT_PT_POOL: usize = 128;
static mut NPT_PT: [AlignedPage; NPT_PT_POOL] = [ZERO_PAGE; NPT_PT_POOL];

const NPT_WINDOW_CAP: u64 = 64 * 1024 * 1024 * 1024; // 64 GiB table budget

const NP_P: u64 = 1 << 0;
const NP_RW: u64 = 1 << 1;
const NP_US: u64 = 1 << 2;
const NP_PS: u64 = 1 << 7;
const NP_NX: u64 = 1 << 63;
const NP_TABLE_FLAGS: u64 = NP_P | NP_RW | NP_US; // non-leaf entries
const NP_LEAF_FLAGS: u64 = NP_P | NP_RW | NP_US; // leaves add PS/NX as fit

fn npt_w64(page_base: *mut u8, index: usize, value: u64) {
    unsafe {
        core::ptr::write_unaligned(page_base.add(index * 8) as *mut u64, value);
    }
}
fn npt_r64(page_base: *const u8, index: usize) -> u64 {
    unsafe { core::ptr::read_unaligned(page_base.add(index * 8) as *const u64) }
}

/// Is this UEFI memory type physical RAM (as opposed to an MMIO
/// aperture, a reserved span, or firmware bookkeeping)?
fn is_ram_type(ty: u32) -> bool {
    matches!(ty, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 9 | 10)
}
/// Code types execute; the rest of RAM is data.
fn is_code_type(ty: u32) -> bool {
    matches!(ty, 1 | 3 | 5)
}

/// Parse the manifest's memory map into the sorted RAM-region list.
/// Returns (region count, window end before alignment, truncated?).
fn parse_ram_regions(info: &ZBootInfo) -> (usize, u64, bool) {
    let mut n = 0usize;
    let mut window_end = 0u64;
    let mut truncated = false;
    if info.map_desc_size >= 40 && info.map_size > 0 {
        let base = info.map_address as *const u8;
        let count = info.map_size / info.map_desc_size;
        for i in 0..count {
            let (ty, phys, pages) = unsafe {
                let d = base.add((i * info.map_desc_size) as usize);
                (
                    core::ptr::read_unaligned(d as *const u32),
                    core::ptr::read_unaligned(d.add(8) as *const u64),
                    core::ptr::read_unaligned(d.add(24) as *const u64),
                )
            };
            if !is_ram_type(ty) || pages == 0 {
                continue;
            }
            let end = match phys.checked_add(pages * 4096) {
                Some(e) => e,
                None => continue,
            };
            if end > window_end {
                window_end = end;
            }
            if n < MAX_RAM_REGIONS {
                unsafe {
                    let regs = &mut *core::ptr::addr_of_mut!(RAM_REGIONS);
                    regs[n] = RamRegion { start: phys, end, code: is_code_type(ty) };
                }
                n += 1;
            } else {
                truncated = true;
            }
        }
    }
    // Insertion sort by start: the region list is small and this runs
    // once, before anything else needs the machine.
    unsafe {
        let regs = &mut *core::ptr::addr_of_mut!(RAM_REGIONS);
        for i in 1..n {
            let key = regs[i];
            let mut j = i;
            while j > 0 && regs[j - 1].start > key.start {
                regs[j] = regs[j - 1];
                j -= 1;
            }
            regs[j] = key;
        }
    }
    (n, window_end, truncated)
}

/// The class of the first region covering `gpa`, if any: Some(code).
/// `hint` may be advanced across calls with non-decreasing gpa; the
/// first covering region is never skipped (a region is only passed
/// once its end is at or below the query address).
fn cover_hint(regions: &[RamRegion], hint: &mut usize, gpa: u64) -> Option<bool> {
    while *hint < regions.len() && regions[*hint].end <= gpa {
        *hint += 1;
    }
    if *hint < regions.len() {
        let r = &regions[*hint];
        if r.start <= gpa && gpa < r.end {
            return Some(r.code);
        }
    }
    None
}

/// For one 2 MiB block: (covered page count by union of regions,
/// any code region touches it, any data region touches it).
fn block_class(regions: &[RamRegion], block: u64) -> (u64, bool, bool) {
    let block_end = block + 0x20_0000;
    let mut covered: u64 = 0;
    let mut merge_s = u64::MAX;
    let mut merge_e = 0u64;
    let mut has_code = false;
    let mut has_data = false;
    for r in regions {
        if r.end <= block || r.start >= block_end {
            continue;
        }
        if r.code {
            has_code = true;
        } else {
            has_data = true;
        }
        let s = if r.start > block { r.start } else { block };
        let e = if r.end < block_end { r.end } else { block_end };
        if merge_s == u64::MAX {
            merge_s = s;
            merge_e = e;
        } else if s > merge_e {
            covered += merge_e - merge_s;
            merge_s = s;
            merge_e = e;
        } else if e > merge_e {
            merge_e = e;
        }
    }
    if merge_s != u64::MAX {
        covered += merge_e - merge_s;
    }
    (covered / 4096, has_code, has_data)
}

/// Walk the built tables for `gpa`. Some((leaf entry, is_large)).
fn npt_lookup(pml4: *const u8, gpa: u64) -> Option<(u64, bool)> {
    let pml4e = npt_r64(pml4, ((gpa >> 39) & 0x1FF) as usize);
    if pml4e & NP_P == 0 {
        return None;
    }
    let pdpt = (pml4e & 0x000F_FFFF_FFFF_F000) as *const u8;
    let pdpte = npt_r64(pdpt, ((gpa >> 30) & 0x1FF) as usize);
    if pdpte & NP_P == 0 {
        return None;
    }
    let pd = (pdpte & 0x000F_FFFF_FFFF_F000) as *const u8;
    let pde = npt_r64(pd, ((gpa >> 21) & 0x1FF) as usize);
    if pde & NP_P == 0 {
        return None;
    }
    if pde & NP_PS != 0 {
        return Some((pde, true));
    }
    let pt = (pde & 0x000F_FFFF_FFFF_F000) as *const u8;
    let pte = npt_r64(pt, ((gpa >> 12) & 0x1FF) as usize);
    if pte & NP_P == 0 {
        return None;
    }
    Some((pte, false))
}

fn build_npt(manifest_addr: u64) {
    sprint!("npt: groundwork — building nested page tables for the first guest (tables only; NOT wired into the VMCB)\r\n");
    if manifest_addr == 0 {
        sprint!("npt: skipped — no boot manifest, so no RAM window to size the map from\r\n");
        return;
    }
    let info = unsafe { &*(manifest_addr as *const ZBootInfo) };
    if info.magic != ZBOOT_MAGIC {
        sprint!("npt: skipped — manifest magic mismatch, refusing its contents\r\n");
        return;
    }
    let (nregions, raw_window_end, truncated) = parse_ram_regions(info);
    if nregions == 0 {
        sprint!("npt: skipped — manifest describes no RAM regions\r\n");
        return;
    }
    let regions: &[RamRegion] = unsafe { &(&*core::ptr::addr_of!(RAM_REGIONS))[..nregions] };
    let mut window = (raw_window_end + 0x1F_FFFF) & !0x1F_FFFFu64; // 2 MiB align up
    let mut capped = false;
    if window > NPT_WINDOW_CAP {
        window = NPT_WINDOW_CAP;
        capped = true;
    }
    sprint!("npt: window [0, {:#x}) from {} RAM descriptors (rule: highest RAM-descriptor end, 2 MiB aligned{})\r\n",
            window, nregions,
            if capped { "; CAPPED at the 64 GiB table budget — this machine has more RAM than the tables describe" } else { "" });
    if truncated {
        sprint!("npt: WARNING — manifest had more RAM descriptors than the {}-entry region list; the map is built from the first {} only\r\n",
                MAX_RAM_REGIONS, MAX_RAM_REGIONS);
    }

    // Zero the pools explicitly: determinism must not depend on how
    // the loader happened to leave .bss.
    let pml4 = core::ptr::addr_of_mut!(NPT_PML4) as *mut u8;
    let pdpt = core::ptr::addr_of_mut!(NPT_PDPT) as *mut u8;
    let pd_pool = core::ptr::addr_of_mut!(NPT_PD) as *mut u8;
    let pt_pool = core::ptr::addr_of_mut!(NPT_PT) as *mut u8;
    unsafe {
        core::ptr::write_bytes(pml4, 0, 4096);
        core::ptr::write_bytes(pdpt, 0, 4096);
        core::ptr::write_bytes(pd_pool, 0, 64 * 4096);
        core::ptr::write_bytes(pt_pool, 0, NPT_PT_POOL * 4096);
    }

    let npd = ((window + (1 << 30) - 1) >> 30) as usize;
    npt_w64(pml4, 0, (pdpt as u64) | NP_TABLE_FLAGS);

    let mut large: u64 = 0;
    let mut small: u64 = 0;
    let mut pt_used: usize = 0;
    let mut incomplete = false;
    for gi in 0..npd {
        let pd = unsafe { pd_pool.add(gi * 4096) };
        npt_w64(pdpt, gi, (pd as u64) | NP_TABLE_FLAGS);
        for bi in 0..512usize {
            let block = ((gi as u64) << 30) + ((bi as u64) << 21);
            if block >= window {
                break;
            }
            let (covered, has_code, has_data) = block_class(regions, block);
            if covered == 512 && has_code != has_data {
                let nx = if has_data { NP_NX } else { 0 };
                npt_w64(pd, bi, block | NP_LEAF_FLAGS | NP_PS | nx);
                large += 1;
            } else if covered > 0 {
                if pt_used >= NPT_PT_POOL {
                    sprint!("npt: BUILD INCOMPLETE — PT pool ({} pages) exhausted at GPA {:#x}; the map under-covers the window and says so\r\n",
                            NPT_PT_POOL, block);
                    incomplete = true;
                    break;
                }
                let pt = unsafe { pt_pool.add(pt_used * 4096) };
                pt_used += 1;
                npt_w64(pd, bi, (pt as u64) | NP_TABLE_FLAGS);
                let mut hint = 0usize;
                for pi in 0..512u64 {
                    let pg = block + pi * 4096;
                    if let Some(code) = cover_hint(regions, &mut hint, pg) {
                        let nx = if code { 0 } else { NP_NX };
                        npt_w64(pt, pi as usize, pg | NP_LEAF_FLAGS | nx);
                        small += 1;
                    }
                }
            }
        }
        if incomplete {
            break;
        }
    }

    sprint!("npt: tables: root {:#x} — pml4 1 page, pdpt 1 page, pd {} pages, pt {} of {} pool pages ({} table pages total)\r\n",
            pml4 as u64, npd, pt_used, NPT_PT_POOL, 2 + npd + pt_used);
    sprint!("npt: map: {} large pages (2 MiB) + {} small pages (4 KiB) = {} MiB mapped of the {} MiB window; holes (reserved/MMIO) stay not-present\r\n",
            large, small,
            (large * 0x20_0000 + small * 4096) / (1024 * 1024),
            window / (1024 * 1024));

    // ---- self-verification: two independent walks of the tables ----
    // Completeness: every page the rule says is RAM must be present in
    // the tables with the rule's address and flags. Soundness: every
    // present leaf must be a page the rule covers, with matching
    // address/flags — nothing extra may be mapped.
    let mut mismatches: u64 = 0;
    let mut expected_pages: u64 = 0;
    {
        let mut hint = 0usize;
        let mut gpa = 0u64;
        while gpa < window {
            if let Some(code) = cover_hint(regions, &mut hint, gpa) {
                expected_pages += 1;
                match npt_lookup(pml4, gpa) {
                    Some((entry, is_large)) => {
                        let want_addr = if is_large { gpa & !0x1F_FFFFu64 } else { gpa };
                        let got_addr = if is_large {
                            entry & 0x000F_FFFF_FFE0_0000
                        } else {
                            entry & 0x000F_FFFF_FFFF_F000
                        };
                        let want_nx = if code { 0 } else { NP_NX };
                        if got_addr != want_addr
                            || entry & NP_LEAF_FLAGS != NP_LEAF_FLAGS
                            || entry & NP_NX != want_nx
                        {
                            mismatches += 1;
                        }
                    }
                    None => mismatches += 1,
                }
            }
            gpa += 4096;
        }
    }
    let mut present_leaves: u64 = 0;
    let mut walked_pages: u64 = 0;
    for gi in 0..npd {
        let pd = unsafe { pd_pool.add(gi * 4096) } as *const u8;
        for bi in 0..512usize {
            let block = ((gi as u64) << 30) + ((bi as u64) << 21);
            if block >= window {
                break;
            }
            let pde = npt_r64(pd, bi);
            if pde & NP_P == 0 {
                continue;
            }
            if pde & NP_PS != 0 {
                present_leaves += 1;
                walked_pages += 512;
                let mut hint = 0usize;
                let code = cover_hint(regions, &mut hint, block);
                let want_nx = match code {
                    Some(true) => 0,
                    Some(false) => NP_NX,
                    None => {
                        mismatches += 1;
                        continue;
                    }
                };
                if pde & 0x000F_FFFF_FFE0_0000 != block
                    || pde & NP_LEAF_FLAGS != NP_LEAF_FLAGS
                    || pde & NP_NX != want_nx
                {
                    mismatches += 1;
                }
            } else {
                let pt = (pde & 0x000F_FFFF_FFFF_F000) as *const u8;
                let mut hint = 0usize;
                for pi in 0..512usize {
                    let pte = npt_r64(pt, pi);
                    if pte & NP_P == 0 {
                        continue;
                    }
                    present_leaves += 1;
                    walked_pages += 1;
                    let pg = block + (pi as u64) * 4096;
                    match cover_hint(regions, &mut hint, pg) {
                        Some(code) => {
                            let want_nx = if code { 0 } else { NP_NX };
                            if pte & 0x000F_FFFF_FFFF_F000 != pg
                                || pte & NP_LEAF_FLAGS != NP_LEAF_FLAGS
                                || pte & NP_NX != want_nx
                            {
                                mismatches += 1;
                            }
                        }
                        None => mismatches += 1,
                    }
                }
            }
        }
    }
    if mismatches == 0 && !incomplete && walked_pages == expected_pages {
        sprint!("npt: self-check PASS — {} RAM pages expected by the rule, {} page-equivalents found in the tables, {} present leaves, 0 mismatches\r\n",
                expected_pages, walked_pages, present_leaves);
    } else {
        sprint!("npt: self-check FAIL — {} RAM pages expected, {} page-equivalents in tables, {} mismatches{}\r\n",
                expected_pages, walked_pages, mismatches,
                if incomplete { " (build incomplete)" } else { "" });
    }
    sprint!("npt: root {:#x} is the nCR3 candidate — NOT wired: VMCB NP_ENABLE stays 0 and nCR3 is never written this round; wiring belongs to the birth proof on real hardware\r\n",
            pml4 as u64);
}

// ---------------------------------------------------------------- vmx
// The Intel twin of the SVM first guest: same guest program (the
// shared GUEST_CODE: XOR EAX,EAX / CPUID / HLT / spin), same intercept
// policy (CPUID always exits on VMX — there is no control bit for it;
// HLT exiting is armed), same honesty law (a launch that returns
// without the guest executing is detected and stated, never dressed
// as a birth).
//
// The mirror is translated, not copied, where the vendors differ:
//   * guest GPRs ride in physical registers across VMLAUNCH/VMRESUME
//     exactly as across VMRUN, but guest RAX is register-carried on
//     VMX (the VMCS has no RAX field; the VMCB does).
//   * guest EFER is the host's EFER translated: LMA dropped (entry
//     derives it), AMD-only bits (SVME et al.) dropped — set in the
//     VMCS they would be reserved-bit entry failures on Intel.
//   * HOST_RSP / HOST_RIP are written by the launch bridge itself:
//     only the bridge knows the exact stack the exit must land on.
//   * EPT is NOT half-built here: secondary controls stay off, the
//     EPTP field is never written, and the readiness block says EPT
//     is the next groundwork step, the twin of this round's NPT.

use core::arch::global_asm as global_asm_vmx;

global_asm_vmx!(
    ".global vmx_launch",
    "vmx_launch:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "push rdi",             // regs pointer slot: [rsp + 8] after the sub
    "sub rsp, 8",           // [rsp]: scratch slot (guest RAX stash)
    // HOST_RSP = the RSP at this instant: a VM exit lands exactly
    // here, and a failed launch never changed RSP — both paths agree.
    "mov rax, rsp",
    "mov rcx, 0x6C14",      // HOST_RSP field encoding
    "vmwrite rax, rcx",
    "lea rax, [rip + vmx_exit_point]",
    "mov rcx, 0x6C16",      // HOST_RIP field encoding
    "vmwrite rax, rcx",
    // Load the guest's GPRs from the regs struct.
    "mov rax, [rsp + 8]",
    "mov rbx, [rax + 8]",
    "mov rcx, [rax + 16]",
    "mov rdx, [rax + 24]",
    "mov rsi, [rax + 32]",
    "mov rdi, [rax + 40]",
    "mov rbp, [rax + 48]",
    "mov r8,  [rax + 56]",
    "mov r9,  [rax + 64]",
    "mov r10, [rax + 72]",
    "mov r11, [rax + 80]",
    "mov r12, [rax + 88]",
    "mov r13, [rax + 96]",
    "mov r14, [rax + 104]",
    "mov r15, [rax + 112]",
    "cmp byte ptr [rax + 120], 0",  // resume flag
    "mov rax, [rax]",       // guest RAX last; the pointer is spent
    "jne 1f",
    "vmlaunch",
    "jmp vmx_exit_point",   // launch failure falls through with CF/ZF set
    "1:",
    "vmresume",
    "vmx_exit_point:",
    // Both arrival paths (real VM exit, failed launch) hold the
    // to-be-stored register set and meaningful CF/ZF. MOVs only until
    // the flags are captured — nothing may disturb them.
    "mov [rsp], rax",
    "mov rax, [rsp + 8]",
    "mov [rax + 8], rbx",
    "mov [rax + 16], rcx",
    "mov [rax + 24], rdx",
    "mov [rax + 32], rsi",
    "mov [rax + 40], rdi",
    "mov [rax + 48], rbp",
    "mov [rax + 56], r8",
    "mov [rax + 64], r9",
    "mov [rax + 72], r10",
    "mov [rax + 80], r11",
    "mov [rax + 88], r12",
    "mov [rax + 96], r13",
    "mov [rax + 104], r14",
    "mov [rax + 112], r15",
    "mov rcx, [rsp]",       // stashed guest RAX (RCX already stored)
    "mov [rax], rcx",
    "setc byte ptr [rax + 128]",
    "setz byte ptr [rax + 129]",
    "cld",                  // the caller's ABI wants DF clear, always
    "add rsp, 8",
    "pop rdi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
);

extern "C" {
    fn vmx_launch(regs: *mut VmxRegs);
}

// Field offsets are struct-order offsets — the bridge's constants
// (120 = resume, 128/129 = captured CF/ZF) must match this layout.
#[repr(C)]
#[derive(Default)]
struct VmxRegs {
    rax: u64, rbx: u64, rcx: u64, rdx: u64, rsi: u64, rdi: u64, rbp: u64,
    r8: u64, r9: u64, r10: u64, r11: u64, r12: u64, r13: u64,
    r14: u64, r15: u64,
    resume: u64,
    flag_cf: u8,
    flag_zf: u8,
    _pad: [u8; 6],
}

static mut VMXON_REGION: AlignedPage = AlignedPage([0; 4096]);
static mut VMCS_REGION: AlignedPage = AlignedPage([0; 4096]);
static mut VMX_GUEST_STACK: AlignedPage = AlignedPage([0; 4096]);

// VMX MSRs (Intel SDM Vol. 4).
mod msr_vmx {
    pub const FEATURE_CONTROL: u32 = 0x3A;
    pub const VMX_BASIC: u32 = 0x480;
    pub const PINBASED: u32 = 0x481;
    pub const PROCBASED: u32 = 0x482;
    pub const EXIT: u32 = 0x483;
    pub const ENTRY: u32 = 0x484;
    pub const CR0_FIXED0: u32 = 0x486;
    pub const CR0_FIXED1: u32 = 0x487;
    pub const CR4_FIXED0: u32 = 0x488;
    pub const CR4_FIXED1: u32 = 0x489;
    pub const TRUE_PINBASED: u32 = 0x48D;
    pub const TRUE_PROCBASED: u32 = 0x48E;
    pub const TRUE_EXIT: u32 = 0x48F;
    pub const TRUE_ENTRY: u32 = 0x490;
    pub const EFER: u32 = 0xC000_0080;
    pub const PAT: u32 = 0x277;
    pub const SYSENTER_CS: u32 = 0x174;
    pub const SYSENTER_ESP: u32 = 0x175;
    pub const SYSENTER_EIP: u32 = 0x176;
    pub const FS_BASE: u32 = 0xC000_0100;
    pub const GS_BASE: u32 = 0xC000_0101;
}

// VMCS field encodings (Intel SDM Vol. 3, Appendix B).
mod vf {
    // 16-bit guest / host state
    pub const G_ES_SEL: u64 = 0x0800;
    pub const G_CS_SEL: u64 = 0x0802;
    pub const G_SS_SEL: u64 = 0x0804;
    pub const G_DS_SEL: u64 = 0x0806;
    pub const G_FS_SEL: u64 = 0x0808;
    pub const G_GS_SEL: u64 = 0x080A;
    pub const G_LDTR_SEL: u64 = 0x080C;
    pub const G_TR_SEL: u64 = 0x080E;
    pub const H_ES_SEL: u64 = 0x0C00;
    pub const H_CS_SEL: u64 = 0x0C02;
    pub const H_SS_SEL: u64 = 0x0C04;
    pub const H_DS_SEL: u64 = 0x0C06;
    pub const H_FS_SEL: u64 = 0x0C08;
    pub const H_GS_SEL: u64 = 0x0C0A;
    pub const H_TR_SEL: u64 = 0x0C0E;
    // 64-bit guest / host state
    pub const G_LINK_PTR: u64 = 0x2800;
    pub const G_PAT: u64 = 0x2804;
    pub const G_EFER: u64 = 0x2806;
    pub const H_EFER: u64 = 0x2C02;
    // 32-bit control
    pub const PIN_CTLS: u64 = 0x4000;
    pub const PROC_CTLS: u64 = 0x4002;
    pub const EXC_BITMAP: u64 = 0x4004;
    pub const PF_ERR_MASK: u64 = 0x4006;
    pub const PF_ERR_MATCH: u64 = 0x4008;
    pub const CR3_TGT_COUNT: u64 = 0x400A;
    pub const EXIT_CTLS: u64 = 0x400C;
    pub const EXIT_MSR_STORE: u64 = 0x400E;
    pub const EXIT_MSR_LOAD: u64 = 0x4010;
    pub const ENTRY_CTLS: u64 = 0x4012;
    pub const ENTRY_MSR_LOAD: u64 = 0x4014;
    pub const ENTRY_INT_INFO: u64 = 0x4016;
    // 32-bit read-only
    pub const INSTR_ERROR: u64 = 0x4400;
    pub const EXIT_REASON: u64 = 0x4402;
    // 32-bit guest state
    pub const G_ES_LIM: u64 = 0x4800;
    pub const G_CS_LIM: u64 = 0x4802;
    pub const G_SS_LIM: u64 = 0x4804;
    pub const G_DS_LIM: u64 = 0x4806;
    pub const G_FS_LIM: u64 = 0x4808;
    pub const G_GS_LIM: u64 = 0x480A;
    pub const G_LDTR_LIM: u64 = 0x480C;
    pub const G_TR_LIM: u64 = 0x480E;
    pub const G_GDTR_LIM: u64 = 0x4810;
    pub const G_IDTR_LIM: u64 = 0x4812;
    pub const G_ES_AR: u64 = 0x4814;
    pub const G_CS_AR: u64 = 0x4816;
    pub const G_SS_AR: u64 = 0x4818;
    pub const G_DS_AR: u64 = 0x481A;
    pub const G_FS_AR: u64 = 0x481C;
    pub const G_GS_AR: u64 = 0x481E;
    pub const G_LDTR_AR: u64 = 0x4820;
    pub const G_TR_AR: u64 = 0x4822;
    pub const G_INTERRUPTIBILITY: u64 = 0x4824;
    pub const G_ACTIVITY: u64 = 0x4826;
    pub const G_SYSENTER_CS: u64 = 0x482A;
    // 32-bit host state
    pub const H_SYSENTER_CS: u64 = 0x4C00;
    // natural-width control
    pub const CR0_MASK: u64 = 0x6000;
    pub const CR4_MASK: u64 = 0x6002;
    pub const CR0_SHADOW: u64 = 0x6004;
    pub const CR4_SHADOW: u64 = 0x6006;
    // natural-width guest state
    pub const G_CR0: u64 = 0x6800;
    pub const G_CR3: u64 = 0x6802;
    pub const G_CR4: u64 = 0x6804;
    pub const G_ES_BASE: u64 = 0x6806;
    pub const G_CS_BASE: u64 = 0x6808;
    pub const G_SS_BASE: u64 = 0x680A;
    pub const G_DS_BASE: u64 = 0x680C;
    pub const G_FS_BASE: u64 = 0x680E;
    pub const G_GS_BASE: u64 = 0x6810;
    pub const G_LDTR_BASE: u64 = 0x6812;
    pub const G_TR_BASE: u64 = 0x6814;
    pub const G_GDTR_BASE: u64 = 0x6816;
    pub const G_IDTR_BASE: u64 = 0x6818;
    pub const G_DR7: u64 = 0x681A;
    pub const G_RSP: u64 = 0x681C;
    pub const G_RIP: u64 = 0x681E;
    pub const G_RFLAGS: u64 = 0x6820;
    pub const G_PENDING_DBG: u64 = 0x6822;
    pub const G_SYSENTER_ESP: u64 = 0x6824;
    pub const G_SYSENTER_EIP: u64 = 0x6826;
    // natural-width host state
    pub const H_CR0: u64 = 0x6C00;
    pub const H_CR3: u64 = 0x6C02;
    pub const H_CR4: u64 = 0x6C04;
    pub const H_FS_BASE: u64 = 0x6C06;
    pub const H_GS_BASE: u64 = 0x6C08;
    pub const H_TR_BASE: u64 = 0x6C0A;
    pub const H_GDTR_BASE: u64 = 0x6C0C;
    pub const H_IDTR_BASE: u64 = 0x6C0E;
    pub const H_SYSENTER_ESP: u64 = 0x6C10;
    pub const H_SYSENTER_EIP: u64 = 0x6C12;
    // HOST_RSP (0x6C14) and HOST_RIP (0x6C16) have no constants here:
    // the launch bridge writes them, in asm, and owns the encodings.
}

const VMX_EXIT_REASON_CPUID: u64 = 10;
const VMX_EXIT_REASON_HLT: u64 = 12;

fn read_cr(n: u8) -> u64 {
    let v: u64;
    unsafe {
        match n {
            0 => asm!("mov {}, cr0", out(reg) v, options(nostack)),
            3 => asm!("mov {}, cr3", out(reg) v, options(nostack)),
            _ => asm!("mov {}, cr4", out(reg) v, options(nostack)),
        }
    }
    v
}
fn write_cr(n: u8, v: u64) {
    unsafe {
        match n {
            0 => asm!("mov cr0, {}", in(reg) v, options(nostack)),
            3 => asm!("mov cr3, {}", in(reg) v, options(nostack)),
            _ => asm!("mov cr4, {}", in(reg) v, options(nostack)),
        }
    }
}

fn seg_selectors() -> (u16, u16, u16, u16, u16, u16, u16, u16) {
    // (cs, ss, ds, es, fs, gs, tr, ldtr) as the CPU currently holds them
    let (cs, ss, ds, es, fs, gs, tr, ldtr): (u16, u16, u16, u16, u16, u16, u16, u16);
    unsafe {
        asm!("mov {0:x}, cs", out(reg) cs, options(nostack, preserves_flags));
        asm!("mov {0:x}, ss", out(reg) ss, options(nostack, preserves_flags));
        asm!("mov {0:x}, ds", out(reg) ds, options(nostack, preserves_flags));
        asm!("mov {0:x}, es", out(reg) es, options(nostack, preserves_flags));
        asm!("mov {0:x}, fs", out(reg) fs, options(nostack, preserves_flags));
        asm!("mov {0:x}, gs", out(reg) gs, options(nostack, preserves_flags));
        asm!("str {0:x}", out(reg) tr, options(nostack, preserves_flags));
        asm!("sldt {0:x}", out(reg) ldtr, options(nostack, preserves_flags));
    }
    (cs, ss, ds, es, fs, gs, tr, ldtr)
}

/// LAR for `sel`: Some(VMCS-format access rights) or None if the
/// segment is unusable (null selector or LAR fails). VMCS AR layout:
/// bits 7:0 = descriptor access byte, 11:8 = G/D-B/L/AVL, bit 16 =
/// unusable — assembled from LAR's raw result.
fn seg_ar(sel: u16) -> u32 {
    if sel == 0 {
        return 1 << 16;
    }
    let raw: u32;
    let zf: u8;
    unsafe {
        asm!("lar {a:e}, {s:x}", "setz {z}",
             a = out(reg) raw, s = in(reg) sel, z = out(reg_byte) zf,
             options(nostack));
    }
    if zf == 0 {
        1 << 16
    } else {
        ((raw >> 8) & 0xFF) | ((raw >> 12) & 0xF00)
    }
}

fn seg_limit(sel: u16) -> u32 {
    if sel == 0 {
        return 0;
    }
    let lim: u32;
    let zf: u8;
    unsafe {
        asm!("lsl {l:e}, {s:x}", "setz {z}",
             l = out(reg) lim, s = in(reg) sel, z = out(reg_byte) zf,
             options(nostack));
    }
    if zf == 0 {
        0
    } else {
        lim
    }
}

/// Base of the GDT descriptor `sel` names, including the high dword
/// of 16-byte system descriptors (TSS/LDT). LDT-indexed selectors and
/// null selectors base at 0 (the LDTR itself is mirrored separately).
fn gdt_desc_base(gdt_base: u64, sel: u16) -> u64 {
    if sel == 0 || sel & 0x4 != 0 {
        return 0;
    }
    let idx = (sel >> 3) as u64;
    let d = unsafe { core::ptr::read_unaligned((gdt_base + idx * 8) as *const u64) };
    let mut base = ((d >> 16) & 0xFFFF) | (((d >> 32) & 0xFF) << 16) | (((d >> 56) & 0xFF) << 24);
    let ty = (d >> 40) & 0xF;
    let s = (d >> 44) & 1;
    if s == 0 && (ty == 2 || ty == 9 || ty == 11) {
        let hi = unsafe { core::ptr::read_unaligned((gdt_base + idx * 8 + 8) as *const u32) } as u64;
        base |= hi << 32;
    }
    base
}

fn dt_regs() -> (u64, u32, u64, u32) {
    // (gdt base, gdt limit, idt base, idt limit)
    let mut gdtr = [0u8; 10];
    let mut idtr = [0u8; 10];
    unsafe {
        asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
        asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack));
    }
    (
        u64::from_le_bytes(gdtr[2..10].try_into().unwrap()),
        u16::from_le_bytes(gdtr[0..2].try_into().unwrap()) as u32,
        u64::from_le_bytes(idtr[2..10].try_into().unwrap()),
        u16::from_le_bytes(idtr[0..2].try_into().unwrap()) as u32,
    )
}

/// One VMX instruction with a physical-address operand: returns
/// (CF, ZF) exactly as the instruction left them.
unsafe fn vmx_op_pa(mnemonic: u8, pa: u64) -> (bool, bool) {
    let slot = pa;
    let addr = &slot as *const u64 as u64;
    let cf: u8;
    let zf: u8;
    match mnemonic {
        0 => asm!("vmxon [{p}]", "setc {c}", "setz {z}",
                  p = in(reg) addr, c = out(reg_byte) cf, z = out(reg_byte) zf,
                  options(nostack)),
        1 => asm!("vmclear [{p}]", "setc {c}", "setz {z}",
                  p = in(reg) addr, c = out(reg_byte) cf, z = out(reg_byte) zf,
                  options(nostack)),
        _ => asm!("vmptrld [{p}]", "setc {c}", "setz {z}",
                  p = in(reg) addr, c = out(reg_byte) cf, z = out(reg_byte) zf,
                  options(nostack)),
    }
    (cf != 0, zf != 0)
}

unsafe fn vmxoff() -> (bool, bool) {
    let cf: u8;
    let zf: u8;
    asm!("vmxoff", "setc {c}", "setz {z}",
         c = out(reg_byte) cf, z = out(reg_byte) zf, options(nostack));
    (cf != 0, zf != 0)
}

fn vmwrite_checked(field: u64, value: u64) -> bool {
    let cf: u8;
    let zf: u8;
    unsafe {
        asm!("vmwrite {v}, {f}", "setc {c}", "setz {z}",
             v = in(reg) value, f = in(reg) field,
             c = out(reg_byte) cf, z = out(reg_byte) zf, options(nostack));
    }
    cf == 0 && zf == 0
}

fn vmread_checked(field: u64) -> Option<u64> {
    let value: u64;
    let cf: u8;
    let zf: u8;
    unsafe {
        asm!("vmread {f}, {v}", "setc {c}", "setz {z}",
             f = in(reg) field, v = out(reg) value,
             c = out(reg_byte) cf, z = out(reg_byte) zf, options(nostack));
    }
    if cf == 0 && zf == 0 {
        Some(value)
    } else {
        None
    }
}

/// Compute a VMX control field the only honest way: desired bits,
/// forced through the capability MSR's allowed-0 (must-be-1) and
/// allowed-1 (may-be-1) masks. Never a hard-coded final value.
fn adjust_controls(msr: u32, desired: u32) -> u32 {
    let caps = rdmsr(msr);
    (desired | (caps as u32)) & ((caps >> 32) as u32)
}

fn vmx_readiness_absent() {
    sprint!("vmx: readiness: CPUID leaf 1 reports NO VMX on this host (measured) — VMXON, VMCS and entry not attempted; the Intel twin path is built and awaits Intel hardware (or a KVM host on Intel)\r\n");
}

fn vmx_teardown(vmcs_pa: u64, vmcs_current: bool, orig_cr0: u64, orig_cr4: u64) {
    if vmcs_current {
        let _ = unsafe { vmx_op_pa(1, vmcs_pa) }; // VMCLEAR
    }
    let _ = unsafe { vmxoff() };
    write_cr(0, orig_cr0);
    write_cr(4, orig_cr4);
    sprint!("vmx: teardown: VMXOFF done, host cr0/cr4 restored to their pre-VMX values\r\n");
}

fn vmx_first_guest() {
    sprint!("vmx: readiness: CPUID.VMX present — building the Intel twin of the first guest\r\n");

    // ---- firmware gate: IA32_FEATURE_CONTROL ----
    let mut fc = rdmsr(msr_vmx::FEATURE_CONTROL);
    if fc & 1 == 0 {
        wrmsr(msr_vmx::FEATURE_CONTROL, fc | 0x7); // lock + VMX in/out of SMX
        fc = rdmsr(msr_vmx::FEATURE_CONTROL);
        sprint!("vmx: IA32_FEATURE_CONTROL was unlocked — locked it with VMX enabled, now {:#x}\r\n", fc);
    }
    if fc & (1 << 2) == 0 {
        sprint!("vmx: readiness: firmware has VMX disabled (IA32_FEATURE_CONTROL {:#x}: locked, VMX-outside-SMX clear) — VMXON not attempted on this machine as configured\r\n", fc);
        return;
    }
    let basic = rdmsr(msr_vmx::VMX_BASIC);
    let revision = basic & 0x7FFF_FFFF;
    let use_true = basic & (1 << 55) != 0;
    sprint!("vmx: IA32_VMX_BASIC {:#x} (vmcs revision {}, true-controls {})\r\n",
            basic, revision, use_true);

    // ---- CR0/CR4 into their VMX-fixed ranges; VMXE on ----
    let orig_cr0 = read_cr(0);
    let orig_cr3 = read_cr(3);
    let orig_cr4 = read_cr(4);
    let adj_cr0 = (orig_cr0 | rdmsr(msr_vmx::CR0_FIXED0)) & rdmsr(msr_vmx::CR0_FIXED1);
    let adj_cr4 = ((orig_cr4 | rdmsr(msr_vmx::CR4_FIXED0)) & rdmsr(msr_vmx::CR4_FIXED1)) | (1 << 13);
    write_cr(0, adj_cr0);
    write_cr(4, adj_cr4);
    sprint!("vmx: cr0 {:#x} cr4 {:#x} (fixed-range adjusted, CR4.VMXE set)\r\n", adj_cr0, adj_cr4);

    // ---- VMXON ----
    let vmxon_pa = core::ptr::addr_of!(VMXON_REGION) as u64;
    unsafe {
        core::ptr::write_bytes(core::ptr::addr_of_mut!(VMXON_REGION) as *mut u8, 0, 4096);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!(VMXON_REGION) as *mut u32, revision as u32);
    }
    let (cf, zf) = unsafe { vmx_op_pa(0, vmxon_pa) };
    if cf || zf {
        sprint!("vmx: VMXON FAILED (cf {} zf {}) — the twin path stops here; no VMCS, no entry\r\n", cf as u8, zf as u8);
        write_cr(0, orig_cr0);
        write_cr(4, orig_cr4);
        return;
    }
    sprint!("vmx: VMXON region {:#x} (revision {} written from IA32_VMX_BASIC) — VMXON ok\r\n",
            vmxon_pa, revision);

    // ---- VMCS ----
    let vmcs_pa = core::ptr::addr_of!(VMCS_REGION) as u64;
    unsafe {
        core::ptr::write_bytes(core::ptr::addr_of_mut!(VMCS_REGION) as *mut u8, 0, 4096);
        // VMCS revision ID; bit 31 clear = this is not a shadow VMCS.
        core::ptr::write_unaligned(core::ptr::addr_of_mut!(VMCS_REGION) as *mut u32, revision as u32);
    }
    let (cf, zf) = unsafe { vmx_op_pa(1, vmcs_pa) };
    if cf || zf {
        sprint!("vmx: VMCLEAR FAILED (cf {} zf {}) — twin path stops\r\n", cf as u8, zf as u8);
        vmx_teardown(vmcs_pa, false, orig_cr0, orig_cr4);
        return;
    }
    let (cf, zf) = unsafe { vmx_op_pa(2, vmcs_pa) };
    if cf || zf {
        sprint!("vmx: VMPTRLD FAILED (cf {} zf {}) — twin path stops\r\n", cf as u8, zf as u8);
        vmx_teardown(vmcs_pa, false, orig_cr0, orig_cr4);
        return;
    }
    sprint!("vmx: VMCS region {:#x} (revision {}, shadow indicator clear) — VMCLEAR + VMPTRLD ok\r\n",
            vmcs_pa, revision);

    // ---- controls, computed from the capability MSRs ----
    let pin_msr = if use_true { msr_vmx::TRUE_PINBASED } else { msr_vmx::PINBASED };
    let proc_msr = if use_true { msr_vmx::TRUE_PROCBASED } else { msr_vmx::PROCBASED };
    let exit_msr = if use_true { msr_vmx::TRUE_EXIT } else { msr_vmx::EXIT };
    let entry_msr = if use_true { msr_vmx::TRUE_ENTRY } else { msr_vmx::ENTRY };
    let pin_ctls = adjust_controls(pin_msr, 0);
    // HLT exiting (primary bit 7). CPUID needs no bit: it exits
    // unconditionally under VMX. Secondary controls stay off, so
    // EPT stays unbuilt this round — see the section header.
    let proc_ctls = adjust_controls(proc_msr, 1 << 7);
    // Exit: 64-bit host (bit 9), save guest EFER (20), load host EFER (21).
    let exit_ctls = adjust_controls(exit_msr, (1 << 9) | (1 << 20) | (1 << 21));
    // Entry: IA-32e-mode guest (bit 9), load guest EFER (bit 16).
    let entry_ctls = adjust_controls(entry_msr, (1 << 9) | (1 << 16));
    sprint!("vmx: controls from capability MSRs ({}): pin {:#x} proc {:#x} exit {:#x} entry {:#x}\r\n",
            if use_true { "true-controls" } else { "legacy controls" },
            pin_ctls, proc_ctls, exit_ctls, entry_ctls);
    sprint!("vmx: intercept policy mirrors the SVM path: HLT exiting armed; CPUID exits unconditionally (no control bit exists); EPT untouched — secondary controls off, EPTP never written; EPT is the next groundwork step\r\n");

    // ---- the guest world: the host's own, mirrored (and translated) ----
    let efer_now = rdmsr(msr_vmx::EFER);
    // Guest EFER: host bits SCE|LME|NXE only — LMA is entry-derived,
    // and AMD-only bits (SVME & friends) would be reserved-bit entry
    // failures on Intel. The mirror is translated, not copied.
    let guest_efer = efer_now & 0x901;
    let guest_cr0 = orig_cr0;
    let guest_cr3 = orig_cr3;
    let guest_cr4 = orig_cr4 & !(1 << 13); // a guest does not hold VMXE
    let (cs, ss, ds, es, fs, gs, tr, ldtr) = seg_selectors();
    let (gdt_base, gdt_limit, idt_base, idt_limit) = dt_regs();
    let dr7: u64;
    unsafe { asm!("mov {}, dr7", out(reg) dr7, options(nostack)) };
    let code_pa = GUEST_CODE.as_ptr() as u64;
    let guest_rsp = core::ptr::addr_of!(VMX_GUEST_STACK) as u64 + 4096;

    let mut wfail = 0u32;
    let mut wcount = 0u32;
    let mut w = |field: u64, value: u64| {
        wcount += 1;
        if !vmwrite_checked(field, value) {
            wfail += 1;
        }
    };
    // Control fields.
    w(vf::PIN_CTLS, pin_ctls as u64);
    w(vf::PROC_CTLS, proc_ctls as u64);
    w(vf::EXC_BITMAP, 0);
    w(vf::PF_ERR_MASK, 0);
    w(vf::PF_ERR_MATCH, 0);
    w(vf::CR3_TGT_COUNT, 0);
    w(vf::EXIT_CTLS, exit_ctls as u64);
    w(vf::EXIT_MSR_STORE, 0);
    w(vf::EXIT_MSR_LOAD, 0);
    w(vf::ENTRY_CTLS, entry_ctls as u64);
    w(vf::ENTRY_MSR_LOAD, 0);
    w(vf::ENTRY_INT_INFO, 0);
    w(vf::CR0_MASK, 0); // the host owns no CR bits; the guest sees its own
    w(vf::CR4_MASK, 0);
    w(vf::CR0_SHADOW, guest_cr0);
    w(vf::CR4_SHADOW, guest_cr4);
    // Host state: the machine as it stands now (the exit lands here).
    w(vf::H_CR0, adj_cr0);
    w(vf::H_CR3, orig_cr3);
    w(vf::H_CR4, adj_cr4);
    w(vf::H_ES_SEL, es as u64);
    w(vf::H_CS_SEL, cs as u64);
    w(vf::H_SS_SEL, ss as u64);
    w(vf::H_DS_SEL, ds as u64);
    w(vf::H_FS_SEL, fs as u64);
    w(vf::H_GS_SEL, gs as u64);
    w(vf::H_TR_SEL, tr as u64);
    w(vf::H_FS_BASE, rdmsr(msr_vmx::FS_BASE));
    w(vf::H_GS_BASE, rdmsr(msr_vmx::GS_BASE));
    w(vf::H_TR_BASE, gdt_desc_base(gdt_base, tr));
    w(vf::H_GDTR_BASE, gdt_base);
    w(vf::H_IDTR_BASE, idt_base);
    w(vf::H_SYSENTER_CS, rdmsr(msr_vmx::SYSENTER_CS) & 0xFFFF_FFFF);
    w(vf::H_SYSENTER_ESP, rdmsr(msr_vmx::SYSENTER_ESP));
    w(vf::H_SYSENTER_EIP, rdmsr(msr_vmx::SYSENTER_EIP));
    w(vf::H_EFER, efer_now);
    // HOST_RSP / HOST_RIP are the bridge's to write (see section header).
    // Guest state: the host's world, mirrored.
    w(vf::G_LINK_PTR, u64::MAX);
    w(vf::G_PAT, rdmsr(msr_vmx::PAT));
    w(vf::G_EFER, guest_efer);
    w(vf::G_CR0, guest_cr0);
    w(vf::G_CR3, guest_cr3);
    w(vf::G_CR4, guest_cr4);
    w(vf::G_ES_SEL, es as u64);
    w(vf::G_CS_SEL, cs as u64);
    w(vf::G_SS_SEL, ss as u64);
    w(vf::G_DS_SEL, ds as u64);
    w(vf::G_FS_SEL, fs as u64);
    w(vf::G_GS_SEL, gs as u64);
    w(vf::G_LDTR_SEL, ldtr as u64);
    w(vf::G_TR_SEL, tr as u64);
    w(vf::G_ES_BASE, gdt_desc_base(gdt_base, es));
    w(vf::G_CS_BASE, gdt_desc_base(gdt_base, cs));
    w(vf::G_SS_BASE, gdt_desc_base(gdt_base, ss));
    w(vf::G_DS_BASE, gdt_desc_base(gdt_base, ds));
    w(vf::G_FS_BASE, rdmsr(msr_vmx::FS_BASE));
    w(vf::G_GS_BASE, rdmsr(msr_vmx::GS_BASE));
    w(vf::G_LDTR_BASE, gdt_desc_base(gdt_base, ldtr));
    w(vf::G_TR_BASE, gdt_desc_base(gdt_base, tr));
    w(vf::G_GDTR_BASE, gdt_base);
    w(vf::G_IDTR_BASE, idt_base);
    w(vf::G_ES_LIM, seg_limit(es) as u64);
    w(vf::G_CS_LIM, seg_limit(cs) as u64);
    w(vf::G_SS_LIM, seg_limit(ss) as u64);
    w(vf::G_DS_LIM, seg_limit(ds) as u64);
    w(vf::G_FS_LIM, seg_limit(fs) as u64);
    w(vf::G_GS_LIM, seg_limit(gs) as u64);
    w(vf::G_LDTR_LIM, seg_limit(ldtr) as u64);
    w(vf::G_TR_LIM, seg_limit(tr) as u64);
    w(vf::G_GDTR_LIM, gdt_limit as u64);
    w(vf::G_IDTR_LIM, idt_limit as u64);
    w(vf::G_ES_AR, seg_ar(es) as u64);
    w(vf::G_CS_AR, seg_ar(cs) as u64);
    w(vf::G_SS_AR, seg_ar(ss) as u64);
    w(vf::G_DS_AR, seg_ar(ds) as u64);
    w(vf::G_FS_AR, seg_ar(fs) as u64);
    w(vf::G_GS_AR, seg_ar(gs) as u64);
    w(vf::G_LDTR_AR, seg_ar(ldtr) as u64);
    w(vf::G_TR_AR, seg_ar(tr) as u64);
    w(vf::G_DR7, dr7);
    w(vf::G_RSP, guest_rsp);
    w(vf::G_RIP, code_pa);
    w(vf::G_RFLAGS, 0x2);
    w(vf::G_INTERRUPTIBILITY, 0);
    w(vf::G_ACTIVITY, 0);
    w(vf::G_PENDING_DBG, 0);
    w(vf::G_SYSENTER_CS, rdmsr(msr_vmx::SYSENTER_CS) & 0xFFFF_FFFF);
    w(vf::G_SYSENTER_ESP, rdmsr(msr_vmx::SYSENTER_ESP));
    w(vf::G_SYSENTER_EIP, rdmsr(msr_vmx::SYSENTER_EIP));
    if wfail > 0 {
        sprint!("vmx: {} VMCS field writes FAILED — aborting before any launch attempt\r\n", wfail);
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    sprint!("vmx: VMCS filled (host state = this machine now; guest state = its mirror; {} fields written, 0 write failures)\r\n",
            wcount);

    // ---- first entry ----
    const RAX_SENTINEL: u64 = 0x5EED_5EED_5EED_5EED;
    let mut regs = VmxRegs { rax: RAX_SENTINEL, ..Default::default() };
    sprint!("vmx: VMLAUNCH #1 — guest enters at {:#x} (the same 7-byte program as the SVM guest)\r\n", code_pa);
    unsafe { vmx_launch(&mut regs) };
    if regs.flag_cf != 0 {
        let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
        sprint!("vmx: VMLAUNCH FAILED — VMfailValid, VMCS instruction error {} (read back, not guessed); no guest ran\r\n", err);
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    if regs.flag_zf != 0 {
        sprint!("vmx: VMLAUNCH FAILED — VMfailInvalid (no current VMCS or VMCS misconfigured); no guest ran\r\n");
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    let reason = vmread_checked(vf::EXIT_REASON).unwrap_or(0) & 0xFFFF;
    let guest_rip = vmread_checked(vf::G_RIP).unwrap_or(0);
    sprint!("vmx: exit #1 reason {} (cpuid=10, hlt=12) guest rip {:#x} guest rax {:#x}\r\n",
            reason, guest_rip, regs.rax);
    if reason == 0 && guest_rip == code_pa && regs.rax == RAX_SENTINEL {
        // The launch "returned" with the VMCS's exit fields at their
        // initial values and the guest's first instruction (XOR
        // EAX,EAX) demonstrably unexecuted: RAX still holds our
        // sentinel. That is not a birth; it is an environment whose
        // entry path does not enter. Say so plainly, as with SVM.
        sprint!("vmx: guest birth NOT CLAIMED here — VMLAUNCH returned without the guest executing (exit reason 0, guest RIP and RAX untouched); entry in this environment did not occur; the machinery awaits Intel hardware or a KVM host\r\n");
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    if reason != VMX_EXIT_REASON_CPUID {
        sprint!("vmx: guest birth FAILED at first exit (reason {})\r\n", reason);
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }

    // Emulate CPUID leaf 0 the way an Intel part answers it:
    // max leaf in RAX, "GenuineIntel" across RBX/RDX/RCX.
    regs.rax = 0xD;
    regs.rbx = 0x756E_6547; // "Genu"
    regs.rdx = 0x4965_6E69; // "ineI"
    regs.rcx = 0x6C65_746E; // "ntel"
    if !vmwrite_checked(vf::G_RIP, guest_rip + 2) {
        sprint!("vmx: could not advance guest RIP past CPUID — birth incomplete\r\n");
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    sprint!("vmx: cpuid emulated (leaf 0 -> GenuineIntel), VMRESUME #2\r\n");
    regs.resume = 1;
    unsafe { vmx_launch(&mut regs) };
    if regs.flag_cf != 0 || regs.flag_zf != 0 {
        let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
        sprint!("vmx: VMRESUME FAILED (cf {} zf {}, instruction error {}) — birth incomplete\r\n",
                regs.flag_cf, regs.flag_zf, err);
        vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
        return;
    }
    let reason2 = vmread_checked(vf::EXIT_REASON).unwrap_or(0) & 0xFFFF;
    sprint!("vmx: exit #2 reason {}\r\n", reason2);
    if reason2 == VMX_EXIT_REASON_HLT {
        sprint!("vmx: GUEST BORN — ran, took our emulated cpuid (rax={:#x} rbx={:#x}), halted back to ZCore\r\n",
                regs.rax, regs.rbx);
    } else {
        sprint!("vmx: guest birth incomplete — second exit was not HLT\r\n");
    }
    vmx_teardown(vmcs_pa, true, orig_cr0, orig_cr4);
}

// ---------------------------------------------------------------- entry

#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start(manifest_addr: u64) -> ! {
    unsafe { Serial::init() };

    sprint!("\r\nSystemZ ZCore — M1 skeleton\r\n");

    let caps = probe();
    let vendor = core::str::from_utf8(&caps.vendor).unwrap_or("<non-utf8>");
    sprint!("cpu vendor: {}\r\n", vendor);
    sprint!("max basic cpuid leaf: {:#x}\r\n", caps.max_basic_leaf);
    sprint!("vmx (intel): {}\r\n", caps.vmx);
    sprint!("svm (amd):   {}\r\n", caps.svm);
    sprint!("already virtualized: {}\r\n", caps.hypervisor_present);

    if caps.vmx || caps.svm {
        sprint!("verdict: hardware virtualization present — ZCore can grow here\r\n");
    } else {
        sprint!("verdict: NO hardware virtualization — this machine cannot host SystemZ\r\n");
    }

    deep_probe(&caps);

    if manifest_addr != 0 {
        let info = unsafe { &*(manifest_addr as *const ZBootInfo) };
        if info.magic == ZBOOT_MAGIC {
            report_memory(info);
        } else {
            sprint!("zboot: manifest magic MISMATCH at {:#x} — refusing its contents\r\n",
                    manifest_addr);
        }
    } else {
        sprint!("zboot: no manifest (entered without the loader)\r\n");
    }

    if caps.svm {
        svm_first_guest(manifest_addr);
    }

    if caps.vmx {
        vmx_first_guest();
    } else {
        vmx_readiness_absent();
    }

    sprint!("zcore: halting (skeleton ends here, on purpose)\r\n");
    loop {
        unsafe { asm!("hlt", options(nostack)) };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    sprint!("\r\nZCORE PANIC: {}\r\n", info);
    loop {
        unsafe { asm!("hlt", options(nostack)) };
    }
}
