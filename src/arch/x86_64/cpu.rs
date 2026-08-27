use core::arch::asm;

#[derive(Debug)]
pub enum CpuFeaturesError {
    CpuidFeaturesNotSupported,
    InvalidPhysicalAddressWidth,
    InvalidVirtualAddressWidth,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuFeatures {
    pub nx_supported: bool,
    pub nx_enabled: bool,
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
        physical_address_bits: 0,
        virtual_address_bits: 0,
        five_level_paging_active: false,
    };

    let cpuid_result = core::arch::x86_64::__cpuid_count(0x80000000, 0);

    if cpuid_result.eax < 0x80000008 {
        return Err(CpuFeaturesError::CpuidFeaturesNotSupported);
    }

    let cpuid_result = core::arch::x86_64::__cpuid_count(0x80000001, 0);

    features.nx_supported = cpuid_result.edx & (1 << 20) != 0;

    let cpuid_result = core::arch::x86_64::__cpuid_count(0x80000008, 0);

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

    if features.nx_supported {
        let cpuid_result = core::arch::x86_64::__cpuid_count(0x1, 0);
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
