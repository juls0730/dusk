#![feature(abi_x86_interrupt)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod debug;
mod memory;

use crate::debug::serial;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial::init().unwrap();

    arch::init();
    let boot_info = boot::load_boot_info().unwrap();

    let mut allocator = memory::FrameAllocator::new(
        boot_info.memory_regions(),
        memory::DirectMap::new(boot_info.hhdm_offset),
    )
    .expect("failed to create frame allocator");

    let initial = allocator.free_frames();

    let first = allocator.alloc().unwrap();
    let second = allocator.alloc().unwrap();
    let third = allocator.alloc().unwrap();

    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_eq!(allocator.free_frames(), initial - 3);

    unsafe {
        allocator.dealloc(second);
        allocator.dealloc(first);
        allocator.dealloc(third);
    }

    assert_eq!(allocator.free_frames(), initial);

    let a = allocator.alloc().unwrap();
    let b = allocator.alloc().unwrap();
    let c = allocator.alloc().unwrap();

    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);

    hcf();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("Uh oh, something went wrong!");
    println!("{}", info);

    hcf();
}

pub fn hcf() -> ! {
    loop {
        arch::halt();
    }
}
