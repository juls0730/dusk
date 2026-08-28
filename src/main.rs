#![feature(abi_x86_interrupt)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod debug;
mod memory;
mod platform;

use crate::{
    debug::serial,
    memory::{
        AddressSpace, KernelStack, MemoryRegionKind, PagePermissions, UserStack, VirtualAddr,
    },
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

    let bootstrap_stack = KernelStack::allocate(&mut address_space, &mut allocator)
        .expect("failed to allocate bootstrap stack");

    let handoff_frame = allocator
        .alloc()
        .expect("failed to allocate frame for kernel handoff");
    let handoff_addr = address_space
        .to_virtual(handoff_frame.frame_address().start_address())
        .expect("failed to map kernel handoff");

    let bootstrap_stack_top = bootstrap_stack.top();
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

    let madt = acpi
        .madt()
        .expect("failed to parse ACPI")
        .expect("MADT not found");

    let interrupt_controller =
        arch::init_interrupt_controller(&madt, &mut allocator, &mut address_space)
            .expect("failed to initialize interrupt controller");

    let mut user_addr_space = address_space
        .new_user(&mut allocator)
        .expect("failed to create user address space");

    let user_stack = UserStack::allocate(&mut user_addr_space, &mut allocator)
        .expect("failed to allocate user stack");

    let user_instruction_pointer = VirtualAddr::new(0x8000);
    let user_code_page = allocator
        .alloc()
        .expect("failed to allocate user code page")
        .frame_address();
    user_addr_space
        .map(
            user_code_page.start_address(),
            user_instruction_pointer,
            PagePermissions::new(true, true, true),
            &mut allocator,
            memory::CachePolicy::WriteBack,
        )
        .expect("failed to map user code page");

    unsafe {
        user_addr_space.activate();

        let user_code: [u8; 5] = [
            0xCC, // INT3
            0xCD, 0x80, // INT 0x80
            0xEB, 0xFE, // JMP -2 (loop forever if exit returns)
        ];

        core::ptr::copy_nonoverlapping(
            user_code.as_ptr(),
            user_instruction_pointer.as_mut_ptr::<u8>(),
            user_code.len(),
        );

        arch::enter_user(user_instruction_pointer, user_stack.top());
    }

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
