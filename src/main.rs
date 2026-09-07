#![feature(abi_x86_interrupt)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod debug;
mod format;
mod memory;
mod platform;
mod syscall;
mod task;

use crate::{
    debug::serial,
    memory::{AddressSpace, MemoryRegionKind, init_frame_allocator, init_kernel_address_space},
};

pub struct KernelHandoff {
    allocator: memory::FrameAllocator,
    address_space: AddressSpace,
    direct_map: memory::DirectMap,
    boot_info: boot::BootInfo,
    handoff_frame: memory::OwnedFrame,
}

const _: () = assert!(core::mem::size_of::<KernelHandoff>() <= memory::FRAME_SIZE);

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial::init().unwrap();

    let arch_state = arch::init();
    let boot_info = boot::load_boot_info().unwrap();
    let direct_map = memory::DirectMap::new(boot_info.hhdm_offset);

    let mut allocator = memory::FrameAllocator::new(boot_info.memory_regions(), direct_map)
        .expect("failed to create frame allocator");

    println!("Initializing page table...");
    let mut address_space = AddressSpace::new_kernel(
        direct_map,
        boot_info.memory_regions(),
        &boot_info.kernel_layout,
        arch_state.paging,
        &mut allocator,
    )
    .expect("failed to create page table");

    println!("Entering kernel main...");

    let kernel_stack =
        crate::task::scheduler::allocate_kernel_stack(&mut address_space, &mut allocator)
            .expect("failed to allocate bootstrap stack");

    let handoff_frame = allocator
        .alloc()
        .expect("failed to allocate frame for kernel handoff");
    let handoff_addr = address_space
        .to_virtual(handoff_frame.frame_address().start_address())
        .expect("failed to map kernel handoff");

    let bootstrap_stack_top = kernel_stack.top();
    let handoff = KernelHandoff {
        allocator,
        address_space,
        direct_map,
        boot_info,
        handoff_frame,
    };

    unsafe {
        handoff_addr.as_mut_ptr::<KernelHandoff>().write(handoff);
        (*handoff_addr.as_mut_ptr::<KernelHandoff>())
            .address_space
            .activate();

        arch::enter_kernel(
            bootstrap_stack_top,
            handoff_addr.as_mut_ptr::<KernelHandoff>(),
        );
    }
}

pub unsafe extern "C" fn kernel_main(handoff: *mut KernelHandoff) -> ! {
    let (mut allocator, mut address_space, direct_map, boot_info, handoff_frame) = unsafe {
        let handoff = handoff.read();
        (
            handoff.allocator,
            handoff.address_space,
            handoff.direct_map,
            handoff.boot_info,
            handoff.handoff_frame,
        )
    };

    unsafe {
        allocator.dealloc(handoff_frame);
    }

    allocator.reclaim_regions(
        boot_info.memory_regions(),
        MemoryRegionKind::BootloaderReclaimable,
    );

    println!("Initializing local ACPI...",);

    let acpi = platform::acpi::init(&boot_info, direct_map).expect("failed to initialize ACPI");

    println!("Parsing MADT...");

    let madt = acpi
        .madt()
        .expect("failed to parse ACPI")
        .expect("MADT not found");

    println!("Initializing interrupt controller...");

    let _interrupt_controller =
        arch::init_interrupt_controller(&madt, &mut allocator, &mut address_space)
            .expect("failed to initialize interrupt controller");

    task::bootstrap::spawn(
        "omega3.elf",
        &boot_info.initramfs,
        &mut address_space,
        &mut allocator,
        direct_map,
    );

    init_frame_allocator(allocator);
    init_kernel_address_space(address_space);

    task::scheduler::start();
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
