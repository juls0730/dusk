#![feature(abi_x86_interrupt)]
#![allow(clippy::needless_return)]
#![no_std]
#![no_main]

mod arch;
mod boot;
mod debug;
mod memory;
mod platform;
mod syscall;
mod task;

use core::arch::global_asm;

use crate::{
    debug::serial,
    memory::{
        AddressSpace, DirectMap, FrameAllocator, KernelStackPool, MemoryRegionKind,
        PagePermissions, UserStack, VirtualAddr,
    },
    task::tcb::Tcb,
};

pub struct KernelHandoff {
    allocator: memory::FrameAllocator,
    address_space: AddressSpace,
    direct_map: memory::DirectMap,
    boot_info: boot::BootInfo,
    kernel_stack_pool: KernelStackPool,
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

    let mut kernel_stack_pool = KernelStackPool::new();

    let kernel_stack = kernel_stack_pool
        .allocate(&mut address_space, &mut allocator)
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
        kernel_stack_pool,
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

unsafe extern "C" {
    static task_a_start: u8;
    static task_a_end: u8;
    static task_b_start: u8;
    static task_b_end: u8;
    static task_c_start: u8;
    static task_c_end: u8;
}

unsafe fn embedded_code(start: *const u8, end: *const u8) -> &'static [u8] {
    let length = unsafe { end.offset_from(start) as usize };
    unsafe { core::slice::from_raw_parts(start, length) }
}

global_asm!(
    r#"
   .global task_a_start
   task_a_start:
       mov r12d, 100

   .Ltask_a_loop:
       mov eax, 3
       mov edi, 1
       lea rsi, [rip + .Ltask_a_message]
       mov edx, 7
       xor r10d, r10d
       syscall

       mov eax, 1
       syscall

       dec r12d
       jnz .Ltask_a_loop

   .Ltask_a_done:
       mov eax, 2
       syscall
       jmp .Ltask_a_done

   .Ltask_a_message:
       .ascii "task A\n"

   .global task_a_end
   task_a_end:


   .global task_b_start
   task_b_start:
       mov r12d, 100

   .Ltask_b_loop:
       mov eax, 3
       mov edi, 1
       lea rsi, [rip + .Ltask_b_message]
       mov edx, 7
       xor r10d, r10d
       syscall

       mov eax, 1
       syscall

       dec r12d
       jnz .Ltask_b_loop

   .Ltask_b_done:
       mov eax, 2
       syscall
       jmp .Ltask_b_done

   .Ltask_b_message:
       .ascii "task B\n"

   .global task_b_end
   task_b_end:

   .global task_c_start
   task_c_start:
       mov r12d, 100

   .Ltask_c_loop:
       mov eax, 3
       mov edi, 1
       lea rsi, [rip + .Ltask_c_message]
       mov edx, 7
       xor r10d, r10d
       syscall

       mov eax, 1
       syscall

       dec r12d
       jnz .Ltask_c_loop

   .Ltask_c_done:
       mov eax, 2
       syscall
       jmp .Ltask_c_done

   .Ltask_c_message:
       .ascii "task C\n"

   .global task_c_end
   task_c_end:
   "#
);

pub unsafe extern "C" fn kernel_main(handoff: *mut KernelHandoff) -> ! {
    let (
        mut allocator,
        mut address_space,
        direct_map,
        boot_info,
        mut kernel_stack_pool,
        handoff_frame,
    ) = unsafe {
        let handoff = handoff.read();
        (
            handoff.allocator,
            handoff.address_space,
            handoff.direct_map,
            handoff.boot_info,
            handoff.kernel_stack_pool,
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

    let interrupt_controller =
        arch::init_interrupt_controller(&madt, &mut allocator, &mut address_space)
            .expect("failed to initialize interrupt controller");

    let task_a_code = unsafe { embedded_code(&task_a_start, &task_a_end) };
    let task_b_code = unsafe { embedded_code(&task_b_start, &task_b_end) };
    let task_c_code = unsafe { embedded_code(&task_c_start, &task_c_end) };

    let task_a = create_test_task(
        task_a_code,
        &mut address_space,
        &mut kernel_stack_pool,
        &mut allocator,
        direct_map,
    );

    let task_b = create_test_task(
        task_b_code,
        &mut address_space,
        &mut kernel_stack_pool,
        &mut allocator,
        direct_map,
    );

    let task_c = create_test_task(
        task_c_code,
        &mut address_space,
        &mut kernel_stack_pool,
        &mut allocator,
        direct_map,
    );

    task::scheduler::add_task(task_a).expect("scheduler is full");
    task::scheduler::add_task(task_b).expect("scheduler is full");
    task::scheduler::add_task(task_c).expect("scheduler is full");
    task::scheduler::start();

    hcf();
}

fn create_test_task(
    code: &[u8],
    kernel_address_space: &mut AddressSpace,
    kernel_stack_pool: &mut KernelStackPool,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
) -> Tcb {
    assert!(code.len() <= memory::FRAME_SIZE);

    let kernel_stack = kernel_stack_pool
        .allocate(kernel_address_space, allocator)
        .expect("failed to allocate task kernel stack");

    let mut user_address_space = kernel_address_space
        .new_user(allocator)
        .expect("failed to create user address space");

    let user_stack = UserStack::allocate(&mut user_address_space, allocator)
        .expect("failed to allocate user stack");

    let entry = VirtualAddr::new(0x8000);
    let code_frame = allocator
        .alloc()
        .expect("failed to allocate code frame")
        .into_raw();

    user_address_space
        .map(
            code_frame.start_address(),
            entry,
            PagePermissions::new(false, true, true),
            allocator,
            memory::CachePolicy::WriteBack,
        )
        .expect("failed to map user code");

    let destination = direct_map
        .translate(code_frame.start_address())
        .expect("code frame outside direct map");

    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), destination.as_mut_ptr(), code.len());
    }

    Tcb::new_user(
        0, // overwritten by add_task for now
        user_address_space,
        kernel_stack,
        entry,
        user_stack.top(),
    )
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
