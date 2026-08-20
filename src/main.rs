#![feature(abi_x86_interrupt)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod debug;
mod memory;

use crate::{
    arch::paging,
    debug::serial,
    memory::{PhysicalAddr, VirtualAddr},
};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial::init().unwrap();

    arch::init();
    let boot_info = boot::load_boot_info().unwrap();
    let direct_map = memory::DirectMap::new(boot_info.hhdm_offset);

    let mut allocator = memory::FrameAllocator::new(boot_info.memory_regions(), direct_map)
        .expect("failed to create frame allocator");

    let mut page_table = paging::AddressSpace::new(
        direct_map,
        boot_info.memory_regions(),
        boot_info.kernel_address,
        &mut allocator,
    )
    .expect("failed to create page table");

    // safety: trust me bro
    unsafe { page_table.activate() };

    let frame = allocator.alloc().unwrap();

    let direct_mapped = direct_map.translate(frame.start_address()).unwrap();
    let translated = page_table.translate(direct_mapped).unwrap();

    println!("{:?}", translated);

    let new_virtual = VirtualAddr::new(0x8000_0000);

    assert!(page_table.translate(new_virtual).is_none());

    let page = paging::Page::from_start_address(new_virtual).unwrap();

    page_table
        .map(
            page,
            frame,
            paging::PagePermissions::KERNEL_DATA,
            &mut allocator,
        )
        .unwrap();

    assert_eq!(
        page_table.translate(new_virtual),
        Some(frame.start_address())
    );

    assert_eq!(
        page_table.translate(VirtualAddr::new(new_virtual.as_usize() + 123)),
        Some(PhysicalAddr::new(frame.start_address().as_usize() + 123))
    );

    // write to the page and read it back via HHDM
    unsafe {
        core::ptr::write_bytes(new_virtual.as_mut_ptr::<u8>(), 0xFF, 0x1000);
    }

    let slice = unsafe { core::slice::from_raw_parts(direct_mapped.as_ptr::<u8>(), 0x1000) };

    assert!(slice.iter().all(|&byte| byte == 0xFF));

    println!("{:#X}", slice[0]);

    let unmapped_frame = page_table.unmap(page, &mut allocator).unwrap();

    assert_eq!(unmapped_frame, frame);

    unsafe { allocator.dealloc(unmapped_frame) };

    assert!(page_table.translate(new_virtual).is_none());

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
