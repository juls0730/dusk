use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::{
    arch::{
        port::write_u8,
        x86_64::{
            cpu::{read_msr, write_msr},
            interrupts::apic_vectors::{
                APIC_ERROR_VECTOR, APIC_SPURIOUS_VECTOR, APIC_TIMER_VECTOR,
            },
        },
    },
    memory::{
        AddressSpace, CachePolicy, FrameAllocator, PagePermissions, PhysicalAddr, VirtualAddr,
    },
    println,
};

const APIC_ID: u32 = 0x20;
// End Of Interrupt
const APIC_EOI: u32 = 0xB0;
// Task Priority Register
const APIC_TPR: u32 = 0x80;
// Spurious Interrupt Vector
const APIC_SVR: u32 = 0xF0;
// Error Status Register
const APIC_ESR: u32 = 0x280;
const APIC_LVT_ERROR: u32 = 0x370;
const APIC_LVT_MASKED: u64 = 1 << 16;

const APIC_LVT_TIMER_MODE_PERIODIC: u64 = 1 << 17;

pub const LOCAL_APIC_VIRTUAL_ADDRESS: VirtualAddr = VirtualAddr::new(0xFFFF_FFFD_0000_0000);

const APIC_LVT_TIMER: u32 = 0x320;
const APIC_TIMER_INITIAL_COUNT: u32 = 0x380;
const APIC_TIMER_CURRENT_COUNT: u32 = 0x390;
const APIC_TIMER_DIVIDE_CONFIG: u32 = 0x3E0;

#[derive(Debug)]
enum LocalApicAccess {
    X2Apic,
    XApic,
}

impl LocalApicAccess {
    fn read(&self, offset: u32) -> u64 {
        match self {
            Self::X2Apic => {
                let msr = 0x800 + offset / 16;
                unsafe { read_msr(msr) }
            }
            Self::XApic => unsafe {
                LOCAL_APIC_VIRTUAL_ADDRESS
                    .as_ptr::<u8>()
                    .add(offset as usize)
                    .cast::<u32>()
                    .read_volatile() as u64
            },
        }
    }

    fn write(&self, offset: u32, value: u64) {
        match self {
            Self::X2Apic => {
                let msr = 0x800 + offset / 16;
                unsafe { write_msr(msr, value) };
            }
            Self::XApic => unsafe {
                LOCAL_APIC_VIRTUAL_ADDRESS
                    .as_mut_ptr::<u8>()
                    .add(offset as usize)
                    .cast::<u32>()
                    .write_volatile(value as u32);
            },
        }
    }

    fn end_of_interrupt(&self) {
        self.write(APIC_EOI, 0);
    }
}

#[derive(Debug)]
pub enum LocalApicError {
    AddressMismatch,
    ApicDisabled,
    FailedToMapApic,
    NotBootSystemProcessor,
}

#[derive(Debug)]
pub struct LocalApic {
    id: u32,
    access: LocalApicAccess,
}

const IA32_APIC_BASE: u32 = 0x1B;

const APIC_BASE_BSP: u64 = 1 << 8;
const APIC_BASE_X2APIC_ENABLE: u64 = 1 << 10;
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;
// TODO: use MAXPHYADDR
const APIC_BASE_ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

impl LocalApic {
    pub fn init(
        local_apic_address: PhysicalAddr,
        allocator: &mut FrameAllocator,
        address_space: &mut AddressSpace,
    ) -> Result<Self, LocalApicError> {
        let apic_base = unsafe { read_msr(IA32_APIC_BASE) };
        let bsp = (apic_base & APIC_BASE_BSP) != 0;
        let x2apix = (apic_base & APIC_BASE_X2APIC_ENABLE) != 0;
        let enabled = (apic_base & APIC_BASE_GLOBAL_ENABLE) != 0;
        let xapic_physical_addr = PhysicalAddr::new((apic_base & APIC_BASE_ADDRESS_MASK) as usize);

        if local_apic_address != xapic_physical_addr {
            return Err(LocalApicError::AddressMismatch);
        }

        if !enabled {
            return Err(LocalApicError::ApicDisabled);
        }

        if !bsp {
            return Err(LocalApicError::NotBootSystemProcessor);
        }

        let access = match x2apix {
            true => LocalApicAccess::X2Apic,
            false => {
                address_space
                    .map(
                        local_apic_address,
                        LOCAL_APIC_VIRTUAL_ADDRESS,
                        PagePermissions::new(true, false, false),
                        allocator,
                        CachePolicy::Uncacheable,
                    )
                    .map_err(|_| LocalApicError::FailedToMapApic)?;

                LocalApicAccess::XApic
            }
        };

        let raw_id = access.read(APIC_ID);

        let id = match access {
            LocalApicAccess::XApic => (raw_id >> 24) as u32,
            LocalApicAccess::X2Apic => raw_id as u32,
        };

        // ensure legacy PIC is disabled
        unsafe {
            write_u8(0x21, 0xFF);
            write_u8(0xA1, 0xFF);
        }

        access.write(APIC_LVT_ERROR, APIC_ERROR_VECTOR as u64 | APIC_LVT_MASKED);
        access.write(APIC_ESR, 0);
        let _ = access.read(APIC_ESR);
        access.write(APIC_TPR, 0);
        access.write(APIC_SVR, (1 << 8) | APIC_SPURIOUS_VECTOR as u64);

        Ok(Self { id, access })
    }

    pub fn start_timer(&self) {
        self.access.write(APIC_LVT_TIMER, APIC_TIMER_VECTOR as u64);
    }

    pub fn start_calibration_counter(&self) {
        self.access
            .write(APIC_LVT_TIMER, APIC_TIMER_VECTOR as u64 | APIC_LVT_MASKED);

        self.access.write(APIC_TIMER_DIVIDE_CONFIG, 0b11);
        self.access.write(APIC_TIMER_INITIAL_COUNT, u32::MAX as u64);
    }

    pub fn stop_timer(&self) {
        self.access
            .write(APIC_LVT_TIMER, APIC_TIMER_VECTOR as u64 | APIC_LVT_MASKED);
        self.access.write(APIC_TIMER_INITIAL_COUNT, 0);
    }

    // pub fn delay_ticks(&self, count: u32) {
    //     if count == 0 {
    //         return;
    //     }

    //     APIC_TIMER_COUNT.store(0, Ordering::SeqCst);

    //     self.access
    //         .write(APIC_LVT_TIMER, APIC_TIMER_VECTOR as u64 | APIC_LVT_MASKED);
    //     self.access.write(APIC_TIMER_DIVIDE_CONFIG, 0b11);
    //     self.access.write(APIC_TIMER_INITIAL_COUNT, count as u64);
    //     self.access.write(APIC_LVT_TIMER, APIC_TIMER_VECTOR as u64);

    //     while APIC_TIMER_COUNT.load(Ordering::SeqCst) == 0 {
    //         unsafe {
    //             core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack));
    //         }
    //     }

    //     self.stop_timer();
    // }

    pub fn id(&self) -> u32 {
        self.id
    }
}

fn current_access() -> LocalApicAccess {
    let apic_base = unsafe { read_msr(IA32_APIC_BASE) };

    if apic_base & APIC_BASE_X2APIC_ENABLE != 0 {
        LocalApicAccess::X2Apic
    } else {
        LocalApicAccess::XApic
    }
}

pub fn current_timer_count() -> u32 {
    current_access().read(APIC_TIMER_CURRENT_COUNT) as u32
}

pub(super) fn end_of_interrupt() {
    current_access().end_of_interrupt();
}

static APIC_ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_APIC_ERROR: AtomicU32 = AtomicU32::new(0);

pub(super) fn record_error() {
    let access = current_access();

    access.write(APIC_ESR, 0);
    let error = access.read(APIC_ESR) as u32;

    LAST_APIC_ERROR.store(error, Ordering::SeqCst);
    APIC_ERROR_COUNT.fetch_add(1, Ordering::SeqCst);
}

static APIC_TIMER_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn record_timer() {
    APIC_TIMER_COUNT.fetch_add(1, Ordering::SeqCst);
}
