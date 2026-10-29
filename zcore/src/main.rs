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
    // OPERAND ORDER (convicted on bytes, 2026-10-29, run 2): the
    // assembler puts VMWRITE's FIRST textual operand in ModRM.reg —
    // the FIELD slot — and the second in r/m, the value slot. The
    // field register is therefore written FIRST below ("vmwrite
    // rcx, rax": rcx = field, rax = value). The previous order
    // executed every bridge write with field and value exchanged;
    // see the vmwrite_checked comment for the full evidence.
    "mov rax, rsp",
    "mov rcx, 0x6C14",      // HOST_RSP field encoding
    "vmwrite rcx, rax",
    "lea rax, [rip + vmx_exit_point]",
    "mov rcx, 0x6C16",      // HOST_RIP field encoding
    "vmwrite rcx, rax",
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
// (The canonical bisect's own GDT/TSS pages stood here until the
// ladder round. Run 6's bisect — a wholesale swap of the segment
// block, refused identically — exonerated the segments as a class,
// so the tables and their builders are retired; the bisect's
// evidence lives in proofs/vm-serial-2026-10-29-run6.log.)

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
    pub const H_TR_SEL: u64 = 0x0C0C;
    // 64-bit guest / host state
    pub const G_LINK_PTR: u64 = 0x2800;
    pub const G_DEBUGCTL: u64 = 0x2802;
    pub const G_PAT: u64 = 0x2804;
    pub const G_EFER: u64 = 0x2806;
    pub const G_PERF_GLOBAL: u64 = 0x2808;
    pub const H_PAT: u64 = 0x2C00;
    pub const H_EFER: u64 = 0x2C02;
    pub const H_PERF_GLOBAL: u64 = 0x2C04;
    // 32-bit control
    pub const PIN_CTLS: u64 = 0x4000;
    pub const PROC_CTLS: u64 = 0x4002;
    pub const PROC_CTLS2: u64 = 0x401E;
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
    pub const EXIT_INT_INFO: u64 = 0x4404;
    // natural-width read-only
    pub const EXIT_QUAL: u64 = 0x6400;
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

/// Human name for a VMCS field encoding — so a failed write names
/// its field on the serial line, not just a bare hex number the
/// reader has to decode against Appendix B from memory.
fn vf_name(field: u64) -> &'static str {
    use vf::*;
    match field {
        G_ES_SEL => "guest ES selector",
        G_CS_SEL => "guest CS selector",
        G_SS_SEL => "guest SS selector",
        G_DS_SEL => "guest DS selector",
        G_FS_SEL => "guest FS selector",
        G_GS_SEL => "guest GS selector",
        G_LDTR_SEL => "guest LDTR selector",
        G_TR_SEL => "guest TR selector",
        H_ES_SEL => "host ES selector",
        H_CS_SEL => "host CS selector",
        H_SS_SEL => "host SS selector",
        H_DS_SEL => "host DS selector",
        H_FS_SEL => "host FS selector",
        H_GS_SEL => "host GS selector",
        H_TR_SEL => "host TR selector",
        G_LINK_PTR => "guest VMCS link pointer",
        G_DEBUGCTL => "guest IA32_DEBUGCTL",
        G_PAT => "guest IA32_PAT",
        G_EFER => "guest IA32_EFER",
        G_PERF_GLOBAL => "guest IA32_PERF_GLOBAL_CTRL",
        H_PAT => "host IA32_PAT",
        H_EFER => "host IA32_EFER",
        H_PERF_GLOBAL => "host IA32_PERF_GLOBAL_CTRL",
        PIN_CTLS => "pin-based controls",
        PROC_CTLS => "primary processor-based controls",
        PROC_CTLS2 => "secondary processor-based controls",
        EXC_BITMAP => "exception bitmap",
        PF_ERR_MASK => "page-fault error-code mask",
        PF_ERR_MATCH => "page-fault error-code match",
        CR3_TGT_COUNT => "CR3-target count",
        EXIT_CTLS => "exit controls",
        EXIT_MSR_STORE => "exit MSR-store count",
        EXIT_MSR_LOAD => "exit MSR-load count",
        ENTRY_CTLS => "entry controls",
        ENTRY_MSR_LOAD => "entry MSR-load count",
        ENTRY_INT_INFO => "entry interruption information",
        INSTR_ERROR => "VM-instruction error (read-only)",
        EXIT_REASON => "exit reason (read-only)",
        EXIT_INT_INFO => "exit interruption information (read-only)",
        EXIT_QUAL => "exit qualification (read-only)",
        G_ES_LIM => "guest ES limit",
        G_CS_LIM => "guest CS limit",
        G_SS_LIM => "guest SS limit",
        G_DS_LIM => "guest DS limit",
        G_FS_LIM => "guest FS limit",
        G_GS_LIM => "guest GS limit",
        G_LDTR_LIM => "guest LDTR limit",
        G_TR_LIM => "guest TR limit",
        G_GDTR_LIM => "guest GDTR limit",
        G_IDTR_LIM => "guest IDTR limit",
        G_ES_AR => "guest ES access rights",
        G_CS_AR => "guest CS access rights",
        G_SS_AR => "guest SS access rights",
        G_DS_AR => "guest DS access rights",
        G_FS_AR => "guest FS access rights",
        G_GS_AR => "guest GS access rights",
        G_LDTR_AR => "guest LDTR access rights",
        G_TR_AR => "guest TR access rights",
        G_INTERRUPTIBILITY => "guest interruptibility state",
        G_ACTIVITY => "guest activity state",
        G_SYSENTER_CS => "guest IA32_SYSENTER_CS",
        H_SYSENTER_CS => "host IA32_SYSENTER_CS",
        CR0_MASK => "CR0 guest/host mask",
        CR4_MASK => "CR4 guest/host mask",
        CR0_SHADOW => "CR0 read shadow",
        CR4_SHADOW => "CR4 read shadow",
        G_CR0 => "guest CR0",
        G_CR3 => "guest CR3",
        G_CR4 => "guest CR4",
        G_ES_BASE => "guest ES base",
        G_CS_BASE => "guest CS base",
        G_SS_BASE => "guest SS base",
        G_DS_BASE => "guest DS base",
        G_FS_BASE => "guest FS base",
        G_GS_BASE => "guest GS base",
        G_LDTR_BASE => "guest LDTR base",
        G_TR_BASE => "guest TR base",
        G_GDTR_BASE => "guest GDTR base",
        G_IDTR_BASE => "guest IDTR base",
        G_DR7 => "guest DR7",
        G_RSP => "guest RSP",
        G_RIP => "guest RIP",
        G_RFLAGS => "guest RFLAGS",
        G_PENDING_DBG => "guest pending debug exceptions",
        G_SYSENTER_ESP => "guest IA32_SYSENTER_ESP",
        G_SYSENTER_EIP => "guest IA32_SYSENTER_EIP",
        H_CR0 => "host CR0",
        H_CR3 => "host CR3",
        H_CR4 => "host CR4",
        H_FS_BASE => "host FS base",
        H_GS_BASE => "host GS base",
        H_TR_BASE => "host TR base",
        H_GDTR_BASE => "host GDTR base",
        H_IDTR_BASE => "host IDTR base",
        H_SYSENTER_ESP => "host IA32_SYSENTER_ESP",
        H_SYSENTER_EIP => "host IA32_SYSENTER_EIP",
        0x6C14 => "host RSP (bridge-written)",
        0x6C16 => "host RIP (bridge-written)",
        _ => "unknown field",
    }
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

/// VMCS-format access rights for `sel`, assembled from LAR's raw
/// result; an unusable segment (null selector, or LAR fails) is
/// the bare unusable bit.
///
/// The two layouts are NOT the same shape, and confusing them is
/// what run 4 convicted (first VMX exit: reason 33, VM-entry
/// failure due to invalid guest state, guest RIP unmoved):
///   LAR raw:  bits 15:8 = access byte, 19:16 = limit 19:16,
///             23:20 = flags nibble (AVL 20, L 21, D/B 22, G 23).
///   VMCS AR:  bits 7:0 = access byte, 11:8 RESERVED (SDM Vol. 3C
///             §26.3.1.2: for CS and for every usable segment these
///             bits must all be 0 — the VMCS format carries no
///             limit nibble; the limit lives in its own field),
///             12 = AVL, 13 = L, 14 = D/B, 15 = G, 16 = unusable.
/// So the access byte shifts down 8 and the flags nibble shifts
/// down 8 into bits 15:12; LAR's limit nibble is dropped. The old
/// assembly put the flags nibble at bits 11:8 — which is the SVM
/// VMCB attribute layout, not this one — so every usable segment
/// carried nonzero reserved bits, the CS (host value 0xA9B) had
/// G = 0 against a 0xFFFF_FFFF limit (§26.3.1.2 requires G = 1
/// there) and L = 0 for a guest entered as IA-32e mode.
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
        ((raw >> 8) & 0xFF) | ((raw >> 8) & 0xF000)
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

/// VMWRITE with the hardware's verdict read off the flags: success
/// is CF=0 and ZF=0; CF=1 is VMfailInvalid, ZF=1 is VMfailValid.
///
/// OPERAND ORDER — CONVICTED AND FIXED (2026-10-29, second KVM run;
/// this comment replaces a first-round audit that misread its own
/// disassembly and exonerated this wrapper — that audit was wrong).
/// The hardware contract: ModRM.reg carries the FIELD encoding,
/// ModRM.r/m carries the VALUE. The assembler, however, puts
/// VMWRITE's FIRST textual operand in ModRM.reg and the second in
/// r/m. The old template was written in Intel operand order —
/// "vmwrite {v}, {f}", value first — so at every emitted site the
/// register holding the VALUE landed in the field slot and the
/// register holding the FIELD landed in the value slot: every write
/// executed with field and value exchanged. Proven from the emitted
/// bytes, with register provenance, at all four VMWRITE sites of
/// the pre-fix binary (write closure `0F 79 D6`, RIP-advance
/// `0F 79 C1`, bridge `0F 79 C1` ×2). The execution evidence agrees
/// exactly: of the 87 writes in a run, the 57 with nonzero values
/// "failed" (the value, sitting in the field slot, is not a valid
/// field encoding) and the 30 with value 0 "succeeded" (field
/// 0x0000 — VPID — is valid, and absorbed the field encoding as its
/// value). The template therefore names the FIELD first. Any future
/// edit here must re-verify the emitted bytes, not the template's
/// look: in the fixed binary, the register provably loaded with the
/// field must be the one in ModRM.reg at every site.
fn vmwrite_checked(field: u64, value: u64) -> bool {
    let cf: u8;
    let zf: u8;
    unsafe {
        asm!("vmwrite {f}, {v}", "setc {c}", "setz {z}",
             v = in(reg) value, f = in(reg) field,
             c = out(reg_byte) cf, z = out(reg_byte) zf, options(nostack));
    }
    cf == 0 && zf == 0
}

/// VMREAD, the mirror conviction: the hardware contract is the
/// same (ModRM.reg = FIELD, ModRM.r/m = destination), but the
/// assembler puts VMREAD's first textual operand in r/m and the
/// second in ModRM.reg — the opposite textual convention from
/// VMWRITE. The old template "vmread {f}, {v}" therefore crossed
/// the operands the same way: the destination register landed in
/// the field slot (its stale contents became the "field"), which is
/// why run 2's read-backs were inconsistent — a read executed
/// against whatever its destination register happened to hold. The
/// template names the DESTINATION first. Same law as
/// vmwrite_checked: trust only the emitted bytes (in the fixed
/// binary, the register loaded with the field constant must sit in
/// ModRM.reg at every site).
fn vmread_checked(field: u64) -> Option<u64> {
    let value: u64;
    let cf: u8;
    let zf: u8;
    unsafe {
        asm!("vmread {v}, {f}", "setc {c}", "setz {z}",
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
    // The NPT block belongs to the AMD path (it is built inside
    // svm_first_guest). Say so on this path, explicitly, so the
    // absence of an "npt: self-check" line in an Intel log can never
    // be misread as a missing or failed step — the verdict script
    // keys on this line.
    sprint!("npt: not applicable on Intel — EPT is this path's nested paging (next groundwork); the NPT tables belong to the AMD path\r\n");

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
    if !vmx_prepare_vmcs(vmcs_pa, revision) {
        vmx_teardown(vmcs_pa, false, orig_cr0, orig_cr4);
        return;
    }

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
    // Entry: IA-32e-mode guest (bit 9). The second desired bit,
    // bit 16, is "load IA32_BNDCFGS" — NOT "load IA32_EFER"
    // (bit 15), whatever this comment used to say. In run 4 the
    // capability mask cleared bit 16 (final value 0x13fb), so
    // neither MSR was loaded and the guest EFER field went
    // unchecked (SDM §26.3.1.1 gates its EFER checks on the load
    // control). The desired expression stays exactly as the runs
    // have flown it — do not "repair" it to bit 15 against the
    // current EFER field (which carries LMA = 0): that pairing
    // would fail entry on the EFER checks instead.
    let entry_ctls = adjust_controls(entry_msr, (1 << 9) | (1 << 16));
    // The load-bit tables the ladder patches and the dump decodes
    // against, verified for the ladder round against the SDM text
    // itself (Vol. 3C, VM-entry controls §24.8.1 and VM-exit
    // controls §24.7.1, two editions) and cross-checked against the
    // Linux kernel's arch/x86/include/asm/vmx.h constants — both
    // sources agree bit for bit:
    //   entry: 2 load DEBUGCTL, 9 IA-32e-mode guest,
    //          13 load IA32_PERF_GLOBAL_CTRL, 14 load IA32_PAT,
    //          15 load IA32_EFER, 16 load IA32_BNDCFGS.
    //   exit:  9 host address-space size (64-bit host),
    //          12 load IA32_PERF_GLOBAL_CTRL, 15 ack-interrupt,
    //          18 save IA32_PAT, 19 load IA32_PAT,
    //          20 save IA32_EFER, 21 load IA32_EFER.
    // A draft of the ladder round's own brief carried the entry
    // table shifted by one (PERF at 12, PAT at 13, EFER at 14) and
    // proposed renumbering this file's annotations to match; the
    // SDM text and the kernel constants convict the draft — the
    // annotations already here (PERF at 13, EFER at 15, BNDCFGS
    // at 16, DEBUGCTL at 2) ARE the SDM numbering, and nothing was
    // renumbered. The desired-bit audit against these tables: pin
    // desired 0 (the flown 0x16 is entirely must-be-1 bits); proc
    // desired bit 7 = HLT exiting (§24.6.2); exit desired bits
    // 9/20/21 as the comment above states; entry desired bits 9
    // and 16 as documented — bit 16 (load IA32_BNDCFGS) being the
    // one fossil of an older misreading, inert because the
    // capability mask clears it on every machine observed, and
    // never load-bearing in any flown value (runs 1-6 all show it
    // clear in the final controls). The ladder does not touch the
    // desired expression; its R3 sets the TRUE load-EFER bit (15)
    // as a rung patch, paired with R1's host-true EFER field —
    // the pairing the bit-16 comment above already demands.
    sprint!("vmx: controls from capability MSRs ({}): pin {:#x} proc {:#x} exit {:#x} entry {:#x}\r\n",
            if use_true { "true-controls" } else { "legacy controls" },
            pin_ctls, proc_ctls, exit_ctls, entry_ctls);
    sprint!("vmx: intercept policy mirrors the SVM path: HLT exiting armed; CPUID exits unconditionally (no control bit exists); EPT untouched — secondary controls off, EPTP never written; EPT is the next groundwork step\r\n");

    // ---- the guest world snapshot: everything the attempts share ----
    // One machine, read once. The controls, the adjusted CRs, the
    // host-state pieces and the shared guest block (CRs, EFER, PAT,
    // SYSENTER, link pointer, RIP/RSP/RFLAGS) are IDENTICAL for the
    // mirror attempt and for every ladder rung — that identity is
    // what makes a rung a single-variable experiment rather than a
    // second guess. A rung differs from the refused mirror ONLY in
    // its patch (see vmx_attempt); the segment block is the
    // mirror's throughout (run 6's bisect exonerated it).
    let efer_now = rdmsr(msr_vmx::EFER);
    let (cs, ss, ds, es, fs, gs, tr, ldtr) = seg_selectors();
    let (gdt_base, gdt_limit, idt_base, idt_limit) = dt_regs();
    let dr7: u64;
    unsafe { asm!("mov {}, dr7", out(reg) dr7, options(nostack)) };
    let world = VmxWorld {
        pin_ctls, proc_ctls, exit_ctls, entry_ctls, entry_msr,
        adj_cr0, adj_cr4, orig_cr0, orig_cr3, orig_cr4,
        efer_now,
        // Guest EFER: host bits SCE|LME|NXE only — LMA is
        // entry-derived, and AMD-only bits (SVME & friends) would
        // be reserved-bit entry failures on Intel. The mirror is
        // translated, not copied.
        guest_efer: efer_now & 0x901,
        guest_cr0: orig_cr0,
        guest_cr3: orig_cr3,
        guest_cr4: orig_cr4 & !(1 << 13), // a guest does not hold VMXE
        cs, ss, ds, es, fs, gs, tr, ldtr,
        gdt_base, gdt_limit, idt_base, idt_limit,
        dr7,
        code_pa: GUEST_CODE.as_ptr() as u64,
        guest_rsp: core::ptr::addr_of!(VMX_GUEST_STACK) as u64 + 4096,
    };
    let mirror = mirror_segs(&world);

    // ---- rung 0: the mirror (the production path, unchanged) ----
    // The control rung: the exact attempt the previous rounds
    // flew. If it is refused again — as run 6 says it will be —
    // the ladder below climbs; if it enters, nothing else runs.
    let mut stored_r0 = Stored::new();
    match vmx_attempt(&world, &mirror, 0, vmcs_pa, &mut stored_r0) {
        Attempt::Refused33 => {}
        _ => return,
    }

    // ---- rung X1: the reference state, flown whole ----
    // Eight runs have exonerated the L1-visible VMCS variable
    // by variable: the guest block (run 6's bisect), the shared
    // scalars (ladder-1), the host state and the control words
    // (ladder-2) — every stored value read back and judged, none
    // convicted. The question is no longer "which of OUR values
    // is wrong" but "what does a WORKING recipe do differently"
    // — and a working recipe exists on this exact stack:
    // kvm-unit-tests' x86/vmx.flat suite launches nested (L2)
    // guests successfully under this QEMU/KVM on this machine,
    // including its MSR-load and activity-state tests. Its VMCS
    // setup (x86/vmx.c: init_vmcs_ctrl, init_vmcs_host,
    // init_vmcs_guest) was extracted and diffed against run 8's
    // read-back dumps; the delta is exactly seven axes —
    // everything else (guest segment rights and limits, RFLAGS,
    // activity state, the link pointer, the CRs) is identical.
    // X1 flies the reference recipe whole, as post-fill patches
    // on the mirror: if it enters, the conviction lives inside
    // the seven deltas and the follow-on bisects them one per
    // run; if it is refused too, no field value separates this
    // state from one that enters here, and the remaining delta
    // is structural (the entry mechanics). The ladders below
    // still climb afterwards, for the record.
    sprint!("vmx: x1: the mirror is refused — before the ladders climb again, one attempt flies the REFERENCE state whole: the VMCS recipe of kvm-unit-tests' vmx suite, which launches nested guests on this stack, applied to the mirror in a single patch list (seven deltas; the patch lines name each field)\r\n");
    let x1_pin = adjust_controls(pin_msr, (1 << 0) | (1 << 3) | (1 << 5));
    let x1_exit = adjust_controls(exit_msr,
                                  (1 << 9) | (1 << 19) | (1 << 20) | (1 << 21));
    let x1_entry = adjust_controls(entry_msr, (1 << 9) | (1 << 15));
    let pat_now = rdmsr(msr_vmx::PAT);
    let gs_base_now = rdmsr(msr_vmx::GS_BASE);
    sprint!("vmx: x1: reference sourcing, read this boot: IA32_PAT MSR {:#x} (the host IA32_PAT field takes it — the fill never writes that field — under an exit word that gains load-IA32_PAT); IA32_GS_BASE MSR {:#x} (host and guest GS bases take it — the fill already sources both from this MSR, so a boot where it reads 0 closes that delta by construction); IA32_SYSENTER_CS, guest and host, takes the live CS selector {:#x} — the selector constant the reference writes, NOT the IA32_SYSENTER_CS MSR, which reads 0 on this vCPU and is why ladder R2's MSR re-source could not move the field\r\n",
            pat_now, gs_base_now, world.cs);
    sprint!("vmx: x1: reference control requests, capability-adjusted through the same MSRs as the flown words: pin {:#x} -> {:#x} (external-interrupt, NMI and virtual-NMI exiting requested atop the must-be-1 set), exit {:#x} -> {:#x} (load IA32_PAT requested alongside the flown 64-bit-host and EFER save/load bits), entry {:#x} -> {:#x} (load IA32_EFER requested with IA-32e-mode guest)\r\n",
            world.pin_ctls, x1_pin, world.exit_ctls, x1_exit,
            world.entry_ctls, x1_entry);
    let x1_patches: [(u64, u64); 11] = [
        (vf::G_SYSENTER_CS, world.cs as u64), // delta 1 (guest)
        (vf::H_SYSENTER_CS, world.cs as u64), // delta 1 (host)
        (vf::G_DR7, 0),                       // delta 2
        (vf::G_LDTR_LIM, 0xFFFF),             // delta 3
        (vf::G_LDTR_AR, 0x82),                // delta 3
        (vf::ENTRY_CTLS, x1_entry as u64),    // delta 4
        (vf::EXIT_CTLS, x1_exit as u64),      // delta 5
        (vf::H_PAT, pat_now),                 // delta 5
        (vf::PIN_CTLS, x1_pin as u64),        // delta 6
        (vf::H_GS_BASE, gs_base_now),         // delta 7
        (vf::G_GS_BASE, gs_base_now),         // delta 7
    ];
    if !vmx_reenter(adj_cr0, adj_cr4, vmxon_pa, revision, vmcs_pa,
                    orig_cr0, orig_cr4) {
        return;
    }
    let mut stored_x1 = Stored::new();
    match vmx_attempt_x1(&world, &mirror, &x1_patches, vmcs_pa,
                         &mut stored_x1) {
        Attempt::Refused33 => {
            sprint!("vmx: x1: the reference state was refused at entry too (reason 33) — the L1-visible state now matches, on every axis the extraction found, a recipe that enters on this stack; the remaining delta is structural (the launch bridge, the VMCS-region handling, the entry mechanics), and that is the next front. The ladders below still climb for the record\r\n");
        }
        _ => return,
    }

    // ---- the ladder: the shared block, one variable at a time ----
    // Run 6's canonical bisect varied the entire segment block at
    // once and was refused identically, which exonerated the
    // segments as a class and confined the refusal to the block
    // every attempt shares. The ladder walks THAT block in the
    // same boot: each rung relaunches the mirror with exactly one
    // variable changed (the patches live in vmx_attempt), stopping
    // at the first rung whose first exit is reason 10 (CPUID) —
    // the entering rung convicts the variable it changed.
    sprint!("vmx: ladder: the mirror guest state was refused at entry (reason 33) — climbing the shared block one variable at a time in the same boot\r\n");
    let mut launched = 0u32;
    let mut refused = 0u32;
    for rung in 1..=4u8 {
        if rung == 4 {
            // R4 probes entry-controls bit 12: a bit with NO row
            // in the SDM §24.8.1 table, present in the flown value
            // 0x13fb only because adjust_controls ORs the
            // capability MSR's must-be-1 dword into the result. If
            // it is must-be-1 on this machine, clearing it would
            // invalidate the control field itself — the launch
            // would fail as a bad control field, not be refused as
            // a bad guest state — and there is no legal rung to
            // fly. (By the adjust formula and run 6's 0x13fb, it
            // IS must-be-1 on the 800 G1's KVM; the check is made
            // here, from the live MSR, not from that deduction.)
            let caps = rdmsr(entry_msr);
            if (caps as u32) & (1 << 12) != 0 {
                sprint!("vmx: ladder R4: NOT APPLICABLE — entry-controls bit 12 is must-be-1 per the capability MSR {:#x} (must-be-1 dword {:#x}); its presence in the flown controls is architectural, not a choice, and clearing it is not a legal experiment on this machine\r\n",
                        caps, caps as u32);
                break;
            }
            sprint!("vmx: ladder R4: entry-controls bit 12 is not must-be-1 on this machine — the rung may clear it\r\n");
        }
        match rung {
            1 => sprint!("vmx: ladder R1: + guest IA32_EFER := the host's live IA32_EFER MSR (the constructed field's clear LMA bit is the suspect)\r\n"),
            2 => sprint!("vmx: ladder R2: + guest SYSENTER CS/ESP/EIP := the host's live IA32_SYSENTER_* MSRs, read and printed at the patch (R1 retained)\r\n"),
            3 => sprint!("vmx: ladder R3: + entry controls: load IA32_EFER (SDM bit 15) set explicitly, so the entry itself loads the host-true EFER (R1+R2 retained)\r\n"),
            _ => sprint!("vmx: ladder R4: + entry controls: bit 12 cleared — the unnamed capability-forced bit (R1+R2+R3 retained)\r\n"),
        }
        if !vmx_reenter(adj_cr0, adj_cr4, vmxon_pa, revision, vmcs_pa,
                        orig_cr0, orig_cr4) {
            return;
        }
        let mut stored_rung = Stored::new();
        launched += 1;
        match vmx_attempt(&world, &mirror, rung, vmcs_pa, &mut stored_rung) {
            Attempt::Refused33 => { refused += 1; }
            _ => return,
        }
    }
    sprint!("vmx: ladder: no rung entered — {} rung(s) launched, {} refused at entry (reason 33); a rung marked NOT APPLICABLE above did not launch. The shared block stands exonerated variable by variable, and the refusal now points beneath ZCore, at how the entry is constructed for a nested guest (KVM's vmcs02 synthesis). The per-rung read-back dumps above are the evidence\r\n",
            launched, refused);

    // ---- ladder-2: the host state and the control words ----
    // Two blocks have been byte-identical across all seven runs:
    // the HOST state (written every time, read back for the
    // first time in this build's dumps) and the four CONTROL
    // words (computed once from the capability MSRs, never
    // varied). Ladder-1 walked the guest block; ladder-2 varies
    // exactly these two, in the same one-boot pattern — but NOT
    // cumulatively: every rung departs from the refused mirror
    // ALONE, so a rung's refusal or entry convicts its own
    // departure and nothing else's. H5 then flies the applicable
    // departures combined.
    //
    // Each rung's departure is computed here, from the live
    // capability MSRs and the live selectors, as the list of
    // (field, value) pairs in which it differs from the flown
    // state. A rung whose list is empty has no variable to
    // change on this machine — the flown state already sits at
    // the floor it would test — and is marked NOT APPLICABLE
    // rather than flown under a second name; a rung whose list
    // repeats an already-launched rung's is marked DUPLICATE.
    // On the 800 G1's KVM, two degeneracies are expected by
    // construction and the code below simply reports which: the
    // flown processor-based word already IS must-be-1 |
    // HLT-exiting (that is all the fill ever desired there), and
    // the entry floor differs from the flown entry word only if
    // the fossil desired bit 16 survives the capability mask
    // somewhere, which seven runs say it does not.
    sprint!("vmx: ladder2: the guest block stands exhausted and the dumps now read the host state back too — what has never varied in seven runs is the HOST state and the CONTROL words. Ladder-2 varies exactly those: single departures from the mirror, nothing cumulative, stopping at the first rung that enters\r\n");
    fn same_patches(a: &[(u64, u64)], b: &[(u64, u64)]) -> bool {
        a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
    }
    let proc_floor = adjust_controls(proc_msr, 1 << 7);
    let exit_floor = adjust_controls(exit_msr, 1 << 9);
    let entry_floor = adjust_controls(entry_msr, 1 << 9);
    // H1: the host data selectors, nulled where they are not
    // already null.
    let mut dep1: [(u64, u64); 8] = [(0, 0); 8];
    let mut n1 = 0usize;
    for (field, cur) in [(vf::H_ES_SEL, world.es), (vf::H_SS_SEL, world.ss),
                         (vf::H_DS_SEL, world.ds), (vf::H_FS_SEL, world.fs),
                         (vf::H_GS_SEL, world.gs)] {
        if cur != 0 {
            dep1[n1] = (field, 0);
            n1 += 1;
        }
    }
    // H2/H3/H4: one control word each, reduced to its floor.
    let mut dep2: [(u64, u64); 8] = [(0, 0); 8];
    let mut n2 = 0usize;
    if proc_floor != world.proc_ctls {
        dep2[0] = (vf::PROC_CTLS, proc_floor as u64);
        n2 = 1;
    }
    let mut dep3: [(u64, u64); 8] = [(0, 0); 8];
    let mut n3 = 0usize;
    if exit_floor != world.exit_ctls {
        dep3[0] = (vf::EXIT_CTLS, exit_floor as u64);
        n3 = 1;
    }
    let mut dep4: [(u64, u64); 8] = [(0, 0); 8];
    let mut n4 = 0usize;
    if entry_floor != world.entry_ctls {
        dep4[0] = (vf::ENTRY_CTLS, entry_floor as u64);
        n4 = 1;
    }
    // H5: the applicable departures combined, in H1..H4 order.
    let mut dep5: [(u64, u64); 8] = [(0, 0); 8];
    let mut n5 = 0usize;
    for src in [&dep1[..n1], &dep2[..n2], &dep3[..n3], &dep4[..n4]] {
        for &pair in src {
            dep5[n5] = pair;
            n5 += 1;
        }
    }
    let deps: [&[(u64, u64)]; 5] = [&dep1[..n1], &dep2[..n2], &dep3[..n3],
                                   &dep4[..n4], &dep5[..n5]];
    let mut flown: [&[(u64, u64)]; 5] = [&[]; 5];
    let mut flown_hr: [u8; 5] = [0; 5];
    let mut flown_n = 0usize;
    let mut launched2 = 0u32;
    let mut refused2 = 0u32;
    for hrung in 1..=5u8 {
        let dep = deps[(hrung - 1) as usize];
        if dep.is_empty() {
            match hrung {
                1 => sprint!("vmx: ladder2 H1: NOT APPLICABLE — the host's live data selectors are already null on this machine; there is no selector variable to change\r\n"),
                2 => sprint!("vmx: ladder2 H2: NOT APPLICABLE — the flown processor-based controls {:#x} already equal the capability floor {:#x} (must-be-1 | HLT-exiting) on this machine; the word has no variable left to change\r\n",
                             world.proc_ctls, proc_floor),
                3 => sprint!("vmx: ladder2 H3: NOT APPLICABLE — the flown exit controls {:#x} already equal the capability floor {:#x} (must-be-1 | 64-bit host) on this machine; the EFER save/load pair is must-be-1 here and cannot come off\r\n",
                             world.exit_ctls, exit_floor),
                4 => sprint!("vmx: ladder2 H4: NOT APPLICABLE — the flown entry controls {:#x} already equal the capability floor {:#x} (must-be-1 | IA-32e-mode guest) on this machine; the word has no variable left to change\r\n",
                             world.entry_ctls, entry_floor),
                _ => sprint!("vmx: ladder2 H5: NOT APPLICABLE — every component departure is individually inapplicable on this machine; the combined state IS the mirror state\r\n"),
            }
            continue;
        }
        let mut dup_of = 0u8;
        for (i, prev) in flown.iter().enumerate() {
            if !prev.is_empty() && same_patches(prev, dep) {
                dup_of = flown_hr[i];
                break;
            }
        }
        if dup_of != 0 {
            sprint!("vmx: ladder2 H{}: DUPLICATE of an earlier rung — this departure computes to the identical patch list as H{} (the other components are already at their floors), whose refusal stands on the record above; not flown twice\r\n",
                    hrung, dup_of);
            continue;
        }
        match hrung {
            1 => sprint!("vmx: ladder2 H1: host data selectors (ES/SS/DS/FS/GS) := 0 — null selectors are legal host state for everything but CS/TR\r\n"),
            2 => sprint!("vmx: ladder2 H2: processor-based controls := the capability floor {:#x} (must-be-1 | HLT-exiting only), from flown {:#x}\r\n",
                         proc_floor, world.proc_ctls),
            3 => sprint!("vmx: ladder2 H3: exit controls := the capability floor {:#x} (must-be-1 | 64-bit host only; the EFER save/load pair comes off), from flown {:#x}\r\n",
                         exit_floor, world.exit_ctls),
            4 => sprint!("vmx: ladder2 H4: entry controls := the capability floor {:#x} (must-be-1 | IA-32e-mode guest only), from flown {:#x}\r\n",
                         entry_floor, world.entry_ctls),
            _ => sprint!("vmx: ladder2 H5: the combined departure — every applicable single departure above, in one state\r\n"),
        }
        if !vmx_reenter(adj_cr0, adj_cr4, vmxon_pa, revision, vmcs_pa,
                        orig_cr0, orig_cr4) {
            return;
        }
        flown[flown_n] = dep;
        flown_hr[flown_n] = hrung;
        flown_n += 1;
        let mut stored_h = Stored::new();
        launched2 += 1;
        match vmx_attempt_h(&world, &mirror, hrung, dep, vmcs_pa,
                            &mut stored_h) {
            Attempt::Refused33 => { refused2 += 1; }
            _ => return,
        }
    }
    sprint!("vmx: ladder2: no rung entered — {} rung(s) launched, {} refused at entry (reason 33); rungs marked NOT APPLICABLE or DUPLICATE above did not launch. The host state and the control words now stand varied or exonerated beside everything else; what remains lies beneath the L1-visible VMCS — KVM's vmcs02 synthesis and the hardware entry checks upon it. The dumps above are the complete L1-side evidence\r\n",
            launched2, refused2);
}

/// Everything the mirror attempt and every ladder rung share,
/// read once from the live machine: the capability-computed
/// controls, the VMX-adjusted CRs the host state needs, and the
/// shared guest block. Keeping these in one struct is what
/// guarantees the ladder's premise — a rung differs from the
/// refused mirror ONLY in its patch (see vmx_attempt), never in
/// anything this struct carries.
struct VmxWorld {
    pin_ctls: u32,
    proc_ctls: u32,
    exit_ctls: u32,
    entry_ctls: u32,
    /// Which capability MSR the entry controls came from (the
    /// true-controls MSR when the BASIC MSR offers it) — the
    /// ladder's R3/R4 patches re-adjust through this same MSR.
    entry_msr: u32,
    adj_cr0: u64,
    adj_cr4: u64,
    orig_cr0: u64,
    orig_cr3: u64,
    orig_cr4: u64,
    efer_now: u64,
    guest_efer: u64,
    guest_cr0: u64,
    guest_cr3: u64,
    guest_cr4: u64,
    cs: u16,
    ss: u16,
    ds: u16,
    es: u16,
    fs: u16,
    gs: u16,
    tr: u16,
    ldtr: u16,
    gdt_base: u64,
    gdt_limit: u32,
    idt_base: u64,
    idt_limit: u32,
    dr7: u64,
    code_pa: u64,
    guest_rsp: u64,
}

/// The guest segment block — built once, from the host's live
/// state, and flown identically by the mirror and every ladder
/// rung (run 6's bisect varied this block wholesale and was
/// refused identically; the ladder never touches it). Arrays are
/// indexed in the order ES, CS, SS, DS, FS, GS, LDTR, TR
/// throughout.
struct GuestSegs {
    sel: [u16; 8],
    base: [u64; 8],
    lim: [u32; 8],
    ar: [u32; 8],
    gdtr_base: u64,
    gdtr_lim: u32,
    idtr_base: u64,
    idtr_lim: u32,
}

/// The mirror's segment block: every value derived from the host's
/// live state (selectors the CPU holds, LAR/LSL against its GDT,
/// bases parsed out of its descriptors) — the derivation the runs
/// have always flown.
fn mirror_segs(w: &VmxWorld) -> GuestSegs {
    let sels = [w.es, w.cs, w.ss, w.ds, w.fs, w.gs, w.ldtr, w.tr];
    let mut base = [0u64; 8];
    let mut lim = [0u32; 8];
    let mut ar = [0u32; 8];
    for i in 0..8 {
        base[i] = gdt_desc_base(w.gdt_base, sels[i]);
        lim[i] = seg_limit(sels[i]);
        ar[i] = seg_ar(sels[i]);
    }
    // FS/GS bases are MSR state on this machine, not GDT state —
    // the mirror takes them from the MSRs, as it always has.
    base[4] = rdmsr(msr_vmx::FS_BASE);
    base[5] = rdmsr(msr_vmx::GS_BASE);
    GuestSegs {
        sel: sels,
        base,
        lim,
        ar,
        gdtr_base: w.gdt_base,
        gdtr_lim: w.gdt_limit,
        idtr_base: w.idt_base,
        idtr_lim: w.idt_limit,
    }
}

/// (build_canonical_tables and canonical_segs stood here until
/// the ladder round: the bisect they served concluded in run 6
/// with the segment block exonerated, and the ladder that
/// replaced it never builds a second segment block. Their
/// evidence is run 6's log, filed in proofs/.)

/// Zero the VMCS region, stamp the revision ID, VMCLEAR, VMPTRLD.
/// Used for the mirror attempt and again, identically, before
/// every ladder rung. Failure lines print here; the caller owns
/// the teardown decision.
fn vmx_prepare_vmcs(vmcs_pa: u64, revision: u64) -> bool {
    unsafe {
        core::ptr::write_bytes(vmcs_pa as *mut u8, 0, 4096);
        // VMCS revision ID; bit 31 clear = this is not a shadow VMCS.
        core::ptr::write_unaligned(vmcs_pa as *mut u32, revision as u32);
    }
    let (cf, zf) = unsafe { vmx_op_pa(1, vmcs_pa) };
    if cf || zf {
        sprint!("vmx: VMCLEAR FAILED (cf {} zf {}) — twin path stops\r\n", cf as u8, zf as u8);
        return false;
    }
    let (cf, zf) = unsafe { vmx_op_pa(2, vmcs_pa) };
    if cf || zf {
        sprint!("vmx: VMPTRLD FAILED (cf {} zf {}) — twin path stops\r\n", cf as u8, zf as u8);
        return false;
    }
    sprint!("vmx: VMCS region {:#x} (revision {}, shadow indicator clear) — VMCLEAR + VMPTRLD ok\r\n",
            vmcs_pa, revision);
    true
}

/// Re-enter VMX before a ladder rung. The previous attempt's
/// teardown already VMXOFFed and restored the host CRs, so this
/// re-adjusts them, rewrites the VMXON region, VMXONs again and
/// prepares a fresh VMCS — the same sequence the first entry
/// flew, factored out so every rung starts from an identical
/// machine state. Prints its own lines; on failure the host CRs
/// are restored (or the teardown restores them) and false comes
/// back, and the ladder stops with the evidence already printed
/// standing as the round's record.
fn vmx_reenter(adj_cr0: u64, adj_cr4: u64, vmxon_pa: u64, revision: u64,
               vmcs_pa: u64, orig_cr0: u64, orig_cr4: u64) -> bool {
    write_cr(0, adj_cr0);
    write_cr(4, adj_cr4);
    unsafe {
        core::ptr::write_bytes(core::ptr::addr_of_mut!(VMXON_REGION) as *mut u8, 0, 4096);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!(VMXON_REGION) as *mut u32, revision as u32);
    }
    let (cf, zf) = unsafe { vmx_op_pa(0, vmxon_pa) };
    if cf || zf {
        sprint!("vmx: ladder: VMXON FAILED on re-entry (cf {} zf {}) — the ladder stops here\r\n",
                cf as u8, zf as u8);
        write_cr(0, orig_cr0);
        write_cr(4, orig_cr4);
        return false;
    }
    sprint!("vmx: ladder: VMX re-entered (cr0/cr4 re-adjusted to {:#x}/{:#x}, VMXON ok)\r\n",
            adj_cr0, adj_cr4);
    if !vmx_prepare_vmcs(vmcs_pa, revision) {
        vmx_teardown(vmcs_pa, false, orig_cr0, orig_cr4);
        return false;
    }
    true
}

/// The record of what a fill INTENDED to write, field by field —
/// the read-back dump judges the VMCS against this, not against a
/// second derivation of the same values.
struct WriteRec {
    fields: [u64; 96],
    values: [u64; 96],
    n: usize,
}

impl WriteRec {
    fn new() -> Self {
        WriteRec { fields: [0; 96], values: [0; 96], n: 0 }
    }
    fn get(&self, field: u64) -> Option<u64> {
        for i in 0..self.n {
            if self.fields[i] == field {
                return Some(self.values[i]);
            }
        }
        None
    }
    /// Overwrite a recorded intent in place. A ladder patch
    /// changes one field AFTER the fill, and the read-back dump
    /// must judge the stored value against the rung's intent, not
    /// the fill's — without this, every patched field would
    /// headline as a false MISMATCH and the dump's verdict would
    /// be unusable on exactly the fields under test.
    fn set(&mut self, field: u64, value: u64) {
        for i in 0..self.n {
            if self.fields[i] == field {
                self.values[i] = value;
                return;
            }
        }
        if self.n < 96 {
            self.fields[self.n] = field;
            self.values[self.n] = value;
            self.n += 1;
        }
    }
}

/// The VMCS writer for one attempt: counts every write, records
/// every intended (field, value), and names every FAILURE on the
/// serial line the moment it happens — field encoding, field
/// name, value attempted. The first failure also captures the
/// VM-instruction error number (VMREAD of field 0x4400) before
/// anything else can overwrite it: that number is the hardware's
/// own classification of the rejection (12 = VMREAD/VMWRITE
/// from/to an unsupported VMCS component, 13 = VMWRITE to a
/// read-only component, per the SDM's VM-instruction error table —
/// run 3's error 12 for the nonexistent field 0xC0E is the table
/// agreeing with itself). This instrumentation exists because the
/// first KVM run (2026-10-29) reported only the bare count — "57
/// VMCS field writes FAILED" — which could not distinguish a bad
/// field, a bad ordering, or a bad state. A count is not a
/// diagnosis; names and the error number are.
struct Fill {
    wfail: u32,
    wcount: u32,
    first_fail: Option<(u64, u64)>,
    rec: WriteRec,
}

impl Fill {
    fn new() -> Self {
        Fill { wfail: 0, wcount: 0, first_fail: None, rec: WriteRec::new() }
    }
    fn w(&mut self, field: u64, value: u64) {
        self.wcount += 1;
        if self.rec.n < 96 {
            self.rec.fields[self.rec.n] = field;
            self.rec.values[self.rec.n] = value;
            self.rec.n += 1;
        }
        if !vmwrite_checked(field, value) {
            self.wfail += 1;
            if self.first_fail.is_none() {
                let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
                self.first_fail = Some((field, err));
                sprint!("vmx: write FAILED: field {:#x} ({}) = {:#x} — VM-instruction error {} (first failure; class per the SDM error table)\r\n",
                        field, vf_name(field), value, err);
            } else {
                sprint!("vmx: write FAILED: field {:#x} ({}) = {:#x}\r\n",
                        field, vf_name(field), value);
            }
        }
    }
    /// A ladder patch: one deliberate post-fill change to a
    /// single field — the rung's variable. NOT routed through
    /// w(): the fill's write count and write sequence belong to
    /// the control rung and stay exactly 87, and the dump must
    /// judge the patched intent (hence rec.set). A failed patch
    /// names itself like any other rejected write and aborts the
    /// rung before any launch attempt.
    fn patch(&mut self, field: u64, value: u64) -> bool {
        if vmwrite_checked(field, value) {
            self.rec.set(field, value);
            true
        } else {
            let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
            sprint!("vmx: ladder patch FAILED: field {:#x} ({}) = {:#x} — VM-instruction error {}; the rung aborts before any launch attempt\r\n",
                    field, vf_name(field), value, err);
            false
        }
    }
}

/// Fill the current VMCS for one attempt: the shared controls and
/// host state from `w`, the guest state from `w` + `s`. The write
/// ORDER and the mid-fill control read-back are exactly what the
/// mirror attempt has always flown, and every ladder rung flies
/// this identical sequence — a rung's single variable is applied
/// as a patch AFTER the fill (see vmx_attempt), never by perturbing
/// the sequence the control rung is compared against.
fn vmx_fill(f: &mut Fill, w: &VmxWorld, s: &GuestSegs) {
    // Control fields.
    f.w(vf::PIN_CTLS, w.pin_ctls as u64);
    f.w(vf::PROC_CTLS, w.proc_ctls as u64);
    f.w(vf::EXC_BITMAP, 0);
    f.w(vf::PF_ERR_MASK, 0);
    f.w(vf::PF_ERR_MATCH, 0);
    f.w(vf::CR3_TGT_COUNT, 0);
    f.w(vf::EXIT_CTLS, w.exit_ctls as u64);
    f.w(vf::EXIT_MSR_STORE, 0);
    f.w(vf::EXIT_MSR_LOAD, 0);
    f.w(vf::ENTRY_CTLS, w.entry_ctls as u64);
    f.w(vf::ENTRY_MSR_LOAD, 0);
    f.w(vf::ENTRY_INT_INFO, 0);
    f.w(vf::CR0_MASK, 0); // the host owns no CR bits; the guest sees its own
    f.w(vf::CR4_MASK, 0);
    f.w(vf::CR0_SHADOW, w.guest_cr0);
    f.w(vf::CR4_SHADOW, w.guest_cr4);
    // Reporter check, before the remaining writes are trusted to
    // the same reporter: read the four control fields back and
    // compare against what was written. Three outcomes, three
    // different diseases, and they must never be conflated:
    //   * read-back equals written  -> the writes land; any failure
    //     later in the sequence is a true, field-specific rejection.
    //   * read-back differs         -> writes are not landing as
    //     reported; every success count downstream is suspect.
    //   * read-back itself fails    -> the current VMCS is not
    //     readable, which contradicts a successful VMPTRLD and
    //     re-opens the state question entirely.
    {
        let rb = [
            ("pin-based", vf::PIN_CTLS, w.pin_ctls as u64),
            ("primary processor-based", vf::PROC_CTLS, w.proc_ctls as u64),
            ("VM-exit", vf::EXIT_CTLS, w.exit_ctls as u64),
            ("VM-entry", vf::ENTRY_CTLS, w.entry_ctls as u64),
        ];
        let mut rb_bad = 0u32;
        for (name, field, written) in rb {
            match vmread_checked(field) {
                Some(read) if read == written => {}
                Some(read) => {
                    rb_bad += 1;
                    sprint!("vmx: control read-back MISMATCH: {} controls written {:#x} but read {:#x} — reported writes are not landing\r\n",
                            name, written, read);
                }
                None => {
                    rb_bad += 1;
                    sprint!("vmx: control read-back FAILED: {} controls ({:#x}) cannot be read back at all\r\n",
                            name, field);
                }
            }
        }
        if rb_bad == 0 {
            sprint!("vmx: control read-back: pin/proc/exit/entry all read back exactly as written\r\n");
        }
    }
    // Host state: the machine as it stands now (the exit lands here).
    f.w(vf::H_CR0, w.adj_cr0);
    f.w(vf::H_CR3, w.orig_cr3);
    f.w(vf::H_CR4, w.adj_cr4);
    f.w(vf::H_ES_SEL, w.es as u64);
    f.w(vf::H_CS_SEL, w.cs as u64);
    f.w(vf::H_SS_SEL, w.ss as u64);
    f.w(vf::H_DS_SEL, w.ds as u64);
    f.w(vf::H_FS_SEL, w.fs as u64);
    f.w(vf::H_GS_SEL, w.gs as u64);
    f.w(vf::H_TR_SEL, w.tr as u64);
    f.w(vf::H_FS_BASE, rdmsr(msr_vmx::FS_BASE));
    f.w(vf::H_GS_BASE, rdmsr(msr_vmx::GS_BASE));
    f.w(vf::H_TR_BASE, gdt_desc_base(w.gdt_base, w.tr));
    f.w(vf::H_GDTR_BASE, w.gdt_base);
    f.w(vf::H_IDTR_BASE, w.idt_base);
    f.w(vf::H_SYSENTER_CS, rdmsr(msr_vmx::SYSENTER_CS) & 0xFFFF_FFFF);
    f.w(vf::H_SYSENTER_ESP, rdmsr(msr_vmx::SYSENTER_ESP));
    f.w(vf::H_SYSENTER_EIP, rdmsr(msr_vmx::SYSENTER_EIP));
    f.w(vf::H_EFER, w.efer_now);
    // HOST_RSP / HOST_RIP are the bridge's to write (see section header).
    // Guest state: the shared block from `w`, the segment block
    // (and GDTR) from `s` — the attempts' only difference.
    f.w(vf::G_LINK_PTR, u64::MAX);
    f.w(vf::G_PAT, rdmsr(msr_vmx::PAT));
    f.w(vf::G_EFER, w.guest_efer);
    f.w(vf::G_CR0, w.guest_cr0);
    f.w(vf::G_CR3, w.guest_cr3);
    f.w(vf::G_CR4, w.guest_cr4);
    let sel_f = [vf::G_ES_SEL, vf::G_CS_SEL, vf::G_SS_SEL, vf::G_DS_SEL,
                 vf::G_FS_SEL, vf::G_GS_SEL, vf::G_LDTR_SEL, vf::G_TR_SEL];
    let base_f = [vf::G_ES_BASE, vf::G_CS_BASE, vf::G_SS_BASE, vf::G_DS_BASE,
                  vf::G_FS_BASE, vf::G_GS_BASE, vf::G_LDTR_BASE, vf::G_TR_BASE];
    let lim_f = [vf::G_ES_LIM, vf::G_CS_LIM, vf::G_SS_LIM, vf::G_DS_LIM,
                 vf::G_FS_LIM, vf::G_GS_LIM, vf::G_LDTR_LIM, vf::G_TR_LIM];
    let ar_f = [vf::G_ES_AR, vf::G_CS_AR, vf::G_SS_AR, vf::G_DS_AR,
                vf::G_FS_AR, vf::G_GS_AR, vf::G_LDTR_AR, vf::G_TR_AR];
    for i in 0..8 {
        f.w(sel_f[i], s.sel[i] as u64);
    }
    for i in 0..8 {
        f.w(base_f[i], s.base[i]);
    }
    f.w(vf::G_GDTR_BASE, s.gdtr_base);
    f.w(vf::G_IDTR_BASE, s.idtr_base);
    for i in 0..8 {
        f.w(lim_f[i], s.lim[i] as u64);
    }
    f.w(vf::G_GDTR_LIM, s.gdtr_lim as u64);
    f.w(vf::G_IDTR_LIM, s.idtr_lim as u64);
    for i in 0..8 {
        f.w(ar_f[i], s.ar[i] as u64);
    }
    f.w(vf::G_DR7, w.dr7);
    f.w(vf::G_RSP, w.guest_rsp);
    f.w(vf::G_RIP, w.code_pa);
    f.w(vf::G_RFLAGS, 0x2);
    f.w(vf::G_INTERRUPTIBILITY, 0);
    f.w(vf::G_ACTIVITY, 0);
    f.w(vf::G_PENDING_DBG, 0);
    f.w(vf::G_SYSENTER_CS, rdmsr(msr_vmx::SYSENTER_CS) & 0xFFFF_FFFF);
    f.w(vf::G_SYSENTER_ESP, rdmsr(msr_vmx::SYSENTER_ESP));
    f.w(vf::G_SYSENTER_EIP, rdmsr(msr_vmx::SYSENTER_EIP));
}

/// The values a dump actually READ BACK from one attempt's VMCS.
/// A field maps to Some(value) when its VMREAD succeeded, to
/// Some(None) when the read was attempted and failed; absence from
/// the set means the dump never read it.
struct Stored {
    fields: [u64; 112],
    values: [u64; 112],
    read_ok: [bool; 112],
    n: usize,
}

impl Stored {
    fn new() -> Self {
        Stored { fields: [0; 112], values: [0; 112], read_ok: [false; 112], n: 0 }
    }
    fn put(&mut self, field: u64, v: Option<u64>) {
        if self.n < 112 {
            self.fields[self.n] = field;
            self.values[self.n] = v.unwrap_or(0);
            self.read_ok[self.n] = v.is_some();
            self.n += 1;
        }
    }
    fn get(&self, field: u64) -> Option<Option<u64>> {
        for i in 0..self.n {
            if self.fields[i] == field {
                return Some(if self.read_ok[i] { Some(self.values[i]) } else { None });
            }
        }
        None
    }
}

/// One scalar line of the read-back dump: the value AS STORED,
/// judged against the intended write. Returns 1 when the field
/// counts as mismatched (stored != intended, or a written field
/// cannot be read back at all), else 0.
fn dump_scalar(rec: &WriteRec, stored: &mut Stored, field: u64) -> u32 {
    let intended = rec.get(field);
    let got = vmread_checked(field);
    stored.put(field, got);
    match (intended, got) {
        (Some(i), Some(v)) if v == i => {
            sprint!("vmx: dump: {} stored {:#x} — as written\r\n", vf_name(field), v);
            0
        }
        (Some(i), Some(v)) => {
            sprint!("vmx: dump: {} stored {:#x} — MISMATCH (intended {:#x})\r\n",
                    vf_name(field), v, i);
            1
        }
        (Some(i), None) => {
            sprint!("vmx: dump: {} — VMREAD FAILED (intended {:#x})\r\n",
                    vf_name(field), i);
            1
        }
        (None, Some(v)) => {
            sprint!("vmx: dump: {} stored {:#x} (never written)\r\n", vf_name(field), v);
            0
        }
        (None, None) => {
            sprint!("vmx: dump: {} — VMREAD FAILED (never written)\r\n", vf_name(field));
            0
        }
    }
}

/// A field the fill never writes, read for the record: the entry
/// controls load none of these, so their stored values are
/// whatever VMCLEAR left — evidence either way, never a mismatch.
fn dump_extra(stored: &mut Stored, field: u64, note: &str) -> u32 {
    let got = vmread_checked(field);
    stored.put(field, got);
    match got {
        Some(v) => sprint!("vmx: dump: {} stored {:#x} ({})\r\n", vf_name(field), v, note),
        None => sprint!("vmx: dump: {} — VMREAD FAILED ({})\r\n", vf_name(field), note),
    }
    0
}

/// One segment of the dump: selector, base, limit and the access
/// rights DECODED on the spot — type, S, DPL, P, AVL, L, D/B, G,
/// unusable as separate printed bits, so a reader never has to
/// re-derive the layout the last three convictions turned on.
/// Returns 1 when any component disagrees with the intended write
/// or a component cannot be read, else 0.
fn dump_seg(rec: &WriteRec, stored: &mut Stored, name: &str,
            sel_f: u64, base_f: u64, lim_f: u64, ar_f: u64) -> u32 {
    let sel = vmread_checked(sel_f);
    let base = vmread_checked(base_f);
    let lim = vmread_checked(lim_f);
    let ar = vmread_checked(ar_f);
    stored.put(sel_f, sel);
    stored.put(base_f, base);
    stored.put(lim_f, lim);
    stored.put(ar_f, ar);
    if let (Some(sel), Some(base), Some(lim), Some(ar)) = (sel, base, lim, ar) {
        let bad = rec.get(sel_f) != Some(sel) || rec.get(base_f) != Some(base)
            || rec.get(lim_f) != Some(lim) || rec.get(ar_f) != Some(ar);
        sprint!("vmx: dump: guest {} sel {:#x} base {:#x} limit {:#x} ar {:#x} [type {} s {} dpl {} p {} avl {} l {} d/b {} g {} unusable {}]{}\r\n",
                name, sel, base, lim, ar,
                ar & 0xF, (ar >> 4) & 1, (ar >> 5) & 3, (ar >> 7) & 1,
                (ar >> 12) & 1, (ar >> 13) & 1, (ar >> 14) & 1,
                (ar >> 15) & 1, (ar >> 16) & 1,
                if bad { " — MISMATCH vs intended (next line)" } else { " — as written" });
        if bad {
            sprint!("vmx: dump: guest {} intended sel {:#x} base {:#x} limit {:#x} ar {:#x}\r\n",
                    name,
                    rec.get(sel_f).unwrap_or(0), rec.get(base_f).unwrap_or(0),
                    rec.get(lim_f).unwrap_or(0), rec.get(ar_f).unwrap_or(0));
            1
        } else {
            0
        }
    } else {
        sprint!("vmx: dump: guest {} — read-back INCOMPLETE (at least one component VMREAD failed)\r\n",
                name);
        1
    }
}

/// One host selector of the dump: the value AS STORED, judged
/// against the intended write, with the selector DECODED on the
/// spot — index, TI, RPL as separate printed fields. The decode
/// exists because the host selector checks (SDM §26.3 and KVM's
/// nested_vmx_check_host_state alike) are stated in exactly
/// these terms: TI must be 0, RPL must be 0, and CS/TR (and SS,
/// off the 64-bit-host path) must not be null. A reader of the
/// log should never have to re-derive them from the hex.
fn dump_host_sel(rec: &WriteRec, stored: &mut Stored, name: &str,
                 sel_f: u64) -> u32 {
    let got = vmread_checked(sel_f);
    stored.put(sel_f, got);
    match (rec.get(sel_f), got) {
        (Some(i), Some(v)) => {
            let bad = v != i;
            sprint!("vmx: dump: host {} selector stored {:#x} [index {} ti {} rpl {}]{}\r\n",
                    name, v, v >> 3, (v >> 2) & 1, v & 3,
                    if bad { " — MISMATCH vs intended (next line)" } else { " — as written" });
            if bad {
                sprint!("vmx: dump: host {} selector intended {:#x}\r\n", name, i);
                1
            } else {
                0
            }
        }
        (Some(i), None) => {
            sprint!("vmx: dump: host {} selector — VMREAD FAILED (intended {:#x})\r\n",
                    name, i);
            1
        }
        (None, Some(v)) => {
            sprint!("vmx: dump: host {} selector stored {:#x} [index {} ti {} rpl {}] (never written)\r\n",
                    name, v, v >> 3, (v >> 2) & 1, v & 3);
            0
        }
        (None, None) => {
            sprint!("vmx: dump: host {} selector — VMREAD FAILED (never written)\r\n",
                    name);
            0
        }
    }
}

/// THE READ-BACK DUMP — this round's reason to exist. The static
/// §26.3.1 audit convicted three checks on paper and the machine
/// still refused the entry, so paper is exhausted: after the fill,
/// VMREAD every guest-state field (and the control block) back and
/// print the values AS STORED — the only values the entry checks
/// will ever judge. This is a diagnostic build by design: the dump
/// is bounded, one line per field (segments print as one decoded
/// line), and it exists so the next refusal has to name its field
/// instead of its class.
///
/// The host-state block joined the dump in the host-state round.
/// For seven runs the dump covered the controls and the guest
/// state only, while the VMCS's other half — the state the
/// processor loads on every VM exit, and the half KVM validates
/// in software for a nested guest (nested_vmx_check_host_state) —
/// was written and never once read back. It is read back now:
/// CR0/CR3/CR4, the SYSENTER trio, EFER, the MSR bases, the
/// descriptor-table and TR bases, and all seven selectors with
/// their index/TI/RPL decoded on the line. HOST_RSP and HOST_RIP
/// are read here too, with their timing stated honestly: the
/// launch bridge writes them inside vmx_launch, AFTER this dump,
/// so the values stored at dump time are VMCLEAR's leftovers —
/// the values a launch actually flew are read back in the entry
/// forensics, after a failed entry, while they still stand.
/// If any stored value differs from the intended write — above
/// all if guest CS access rights are not 0xA09B in the VMCS — the
/// dump headlines it: a second writer or a stale path, not a
/// guest-state problem at all.
fn vmx_dump(tag: &str, rec: &WriteRec, stored: &mut Stored) {
    sprint!("vmx: dump [{}]: state as stored in the VMCS (VMREAD after the fill, before any launch)\r\n",
            tag);
    let mut fields = 0u32;
    let mut mism = 0u32;
    // The control block: the four control fields, the matching
    // machinery around them, both CR read shadows and CR masks,
    // and the MSR-load/store counts — whose nonzero values would
    // move the refusal into the reason-34 class instead.
    for fld in [vf::PIN_CTLS, vf::PROC_CTLS, vf::EXIT_CTLS, vf::ENTRY_CTLS,
                vf::EXC_BITMAP, vf::PF_ERR_MASK, vf::PF_ERR_MATCH,
                vf::CR3_TGT_COUNT, vf::ENTRY_MSR_LOAD, vf::EXIT_MSR_STORE,
                vf::EXIT_MSR_LOAD, vf::ENTRY_INT_INFO,
                vf::CR0_MASK, vf::CR4_MASK, vf::CR0_SHADOW, vf::CR4_SHADOW] {
        mism += dump_scalar(rec, stored, fld);
        fields += 1;
    }
    mism += dump_extra(stored, vf::PROC_CTLS2,
                       "never written — secondary controls inactive (primary bit 31 clear)");
    fields += 1;
    // The load-bit decodes, printed under the verified SDM tables
    // (VM-entry controls 24.8.1, VM-exit controls 24.7.1 — the
    // verification record stands in the controls comment in
    // vmx_first_guest). Annotation lines only: they decode the two
    // stored control values already judged above and add no field
    // to the count — but they put on the record, in the open, the
    // bits the entry will actually act on. In particular: entry
    // bit 13 (load IA32_PERF_GLOBAL_CTRL) has been CLEAR in every
    // flown value, so PERF_GLOBAL_CTRL is not loaded at entry —
    // the fact the shifted table in the ladder brief got wrong —
    // and entry bit 15 (load IA32_EFER) is the bit ladder R3 sets.
    if let Some(Some(ev)) = stored.get(vf::ENTRY_CTLS) {
        sprint!("vmx: dump: entry controls {:#x} decoded (SDM VM-entry controls): load-debug[2]={} ia32e-guest[9]={} load-perf-global[13]={} load-pat[14]={} load-efer[15]={} load-bndcfgs[16]={}\r\n",
                ev, (ev >> 2) & 1, (ev >> 9) & 1, (ev >> 13) & 1,
                (ev >> 14) & 1, (ev >> 15) & 1, (ev >> 16) & 1);
    }
    if let Some(Some(xv)) = stored.get(vf::EXIT_CTLS) {
        sprint!("vmx: dump: exit controls {:#x} decoded (SDM VM-exit controls): host-64[9]={} load-perf-global[12]={} save-pat[18]={} load-pat[19]={} save-efer[20]={} load-efer[21]={}\r\n",
                xv, (xv >> 9) & 1, (xv >> 12) & 1, (xv >> 18) & 1,
                (xv >> 19) & 1, (xv >> 20) & 1, (xv >> 21) & 1);
    }
    // The guest scalars, including the non-register trio and the
    // descriptor-table registers.
    for fld in [vf::G_LINK_PTR, vf::G_PAT, vf::G_EFER,
                vf::G_CR0, vf::G_CR3, vf::G_CR4, vf::G_DR7,
                vf::G_RSP, vf::G_RIP, vf::G_RFLAGS,
                vf::G_INTERRUPTIBILITY, vf::G_ACTIVITY, vf::G_PENDING_DBG,
                vf::G_SYSENTER_CS, vf::G_SYSENTER_ESP, vf::G_SYSENTER_EIP,
                vf::G_GDTR_BASE, vf::G_GDTR_LIM,
                vf::G_IDTR_BASE, vf::G_IDTR_LIM] {
        mism += dump_scalar(rec, stored, fld);
        fields += 1;
    }
    mism += dump_extra(stored, vf::G_DEBUGCTL,
                       "never written — entry does not load it (entry-controls bit 2 clear)");
    mism += dump_extra(stored, vf::G_PERF_GLOBAL,
                       "never written — entry does not load it (entry-controls bit 13 clear)");
    fields += 2;
    // The segments, decoded.
    let names = ["ES", "CS", "SS", "DS", "FS", "GS", "LDTR", "TR"];
    let sel_f = [vf::G_ES_SEL, vf::G_CS_SEL, vf::G_SS_SEL, vf::G_DS_SEL,
                 vf::G_FS_SEL, vf::G_GS_SEL, vf::G_LDTR_SEL, vf::G_TR_SEL];
    let base_f = [vf::G_ES_BASE, vf::G_CS_BASE, vf::G_SS_BASE, vf::G_DS_BASE,
                  vf::G_FS_BASE, vf::G_GS_BASE, vf::G_LDTR_BASE, vf::G_TR_BASE];
    let lim_f = [vf::G_ES_LIM, vf::G_CS_LIM, vf::G_SS_LIM, vf::G_DS_LIM,
                 vf::G_FS_LIM, vf::G_GS_LIM, vf::G_LDTR_LIM, vf::G_TR_LIM];
    let ar_f = [vf::G_ES_AR, vf::G_CS_AR, vf::G_SS_AR, vf::G_DS_AR,
                vf::G_FS_AR, vf::G_GS_AR, vf::G_LDTR_AR, vf::G_TR_AR];
    for i in 0..8 {
        mism += dump_seg(rec, stored, names[i], sel_f[i], base_f[i], lim_f[i], ar_f[i]);
        fields += 1;
    }
    // The CS headline, judged against the canonical value itself
    // and not only against intent — a wrong intent faithfully
    // stored would otherwise pass in silence.
    match stored.get(vf::G_CS_AR) {
        Some(Some(v)) if v == 0xA09B => {}
        Some(Some(v)) => {
            sprint!("vmx: dump: HEADLINE — guest CS access rights as stored are {:#x}, not the canonical 0xa09b this round exists to test\r\n",
                    v);
        }
        Some(None) => {
            sprint!("vmx: dump: HEADLINE — guest CS access rights cannot be read back at all\r\n");
        }
        None => {
            sprint!("vmx: dump: HEADLINE — guest CS access rights are missing from the read-back set\r\n");
        }
    }
    // The host state — the VMCS's other half, dumped for the
    // first time in the host-state round. The fill writes every
    // field below except PAT, PERF_GLOBAL_CTRL, RSP and RIP;
    // those four are read for the record with their gates and
    // timing stated on their lines.
    for fld in [vf::H_CR0, vf::H_CR3, vf::H_CR4,
                vf::H_SYSENTER_CS, vf::H_SYSENTER_ESP, vf::H_SYSENTER_EIP,
                vf::H_EFER, vf::H_FS_BASE, vf::H_GS_BASE,
                vf::H_TR_BASE, vf::H_GDTR_BASE, vf::H_IDTR_BASE] {
        mism += dump_scalar(rec, stored, fld);
        fields += 1;
    }
    let hnames = ["ES", "CS", "SS", "DS", "FS", "GS", "TR"];
    let hsel_f = [vf::H_ES_SEL, vf::H_CS_SEL, vf::H_SS_SEL, vf::H_DS_SEL,
                  vf::H_FS_SEL, vf::H_GS_SEL, vf::H_TR_SEL];
    for i in 0..7 {
        mism += dump_host_sel(rec, stored, hnames[i], hsel_f[i]);
        fields += 1;
    }
    // HOST IA32_PAT is judged whenever the attempt actually
    // wrote it (the reference-state rung X1 writes it, under an
    // exit word carrying load-IA32_PAT); on every other attempt
    // it stays a for-the-record extra, as before.
    if rec.get(vf::H_PAT).is_some() {
        mism += dump_scalar(rec, stored, vf::H_PAT);
    } else {
        mism += dump_extra(stored, vf::H_PAT,
                           "never written — the exit controls' save/load IA32_PAT bits (exit 18/19) are both clear in the flown controls, so no entry check judges it; VMCLEAR's leftover, for the record");
    }
    mism += dump_extra(stored, vf::H_PERF_GLOBAL,
                       "never written — the exit controls' load IA32_PERF_GLOBAL_CTRL bit (exit 12) is clear in the flown controls, so no entry check judges it; VMCLEAR's leftover, for the record");
    // HOST_RSP (0x6C14) / HOST_RIP (0x6C16): encodings owned by
    // the launch bridge (see the vf module header) — read here
    // for the record, written by the bridge later.
    mism += dump_extra(stored, 0x6C14,
                       "bridge-written inside vmx_launch, after this dump — the value stored now is VMCLEAR's leftover; the value the launch flew is read back in the entry forensics");
    mism += dump_extra(stored, 0x6C16,
                       "bridge-written inside vmx_launch, after this dump — the value stored now is VMCLEAR's leftover; the value the launch flew is read back in the entry forensics");
    fields += 4;
    sprint!("vmx: dump [{}]: {} fields read back, {} mismatched against the intended writes\r\n",
            tag, fields, mism);
    if mism > 0 {
        sprint!("vmx: dump: HEADLINE — {} stored value(s) differ from the values written; the MISMATCH lines above are the evidence (a second writer or a stale path). The launch below still runs, but its verdict cannot be read until this is explained\r\n",
                mism);
    }
}

/// (vmx_bisect_diff stood here until the ladder round: the
/// stored-vs-stored diff it printed between the mirror and the
/// canonical state was run 6's decisive exhibit — 10 differing
/// fields, all segment/GDTR/TR, refused identically. The ladder
/// needs no diff: each rung changes one named variable against
/// the mirror, and its own dump is the comparison.)

/// An Option<u64> that prints as its hex value, or as the plain
/// fact of the failed read — for the exit-record fields, where the
/// read failing is itself a datum.
struct OptHex(Option<u64>);

impl core::fmt::Display for OptHex {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{:#x}", v),
            None => write!(f, "<VMREAD FAILED>"),
        }
    }
}

/// Exit forensics for a failed entry: the VM-exit qualification
/// and the VM-exit interruption information, read back while the
/// exit record still exists (BEFORE any teardown — VMCLEAR would
/// destroy it). KVM may carry entry-failure detail in these
/// fields; if they are zero, the log says zero — evidence either
/// way, never filled in by theory.
fn vmx_entry_forensics(tag: &str) {
    let q = vmread_checked(vf::EXIT_QUAL);
    let ii = vmread_checked(vf::EXIT_INT_INFO);
    sprint!("vmx: entry forensics ({}): exit qualification {}, exit interruption-information {} (read back after the failed entry)\r\n",
            tag, OptHex(q), OptHex(ii));
    // HOST_RSP (0x6C14) / HOST_RIP (0x6C16) as the launch bridge
    // left them. The pre-launch dump can only show VMCLEAR's
    // leftovers for these two — the bridge writes them inside
    // vmx_launch — but a failed entry does not consume them, so
    // here they are the values the launch actually flew, read
    // back while they still stand. KVM's host-state validation
    // judges the host RIP's canonicality (against host CR4.LA57);
    // this line puts the judged value on the record.
    let hrsp = vmread_checked(0x6C14);
    let hrip = vmread_checked(0x6C16);
    sprint!("vmx: entry forensics ({}): host RSP {}, host RIP {} (bridge-written values as they stood at the launch attempt)\r\n",
            tag, OptHex(hrsp), OptHex(hrip));
}

/// The outcome of one launch sequence. Refused33 is kept distinct
/// from every other stop because it — and only it — arms the next
/// ladder rung in the caller.
enum Birth {
    Born,
    Refused33,
    Stopped,
}

/// The birth sequence for one attempt: VMLAUNCH the 7-byte guest,
/// judge the first exit, emulate its CPUID, VMRESUME, and require
/// the HLT exit. Every path ends in teardown, exactly as the
/// single-attempt flow always has; the caller learns the outcome
/// from the return value. `tag` names the attempt in the forensics
/// line; `p` prefixes the attempt's own lines ("ladder " for the
/// rungs; the mirror passes "" and its lines stay byte-identical
/// to the runs that produced runs 1–6). `kind` says which
/// sequence the attempt belongs to (0 = the mirror, 1 = the
/// shared-block ladder, 2 = the host-state/controls ladder) and
/// `rung` is the attempt's number within it (0 for the mirror);
/// together they name the born line.
fn vmx_birth(tag: &str, p: &str, kind: u8, rung: u8, w: &VmxWorld, vmcs_pa: u64) -> Birth {
    const RAX_SENTINEL: u64 = 0x5EED_5EED_5EED_5EED;
    let mut regs = VmxRegs { rax: RAX_SENTINEL, ..Default::default() };
    sprint!("vmx: {}VMLAUNCH #1 — guest enters at {:#x} (the same 7-byte program as the SVM guest)\r\n",
            p, w.code_pa);
    unsafe { vmx_launch(&mut regs) };
    if regs.flag_cf != 0 {
        let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
        sprint!("vmx: {}VMLAUNCH FAILED — VMfailValid, VMCS instruction error {} (read back, not guessed); no guest ran\r\n",
                p, err);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Stopped;
    }
    if regs.flag_zf != 0 {
        sprint!("vmx: {}VMLAUNCH FAILED — VMfailInvalid (no current VMCS or VMCS misconfigured); no guest ran\r\n",
                p);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Stopped;
    }
    let reason = vmread_checked(vf::EXIT_REASON).unwrap_or(0) & 0xFFFF;
    let guest_rip = vmread_checked(vf::G_RIP).unwrap_or(0);
    sprint!("vmx: {}exit #1 reason {} (cpuid=10, hlt=12) guest rip {:#x} guest rax {:#x}\r\n",
            p, reason, guest_rip, regs.rax);
    if reason == 0 && guest_rip == w.code_pa && regs.rax == RAX_SENTINEL {
        // The launch "returned" with the VMCS's exit fields at their
        // initial values and the guest's first instruction (XOR
        // EAX,EAX) demonstrably unexecuted: RAX still holds our
        // sentinel. That is not a birth; it is an environment whose
        // entry path does not enter. Say so plainly, as with SVM.
        sprint!("vmx: {}guest birth NOT CLAIMED here — VMLAUNCH returned without the guest executing (exit reason 0, guest RIP and RAX untouched); entry in this environment did not occur; the machinery awaits Intel hardware or a KVM host\r\n",
                p);
        vmx_entry_forensics(tag);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Stopped;
    }
    if reason != VMX_EXIT_REASON_CPUID {
        sprint!("vmx: {}guest birth FAILED at first exit (reason {})\r\n", p, reason);
        vmx_entry_forensics(tag);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return if reason == 33 { Birth::Refused33 } else { Birth::Stopped };
    }

    // Emulate CPUID leaf 0 the way an Intel part answers it:
    // max leaf in RAX, "GenuineIntel" across RBX/RDX/RCX.
    regs.rax = 0xD;
    regs.rbx = 0x756E_6547; // "Genu"
    regs.rdx = 0x4965_6E69; // "ineI"
    regs.rcx = 0x6C65_746E; // "ntel"
    if !vmwrite_checked(vf::G_RIP, guest_rip + 2) {
        sprint!("vmx: {}could not advance guest RIP past CPUID — birth incomplete\r\n", p);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Stopped;
    }
    sprint!("vmx: {}cpuid emulated (leaf 0 -> GenuineIntel), VMRESUME #2\r\n", p);
    regs.resume = 1;
    unsafe { vmx_launch(&mut regs) };
    if regs.flag_cf != 0 || regs.flag_zf != 0 {
        let err = vmread_checked(vf::INSTR_ERROR).unwrap_or(0);
        sprint!("vmx: {}VMRESUME FAILED (cf {} zf {}, instruction error {}) — birth incomplete\r\n",
                p, regs.flag_cf, regs.flag_zf, err);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Stopped;
    }
    let reason2 = vmread_checked(vf::EXIT_REASON).unwrap_or(0) & 0xFFFF;
    sprint!("vmx: {}exit #2 reason {}\r\n", p, reason2);
    if reason2 == VMX_EXIT_REASON_HLT {
        // The attempt is named on the line immediately before
        // GUEST BORN — the verdict keys PROVEN on the GUEST BORN
        // line alone, and reads WHICH state was born off this one.
        if kind == 3 {
            // The reference state entered: a state whose every
            // delta from the refused mirror is a line of another
            // implementation's recipe — kvm-unit-tests' vmx
            // suite, which passes on this stack. The conviction
            // is therefore somewhere inside the seven deltas the
            // rung applied at once; the follow-on bisects them
            // down one per run. The rung's patch lines above
            // carry each delta's field and value.
            sprint!("vmx: born on ladder rung X1 (reference state)\r\n");
        } else if kind == 2 {
            // Ladder-2's rungs depart from the mirror ALONE (no
            // accumulation): the delta names the single departure
            // this attempt flew, and the rung's patch lines above
            // carry the exact fields and bits.
            let delta = match rung {
                1 => "the host data selectors (ES/SS/DS/FS/GS) null, where every refused state carried the host's live data selectors",
                2 => "the processor-based controls reduced to their capability floor (must-be-1 | HLT-exiting only)",
                3 => "the exit controls reduced to their capability floor (must-be-1 | 64-bit host only; the EFER save/load pair off)",
                4 => "the entry controls reduced to their capability floor (must-be-1 | IA-32e-mode guest only)",
                _ => "the combined floor — null host data selectors plus every varied control word at its capability floor, a single state no earlier rung flew",
            };
            sprint!("vmx: born on ladder-2 rung H{} — {}\r\n", rung, delta);
        } else if rung > 0 {
            // The rung is named with its delta: the delta is the
            // conviction — it is the ONLY variable in which this
            // attempt differed from the refused control rung.
            let delta = match rung {
                1 => "guest IA32_EFER holding the host's live MSR value (LMA as the host holds it), where the refused mirror carried the constructed LMA-clear field",
                2 => "the guest SYSENTER trio re-sourced from the host's live MSRs, on top of R1",
                3 => "the entry itself loading IA32_EFER (SDM bit 15), on top of R1+R2",
                _ => "entry-controls bit 12 cleared, on top of R1+R2+R3",
            };
            sprint!("vmx: born on ladder rung R{} — {}\r\n", rung, delta);
        } else {
            sprint!("vmx: born on the mirror state (attempt 1 — the production path)\r\n");
        }
        sprint!("vmx: GUEST BORN — ran, took our emulated cpuid (rax={:#x} rbx={:#x}), halted back to ZCore\r\n",
                regs.rax, regs.rbx);
        vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
        return Birth::Born;
    }
    sprint!("vmx: {}guest birth incomplete — second exit was not HLT\r\n", p);
    vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
    Birth::Stopped
}

/// The entry-controls value a ladder rung flies. Rungs 0–2 keep
/// the world's capability-adjusted value untouched. R3 sets
/// "load IA32_EFER" — SDM §24.8.1 bit 15 per the verified table
/// in vmx_first_guest's controls comment — re-adjusted through
/// the same capability MSR the controls were computed from, so
/// if this machine's nested VMX does not permit the bit, the
/// adjustment clears it and the rung's dump shows a degenerate
/// rung instead of an invalid control field going to VMLAUNCH.
/// R4 additionally clears bit 12 — the bit with no row in the
/// §24.8.1 table, present only via the capability adjustment;
/// the caller has already established it is not must-be-1 on
/// this machine.
fn ladder_entry_ctls(w: &VmxWorld, rung: u8) -> u32 {
    if rung < 3 {
        return w.entry_ctls;
    }
    let mut e = adjust_controls(w.entry_msr, w.entry_ctls | (1 << 15));
    if rung >= 4 {
        e &= !(1 << 12);
    }
    e
}

/// One full VMX attempt: fill, judge the writes, apply the
/// rung's patch (if any), dump the stored state, launch. Rung 0
/// is the mirror — the control attempt, byte-for-byte the flow
/// the previous rounds flew. Rungs 1–4 relaunch the mirror with
/// exactly one shared-block variable changed, cumulatively:
///   R1: guest IA32_EFER := the host's live IA32_EFER MSR. The
///       mirror's field is constructed (SCE|LME|NXE, LMA clear)
///       while the entry is IA-32e mode; with load-EFER clear,
///       bare-metal entry ignores the field — but KVM manages a
///       nested guest's EFER actively when it synthesizes vmcs02,
///       and a stored EFER whose LMA contradicts the IA-32e entry
///       bit is a live suspect for the synthesized reason 33.
///       HYPOTHESIS about KVM behaviour, marked as such: the rung
///       is the empirical test, not the assertion.
///   R2: + guest SYSENTER CS/ESP/EIP := the host's live
///       IA32_SYSENTER_* MSRs. The fill already sources these
///       fields from the same MSRs, so on a machine whose
///       SYSENTER MSRs read 0 (as run 6's stored zeros prove the
///       800 G1's do) the patch is value-identical by
///       construction — the rung's work is to READ the MSRs on
///       the record and certify the zeros at their source, the
///       last shared zeros never examined at the source.
///   R3: + entry controls: load IA32_EFER (SDM bit 15) set, so
///       the entry itself loads R1's host-true EFER — the
///       pairing §26.3.1.1's EFER checks then judge: LMA = LME =
///       IA-32e bit, all 1, from the host's own mouth.
///   R4: + entry controls: bit 12 cleared (the unnamed,
///       capability-forced bit; caller verified it is not
///       must-be-1 on this machine).
/// The write-failure abort every attempt path shares: if the
/// fill recorded any rejected write, name the census and the
/// first failure, run the post-mortem stickiness check (guest
/// RIP — written late in the sequence — is the reporter's
/// character witness), tear down, and report the abort. Returns
/// true when the attempt must stop. The printed lines are the
/// mirror attempt's historical lines, shared verbatim.
fn vmx_wfail_abort(p: &str, f: &Fill, w: &VmxWorld, vmcs_pa: u64) -> bool {
    if f.wfail == 0 {
        return false;
    }
    let (ff, ferr) = f.first_fail.unwrap_or((0, 0));
    sprint!("vmx: {}{} of {} VMCS field writes FAILED (first failure: field {:#x} ({}), VM-instruction error {}) — aborting before any launch attempt\r\n",
            p, f.wfail, f.wcount, ff, vf_name(ff), ferr);
    // Post-mortem stickiness check, the mirror of the control
    // read-back: guest RIP was written late in the sequence and
    // (in every run so far) reported as written. If it still
    // holds its value, the reporter's successes are real and the
    // failures named above are true per-field rejections. If it
    // does not hold, nothing the reporter said can be trusted.
    match vmread_checked(vf::G_RIP) {
        Some(read) if read == w.code_pa => {
            sprint!("vmx: post-mortem read-back: guest RIP holds {:#x} as written — reported successes are real; the failures above are true rejections of the named fields\r\n",
                    read);
        }
        Some(read) => {
            sprint!("vmx: post-mortem read-back: guest RIP reads {:#x}, expected {:#x} — reported successes did NOT stick; the reporter itself is suspect\r\n",
                    read, w.code_pa);
        }
        None => {
            sprint!("vmx: post-mortem read-back: guest RIP cannot be read back at all — the VMCS state itself is in question\r\n");
        }
    }
    vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
    true
}

enum Attempt {
    Born,
    Refused33,
    Stopped,
}

fn vmx_attempt(w: &VmxWorld, s: &GuestSegs, rung: u8,
               vmcs_pa: u64, stored: &mut Stored) -> Attempt {
    let p = if rung == 0 { "" } else { "ladder " };
    let mut f = Fill::new();
    vmx_fill(&mut f, w, s);
    if vmx_wfail_abort(p, &f, w, vmcs_pa) {
        return Attempt::Stopped;
    }
    if rung == 0 {
        sprint!("vmx: VMCS filled (host state = this machine now; guest state = its mirror; {} fields written, 0 write failures)\r\n",
                f.wcount);
    } else {
        sprint!("vmx: ladder VMCS filled (host state = this machine now; guest state = its mirror, rung patches follow; {} fields written, 0 write failures)\r\n",
                f.wcount);
    }
    // ---- the rung's patch: its single variable, applied after
    // the fill so the fill's sequence is never perturbed ----
    if rung >= 1 {
        sprint!("vmx: ladder patch (R1): guest IA32_EFER {:#x} (as filled) -> {:#x} (the host's live IA32_EFER MSR, read this boot)\r\n",
                w.guest_efer, w.efer_now);
        if !f.patch(vf::G_EFER, w.efer_now) {
            vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
            return Attempt::Stopped;
        }
    }
    if rung >= 2 {
        let scs = rdmsr(msr_vmx::SYSENTER_CS);
        let sesp = rdmsr(msr_vmx::SYSENTER_ESP);
        let seip = rdmsr(msr_vmx::SYSENTER_EIP);
        sprint!("vmx: ladder patch (R2): host IA32_SYSENTER_CS/ESP/EIP MSRs read {:#x}/{:#x}/{:#x} this boot; guest fields re-written from them (CS through the fill's own 32-bit mask: {:#x})\r\n",
                scs, sesp, seip, scs & 0xFFFF_FFFF);
        if !f.patch(vf::G_SYSENTER_CS, scs & 0xFFFF_FFFF)
            || !f.patch(vf::G_SYSENTER_ESP, sesp)
            || !f.patch(vf::G_SYSENTER_EIP, seip) {
            vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
            return Attempt::Stopped;
        }
    }
    if rung >= 3 {
        let e = ladder_entry_ctls(w, rung);
        if rung == 3 {
            sprint!("vmx: ladder patch (R3): entry controls {:#x} -> {:#x} (load IA32_EFER, SDM bit 15, requested, then capability-adjusted through the same MSR)\r\n",
                    w.entry_ctls, e);
            if e & (1 << 15) == 0 {
                sprint!("vmx: ladder patch (R3): NOTE — the capability MSR does not permit entry bit 15 on this machine; the adjusted value carries no load-EFER and this rung's stored entry controls equal R2's. The dump below proves it; read the rung as degenerate, not as an EFER verdict\r\n");
            }
        } else {
            sprint!("vmx: ladder patch (R4): entry controls {:#x} -> {:#x} (R3's load-EFER retained; bit 12 — the unnamed capability-forced bit — cleared)\r\n",
                    w.entry_ctls, e);
        }
        if !f.patch(vf::ENTRY_CTLS, e as u64) {
            vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
            return Attempt::Stopped;
        }
    }
    let tag = match rung {
        0 => "mirror",
        1 => "ladder R1",
        2 => "ladder R2",
        3 => "ladder R3",
        _ => "ladder R4",
    };
    vmx_dump(tag, &f.rec, stored);
    match vmx_birth(tag, p, if rung == 0 { 0 } else { 1 }, rung, w, vmcs_pa) {
        Birth::Born => Attempt::Born,
        Birth::Refused33 => Attempt::Refused33,
        Birth::Stopped => Attempt::Stopped,
    }
}

/// One ladder-2 attempt: the same fill, the same dump, the same
/// birth sequence as every attempt — with the rung's departure
/// applied as patches after the fill, exactly as ladder-1's
/// rungs patch theirs. The difference is what the patches touch
/// and how they compose: ladder-1 walked the GUEST block
/// cumulatively (EFER, SYSENTER, the entry load bit);
/// ladder-2's rungs each depart from the MIRROR ALONE, and they
/// touch the two blocks seven runs never varied — the HOST
/// state and the CONTROL words:
///
///   H1: host data selectors (ES/SS/DS/FS/GS) := 0. Null
///       selectors are legal host state for everything except
///       CS/TR (SDM §26.3; KVM's nested host-state check tests
///       only RPL/TI and the CS/TR/SS null rules) — the rung
///       removes the host's live GDT indices from the exit
///       state entirely.
///   H2: processor-based controls := the capability floor
///       (must-be-1 | HLT-exiting).
///   H3: exit controls := the capability floor (must-be-1 |
///       64-bit host) — the flown word's chosen extras, the
///       IA32_EFER save/load pair, come off.
///   H4: entry controls := the capability floor (must-be-1 |
///       IA-32e-mode guest).
///   H5: H1+H2+H3+H4 in a single state.
///
/// The caller computes each rung's patch list against the flown
/// values and passes only the pairs that actually differ — a
/// rung whose variable is already at its floor on this machine
/// never reaches this function (the caller marks it NOT
/// APPLICABLE instead of flying the control state under a
/// second name). `hrung` is 1–5.
fn vmx_attempt_h(w: &VmxWorld, s: &GuestSegs, hrung: u8,
                 patches: &[(u64, u64)], vmcs_pa: u64,
                 stored: &mut Stored) -> Attempt {
    let p = "ladder2 ";
    let mut f = Fill::new();
    vmx_fill(&mut f, w, s);
    if vmx_wfail_abort(p, &f, w, vmcs_pa) {
        return Attempt::Stopped;
    }
    sprint!("vmx: ladder2 VMCS filled (host state = this machine now; guest state = its mirror, rung patches follow; {} fields written, 0 write failures)\r\n",
            f.wcount);
    for &(field, value) in patches {
        sprint!("vmx: ladder2 patch (H{}): {} {:#x} (as filled) -> {:#x}\r\n",
                hrung, vf_name(field), f.rec.get(field).unwrap_or(0), value);
        if !f.patch(field, value) {
            vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
            return Attempt::Stopped;
        }
    }
    let tag = match hrung {
        1 => "ladder2 H1",
        2 => "ladder2 H2",
        3 => "ladder2 H3",
        4 => "ladder2 H4",
        _ => "ladder2 H5",
    };
    vmx_dump(tag, &f.rec, stored);
    match vmx_birth(tag, p, 2, hrung, w, vmcs_pa) {
        Birth::Born => Attempt::Born,
        Birth::Refused33 => Attempt::Refused33,
        Birth::Stopped => Attempt::Stopped,
    }
}

/// The reference-state attempt (rung X1): the same fill, the
/// same dump, the same birth sequence as every attempt — with
/// the caller's patch list applied after the fill. Where the
/// ladders each change ONE variable against the refused mirror,
/// X1 changes SEVEN at once, deliberately: every axis on which
/// a working reference recipe — kvm-unit-tests' x86/vmx.c
/// (init_vmcs_ctrl / init_vmcs_host / init_vmcs_guest), whose
/// nested guests launch successfully on this very stack —
/// differs from the mirror's stored state, as extracted field
/// by field against run 8's dumps. The deltas, in the order the
/// caller lists them:
///
///   1. guest + host IA32_SYSENTER_CS := the live CS selector
///      (the reference writes its kernel CS constant; the fill
///      sources the field from the IA32_SYSENTER_CS MSR, which
///      reads 0 on this vCPU — the value ladder R2 certified at
///      its source without ever changing it).
///   2. guest DR7 := 0 (the mirror flies the host's live DR7,
///      0x400 in every run so far).
///   3. guest LDTR := the usable LDT form — selector 0, base 0
///      (both already the fill's), limit 0xFFFF, access rights
///      0x82 (type 2 = LDT, present). The fill flies the
///      unusable form (limit 0, AR 0x10000) the host's null
///      LDTR derives.
///   4. entry controls := load-IA32_EFER | IA-32e-mode guest,
///      capability-adjusted (the reference's request; ladder
///      R3 flew load-EFER too, but with deltas 1–3 and 5–7
///      absent — the combination is what is on trial here).
///   5. exit controls := the flown request + load-IA32_PAT,
///      capability-adjusted, and host IA32_PAT := the live
///      IA32_PAT MSR (the fill never writes that field at all).
///   6. pin controls := external-interrupt, NMI and
///      virtual-NMI exiting requested atop the must-be-1 set,
///      capability-adjusted (the mirror flies the must-be-1
///      residue alone, 0x16 in every run).
///   7. host + guest GS base := the live IA32_GS_BASE MSR. The
///      fill already sources both fields from this MSR, so on a
///      boot where it reads 0 the two patches are value-
///      identical by construction and the dump proves it — the
///      delta's sourcing is honoured and its value, on this
///      machine, is the one already flown.
///
/// If X1 enters, the seven deltas hold the conviction between
/// them and the follow-on bisects them one per run. If X1 is
/// refused too, no L1-visible field value separates ZCore's
/// state from a recipe that enters on this stack, and the
/// delta is structural — the launch bridge and entry
/// mechanics — which is where the hunt goes next.
fn vmx_attempt_x1(w: &VmxWorld, s: &GuestSegs, patches: &[(u64, u64)],
                  vmcs_pa: u64, stored: &mut Stored) -> Attempt {
    let p = "x1 ";
    let mut f = Fill::new();
    vmx_fill(&mut f, w, s);
    if vmx_wfail_abort(p, &f, w, vmcs_pa) {
        return Attempt::Stopped;
    }
    sprint!("vmx: x1 VMCS filled (host state = this machine now; guest state = its mirror, reference-state patches follow; {} fields written, 0 write failures)\r\n",
            f.wcount);
    for &(field, value) in patches {
        match f.rec.get(field) {
            Some(old) => sprint!("vmx: x1 patch: {} {:#x} (as filled) -> {:#x}\r\n",
                                 vf_name(field), old, value),
            None => sprint!("vmx: x1 patch: {} — never written by the fill (VMCLEAR's leftover stands there) -> {:#x}\r\n",
                            vf_name(field), value),
        }
        if !f.patch(field, value) {
            vmx_teardown(vmcs_pa, true, w.orig_cr0, w.orig_cr4);
            return Attempt::Stopped;
        }
    }
    vmx_dump("reference X1", &f.rec, stored);
    match vmx_birth("reference X1", p, 3, 0, w, vmcs_pa) {
        Birth::Born => Attempt::Born,
        Birth::Refused33 => Attempt::Refused33,
        Birth::Stopped => Attempt::Stopped,
    }
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
