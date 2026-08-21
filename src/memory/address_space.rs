use crate::{
    arch::{PageTable, PageTableCreateError, PageTableMapError, PageTableUnmapError, PagingConfig},
    memory::{
        DirectMap, FRAME_SIZE, FrameAllocator, KernelMemoryLayout, MemoryRegion, MemoryRegionKind,
        PagePermissions, PhysicalAddr, PhysicalFrame, VirtualAddr,
    },
};

#[derive(Debug)]
pub enum MapError {
    InvalidVirtualAddress,
    VirtualAddressUnaligned,
    PhysicalAddressTooLarge,
    PhysicalAddressUnaligned,
    RangeLengthUnaligned,
    AddressOverflow,
    AlreadyMapped,
    MappingConflict,
    UnsupportedPermissions,
    OutsideAddressSpace,
    OutOfMemory,
    PageTableUnavailable,
    CorruptedPageTable,
}

impl From<PageTableMapError> for MapError {
    fn from(error: PageTableMapError) -> Self {
        match error {
            PageTableMapError::InvalidVirtualAddress => Self::InvalidVirtualAddress,
            PageTableMapError::VirtualAddressUnaligned => Self::VirtualAddressUnaligned,
            PageTableMapError::PhysicalAddressTooLarge => Self::PhysicalAddressTooLarge,
            PageTableMapError::PageAlreadyMapped => Self::AlreadyMapped,
            PageTableMapError::HugePageConflict => Self::MappingConflict,
            PageTableMapError::NoExecuteUnsupported => Self::UnsupportedPermissions,
            PageTableMapError::OutOfFrames => Self::OutOfMemory,
            PageTableMapError::PageTableOutsideDirectMap => Self::PageTableUnavailable,
            PageTableMapError::InvalidPageTableEntry => Self::CorruptedPageTable,
        }
    }
}

#[derive(Debug)]
pub enum UnmapError {
    InvalidVirtualAddress,
    VirtualAddressUnaligned,
    NotMapped,
    MappingConflict,
    OutsideAddressSpace,
    PageTableUnavailable,
    CorruptedPageTable,
}

impl From<PageTableUnmapError> for UnmapError {
    fn from(error: PageTableUnmapError) -> Self {
        match error {
            PageTableUnmapError::InvalidVirtualAddress => Self::InvalidVirtualAddress,
            PageTableUnmapError::VirtualAddressUnaligned => Self::VirtualAddressUnaligned,
            PageTableUnmapError::PageNotMapped => Self::NotMapped,
            PageTableUnmapError::HugePageConflict => Self::MappingConflict,
            PageTableUnmapError::PageTableOutsideDirectMap => Self::PageTableUnavailable,
            PageTableUnmapError::InvalidPageTableEntry => Self::CorruptedPageTable,
        }
    }
}

#[derive(Debug)]
pub enum AddressSpaceCreateError {
    AddressOutsideDirectMap,
    PhysicalAddressTooLarge,
    OutOfMemory,
    Map(MapError),
}

impl From<PageTableCreateError> for AddressSpaceCreateError {
    fn from(error: PageTableCreateError) -> Self {
        match error {
            PageTableCreateError::PhysicalAddressTooLarge => Self::PhysicalAddressTooLarge,
            PageTableCreateError::OutOfFrames => Self::OutOfMemory,
        }
    }
}

enum AddressSpaceKind {
    Kernel,
    User,
}

pub struct AddressSpace {
    root: PageTable,
    kind: AddressSpaceKind,
}

impl AddressSpace {
    pub fn new_kernel<I: Iterator<Item = MemoryRegion> + Clone>(
        direct_map: DirectMap,
        memory_regions: I,
        layout: &KernelMemoryLayout,
        paging_config: PagingConfig,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, AddressSpaceCreateError> {
        let mut space = AddressSpace {
            root: PageTable::new(direct_map, paging_config, allocator)?,
            kind: AddressSpaceKind::Kernel,
        };

        // map hhdm
        for region in memory_regions.clone() {
            if region.kind == MemoryRegionKind::Reserved
                || region.kind == MemoryRegionKind::BadMemory
            {
                continue;
            }

            let res = space.map_range(
                region.start,
                direct_map
                    .translate(region.start)
                    .ok_or(AddressSpaceCreateError::AddressOutsideDirectMap)?,
                region.length,
                PagePermissions {
                    writable: true,
                    executable: false,
                    user_accessible: false,
                },
                allocator,
            );

            if let Err(err) = res {
                space.destroy(allocator);
                return Err(AddressSpaceCreateError::Map(err));
            }
        }

        let mut old_segment_stop: Option<usize> = None;

        for segment in layout.segments.iter() {
            if let Some(stop) = old_segment_stop {
                debug_assert_eq!(segment.physical_base.as_usize(), stop);
            }

            old_segment_stop = Some(segment.physical_base.as_usize() + segment.length);

            let res = space.map_range(
                segment.physical_base,
                segment.virtual_base,
                segment.length,
                segment.permissions,
                allocator,
            );

            if let Err(err) = res {
                space.destroy(allocator);
                return Err(AddressSpaceCreateError::Map(err));
            }
        }

        Ok(space)
    }

    pub fn new_user(kernel_space: &AddressSpace, allocator: &mut FrameAllocator) -> Self {
        todo!()
    }

    pub fn map(
        &mut self,
        physical_addr: PhysicalAddr,
        virtual_addr: VirtualAddr,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        let frame = PhysicalFrame::from_start_address(physical_addr)
            .ok_or(MapError::PhysicalAddressUnaligned)?;

        self.root
            .map(virtual_addr, frame, permissions, allocator)
            .map_err(MapError::from)
    }

    pub fn map_range(
        &mut self,
        physical_start: PhysicalAddr,
        virtual_start: VirtualAddr,
        length: usize,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
    ) -> Result<(), MapError> {
        if length == 0 {
            return Ok(());
        }

        if physical_start.as_usize() % FRAME_SIZE != 0 {
            return Err(MapError::PhysicalAddressUnaligned);
        }

        if virtual_start.as_usize() % FRAME_SIZE != 0 {
            return Err(MapError::VirtualAddressUnaligned);
        }

        if length % FRAME_SIZE != 0 {
            return Err(MapError::RangeLengthUnaligned);
        }

        let last_offset = length - FRAME_SIZE;

        physical_start
            .as_usize()
            .checked_add(last_offset)
            .ok_or(MapError::AddressOverflow)?;
        virtual_start
            .as_usize()
            .checked_add(last_offset)
            .ok_or(MapError::AddressOverflow)?;

        let page_count = length / FRAME_SIZE;
        let mut mapped_pages = 0;

        while mapped_pages < page_count {
            let offset = mapped_pages * FRAME_SIZE;
            let physical_addr = PhysicalAddr::new(physical_start.as_usize() + offset);
            let virtual_addr = VirtualAddr::new(virtual_start.as_usize() + offset);

            if let Err(err) = self.map(physical_addr, virtual_addr, permissions, allocator) {
                for rollback_idx in (0..mapped_pages).rev() {
                    let rollback_offset = rollback_idx * FRAME_SIZE;
                    let rollback_physical_addr =
                        PhysicalAddr::new(physical_start.as_usize() + rollback_offset);

                    unsafe {
                        self.unmap(
                            VirtualAddr::new(virtual_start.as_usize() + rollback_offset),
                            allocator,
                        )
                        .expect("failed to roll back a mapped page");
                    }
                }

                return Err(err);
            }

            mapped_pages += 1;
        }

        Ok(())
    }

    /// # Safety
    ///
    /// The caller must ensure:
    /// - The caller must ensure that the page is not currently in use
    pub unsafe fn unmap(
        &mut self,
        virtual_addr: VirtualAddr,
        allocator: &mut FrameAllocator,
    ) -> Result<PhysicalFrame, UnmapError> {
        unsafe { self.root.unmap(virtual_addr, allocator) }.map_err(UnmapError::from)
    }

    pub fn translate(&self, virtual_addr: VirtualAddr) -> Option<PhysicalAddr> {
        self.root.translate(virtual_addr)
    }

    pub unsafe fn activate(&self) {
        unsafe { self.root.activate() }
    }

    pub fn destroy(self, allocator: &mut FrameAllocator) {
        todo!("destroy address space")
    }
}
