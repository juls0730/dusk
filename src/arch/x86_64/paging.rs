use core::arch::asm;

use crate::{
    memory::{
        DirectMap, FrameAllocator, KernelImage, MemoryRegion, MemoryRegionKind, PhysicalAddr,
        PhysicalFrame, VirtualAddr,
    },
    println,
};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512;

const ADDRESS_MASK: usize = 0x000F_FFFF_FFFF_F000;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PageTableEntry(u64);

impl PageTableEntry {
    const PRESENT: u64 = 1 << 0;
    const WRITABLE: u64 = 1 << 1;
    const USER_ACCESSIBLE: u64 = 1 << 2;
    const LARGE_PAGE: u64 = 1 << 7;
    const NX: u64 = 1 << 63;

    // TODO: validate that page is withing the CPU's mappable address space
    const fn new(physical_address: PhysicalAddr, permissions: PagePermissions) -> Self {
        let mut value = physical_address.as_usize() as u64 | Self::PRESENT;

        if permissions.writable {
            value |= Self::WRITABLE;
        }
        if permissions.user_accessible {
            value |= Self::USER_ACCESSIBLE;
        }
        if !permissions.executable {
            value |= Self::NX;
        }

        Self(value)
    }

    fn new_table(frame: PhysicalFrame, user_accessible: bool) -> Self {
        let mut value = frame.start_address().as_usize() as u64 | Self::PRESENT | Self::WRITABLE;
        if user_accessible {
            value |= Self::USER_ACCESSIBLE;
        }

        Self(value)
    }

    const fn null() -> Self {
        Self(0)
    }

    fn physical_address(&self) -> PhysicalAddr {
        PhysicalAddr::new(self.0 as usize & ADDRESS_MASK)
    }

    fn is_present(&self) -> bool {
        self.0 & Self::PRESENT != 0
    }

    fn is_user_accessible(&self) -> bool {
        self.0 & Self::USER_ACCESSIBLE != 0
    }

    fn is_huge(&self) -> bool {
        self.0 & Self::LARGE_PAGE != 0
    }

    fn frame(&self) -> Option<PhysicalFrame> {
        if !self.is_present() || self.is_huge() {
            return None;
        }

        PhysicalFrame::from_start_address(self.physical_address())
    }
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [PageTableEntry; PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|&entry| !entry.is_present())
    }
}

const _: () = assert!(core::mem::size_of::<PageTable>() == 4096);

fn translate_huge_page(
    entry: PageTableEntry,
    virtual_addr: usize,
    page_size: usize,
) -> Option<PhysicalAddr> {
    let physical_base = entry.physical_address().as_usize() & !(page_size - 1);

    let offset = virtual_addr & (page_size - 1);

    physical_base.checked_add(offset).map(PhysicalAddr::new)
}

#[derive(Debug)]
pub enum PageError {
    NotCanonical,
    Unaligned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePermissions {
    pub writable: bool,
    pub executable: bool,
    pub user_accessible: bool,
}

impl PagePermissions {
    // TODO: give more fine grained permissions
    pub const HHDM: Self = Self::new(true, false, false);
    pub const KERNEL: Self = Self::new(true, true, false);
    pub const KERNEL_DATA: Self = Self::new(true, false, false);

    pub const fn new(writable: bool, executable: bool, user_accessible: bool) -> Self {
        Self {
            writable,
            executable,
            user_accessible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    start: VirtualAddr,
}

impl Page {
    pub fn from_start_address(start: VirtualAddr) -> Result<Self, PageError> {
        if !is_canonical_48_bit(start.as_usize()) {
            return Err(PageError::NotCanonical);
        }

        if start.as_usize() & (4096 - 1) != 0 {
            return Err(PageError::Unaligned);
        }

        Ok(Self { start })
    }
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

unsafe fn read_cr3() -> PhysicalFrame {
    let value: usize;
    unsafe {
        asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    PhysicalFrame::from_start_address(PhysicalAddr::new(value & ADDRESS_MASK))
        .expect("CR3 contains an unaligned page-table address")
}

#[derive(Debug)]
pub enum MapError {
    MisalignedPage,
    PageAlreadyMapped,
    ParentIsHuge,
    FrameAllocatorError,
    AddressOverflow,
    UserPageInKernelHalf,
}

#[derive(Debug)]
pub enum UnmapError {
    PageNotWithinHHDM,
    PageNotPresent,
    ParentIsHuge,
    ParentNotPresent,
    FrameAllocatorError,
}

type PageTableRoot = PhysicalFrame;

#[derive(Clone, Copy)]
struct NewTable {
    parent: PhysicalFrame,
    index: usize,
    child: PhysicalFrame,
}

#[derive(Debug)]
pub enum AddressSpaceError {
    MalformedMemmap,
    Map(MapError),
    FrameAllocatorError,
}

pub struct AddressSpace {
    root: PageTableRoot,
    direct_map: DirectMap,
}

impl AddressSpace {
    pub fn new<I: Iterator<Item = MemoryRegion> + Clone>(
        direct_map: DirectMap,
        memmap: I,
        kernel_address: KernelImage,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, AddressSpaceError> {
        let frame = allocator
            .alloc()
            .ok_or(AddressSpaceError::FrameAllocatorError)?;
        let mut space = AddressSpace {
            root: frame,
            direct_map,
        };

        // TODO: cleanup page tables if any allocations fail
        // map hhdm
        for region in memmap.clone() {
            if region.kind == MemoryRegionKind::Reserved
                || region.kind == MemoryRegionKind::BadMemory
            {
                continue;
            }

            println!(
                "Mapping: {:?} at {:#X} with length {:#X}",
                region.kind,
                region.start.as_usize(),
                region.length
            );

            space
                .map_range(
                    direct_map
                        .translate(region.start)
                        .ok_or(AddressSpaceError::MalformedMemmap)?,
                    region.start,
                    region.length,
                    PagePermissions::HHDM,
                    allocator,
                )
                .map_err(|err| AddressSpaceError::Map(err))?;
        }

        // map kernel
        println!(
            "Mapping kernel at {:#X} with length {:#X}",
            kernel_address.virtual_base.as_usize(),
            kernel_address.length
        );

        space
            .map_range(
                kernel_address.virtual_base,
                kernel_address.physical_base,
                kernel_address.length,
                PagePermissions::KERNEL,
                allocator,
            )
            .map_err(|err| AddressSpaceError::Map(err))?;

        Ok(space)
    }

    fn is_active(&self) -> bool {
        let cr3 = unsafe { read_cr3() };

        cr3.start_address() == self.root.start_address()
    }

    fn table(&self, frame: PhysicalFrame) -> Option<&PageTable> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &*(virtual_addr.as_ptr::<PageTable>()) })
    }

    fn table_mut(&mut self, frame: PhysicalFrame) -> Option<&mut PageTable> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &mut *(virtual_addr.as_mut_ptr::<PageTable>()) })
    }

    pub fn translate(&self, addr: VirtualAddr) -> Option<PhysicalAddr> {
        let addr = addr.as_usize();

        if !is_canonical_48_bit(addr) {
            return None;
        }

        let p4 = self.table(self.root)?;
        let p4_entry = p4.entries[p4_index(addr)];

        if !p4_entry.is_present() {
            return None;
        }

        let p3 = self.table(p4_entry.frame()?)?;
        let p3_entry = p3.entries[p3_index(addr)];

        if !p3_entry.is_present() {
            return None;
        }

        if p3_entry.is_huge() {
            return translate_huge_page(p3_entry, addr, 1 << 30);
        }

        let p2 = self.table(p3_entry.frame()?)?;
        let p2_entry = p2.entries[p2_index(addr)];

        if !p2_entry.is_present() {
            return None;
        }

        if p2_entry.is_huge() {
            return translate_huge_page(p2_entry, addr, 1 << 21);
        }

        let p1 = self.table(p2_entry.frame()?)?;
        let p1_entry = p1.entries[p1_index(addr)];

        if !p1_entry.is_present() {
            return None;
        }

        let physical_base = p1_entry.physical_address().as_usize();

        physical_base
            .checked_add(page_offset(addr))
            .map(PhysicalAddr::new)
    }

    fn get_next_level(
        &self,
        parent: PhysicalFrame,
        index: usize,
    ) -> Result<PhysicalFrame, UnmapError> {
        let parent_table = self.table(parent).ok_or(UnmapError::PageNotWithinHHDM)?;

        // TODO: encode some level so we dont spuriously think a page is huge
        if parent_table.entries[index].is_huge() {
            return Err(UnmapError::ParentIsHuge);
        }

        Ok(parent_table.entries[index]
            .frame()
            .ok_or(UnmapError::ParentNotPresent)?)
    }

    fn get_next_level_or_allocate(
        &mut self,
        parent: PhysicalFrame,
        index: usize,
        user_accessible: bool,
        allocator: &mut FrameAllocator,
    ) -> Result<(PhysicalFrame, bool), MapError> {
        let parent_table = self
            .table_mut(parent)
            .ok_or(MapError::FrameAllocatorError)?;

        let entry = &mut parent_table.entries[index];

        if !entry.is_present() {
            let frame = allocator.alloc().ok_or(MapError::FrameAllocatorError)?;
            *entry = PageTableEntry::new_table(frame, user_accessible);
            return Ok((frame, true));
        }

        if entry.is_huge() {
            return Err(MapError::ParentIsHuge);
        }

        // TODO: we upgrade intermediate entries, and dont carefully rollback if we fail
        if user_accessible && !entry.is_user_accessible() {
            entry.0 |= PageTableEntry::USER_ACCESSIBLE;
        }

        entry
            .frame()
            .map(|frame| (frame, false))
            .ok_or(MapError::FrameAllocatorError)
    }

    fn rollback_tables(
        &mut self,
        new_tables: &[Option<NewTable>],
        count: usize,
        allocator: &mut FrameAllocator,
    ) {
        for table in new_tables[..count].iter().rev().flatten() {
            self.table_mut(table.parent).unwrap().entries[table.index] = PageTableEntry::null();

            unsafe { allocator.dealloc(table.child) };
        }
    }

    pub fn map(
        &mut self,
        page: Page,
        frame: PhysicalFrame,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if permissions.user_accessible && p4_index(page.start.as_usize()) >= 256 {
            return Err(MapError::UserPageInKernelHalf);
        }

        let mut new_tables: [Option<NewTable>; 3] = [None; 3];
        let mut new_table_count = 0;

        let result = (|| {
            let p4_entry_index = p4_index(page.start.as_usize());
            let (pdpt_frame, allocated) = self.get_next_level_or_allocate(
                self.root,
                p4_entry_index,
                permissions.user_accessible,
                allocator,
            )?;
            if allocated {
                new_tables[new_table_count] = Some(NewTable {
                    parent: self.root,
                    index: p4_entry_index,
                    child: pdpt_frame,
                });
                new_table_count += 1;
            }

            let p3_entry_index = p3_index(page.start.as_usize());
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

            let p2_entry_index = p2_index(page.start.as_usize());
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

            let pt_table = self
                .table_mut(pt_frame)
                .ok_or(MapError::FrameAllocatorError)?;
            let entry = &mut pt_table.entries[p1_index(page.start.as_usize())];
            if entry.is_present() {
                return Err(MapError::PageAlreadyMapped);
            }

            *entry = PageTableEntry::new(frame.start_address(), permissions);
            Ok(())
        })();

        if result.is_err() {
            self.rollback_tables(&new_tables, new_table_count, allocator);
            return result;
        }

        self.flush_tlb_if_active(page);

        Ok(())
    }

    pub fn map_range(
        &mut self,
        virtual_start: VirtualAddr,
        physical_start: PhysicalAddr,
        length: usize,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if length % PAGE_SIZE != 0 {
            return Err(MapError::MisalignedPage);
        }

        let virtual_start =
            Page::from_start_address(virtual_start).map_err(|_| MapError::MisalignedPage)?;
        let physical_start =
            PhysicalFrame::from_start_address(physical_start).ok_or(MapError::MisalignedPage)?;
        virtual_start
            .start
            .as_usize()
            .checked_add(length)
            .ok_or(MapError::AddressOverflow)?;
        physical_start
            .start_address()
            .as_usize()
            .checked_add(length)
            .ok_or(MapError::AddressOverflow)?;

        for page_idx in 0..length / PAGE_SIZE {
            let offset = page_idx * PAGE_SIZE;
            let page =
                Page::from_start_address(VirtualAddr::new(virtual_start.start.as_usize() + offset))
                    .unwrap();
            let frame = PhysicalFrame::from_start_address(PhysicalAddr::new(
                physical_start.start_address().as_usize() + offset,
            ))
            .unwrap();

            if let Err(error) = self.map(page, frame, permissions, allocator) {
                for rollback_idx in (0..page_idx).rev() {
                    let rollback_page = Page::from_start_address(VirtualAddr::new(
                        virtual_start.start.as_usize() + rollback_idx * PAGE_SIZE,
                    ))
                    .unwrap();
                    self.unmap(rollback_page, allocator)
                        .expect("failed to roll back a mapped page");
                }

                return Err(error);
            }
        }

        Ok(())
    }

    pub fn unmap(
        &mut self,
        page: Page,
        allocator: &mut FrameAllocator,
    ) -> Result<PhysicalFrame, UnmapError> {
        let pdpt_frame = self.get_next_level(self.root, p4_index(page.start.as_usize()))?;

        let pd_frame = self.get_next_level(pdpt_frame, p3_index(page.start.as_usize()))?;

        let pt_frame = self.get_next_level(pd_frame, p2_index(page.start.as_usize()))?;
        let pt_table = self
            .table_mut(pt_frame)
            .ok_or(UnmapError::FrameAllocatorError)?;

        let entry = pt_table.entries[p1_index(page.start.as_usize())];

        if !entry.is_present() {
            return Err(UnmapError::PageNotPresent);
        }

        let frame = entry.frame().ok_or(UnmapError::FrameAllocatorError)?;
        pt_table.entries[p1_index(page.start.as_usize())] = PageTableEntry::null();

        if pt_table.is_empty() {
            self.table_mut(pd_frame).unwrap().entries[p2_index(page.start.as_usize())] =
                PageTableEntry::null();
            unsafe { allocator.dealloc(pt_frame) };

            if self.table(pd_frame).unwrap().is_empty() {
                self.table_mut(pdpt_frame).unwrap().entries[p3_index(page.start.as_usize())] =
                    PageTableEntry::null();
                unsafe { allocator.dealloc(pd_frame) };

                if self.table(pdpt_frame).unwrap().is_empty() {
                    self.table_mut(self.root).unwrap().entries[p4_index(page.start.as_usize())] =
                        PageTableEntry::null();
                    unsafe { allocator.dealloc(pdpt_frame) };
                }
            }
        }

        self.flush_tlb_if_active(page);

        Ok(frame)
    }

    // TODO: unmap range

    fn flush_tlb_if_active(&self, page: Page) {
        if self.is_active() {
            println!("Flushing TLB");
            unsafe {
                asm!("invlpg [{}]", in(reg) page.start.as_usize(), options(nostack, preserves_flags));
            }
        }
    }

    pub unsafe fn activate(&self) -> PageTableRoot {
        println!("Activating");
        unsafe {
            asm!(
                "mov cr3, {}",
                in(reg) self.root.start_address().as_usize(),
                options(nostack, preserves_flags)
            );
        }

        self.root
    }
}

fn is_canonical_48_bit(addr: usize) -> bool {
    let upper = addr >> 48;
    let sign_bit = (addr >> 47) & 1;

    if sign_bit == 0 {
        upper == 0
    } else {
        upper == 0xFFFF
    }
}
