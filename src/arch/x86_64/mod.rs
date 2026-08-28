pub mod apic;
mod cpu;
mod gdt;
mod interrupts;
pub mod io_apic;
mod paging;
mod pit;
pub mod port;
pub mod timer;

use core::arch::asm;

pub use interrupts::disable_interrupts;
pub(crate) use paging::{
    MapError as PageTableMapError, PageTableCreateError, UnmapError as PageTableUnmapError,
};
pub use paging::{PageTable, PagingConfig};

pub struct ArchState {
    pub paging: PagingConfig,
}

use crate::{
    KernelHandoff,
    arch::{
        apic::LocalApic,
        io_apic::{IOAPIC_VIRTUAL_ADDRESS, IoApic},
        x86_64::interrupts::apic_vectors::PIT_CALIBRATION_VECTOR,
    },
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
    local_apic: LocalApic,
    io_apic: IoApic,
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
        IOAPIC_VIRTUAL_ADDRESS,
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
                vector: PIT_CALIBRATION_VECTOR,
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

// #[derive(Debug)]
// pub enum TimerError {
//     DurationOverflow,
// }

// impl InterruptController {
//     pub fn delay(&self, duration: core::time::Duration) -> Result<(), TimerError> {
//         let nanoseconds = duration.as_nanos();

//         let ticks = (nanoseconds
//             .checked_mul(self.local_timer_frequency as u128)
//             .ok_or(TimerError::DurationOverflow)?
//             + 999_999_999)
//             / 1_000_000_000;

//         if ticks == 0 {
//             return Ok(());
//         }

//         let mut remaining = ticks;

//         while remaining > 0 {
//             let chunk = remaining.min(u32::MAX as u128) as u32;
//             self.local_apic.delay_ticks(chunk);
//             remaining -= chunk as u128;
//         }

//         Ok(())
//     }
// }

/// # Safety
///
/// The caller must ensure:
/// - The stack is currently mapped, writable, and 16-byte aligned
pub unsafe fn enter_kernel(stack_top: VirtualAddr, handoff: *mut KernelHandoff) -> ! {
    unsafe {
        gdt::set_kernel_stack(stack_top);

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
            "mov gs, {user_data_selector:x}", // ss is handled by iretq

            "push {user_data_selector}",
            "push {user_stack_pointer}",
            "pushfq",
            "push {user_code_selector}",
            "push {user_instruction_pointer}",
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
