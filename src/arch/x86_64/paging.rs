use core::arch::asm;

use crate::{
    arch::x86_64::cpu::CpuFeatures,
    memory::{
        DirectMap, FrameAllocator, PagePermissions, PhysicalAddr, PhysicalFrame, VirtualAddr,
    },
};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct PagingConfig {
    physical_address_bits: u8,
    nx_enabled: bool,
    mode: PagingMode,
}

impl PagingConfig {
    pub fn from_features(features: CpuFeatures) -> Self {
        Self {
            physical_address_bits: features.physical_address_bits,
            nx_enabled: features.nx_enabled,
            mode: PagingMode::FourLevel,
        }
    }

    pub const fn physical_address_mask(&self) -> usize {
        ((1 << self.physical_address_bits) - 1) & !0xFFF
    }

    pub const fn physical_address_limit(&self) -> usize {
        1 << self.physical_address_bits
    }
}

#[derive(Clone, Copy, Debug)]
enum PagingMode {
    FourLevel,
    FiveLevel,
}

impl PagingMode {
    const fn virtual_address_bits(&self) -> u32 {
        match self {
            PagingMode::FourLevel => 48,
            PagingMode::FiveLevel => 57,
        }
    }
}

#[derive(Debug)]
enum PageTableEntryError {
    PhysicalAddressTooLarge,
    NoExecuteUnsupported,
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageTableEntry(u64);

impl PageTableEntry {
    const PRESENT: u64 = 1 << 0;
    const WRITABLE: u64 = 1 << 1;
    const USER_ACCESSIBLE: u64 = 1 << 2;
    const HUGE_PAGE: u64 = 1 << 7;
    const NX: u64 = 1 << 63;

    const fn new(
        physical_address: PhysicalAddr,
        permissions: PagePermissions,
        config: PagingConfig,
    ) -> Result<Self, PageTableEntryError> {
        if physical_address.as_usize() >= config.physical_address_limit() {
            return Err(PageTableEntryError::PhysicalAddressTooLarge);
        }

        let mut value = physical_address.as_usize() as u64 | Self::PRESENT;

        if permissions.writable {
            value |= Self::WRITABLE;
        }
        if permissions.user_accessible {
            value |= Self::USER_ACCESSIBLE;
        }
        if !permissions.executable {
            if !config.nx_enabled {
                return Err(PageTableEntryError::NoExecuteUnsupported);
            }

            value |= Self::NX;
        }

        Ok(Self(value))
    }

    fn new_table(
        frame: PhysicalFrame,
        user_accessible: bool,
        config: PagingConfig,
    ) -> Result<Self, PageTableEntryError> {
        if frame.start_address().as_usize() >= config.physical_address_limit() {
            return Err(PageTableEntryError::PhysicalAddressTooLarge);
        }

        let mut value = frame.start_address().as_usize() as u64 | Self::PRESENT | Self::WRITABLE;
        if user_accessible {
            value |= Self::USER_ACCESSIBLE;
        }

        Ok(Self(value))
    }

    const fn null() -> Self {
        Self(0)
    }

    fn physical_address(&self, config: PagingConfig) -> PhysicalAddr {
        PhysicalAddr::new(self.0 as usize & config.physical_address_mask())
    }

    fn is_present(&self) -> bool {
        self.0 & Self::PRESENT != 0
    }

    fn is_user_accessible(&self) -> bool {
        self.0 & Self::USER_ACCESSIBLE != 0
    }

    fn is_huge(&self) -> bool {
        self.0 & Self::HUGE_PAGE != 0
    }

    fn frame(&self, config: PagingConfig) -> Option<PhysicalFrame> {
        if !self.is_present() || self.is_huge() {
            return None;
        }

        PhysicalFrame::from_start_address(self.physical_address(config))
    }
}

#[derive(Debug)]
pub(crate) enum MapError {
    InvalidVirtualAddress,
    VirtualAddressUnaligned,
    PhysicalAddressTooLarge,
    PageAlreadyMapped,
    HugePageConflict,
    NoExecuteUnsupported,
    OutOfFrames,
    PageTableOutsideDirectMap,
    InvalidPageTableEntry,
}

#[derive(Debug)]
pub(crate) enum UnmapError {
    InvalidVirtualAddress,
    VirtualAddressUnaligned,
    PageNotMapped,
    HugePageConflict,
    PageTableOutsideDirectMap,
    InvalidPageTableEntry,
}

#[derive(Clone, Copy)]
struct NewTable {
    parent: PhysicalFrame,
    index: usize,
    child: PhysicalFrame,
}

#[derive(Debug)]
pub(crate) enum PageTableCreateError {
    PhysicalAddressTooLarge,
    OutOfFrames,
}

pub struct PageTable {
    frame: PhysicalFrame,
    direct_map: DirectMap,
    config: PagingConfig,
}

impl PageTable {
    pub fn new(
        direct_map: DirectMap,
        config: PagingConfig,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, PageTableCreateError> {
        let frame = allocator.alloc().ok_or(PageTableCreateError::OutOfFrames)?;

        if frame.start_address().as_usize() >= config.physical_address_limit() {
            unsafe { allocator.dealloc(frame) };

            return Err(PageTableCreateError::PhysicalAddressTooLarge);
        }

        Ok(Self {
            frame,
            direct_map,
            config,
        })
    }

    fn is_active(&self) -> bool {
        let cr3 = unsafe { read_cr3(self.config) };

        cr3.start_address() == self.frame.start_address()
    }

    fn table(&self, frame: PhysicalFrame) -> Option<&[PageTableEntry; PAGE_TABLE_ENTRIES]> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &*(virtual_addr.as_ptr::<[PageTableEntry; PAGE_TABLE_ENTRIES]>()) })
    }

    fn table_mut(
        &mut self,
        frame: PhysicalFrame,
    ) -> Option<&mut [PageTableEntry; PAGE_TABLE_ENTRIES]> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &mut *(virtual_addr.as_mut_ptr::<[PageTableEntry; PAGE_TABLE_ENTRIES]>()) })
    }

    fn is_canonical(&self, addr: usize) -> bool {
        let bits = self.config.mode.virtual_address_bits();
        let shift = usize::BITS - bits;

        (((addr << shift) as isize >> shift) as usize) == addr
    }

    pub fn translate(&self, addr: VirtualAddr) -> Option<PhysicalAddr> {
        let addr = addr.as_usize();

        if !self.is_canonical(addr) {
            return None;
        }

        let p4 = self.table(self.frame)?;
        let p4_entry = p4[p4_index(addr)];

        if !p4_entry.is_present() {
            return None;
        }

        let p3 = self.table(p4_entry.frame(self.config)?)?;
        let p3_entry = p3[p3_index(addr)];

        if !p3_entry.is_present() {
            return None;
        }

        if p3_entry.is_huge() {
            return translate_huge_page(p3_entry, addr, 1 << 30, self.config);
        }

        let p2 = self.table(p3_entry.frame(self.config)?)?;
        let p2_entry = p2[p2_index(addr)];

        if !p2_entry.is_present() {
            return None;
        }

        if p2_entry.is_huge() {
            return translate_huge_page(p2_entry, addr, 1 << 21, self.config);
        }

        let p1 = self.table(p2_entry.frame(self.config)?)?;
        let p1_entry = p1[p1_index(addr)];

        if !p1_entry.is_present() {
            return None;
        }

        let physical_base = p1_entry.physical_address(self.config).as_usize();

        physical_base
            .checked_add(page_offset(addr))
            .map(PhysicalAddr::new)
    }

    fn get_next_level(
        &self,
        parent: PhysicalFrame,
        index: usize,
    ) -> Result<PhysicalFrame, UnmapError> {
        let parent_table = self
            .table(parent)
            .ok_or(UnmapError::PageTableOutsideDirectMap)?;

        // TODO: encode the level so bit 7 is only interpreted where huge pages are valid.
        if parent_table[index].is_huge() {
            return Err(UnmapError::HugePageConflict);
        }

        parent_table[index]
            .frame(self.config)
            .ok_or(UnmapError::PageNotMapped)
    }

    fn get_next_level_or_allocate(
        &mut self,
        parent: PhysicalFrame,
        index: usize,
        user_accessible: bool,
        allocator: &mut FrameAllocator,
    ) -> Result<(PhysicalFrame, bool), MapError> {
        let config = self.config;

        let parent_table = self
            .table_mut(parent)
            .ok_or(MapError::PageTableOutsideDirectMap)?;

        let entry = &mut parent_table[index];

        if !entry.is_present() {
            let frame = allocator.alloc().ok_or(MapError::OutOfFrames)?;
            let table = match PageTableEntry::new_table(frame, user_accessible, config) {
                Ok(table) => table,
                Err(PageTableEntryError::PhysicalAddressTooLarge) => {
                    unsafe { allocator.dealloc(frame) };
                    return Err(MapError::PhysicalAddressTooLarge);
                }
                Err(PageTableEntryError::NoExecuteUnsupported) => unreachable!(),
            };
            *entry = table;
            return Ok((frame, true));
        }

        if entry.is_huge() {
            return Err(MapError::HugePageConflict);
        }

        // TODO: we upgrade intermediate entries, and dont carefully rollback if we fail
        if user_accessible && !entry.is_user_accessible() {
            entry.0 |= PageTableEntry::USER_ACCESSIBLE;
        }

        entry
            .frame(config)
            .map(|frame| (frame, false))
            .ok_or(MapError::InvalidPageTableEntry)
    }

    fn rollback_tables(
        &mut self,
        new_tables: &[Option<NewTable>],
        count: usize,
        allocator: &mut FrameAllocator,
    ) {
        for table in new_tables[..count].iter().rev().flatten() {
            self.table_mut(table.parent).unwrap()[table.index] = PageTableEntry::null();

            unsafe { allocator.dealloc(table.child) };
        }
    }

    pub fn map(
        &mut self,
        mapped_addr: VirtualAddr,
        frame: PhysicalFrame,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if !self.is_canonical(mapped_addr.as_usize()) {
            return Err(MapError::InvalidVirtualAddress);
        }

        if mapped_addr.as_usize() % PAGE_SIZE != 0 {
            return Err(MapError::VirtualAddressUnaligned);
        }

        if frame.start_address().as_usize() >= self.config.physical_address_limit() {
            return Err(MapError::PhysicalAddressTooLarge);
        }

        let mut new_tables: [Option<NewTable>; 3] = [None; 3];
        let mut new_table_count = 0;

        let result = (|| {
            let p4_entry_index = p4_index(mapped_addr.as_usize());
            let (pdpt_frame, allocated) = self.get_next_level_or_allocate(
                self.frame,
                p4_entry_index,
                permissions.user_accessible,
                allocator,
            )?;
            if allocated {
                new_tables[new_table_count] = Some(NewTable {
                    parent: self.frame,
                    index: p4_entry_index,
                    child: pdpt_frame,
                });
                new_table_count += 1;
            }

            let p3_entry_index = p3_index(mapped_addr.as_usize());
            let (pd_frame, allocated) = self.get_next_level_or_allocate(
                pdpt_frame,
                p3_entry_index,
                permissions.user_accessible,
                allocator,
            )?;
            if allocated {
                new_tables[new_table_count] = Some(NewTable {
                    parent: pdpt_frame,
                    index: p3_entry_index,
                    child: pd_frame,
                });
                new_table_count += 1;
            }

            let p2_entry_index = p2_index(mapped_addr.as_usize());
            let (pt_frame, allocated) = self.get_next_level_or_allocate(
                pd_frame,
                p2_entry_index,
                permissions.user_accessible,
                allocator,
            )?;
            if allocated {
                new_tables[new_table_count] = Some(NewTable {
                    parent: pd_frame,
                    index: p2_entry_index,
                    child: pt_frame,
                });
                new_table_count += 1;
            }

            let config = self.config.clone();

            let pt_table = self
                .table_mut(pt_frame)
                .ok_or(MapError::PageTableOutsideDirectMap)?;
            let entry = &mut pt_table[p1_index(mapped_addr.as_usize())];
            if entry.is_present() {
                return Err(MapError::PageAlreadyMapped);
            }

            *entry =
                PageTableEntry::new(frame.start_address(), permissions, config).map_err(|err| {
                    match err {
                        PageTableEntryError::PhysicalAddressTooLarge => {
                            MapError::PhysicalAddressTooLarge
                        }
                        PageTableEntryError::NoExecuteUnsupported => MapError::NoExecuteUnsupported,
                    }
                })?;
            Ok(())
        })();

        if result.is_err() {
            self.rollback_tables(&new_tables, new_table_count, allocator);
            return result;
        }

        self.flush_tlb_if_active(mapped_addr);

        Ok(())
    }

    /// # Safety
    ///
    /// The caller must ensure:
    /// - The page being unmapped does not unmap the HHDM, kernel image, or stack
    /// - The caller must ensure that the page is not currently in use
    pub unsafe fn unmap(
        &mut self,
        mapped_addr: VirtualAddr,
        allocator: &mut FrameAllocator,
    ) -> Result<PhysicalFrame, UnmapError> {
        if !self.is_canonical(mapped_addr.as_usize()) {
            return Err(UnmapError::InvalidVirtualAddress);
        }

        if mapped_addr.as_usize() % PAGE_SIZE != 0 {
            return Err(UnmapError::VirtualAddressUnaligned);
        }

        let config = self.config;

        let pdpt_frame = self.get_next_level(self.frame, p4_index(mapped_addr.as_usize()))?;

        let pd_frame = self.get_next_level(pdpt_frame, p3_index(mapped_addr.as_usize()))?;

        let pt_frame = self.get_next_level(pd_frame, p2_index(mapped_addr.as_usize()))?;
        let pt_table = self
            .table_mut(pt_frame)
            .ok_or(UnmapError::PageTableOutsideDirectMap)?;

        let entry = pt_table[p1_index(mapped_addr.as_usize())];

        if !entry.is_present() {
            return Err(UnmapError::PageNotMapped);
        }

        let frame = entry
            .frame(config)
            .ok_or(UnmapError::InvalidPageTableEntry)?;
        pt_table[p1_index(mapped_addr.as_usize())] = PageTableEntry::null();

        if pt_table.is_empty() {
            self.table_mut(pd_frame).unwrap()[p2_index(mapped_addr.as_usize())] =
                PageTableEntry::null();
            unsafe { allocator.dealloc(pt_frame) };

            if self.table(pd_frame).unwrap().is_empty() {
                self.table_mut(pdpt_frame).unwrap()[p3_index(mapped_addr.as_usize())] =
                    PageTableEntry::null();
                unsafe { allocator.dealloc(pd_frame) };

                if self.table(pdpt_frame).unwrap().is_empty() {
                    self.table_mut(self.frame).unwrap()[p4_index(mapped_addr.as_usize())] =
                        PageTableEntry::null();
                    unsafe { allocator.dealloc(pdpt_frame) };
                }
            }
        }

        self.flush_tlb_if_active(mapped_addr);

        Ok(frame)
    }

    fn flush_tlb_if_active(&self, page: VirtualAddr) {
        debug_assert!(self.is_canonical(page.as_usize()));

        if self.is_active() {
            unsafe {
                asm!("invlpg [{}]", in(reg) page.as_usize(), options(nostack, preserves_flags));
            }
        }
    }

    /// # Safety
    ///
    /// The caller must ensure:
    /// - The new space must map the kernel image, stack, and the code being executed
    /// - The new space must map the HHDM
    pub unsafe fn activate(&self) {
        unsafe {
            asm!(
                "mov cr3, {}",
                in(reg) self.frame.start_address().as_usize(),
                options(nostack, preserves_flags)
            );
        };
    }
}

fn translate_huge_page(
    entry: PageTableEntry,
    virtual_addr: usize,
    page_size: usize,
    config: PagingConfig,
) -> Option<PhysicalAddr> {
    let physical_base = entry.physical_address(config).as_usize() & !(page_size - 1);

    let offset = virtual_addr & (page_size - 1);

    physical_base.checked_add(offset).map(PhysicalAddr::new)
}

fn p4_index(addr: usize) -> usize {
    (addr >> 39) & 0x1FF
}

fn p3_index(addr: usize) -> usize {
    (addr >> 30) & 0x1FF
}

fn p2_index(addr: usize) -> usize {
    (addr >> 21) & 0x1FF
}

fn p1_index(addr: usize) -> usize {
    (addr >> 12) & 0x1FF
}

fn page_offset(addr: usize) -> usize {
    addr & 0xFFF
}

unsafe fn read_cr3(config: PagingConfig) -> PhysicalFrame {
    let value: usize;
    unsafe {
        asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    PhysicalFrame::from_start_address(PhysicalAddr::new(value & config.physical_address_mask()))
        .expect("CR3 contains an unaligned page-table address")
}
