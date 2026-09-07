use core::arch::{asm, naked_asm};

use crate::memory::VirtualAddr;

#[repr(C, align(64))]
pub struct CpuLocal {
    pub kernel_stack_top: usize,
    pub user_rsp_scratch: usize,
    pub cpu_id: u32,
}

pub static mut BOOT_CPU: CpuLocal = CpuLocal {
    kernel_stack_top: 0,
    user_rsp_scratch: 0,
    cpu_id: 0,
};

#[derive(Debug)]
pub struct ThreadContext {
    rsp: usize,
}

impl ThreadContext {
    pub fn new(
        user_entry: VirtualAddr,
        user_stack: VirtualAddr,
        kernel_stack_top: VirtualAddr,
    ) -> Self {
        // Stack layout (grows downwards from kernel_stack_top):
        // [top - 8]  = user_thread_entry (popped by `ret`)
        // [top - 16] = rbp (0)
        // [top - 24] = rbx (user_stack)
        // [top - 32] = r12 (user_entry)
        // [top - 40] = r13 (0)
        // [top - 48] = r14 (0)
        // [top - 56] = r15 (0) <- initial rsp

        let stack_ptr = (kernel_stack_top.as_usize() - 56) as *mut usize;

        unsafe {
            stack_ptr.add(0).write(0); // r15
            stack_ptr.add(1).write(0); // r14
            stack_ptr.add(2).write(0); // r13
            stack_ptr.add(3).write(user_entry.as_usize()); // r12 (user_entry)
            stack_ptr.add(4).write(user_stack.as_usize()); // rbx (user_stack)
            stack_ptr.add(5).write(0); // rbp (0)
            stack_ptr
                .add(6)
                .write(user_thread_entry as *const () as usize); // return address
        }

        Self {
            rsp: kernel_stack_top.as_usize() - 56,
        }
    }

    pub fn empty() -> Self {
        Self { rsp: 0 }
    }
}

#[unsafe(naked)]
unsafe extern "C" fn user_thread_entry() -> ! {
    naked_asm!(
        // r12 = user_entry, rbx = user_stack
        "mov rdi, r12",
        "mov rsi, rbx",
        "call {enter_user}",
        enter_user = sym crate::arch::enter_user,
    );
}

#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(prev: *mut ThreadContext, next: *const ThreadContext) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "",
        // save current rsp into prev.rsp
        "mov [rdi], rsp",
        // load next rsp into rsp
        "mov rsp, [rsi]",
        "",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

#[derive(Debug)]
pub enum CpuFeaturesError {
    CpuidFeaturesNotSupported,
    SyscallNotSupported,
    InvalidPhysicalAddressWidth,
    InvalidVirtualAddressWidth,
}

#[derive(Clone, Copy)]
pub(crate) struct CpuFeatures {
    pub nx_supported: bool,
    pub nx_enabled: bool,
    pub global_pages: bool,
    pub physical_address_bits: u8,
    pub virtual_address_bits: u8,
    pub five_level_paging_active: bool,
}

// Extended features
const IA32_EFER: u32 = 0xC0000080;

pub fn detect_features_and_enable() -> Result<CpuFeatures, CpuFeaturesError> {
    let mut features = CpuFeatures {
        nx_supported: false,
        nx_enabled: false,
        global_pages: false,
        physical_address_bits: 0,
        virtual_address_bits: 0,
        five_level_paging_active: false,
    };

    let cpuid_result = core::arch::x86_64::__cpuid(0x80000000);

    if cpuid_result.eax < 0x80000008 {
        return Err(CpuFeaturesError::CpuidFeaturesNotSupported);
    }

    let cpuid_result = core::arch::x86_64::__cpuid(0x80000001);

    features.nx_supported = cpuid_result.edx & (1 << 20) != 0;

    // TODO: on AMD K6 *only*, this bit is bit 10, should we consider that edge case?
    let syscall_supported = cpuid_result.edx & (1 << 11) != 0;
    if !syscall_supported {
        return Err(CpuFeaturesError::SyscallNotSupported);
    }

    unsafe {
        let efer = read_msr(IA32_EFER);
        let value = efer | 1;
        write_msr(IA32_EFER, value);
    };

    let cpuid_result = core::arch::x86_64::__cpuid(0x80000008);

    features.physical_address_bits = (cpuid_result.eax & 0xFF) as u8;
    if !(12..=52).contains(&features.physical_address_bits) {
        return Err(CpuFeaturesError::InvalidPhysicalAddressWidth);
    }

    features.virtual_address_bits = (cpuid_result.eax >> 8 & 0xFF) as u8;
    features.five_level_paging_active = read_cr4() & (1 << 12) != 0;

    let required_virtual_address_bits = if features.five_level_paging_active {
        57
    } else {
        48
    };
    if features.virtual_address_bits < required_virtual_address_bits {
        return Err(CpuFeaturesError::InvalidVirtualAddressWidth);
    }

    let cpuid_result = core::arch::x86_64::__cpuid(0x1);

    features.global_pages = cpuid_result.edx & (1 << 13) != 0;

    if features.global_pages {
        let cr4 = read_cr4();
        write_cr4(cr4 | 1 << 7);
    }

    if features.nx_supported {
        let msr_supported = cpuid_result.edx & (1 << 5) != 0;

        if !msr_supported {
            return Err(CpuFeaturesError::CpuidFeaturesNotSupported);
        }

        // mother efer
        let efer = unsafe { read_msr(IA32_EFER) };

        unsafe {
            write_msr(IA32_EFER, efer | (1 << 11));
        }

        features.nx_enabled = unsafe { read_msr(IA32_EFER) } & (1 << 11) != 0;
    }

    Ok(features)
}

fn write_cr4(value: usize) {
    unsafe {
        asm!(
            "mov cr4, {}",
            in(reg) value,
            options(nostack, preserves_flags)
        );
    }
}

fn read_cr4() -> usize {
    let value: usize;

    unsafe {
        asm!(
            "mov {}, cr4",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}

pub(super) unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;

    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }

    ((high as u64) << 32) | low as u64
}

pub(super) unsafe fn write_msr(msr: u32, value: u64) {
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}
