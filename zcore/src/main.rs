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

// ---------------------------------------------------------------- entry

#[link_section = ".text.boot"]
#[no_mangle]
pub extern "C" fn _start() -> ! {
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
