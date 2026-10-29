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
//! What does NOT exist yet: the VMX twin of the guest, nested paging
//! (EPT/NPT), vCPU scheduling, ZBus. This file will say so until they
//! land.

#![no_std]
#![no_main]

use core::arch::asm;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
mod boot_screen;

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

const _: () = assert!(core::mem::size_of::<ZBootInfo>() == 72);

fn boot_manifest(address: u64) -> Option<&'static ZBootInfo> {
    if address == 0 { return None; }
    let info = unsafe { &*(address as *const ZBootInfo) };
    if info.magic != ZBOOT_MAGIC ||
        !((info.version == 1 && info.info_size >= 72) ||
          (info.version == 2 && info.info_size >= 128)) {
        sprint!("zboot: manifest magic/version/size rejected at {:#x}\r\n", address);
        return None;
    }
    if info.version == 2 {
        let graphics = unsafe {
            &*((address + core::mem::size_of::<ZBootInfo>() as u64)
                as *const boot_screen::Graphics)
        };
        if boot_screen::attach(graphics) {
            sprint!("zboot: framebuffer accepted {}x{} stride {} format {}\r\n",
                    graphics.framebuffer.width, graphics.framebuffer.height,
                    graphics.framebuffer.stride, graphics.framebuffer.format);
        } else {
            sprint!("zboot: graphical splash disabled - serial diagnostics retained\r\n");
        }
    }
    Some(info)
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

enum GuestResult { NotEntered, Failed, Born, Incomplete }

fn svm_first_guest() -> GuestResult {
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
        asm!("str {}", out(reg) trv, options(nostack));
        asm!("sldt {}", out(reg) ldtv, options(nostack));
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
        return GuestResult::NotEntered;
    }
    if exit1 != EXIT_CPUID {
        sprint!("svm: guest birth FAILED at first exit\r\n");
        return GuestResult::Failed;
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
        GuestResult::Born
    } else {
        sprint!("svm: guest birth incomplete — second exit was not HLT\r\n");
        GuestResult::Incomplete
    }
}

// ---------------------------------------------------------------- entry

#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start(manifest_addr: u64) -> ! {
    unsafe { Serial::init() };

    sprint!("\r\nSystemZ ZCore — M1 skeleton\r\n");
    let manifest = boot_manifest(manifest_addr);
    boot_screen::step(6, b"ZCORE ENTERED - PROBING PROCESSOR\0");

    let caps = probe();
    boot_screen::step(7, b"PROCESSOR PROBED - READING BOOT MANIFEST\0");
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

    if let Some(info) = manifest {
        report_memory(info);
    } else {
        sprint!("zboot: no manifest (entered without the loader)\r\n");
    }

    boot_screen::step(8, b"MEMORY MAP REPORTED - CHECKING GUEST EXECUTION\0");
    let guest = if caps.svm { Some(svm_first_guest()) } else { None };
    boot_screen::step(9, b"EARLY BOOT CHECKS FINISHED - OPENING DIAGNOSTICS\0");

    let cpu: &'static [u8] = if caps.svm { b"CPU: AMD SVM PRESENT\0" }
        else if caps.vmx { b"CPU: INTEL VMX PRESENT\0" }
        else { b"CPU: HARDWARE VIRTUALIZATION ABSENT\0" };
    let (guest_line, result): (&'static [u8], &'static [u8]) = match guest {
        Some(GuestResult::Born) => (b"SVM GUEST: EXECUTED CPUID / HLT\0",
                                   b"FIRST GUEST PROVEN - EARLY CORE BOUNDARY\0"),
        Some(GuestResult::NotEntered) => (b"SVM GUEST: DID NOT ENTER - BIRTH NOT CLAIMED\0",
                                        b"GUEST PROOF REQUIRES A CAPABLE HOST\0"),
        Some(GuestResult::Failed) => (b"SVM GUEST: FIRST EXIT FAILED\0",
                                    b"GUEST EXECUTION FAILED - SEE COM1\0"),
        Some(GuestResult::Incomplete) => (b"SVM GUEST: SECOND EXIT WAS NOT HLT\0",
                                        b"GUEST EXECUTION INCOMPLETE - SEE COM1\0"),
        None if caps.vmx => (b"VMX GUEST: NOT IMPLEMENTED\0",
                            b"VMX CAPABILITY DETECTED - GUEST PATH PENDING\0"),
        None => (b"GUEST: UNAVAILABLE\0", b"UNSUPPORTED CPU - SYSTEMZ CANNOT START\0"),
    };
    boot_screen::finish(cpu, guest_line, result);
    sprint!("splash: diagnostics handoff complete (operator shell not implemented)\r\n");

    sprint!("zcore: halting (skeleton ends here, on purpose)\r\n");
    loop {
        unsafe { asm!("hlt", options(nostack)) };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    boot_screen::panic_screen();
    sprint!("\r\nZCORE PANIC: {}\r\n", info);
    loop {
        unsafe { asm!("hlt", options(nostack)) };
    }
}

