pub(super) mod apic;
mod cpu;
mod gdt;
mod interrupts;
pub(super) mod io_apic;
mod paging;
mod pit;
pub mod port;
mod syscall;
pub mod timer;

use core::arch::asm;

pub use cpu::{ThreadContext, switch_context};
pub use interrupts::{disable_interrupts, disable_interrupts_and_save, restore_interrupts};
pub(crate) use paging::{
    MapError as PageTableMapError, PageTableCreateError, UnmapError as PageTableUnmapError,
};
pub use paging::{PageTable, PagingConfig};

pub struct ArchState {
    pub paging: PagingConfig,
}

use crate::{
    KernelHandoff,
    arch::x86_64::cpu::BOOT_CPU,
    memory::{AddressSpace, FrameAllocator, VirtualAddr},
    platform::acpi::Madt,
    println,
};

pub fn init() -> ArchState {
    disable_interrupts();
    println!("Loading GDT...");
    gdt::init();
    println!("Loading IDT...");
    interrupts::init();
    println!("Detecting CPU features...");
    let cpu_features = cpu::detect_features_and_enable();
    let paging =
        PagingConfig::from_features(cpu_features.expect("required CPU features are not supported"));
    ArchState { paging }
}

pub fn set_kernel_stack(stack_top: VirtualAddr) {
    gdt::set_kernel_stack(stack_top);

    unsafe {
        BOOT_CPU.kernel_stack_top = stack_top.as_usize();
    }
}

#[derive(Debug)]
#[allow(unused)]
pub enum InterruptInitError {
    InvalidLocalApicId,
    InvalidLocalApicAddress,
    FailedToGetIoApic,
    IoApicError(io_apic::IoApicError),
    LocalApicError(apic::LocalApicError),
    TimerCalibrationError(timer::TimerCalibrationError),
    MalformedMadt,
    PitNotHandled,
}

#[derive(Debug)]
pub struct InterruptController {
    local_apic: apic::LocalApic,
    io_apic: io_apic::IoApic,
    local_timer_frequency: u64,
}

pub fn init_interrupt_controller(
    madt: &Madt<'_>,
    allocator: &mut FrameAllocator,
    address_space: &mut AddressSpace,
) -> Result<InterruptController, InterruptInitError> {
    let local_apic_address = madt
        .effective_local_apic_address()
        .map_err(|_| InterruptInitError::InvalidLocalApicAddress)?;

    let mut local_apic = apic::LocalApic::init(local_apic_address, allocator, address_space)
        .map_err(|err| InterruptInitError::LocalApicError(err))?;

    let io_apic_info = madt
        .sole_io_apic()
        .map_err(|_| InterruptInitError::FailedToGetIoApic)?;

    let pit_route = madt
        .isa_irq_route(0x0)
        .map_err(|_| InterruptInitError::MalformedMadt)?;

    let mut io_apic = io_apic::IoApic::new(
        io_apic_info.id,
        io_apic_info.apic_address,
        io_apic_info.global_system_interrupt_base,
        io_apic::IOAPIC_VIRTUAL_ADDRESS,
        allocator,
        address_space,
    )
    .map_err(|err| InterruptInitError::IoApicError(err))?;

    let pit_handled = io_apic.handles_gsi(pit_route.gsi);
    if !pit_handled {
        return Err(InterruptInitError::PitNotHandled);
    }

    let destination =
        u8::try_from(local_apic.id()).map_err(|_| InterruptInitError::InvalidLocalApicId)?;

    io_apic
        .configure_masked(
            pit_route.gsi,
            io_apic::RedirectionConfig {
                vector: interrupts::apic_vectors::PIT_CALIBRATION_VECTOR,
                destination,
                polarity: pit_route.polarity,
                trigger: pit_route.trigger,
            },
        )
        .map_err(|err| InterruptInitError::IoApicError(err))?;

    let local_timer_frequency =
        timer::calibrate_local_apic(&mut local_apic, &mut io_apic, pit_route)
            .map_err(|err| InterruptInitError::TimerCalibrationError(err))?;

    Ok(InterruptController {
        local_apic,
        io_apic,
        local_timer_frequency,
    })
}

/// # Safety
///
/// The caller must ensure:
/// - The stack is currently mapped, writable, and 16-byte aligned
pub unsafe fn enter_kernel(stack_top: VirtualAddr, handoff: *mut KernelHandoff) -> ! {
    unsafe {
        gdt::set_kernel_stack(stack_top);

        BOOT_CPU.kernel_stack_top = stack_top.as_usize();
        syscall::init(&raw const BOOT_CPU);

        asm!(
            "mov rsp, {stack_top}",
            "xor rbp, rbp",
            "mov rdi, {handoff}",
            "call {kernel_main}",
            stack_top = in(reg) stack_top.as_usize(),
            handoff = in(reg) handoff,
            kernel_main = sym crate::kernel_main,
            options(noreturn)
        );
    };
}

/// # Safety
///
/// - `user_instruction_pointer` and `user_stack_pointer` must be valid user mappings.
/// - The active address space must contain the kernel and supplied user mappings.
pub unsafe fn enter_user(
    user_instruction_pointer: VirtualAddr,
    user_stack_pointer: VirtualAddr,
) -> ! {
    println!("Entering user mode");

    unsafe {
        asm!(
            "mov ds, {user_data_selector:x}",
            "mov es, {user_data_selector:x}",
            "mov fs, {user_data_selector:x}",

            "push {user_data_selector}",
            "push {user_stack_pointer}",
            "push 0x202", // RFLAGS (IF=1, bit 1 reserved=1)
            "push {user_code_selector}",
            "push {user_instruction_pointer}",

            // clear GPRs
            "xor rax, rax",
            "xor rbx, rbx",
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor rsi, rsi",
            "xor rdi, rdi",
            "xor rbp, rbp",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",

            // Kernel GS is CpuLocal; leave it in IA32_KERNEL_GS_BASE so
            // syscall_entry can recover it with SWAPGS.
            "swapgs",
            "mov gs, {user_data_selector:x}",
            "iretq",
            user_data_selector = in(reg) gdt::USER_DATA_SELECTOR as usize,
            user_code_selector = in(reg) gdt::USER_CODE_SELECTOR as usize,
            user_instruction_pointer = in(reg) user_instruction_pointer.as_usize(),
            user_stack_pointer = in(reg) user_stack_pointer.as_usize(),
            options(noreturn)
        );
    }
}

pub fn halt() {
    unsafe {
        asm!("hlt");
    }
}
