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
    memory::{AddressSpace, MemoryRegionKind, VirtualAddr},
};

#[derive(Debug)]
#[allow(unused)]
enum KernelStackCreateError {
    AddressOverflow,
    OutOfFrames,
    GuardPageMapped,
    Map(memory::MapError),
}

struct KernelStack {
    start: memory::VirtualAddr,
    pages: usize,
}

const BOOTSTRAP_STACK_TOP: usize = 0xFFFF_FFFE_0000_0000;
const KERNEL_STACK_SIZE: usize = 64 * 1024;
const KERNEL_STACK_GUARD_SIZE: usize = memory::FRAME_SIZE;

const BOOTSTRAP_STACK_START: usize = BOOTSTRAP_STACK_TOP - KERNEL_STACK_SIZE;
const BOOTSTRAP_STACK_GUARD: usize = BOOTSTRAP_STACK_START - KERNEL_STACK_GUARD_SIZE;

impl KernelStack {
    fn allocate(
        address_space: &mut AddressSpace,
        allocator: &mut memory::FrameAllocator,
    ) -> Result<Self, KernelStackCreateError> {
        let kernel_stack_start = VirtualAddr::new(BOOTSTRAP_STACK_START);

        let guard_page = VirtualAddr::new(BOOTSTRAP_STACK_GUARD);

        if address_space.to_physical(guard_page).is_some() {
            // guard page should be *unmapped* so we get a page fault if we try to access it
            return Err(KernelStackCreateError::GuardPageMapped);
        }

        let mut i = 0;
        // on error we will just leak the frames
        // because like what are going to do if we recover them? Die happily without leaking frames? idgaf
        while i < KERNEL_STACK_SIZE {
            let frame = allocator
                .alloc()
                .ok_or(KernelStackCreateError::OutOfFrames)?;
            address_space
                .map(
                    frame.frame_address().start_address(),
                    VirtualAddr::new(
                        kernel_stack_start
                            .as_usize()
                            .checked_add(i)
                            .ok_or(KernelStackCreateError::AddressOverflow)?,
                    ),
                    memory::PagePermissions::new(true, false, false),
                    allocator,
                    memory::CachePolicy::WriteBack,
                )
                .map_err(|err| KernelStackCreateError::Map(err))?;
            i += memory::FRAME_SIZE;
        }

        debug_assert!(address_space.to_physical(guard_page).is_none());

        Ok(Self {
            start: kernel_stack_start,
            pages: i / memory::FRAME_SIZE,
        })
    }

    pub fn top(&self) -> Result<VirtualAddr, KernelStackCreateError> {
        Ok(VirtualAddr::new(
            self.start
                .as_usize()
                .checked_add(self.pages * memory::FRAME_SIZE)
                .ok_or(KernelStackCreateError::AddressOverflow)?,
        ))
    }
}

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

    let bootstrap_stack_top = bootstrap_stack.top().unwrap();
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

    println!(
        "Initializing local ACPI... {:#X}",
        boot_info.rsdp.as_usize()
    );

    let acpi = platform::acpi::init(&boot_info, direct_map).expect("failed to initialize ACPI");

    let madt = acpi
        .madt()
        .expect("failed to parse ACPI")
        .expect("MADT not found");

    let interrupt_controller =
        arch::init_interrupt_controller(&madt, &mut allocator, &mut address_space)
            .expect("failed to initialize interrupt controller");

    println!("interrupt controller: {:?}", interrupt_controller);

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
