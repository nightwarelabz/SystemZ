//! ZCore — the SystemZ microhypervisor (SPEC.md §4).
//!
//! M1 skeleton. What exists today, honestly:
//! - a freestanding entry point (`_start`) for our UEFI loader to enter,
//! - a COM1 serial writer (the flight recorder's first voice),
//! - hardware capability detection read straight from the CPU:
//!   vendor, VMX (Intel) / SVM (AMD), and the hypervisor-present bit,
//! - then a halt loop. Nothing else is claimed.
//!
//! What does NOT exist yet: VMXON, EPT construction, vCPU scheduling,
//! ZBus. Those are M1/M2 work and this file will say so until they land.

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
