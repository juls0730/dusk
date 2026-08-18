#![feature(abi_x86_interrupt, negative_impls)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod drivers;

use boot::limine::{BASE_REVISION, FRAMEBUFFER_REQUEST};

use crate::drivers::serial;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial::init().unwrap();

    assert!(BASE_REVISION.is_supported());

    draw_gradient();

    hcf();
}

fn draw_gradient() {
    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.response() {
        if let Some(&framebuffer) = framebuffer_response.framebuffers().first() {
            let buffer = unsafe {
                core::slice::from_raw_parts_mut(
                    framebuffer.address().cast::<u32>(),
                    framebuffer.size() / 4,
                )
            };

            for y in 0..framebuffer.height {
                for x in 0..framebuffer.width {
                    let r = (255 * x) / (framebuffer.width - 1);
                    let g = (255 * y) / (framebuffer.height - 1);
                    let b = 255 - r;

                    let pixel = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                    buffer
                        [(((y * framebuffer.pitch) / (framebuffer.bpp as u64 / 8)) + x) as usize] =
                        pixel
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    hcf();
}

pub fn hcf() -> ! {
    loop {
        unsafe {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            core::arch::asm!("hlt");

            #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
            core::arch::asm!("wfi");
        }
    }
}
