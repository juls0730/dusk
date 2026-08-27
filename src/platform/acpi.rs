use crate::memory::{DirectMap, PhysicalAddr, VirtualAddr};

#[derive(Debug)]
pub enum AcpiError {
    InvalidInput,
    AddressOverflow,
    InvalidSdtLength,
    InvalidRootTableLength,
    MalformedAcpiTable,
    MalformedMadt,
    MissingIoApic,
    MultipleIoApicsUnsupported,
}

#[derive(Debug)]
pub struct AcpiTables {
    direct_map: DirectMap,
    root: RootTable,
}

impl AcpiTables {
    pub fn madt(&self) -> Result<Option<Madt<'_>>, AcpiError> {
        if let Some(table) = self.find_table(*b"APIC")? {
            return Ok(Some(Madt::parse(self, table)?));
        };

        return Ok(None);
    }

    fn read<T: Copy>(&self, physical_addr: PhysicalAddr) -> Result<T, AcpiError> {
        let virtual_addr = self
            .direct_map
            .translate(physical_addr)
            .ok_or(AcpiError::AddressOverflow)?;

        Ok(unsafe { core::ptr::read_unaligned(virtual_addr.as_ptr::<T>()) })
    }

    fn checksum_valid(
        &self,
        physical_addr: PhysicalAddr,
        length: usize,
    ) -> Result<bool, AcpiError> {
        let virtual_addr = self
            .direct_map
            .translate(physical_addr)
            .ok_or(AcpiError::AddressOverflow)?;

        let mut sum: u8 = 0;
        for i in 0..length {
            let byte_addr = virtual_addr
                .as_usize()
                .checked_add(i)
                .ok_or(AcpiError::AddressOverflow)?;

            let byte = unsafe { core::ptr::read_unaligned(byte_addr as *const u8) };
            sum = sum.wrapping_add(byte);
        }

        Ok(sum == 0)
    }

    pub unsafe fn from_rsdp(
        rsdp_ptr: VirtualAddr,
        direct_map: DirectMap,
    ) -> Result<Self, AcpiError> {
        let rsdp = unsafe { core::ptr::read_unaligned(rsdp_ptr.as_ptr::<Rsdp>()) };

        let mut sum: u8 = 0;
        for i in 0..core::mem::size_of::<Rsdp>() as usize {
            sum = sum.wrapping_add(unsafe {
                core::ptr::read_unaligned::<u8>(rsdp_ptr.as_ptr::<u8>().add(i))
            });
        }

        if sum != 0 {
            return Err(AcpiError::MalformedAcpiTable);
        }

        if rsdp.signature != *b"RSD PTR " {
            return Err(AcpiError::MalformedAcpiTable);
        }

        let root_table = if rsdp.revision >= 2 {
            let xsdp = unsafe { core::ptr::read_unaligned(rsdp_ptr.as_ptr::<Xsdp>()) };

            if xsdp.length < core::mem::size_of::<Xsdp>() as u32 {
                return Err(AcpiError::MalformedAcpiTable);
            }

            sum = 0;
            for i in 0..xsdp.length as usize {
                sum = sum.wrapping_add(unsafe {
                    core::ptr::read_unaligned::<u8>(rsdp_ptr.as_ptr::<u8>().add(i))
                });
            }

            if sum != 0 {
                return Err(AcpiError::MalformedAcpiTable);
            }

            let xsdt_address = PhysicalAddr::new(xsdp.xsdt_address as usize);
            let xsdt_virtual_addr = unsafe {
                direct_map
                    .translate(xsdt_address)
                    .ok_or(AcpiError::AddressOverflow)?
                    .as_ptr::<SDTHeader>()
            };
            let xsdt = unsafe { core::ptr::read_unaligned(xsdt_virtual_addr) };

            sum = 0;
            for i in 0..xsdt.length as usize {
                sum = sum.wrapping_add(unsafe {
                    core::ptr::read_unaligned::<u8>(xsdt_virtual_addr.cast::<u8>().add(i))
                });
            }

            if sum != 0 {
                return Err(AcpiError::MalformedAcpiTable);
            }

            if &xsdt.signature != b"XSDT" {
                return Err(AcpiError::MalformedAcpiTable);
            }

            RootTable::Xsdt(Sdt {
                physical_addr: xsdt_address,
                length: xsdt.length as usize,
                signature: xsdt.signature,
            })
        } else {
            let rsdt_address = PhysicalAddr::new(rsdp.rsdt_address as usize);
            let rsdt_virtual_addr = unsafe {
                direct_map
                    .translate(rsdt_address)
                    .ok_or(AcpiError::AddressOverflow)?
                    .as_ptr::<SDTHeader>()
            };

            let rsdt = unsafe { core::ptr::read_unaligned(rsdt_virtual_addr) };

            sum = 0;
            for i in 0..rsdt.length as usize {
                sum = sum.wrapping_add(unsafe {
                    core::ptr::read_unaligned::<u8>(rsdt_virtual_addr.cast::<u8>().add(i))
                });
            }

            if sum != 0 {
                return Err(AcpiError::MalformedAcpiTable);
            }

            if &rsdt.signature != b"RSDT" {
                return Err(AcpiError::MalformedAcpiTable);
            }

            RootTable::Rsdt(Sdt {
                physical_addr: rsdt_address,
                length: rsdt.length as usize,
                signature: rsdt.signature,
            })
        };

        Ok(Self {
            direct_map: direct_map,
            root: root_table,
        })
    }

    pub fn find_table(&self, signature: [u8; 4]) -> Result<Option<Sdt>, AcpiError> {
        let root = self.root.table();
        let entry_width = self.root.entry_width();
        let entries_start = root
            .physical_addr
            .as_usize()
            .checked_add(size_of::<SDTHeader>())
            .ok_or(AcpiError::AddressOverflow)?;

        for i in 0..self.root.entry_count()? {
            let entry_addr = entries_start
                .checked_add(
                    i.checked_mul(entry_width)
                        .ok_or(AcpiError::AddressOverflow)?,
                )
                .ok_or(AcpiError::AddressOverflow)?;

            let table_addr = match self.root {
                RootTable::Rsdt(_) => self.read::<u32>(PhysicalAddr::new(entry_addr))? as usize,
                RootTable::Xsdt(_) => self.read::<u64>(PhysicalAddr::new(entry_addr))? as usize,
            };

            let physical_addr = PhysicalAddr::new(table_addr);
            let header = self.read::<SDTHeader>(physical_addr)?;

            if header.length < size_of::<SDTHeader>() as u32 {
                return Err(AcpiError::InvalidSdtLength);
            }

            if header.signature != signature {
                continue;
            }

            if !self.checksum_valid(physical_addr, header.length as usize)? {
                return Err(AcpiError::MalformedAcpiTable);
            }

            return Ok(Some(Sdt {
                physical_addr,
                length: header.length as usize,
                signature: header.signature,
            }));
        }

        Ok(None)
    }
}

#[derive(Debug)]
enum RootTable {
    Rsdt(Sdt),
    Xsdt(Sdt),
}

impl RootTable {
    fn table(&self) -> &Sdt {
        match self {
            RootTable::Rsdt(sdt) | RootTable::Xsdt(sdt) => sdt,
        }
    }

    const fn entry_width(&self) -> usize {
        match self {
            RootTable::Rsdt(_) => core::mem::size_of::<u32>(),
            RootTable::Xsdt(_) => core::mem::size_of::<u64>(),
        }
    }

    fn entry_count(&self) -> Result<usize, AcpiError> {
        let payload_length = self
            .table()
            .length
            .checked_sub(size_of::<SDTHeader>())
            .ok_or(AcpiError::InvalidSdtLength)?;

        if payload_length % self.entry_width() != 0 {
            return Err(AcpiError::InvalidRootTableLength);
        }

        Ok(payload_length / self.entry_width())
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
struct Rsdp {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
struct Xsdp {
    rsdp: Rsdp,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
struct SDTHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[derive(Debug)]
pub struct Sdt {
    physical_addr: PhysicalAddr,
    length: usize,
    signature: [u8; 4],
}

#[derive(Debug)]
#[allow(unused)]
pub struct Madt<'a> {
    acpi: &'a AcpiTables,
    table: Sdt,
    pub local_apic_address: PhysicalAddr,
    flags: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
struct MadtBody {
    local_apic_address: u32,
    flags: u32,
}

const MADT_ENTRIES_OFFSET: usize = size_of::<SDTHeader>() + size_of::<MadtBody>();

impl<'a> Madt<'a> {
    pub fn entries(
        &self,
    ) -> Result<impl Iterator<Item = Result<MadtEntry, AcpiError>> + '_, AcpiError> {
        Ok(MadtEntries {
            acpi: self.acpi,
            current: PhysicalAddr::new(
                self.table
                    .physical_addr
                    .as_usize()
                    .checked_add(MADT_ENTRIES_OFFSET)
                    .ok_or(AcpiError::AddressOverflow)?,
            ),
            end: PhysicalAddr::new(
                self.table
                    .physical_addr
                    .as_usize()
                    .checked_add(self.table.length)
                    .ok_or(AcpiError::AddressOverflow)?,
            ),
        })
    }

    pub fn parse(acpi: &'a AcpiTables, table: Sdt) -> Result<Self, AcpiError> {
        if table.signature != *b"APIC" {
            return Err(AcpiError::InvalidInput);
        }

        if table.length < size_of::<SDTHeader>() + size_of::<MadtBody>() {
            return Err(AcpiError::MalformedAcpiTable);
        }

        let body_addr = table
            .physical_addr
            .as_usize()
            .checked_add(size_of::<SDTHeader>())
            .ok_or(AcpiError::AddressOverflow)?;
        let body = acpi.read::<MadtBody>(PhysicalAddr::new(body_addr))?;

        Ok(Madt {
            acpi,
            table,
            local_apic_address: PhysicalAddr::new(body.local_apic_address as usize),
            flags: body.flags,
        })
    }

    pub fn effective_local_apic_address(&self) -> Result<PhysicalAddr, AcpiError> {
        let mut address = self.local_apic_address;

        for result in self.entries()? {
            let entry = result?;

            match entry {
                MadtEntry::LocalApicAddressOverride(local_apic_address_override) => {
                    address =
                        PhysicalAddr::new(local_apic_address_override.local_apic_address as usize);
                }
                _ => {}
            }
        }

        Ok(address)
    }

    pub fn io_apics(
        &self,
    ) -> Result<impl Iterator<Item = Result<IoApicInfo, AcpiError>> + '_, AcpiError> {
        Ok(self.entries()?.filter_map(|result| match result {
            Ok(MadtEntry::IoApic(io_apic)) => Some(Ok(IoApicInfo {
                id: io_apic.id,
                apic_address: PhysicalAddr::new(io_apic.apic_address as usize),
                global_system_interrupt_base: io_apic.global_system_interrupt_base,
            })),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        }))
    }

    pub fn sole_io_apic(&self) -> Result<IoApicInfo, AcpiError> {
        let mut entries = self.io_apics()?;

        let first = entries
            .next()
            .transpose()?
            .ok_or(AcpiError::MissingIoApic)?;

        if entries.next().transpose()?.is_some() {
            return Err(AcpiError::MultipleIoApicsUnsupported);
        }

        Ok(first)
    }

    pub fn isa_irq_route(&self, irq: u8) -> Result<IsaIrqRoute, AcpiError> {
        for entry in self.entries()? {
            match entry? {
                MadtEntry::InterruptSourceOverride(interrupt_source_override) => {
                    if interrupt_source_override.bus != 0 {
                        continue;
                    }

                    if interrupt_source_override.source != irq {
                        continue;
                    }

                    return Ok(IsaIrqRoute {
                        gsi: interrupt_source_override.global_interrupt,
                        polarity: match interrupt_source_override.flags & 0b11 {
                            0 => InterruptPolarity::ActiveHigh,
                            1 => InterruptPolarity::ActiveHigh,
                            3 => InterruptPolarity::ActiveLow,
                            _ => Err(AcpiError::MalformedMadt)?,
                        },
                        trigger: match interrupt_source_override.flags >> 2 & 0b11 {
                            0 => TriggerMode::Edge,
                            1 => TriggerMode::Edge,
                            3 => TriggerMode::Level,
                            _ => Err(AcpiError::MalformedMadt)?,
                        },
                    });
                }
                _ => {}
            }
        }

        Ok(IsaIrqRoute {
            gsi: irq as u32,
            polarity: InterruptPolarity::ActiveHigh,
            trigger: TriggerMode::Edge,
        })
    }
}

pub struct MadtEntries<'a> {
    acpi: &'a AcpiTables,
    current: PhysicalAddr,
    end: PhysicalAddr,
}

impl MadtEntries<'_> {
    fn read_body<T: Copy>(&self, header: &MadtEntryHeader) -> Result<T, AcpiError> {
        let required_length = size_of::<MadtEntryHeader>() + size_of::<T>();

        if (header.length as usize) < required_length {
            return Err(AcpiError::MalformedAcpiTable);
        }

        let body_addr = self
            .current
            .as_usize()
            .checked_add(size_of::<MadtEntryHeader>())
            .ok_or(AcpiError::AddressOverflow)?;

        self.acpi.read::<T>(PhysicalAddr::new(body_addr))
    }

    fn read_next(&mut self) -> Result<MadtEntry, AcpiError> {
        let header = self.acpi.read::<MadtEntryHeader>(self.current)?;

        let entry = match header.kind {
            0 => MadtEntry::LocalApic(self.read_body(&header)?),
            1 => MadtEntry::IoApic(self.read_body(&header)?),
            2 => MadtEntry::InterruptSourceOverride(self.read_body(&header)?),
            3 => MadtEntry::IoApicNmi(self.read_body(&header)?),
            4 => MadtEntry::LocalApicNmi(self.read_body(&header)?),
            5 => MadtEntry::LocalApicAddressOverride(self.read_body(&header)?),
            9 => MadtEntry::LocalX2Apic(self.read_body(&header)?),
            _ => MadtEntry::Unknown {
                kind: header.kind,
                length: header.length,
            },
        };

        self.current = PhysicalAddr::new(
            self.current
                .as_usize()
                .checked_add(header.length as usize)
                .ok_or(AcpiError::AddressOverflow)?,
        );

        Ok(entry)
    }
}

impl<'a> Iterator for MadtEntries<'a> {
    type Item = Result<MadtEntry, AcpiError>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.as_usize();
        let end = self.end.as_usize();

        if current == end {
            return None;
        }

        if current > end {
            self.current = self.end;
            return Some(Err(AcpiError::MalformedAcpiTable));
        }

        let remaining = end - current;

        if remaining < size_of::<MadtEntryHeader>() {
            self.current = self.end;
            return Some(Err(AcpiError::MalformedAcpiTable));
        }

        let header = match self.acpi.read::<MadtEntryHeader>(self.current) {
            Ok(header) => header,
            Err(error) => {
                self.current = self.end;
                return Some(Err(error));
            }
        };

        let length = header.length as usize;

        if length < size_of::<MadtEntryHeader>() || length > remaining {
            self.current = self.end;
            return Some(Err(AcpiError::MalformedAcpiTable));
        }

        let res = self.read_next();

        if res.is_err() {
            self.current = self.end;
        }

        Some(res)
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct MadtEntryHeader {
    kind: u8,
    length: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LocalApicEntry {
    processor_id: u8,
    id: u8,
    flags: u32,
}

#[derive(Debug)]
pub struct IoApicInfo {
    pub id: u8,
    pub apic_address: PhysicalAddr,
    pub global_system_interrupt_base: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct IoApicEntry {
    id: u8,
    reserved: u8,
    apic_address: u32,
    global_system_interrupt_base: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct InterruptSourceOverride {
    bus: u8,
    source: u8,
    global_interrupt: u32,
    flags: u16,
}

#[derive(Clone, Copy, Debug)]
pub enum InterruptPolarity {
    ActiveHigh,
    ActiveLow,
}

#[derive(Clone, Copy, Debug)]
pub enum TriggerMode {
    Edge,
    Level,
}

#[derive(Clone, Copy, Debug)]
pub struct IsaIrqRoute {
    pub gsi: u32,
    pub polarity: InterruptPolarity,
    pub trigger: TriggerMode,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct IoApicNmiEntry {
    nmi_source: u8,
    reserved: u8,
    flags: u16,
    global_system_interrupt: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LocalApicNmiEntry {
    processor_id: u8,
    flags: u16,
    lint_num: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LocalApicAddressOverride {
    reserved: u16,
    local_apic_address: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct LocalX2ApicEntry {
    reserved: u16,
    local_x2apic_id: u32,
    flags: u32,
    acpi_processor_uid: u32,
}

#[derive(Clone, Copy, Debug)]
#[allow(unused)]
pub enum MadtEntry {
    LocalApic(LocalApicEntry),
    IoApic(IoApicEntry),
    InterruptSourceOverride(InterruptSourceOverride),
    IoApicNmi(IoApicNmiEntry),
    LocalApicNmi(LocalApicNmiEntry),
    LocalApicAddressOverride(LocalApicAddressOverride),
    LocalX2Apic(LocalX2ApicEntry),
    Unknown { kind: u8, length: u8 },
}

pub fn init(
    boot_info: &crate::boot::BootInfo,
    direct_map: DirectMap,
) -> Result<AcpiTables, AcpiError> {
    let acpi_table = unsafe { AcpiTables::from_rsdp(boot_info.rsdp, direct_map)? };

    Ok(acpi_table)
}
