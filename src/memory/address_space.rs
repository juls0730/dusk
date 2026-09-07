use core::cell::UnsafeCell;

use crate::{
    arch::{PageTable, PageTableCreateError, PageTableMapError, PageTableUnmapError, PagingConfig},
    memory::{
        CachePolicy, DirectMap, FRAME_SIZE, FrameAddr, FrameAllocator, KernelMemoryLayout,
        MemoryRegion, MemoryRegionKind, PagePermissions, PhysicalAddr, USER_SPACE_END, VirtualAddr,
    },
};

const MAX_ADDRESS_SPACES: usize = 32;
struct AddressSpaceTable {
    entries: [Option<AddressSpace>; MAX_ADDRESS_SPACES],
}

impl AddressSpaceTable {
    const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_ADDRESS_SPACES],
        }
    }

    fn insert(&mut self, address_space: AddressSpace) -> Option<usize> {
        for (i, slot) in self.entries.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(address_space);
                return Some(i);
            }
        }

        None
    }

    fn get(&self, id: usize) -> Option<&AddressSpace> {
        self.entries.get(id).and_then(Option::as_ref)
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut AddressSpace> {
        self.entries.get_mut(id).and_then(Option::as_mut)
    }

    fn remove(&mut self, id: usize) -> Option<AddressSpace> {
        self.entries.get_mut(id).and_then(Option::take)
    }
}

struct GlobalAddressSpaceTable(UnsafeCell<AddressSpaceTable>);

unsafe impl Sync for GlobalAddressSpaceTable {}

static ADDRESS_SPACE_TABLE: GlobalAddressSpaceTable =
    GlobalAddressSpaceTable(UnsafeCell::new(AddressSpaceTable::new()));

pub fn insert_address_space(address_space: AddressSpace) -> Option<usize> {
    let table = unsafe { &mut *ADDRESS_SPACE_TABLE.0.get() };

    let id = table.insert(address_space);
    id
}

pub fn remove_address_space(id: usize) -> Option<AddressSpace> {
    let table = unsafe { &mut *ADDRESS_SPACE_TABLE.0.get() };

    table.remove(id)
}

pub fn with_address_space<R>(id: usize, f: impl FnOnce(&AddressSpace) -> R) -> Option<R> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let table = unsafe { &*ADDRESS_SPACE_TABLE.0.get() };
    let res = table.get(id).map(f);
    crate::arch::restore_interrupts(interrupt_state);
    res
}

pub fn with_address_space_mut<R>(id: usize, f: impl FnOnce(&mut AddressSpace) -> R) -> Option<R> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let table = unsafe { &mut *ADDRESS_SPACE_TABLE.0.get() };
    let res = table.get_mut(id).map(f);
    crate::arch::restore_interrupts(interrupt_state);
    res
}

struct GlobalKernelAddressSpace(UnsafeCell<Option<AddressSpace>>);

unsafe impl Sync for GlobalKernelAddressSpace {}

static KERNEL_ADDRESS_SPACE: GlobalKernelAddressSpace =
    GlobalKernelAddressSpace(UnsafeCell::new(None));

pub fn init_kernel_address_space(address_space: AddressSpace) {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    unsafe {
        *KERNEL_ADDRESS_SPACE.0.get() = Some(address_space);
    }
    crate::arch::restore_interrupts(interrupt_state);
}

pub fn with_kernel_address_space<R>(f: impl FnOnce(&mut AddressSpace) -> R) -> R {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let space = unsafe {
        (&mut *KERNEL_ADDRESS_SPACE.0.get())
            .as_mut()
            .expect("kernel address space not initialized")
    };
    let res = f(space);
    crate::arch::restore_interrupts(interrupt_state);
    res
}

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
    OutOfMemory,
    PageTableUnavailable,
    CorruptedPageTable,
    InvalidUserAddress,
    InvalidUserMap,
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
    PageTableUnavailable,
    CorruptedPageTable,
    InvalidUserAddress,
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
#[allow(unused)]
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

#[derive(PartialEq, Eq)]
enum AddressSpaceKind {
    Kernel,
    User,
}

#[derive(PartialEq, Eq)]
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
            // we exlcude KernelAndModules from the hhdm because if it were mapped, it would
            // undermind the permissions of the explicitly mapped kernel image
            if matches!(
                region.kind,
                MemoryRegionKind::Reserved | MemoryRegionKind::BadMemory
            ) {
                continue;
            }

            // we shouldnt HHDM the kernel image, but we should map modules
            if region.kind == MemoryRegionKind::KernelAndModules
                && region.start == layout.segments[0].physical_base
            {
                continue;
            }

            let cache_policy = if matches!(
                region.kind,
                MemoryRegionKind::MappedReserved | MemoryRegionKind::Framebuffer
            ) {
                CachePolicy::Uncacheable
            } else {
                CachePolicy::WriteBack
            };

            let res = space.map_range(
                region.start,
                direct_map
                    .translate(region.start)
                    .ok_or(AddressSpaceCreateError::AddressOutsideDirectMap)?,
                region.length,
                PagePermissions::new(true, false, false),
                allocator,
                cache_policy,
            );

            if let Err(err) = res {
                unsafe { space.destroy(allocator) };
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
                CachePolicy::WriteBack,
            );

            if let Err(err) = res {
                unsafe { space.destroy(allocator) };
                return Err(AddressSpaceCreateError::Map(err));
            }
        }

        Ok(space)
    }

    pub fn new_user(&self, allocator: &mut FrameAllocator) -> Result<Self, PageTableCreateError> {
        let mut user_root = PageTable::new(self.root.direct_map, self.root.config(), allocator)?;

        self.root.copy_kernel_mappings_to(&mut user_root);

        Ok(Self {
            root: user_root,
            kind: AddressSpaceKind::User,
        })
    }

    pub fn map(
        &mut self,
        physical_addr: PhysicalAddr,
        virtual_addr: VirtualAddr,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
        cache_policy: CachePolicy,
    ) -> Result<(), MapError> {
        let global = self.kind == AddressSpaceKind::Kernel;

        if self.kind == AddressSpaceKind::User {
            if virtual_addr.as_usize() >= USER_SPACE_END.as_usize() {
                return Err(MapError::InvalidUserAddress);
            }

            if !permissions.user_accessible {
                return Err(MapError::InvalidUserMap);
            }

            // TODO: a user address space should not be able to map kernel memory
            // or ACPI memory, or anything like that
        }

        let frame = FrameAddr::from_start_address(physical_addr)
            .ok_or(MapError::PhysicalAddressUnaligned)?;

        self.root
            .map(
                virtual_addr,
                frame,
                permissions,
                allocator,
                cache_policy,
                global,
            )
            .map_err(MapError::from)
    }

    pub fn map_range(
        &mut self,
        physical_start: PhysicalAddr,
        virtual_start: VirtualAddr,
        length: usize,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
        cache_policy: CachePolicy,
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

            if let Err(err) = self.map(
                physical_addr,
                virtual_addr,
                permissions,
                allocator,
                cache_policy,
            ) {
                for rollback_idx in (0..mapped_pages).rev() {
                    let rollback_offset = rollback_idx * FRAME_SIZE;

                    unsafe {
                        // make sure we use the root page tableq directl since the public AddressSpace API
                        // might *at some point* reject kernel mappings
                        self.root
                            .unmap(
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
    /// - The page is not currently in use
    pub unsafe fn unmap(
        &mut self,
        virtual_addr: VirtualAddr,
        allocator: &mut FrameAllocator,
    ) -> Result<FrameAddr, UnmapError> {
        if self.kind == AddressSpaceKind::User && virtual_addr.as_usize() >= 0x0000_8000_0000_0000 {
            return Err(UnmapError::InvalidUserAddress);
        }

        unsafe { self.root.unmap(virtual_addr, allocator) }.map_err(UnmapError::from)
    }

    pub fn to_physical(&self, virtual_addr: VirtualAddr) -> Option<PhysicalAddr> {
        self.root.to_physical(virtual_addr)
    }

    pub fn to_virtual(&self, physical_addr: PhysicalAddr) -> Option<VirtualAddr> {
        self.root.to_virtual(physical_addr)
    }

    pub unsafe fn activate(&self) {
        unsafe { self.root.activate() }
    }

    /// # Safety
    ///
    /// The caller must ensure this apge table is not active on any CPU and
    /// no CPU or kernel operation can access its paging structures.
    pub unsafe fn destroy(self, allocator: &mut FrameAllocator) {
        unsafe {
            match self.kind {
                AddressSpaceKind::Kernel => self.root.destroy(allocator),
                AddressSpaceKind::User => self.root.destroy_user(allocator),
            }
        }
    }
}
