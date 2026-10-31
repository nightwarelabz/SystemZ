//! Direct framebuffer only. No UEFI calls are legal on this side of handoff.
use core::sync::atomic::{AtomicPtr, Ordering};

#[repr(C)]
pub struct Framebuffer {
    pub address: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct Graphics {
    pub framebuffer: Framebuffer,
    pub flags: u32,
    pub frame: u32,
}

const _: () = assert!(core::mem::size_of::<Framebuffer>() == 48);
const _: () = assert!(core::mem::size_of::<Graphics>() == 56);

extern "C" {
    fn sz_framebuffer_valid(fb: *const Framebuffer) -> i32;
    fn sz_splash_draw(fb: *const Framebuffer, completed: u32, frame: u32,
                      status: *const u8, state: u32, code: u64);
    fn sz_console_draw(fb: *const Framebuffer, cpu: *const u8,
                       guest: *const u8, result: *const u8);
}

// Published once on the boot CPU. The manifest remains reserved; no
// allocator reclaims it. An eventual page-table switch must retain MMIO.
static FB: AtomicPtr<Framebuffer> = AtomicPtr::new(core::ptr::null_mut());

pub fn attach(graphics: &Graphics) -> bool {
    if graphics.flags & 1 != 0 && unsafe { sz_framebuffer_valid(&graphics.framebuffer) } != 0 {
        FB.store(core::ptr::addr_of!(graphics.framebuffer) as *mut _, Ordering::Release);
        return true;
    }
    false
}

pub fn step(completed: u32, status: &'static [u8]) {
    let fb = FB.load(Ordering::Acquire);
    if !fb.is_null() {
        // All call sites pass static NUL-terminated strings. Animation
        // follows actual stage changes; no firmware timer survives exit.
        unsafe { sz_splash_draw(fb, completed, completed * 3, status.as_ptr(), 0, 0) };
    }
}

pub fn finish(cpu: &'static [u8], guest: &'static [u8], result: &'static [u8]) {
    let fb = FB.load(Ordering::Acquire);
    if !fb.is_null() {
        unsafe { sz_console_draw(fb, cpu.as_ptr(), guest.as_ptr(), result.as_ptr()) };
    }
}

pub fn panic_screen() {
    let fb = FB.load(Ordering::Acquire);
    if !fb.is_null() {
        unsafe { sz_splash_draw(fb, 6, 0, b"ZCORE PANIC - SEE COM1\0".as_ptr(), 1, 0) };
    }
}
