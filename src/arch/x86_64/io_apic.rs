use crate::{
    memory::{
        AddressSpace, CachePolicy, FrameAllocator, PagePermissions, PhysicalAddr, VirtualAddr,
    },
    platform::acpi::{InterruptPolarity, TriggerMode},
    println,
};

const IOWIN: usize = 0x10;

const IOAPIC_ID: u8 = 0x00;
const IOAPIC_VERSION: u8 = 0x01;
const IOAPIC_REDIRECTION_BASE: u8 = 0x10;

const MASKED: u32 = 1 << 16;

pub const IOAPIC_VIRTUAL_ADDRESS: VirtualAddr = VirtualAddr::new(0xFFFF_FFFD_1000_0000);

#[derive(Debug)]
pub enum IoApicError {
    IdMismatch { expected: u8, actual: u8 },
    GsiOutsideRange,
    FailedToMapIoApic,
    InvalidRedirectionIndex,
}

pub struct RedirectionConfig {
    pub vector: u8,
    pub destination: u8,
    pub polarity: InterruptPolarity,
    pub trigger: TriggerMode,
}

#[derive(Debug)]
pub struct IoApic {
    base: VirtualAddr,
    global_interrupt_base: u32,
    redirection_entry_count: u32,
}

impl IoApic {
    pub fn new(
        expected_id: u8,
        physical_address: PhysicalAddr,
        global_interrupt_base: u32,
        virtual_address: VirtualAddr,
        allocator: &mut FrameAllocator,
        address_space: &mut AddressSpace,
    ) -> Result<Self, IoApicError> {
        address_space
            .map(
                physical_address,
                virtual_address,
                PagePermissions::new(true, false, false),
                allocator,
                CachePolicy::Uncacheable,
            )
            .map_err(|_| IoApicError::FailedToMapIoApic)?;

        let mut io_apic = Self {
            base: virtual_address,
            global_interrupt_base: global_interrupt_base,
            redirection_entry_count: 0,
        };

        let version = io_apic.read(IOAPIC_VERSION);
        io_apic.redirection_entry_count = ((version >> 16) & 0xFF) + 1;

        let id = ((io_apic.read(IOAPIC_ID) >> 24) & 0xF) as u8;
        if id != expected_id {
            return Err(IoApicError::IdMismatch {
                expected: expected_id,
                actual: id,
            });
        }
        println!("IOAPIC ID: {:#X}", id);
        println!("IOAPIC version: {:#X}", version & 0xFF);

        println!(
            "IOAPIC redirection entry count: {:#X}",
            io_apic.redirection_entry_count
        );

        Ok(io_apic)
    }

    fn read(&mut self, register: u8) -> u32 {
        unsafe {
            self.base
                .as_mut_ptr::<u32>()
                .write_volatile(register as u32);

            self.base
                .as_ptr::<u8>()
                .add(IOWIN)
                .cast::<u32>()
                .read_volatile()
        }
    }

    fn write(&mut self, register: u8, value: u32) {
        unsafe {
            self.base
                .as_mut_ptr::<u32>()
                .write_volatile(register as u32);

            self.base
                .as_mut_ptr::<u8>()
                .add(IOWIN)
                .cast::<u32>()
                .write_volatile(value);
        }
    }

    fn redirection_index(&self, gsi: u32) -> Result<u32, IoApicError> {
        let index = gsi
            .checked_sub(self.global_interrupt_base)
            .ok_or(IoApicError::GsiOutsideRange)?;

        if index >= self.redirection_entry_count {
            return Err(IoApicError::GsiOutsideRange);
        }

        Ok(index)
    }

    pub fn handles_gsi(&mut self, gsi: u32) -> bool {
        match gsi.checked_sub(self.global_interrupt_base) {
            Some(index) => index < self.redirection_entry_count,
            None => false,
        }
    }

    fn redirection_registers(&mut self, gsi: u32) -> Result<(u8, u8), IoApicError> {
        let index = self.redirection_index(gsi)?;
        let low_register = u8::try_from(IOAPIC_REDIRECTION_BASE as u32 + index * 2)
            .map_err(|_| IoApicError::InvalidRedirectionIndex)?;

        let high_register = u8::try_from(IOAPIC_REDIRECTION_BASE as u32 + index * 2 + 1)
            .map_err(|_| IoApicError::InvalidRedirectionIndex)?;

        Ok((low_register, high_register))
    }

    pub fn configure_masked(
        &mut self,
        gsi: u32,
        config: RedirectionConfig,
    ) -> Result<(), IoApicError> {
        let mut entry = config.vector as u64;

        match config.polarity {
            InterruptPolarity::ActiveHigh => {
                entry |= 0 << 13;
            }
            InterruptPolarity::ActiveLow => {
                entry |= 1 << 13;
            }
        }

        match config.trigger {
            TriggerMode::Edge => {
                entry |= 0 << 15;
            }
            TriggerMode::Level => {
                entry |= 1 << 15;
            }
        }

        entry |= 1 << 16;
        entry |= (config.destination as u64) << 56;

        let (low_register, high_register) = self.redirection_registers(gsi)?;

        let old_low = self.read(low_register);
        self.write(low_register, old_low | MASKED);

        self.write(high_register, (entry >> 32) as u32);
        self.write(low_register, entry as u32 | MASKED);

        Ok(())
    }

    pub fn unmask(&mut self, gsi: u32) -> Result<(), IoApicError> {
        let (low_register, _) = self.redirection_registers(gsi)?;
        let low = self.read(low_register);

        self.write(low_register, low & !MASKED);

        Ok(())
    }

    pub fn mask(&mut self, gsi: u32) -> Result<(), IoApicError> {
        let (low_register, _) = self.redirection_registers(gsi)?;
        let low = self.read(low_register);

        self.write(low_register, low | MASKED);

        Ok(())
    }
}
