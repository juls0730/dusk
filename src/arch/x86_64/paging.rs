use core::arch::asm;

use crate::{
    arch::x86_64::cpu::CpuFeatures,
    memory::{
        CachePolicy, DirectMap, FrameAddr, FrameAllocator, OwnedFrame, PagePermissions,
        PhysicalAddr, VirtualAddr,
    },
};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512;

#[derive(Clone, Copy)]
pub struct PagingConfig {
    physical_address_bits: u8,
    global_pages: bool,
    nx_enabled: bool,
    mode: PagingMode,
}

impl PagingConfig {
    pub fn from_features(features: CpuFeatures) -> Self {
        Self {
            physical_address_bits: features.physical_address_bits,
            nx_enabled: features.nx_enabled,
            global_pages: features.global_pages,
            mode: if features.five_level_paging_active {
                PagingMode::FiveLevel
            } else {
                PagingMode::FourLevel
            },
        }
    }

    pub const fn physical_address_mask(&self) -> usize {
        ((1 << self.physical_address_bits) - 1) & !0xFFF
    }

    pub const fn physical_address_limit(&self) -> usize {
        1 << self.physical_address_bits
    }
}

#[derive(Clone, Copy)]
enum PagingMode {
    FourLevel,
    FiveLevel,
}

const MAX_INTERMEDIATE_LEVELS: usize = 4;
const FOUR_LEVEL_INTERMEDIATES: [PageTableLevel; 3] = [
    PageTableLevel::Pml4,
    PageTableLevel::Pdpt,
    PageTableLevel::PageDirectory,
];
const FIVE_LEVEL_INTERMEDIATES: [PageTableLevel; 4] = [
    PageTableLevel::Pml5,
    PageTableLevel::Pml4,
    PageTableLevel::Pdpt,
    PageTableLevel::PageDirectory,
];

impl PagingMode {
    const fn virtual_address_bits(&self) -> u32 {
        match self {
            PagingMode::FourLevel => 48,
            PagingMode::FiveLevel => 57,
        }
    }

    fn intermediate_levels(&self) -> &'static [PageTableLevel] {
        match self {
            PagingMode::FourLevel => &FOUR_LEVEL_INTERMEDIATES,
            PagingMode::FiveLevel => &FIVE_LEVEL_INTERMEDIATES,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PageTableLevel {
    Pml5,
    Pml4,
    Pdpt,
    PageDirectory,
}

impl PageTableLevel {
    const fn index(self, address: usize) -> usize {
        let shift = match self {
            Self::Pml5 => 48,
            Self::Pml4 => 39,
            Self::Pdpt => 30,
            Self::PageDirectory => 21,
        };

        address >> shift & 0x1FF
    }

    const fn large_page_size(self) -> Option<usize> {
        match self {
            Self::Pdpt => Some(1 << 30),
            Self::PageDirectory => Some(1 << 21),
            Self::Pml5 | Self::Pml4 => None,
        }
    }
}

enum PageTableEntryError {
    PhysicalAddressTooLarge,
    NoExecuteUnsupported,
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct PageTableEntry(u64);

impl PageTableEntry {
    const PRESENT: u64 = 1 << 0;
    const WRITABLE: u64 = 1 << 1;
    const USER_ACCESSIBLE: u64 = 1 << 2;
    const HUGE_PAGE: u64 = 1 << 7;
    const GLOBAL: u64 = 1 << 8;
    const NX: u64 = 1 << 63;

    const WRITE_THROUGH: u64 = 1 << 3;
    const CACHE_DISABLED: u64 = 1 << 4;
    // const PAT: u64 = 1 << 7;

    const fn new(
        physical_address: PhysicalAddr,
        permissions: PagePermissions,
        cache_policy: CachePolicy,
        config: PagingConfig,
        global: bool,
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

        if global && config.global_pages {
            value |= Self::GLOBAL;
        }

        // TODO: these bit positions very by page size
        // set PAT
        match cache_policy {
            CachePolicy::WriteBack => {}
            CachePolicy::Uncacheable => {
                value |= Self::CACHE_DISABLED | Self::WRITE_THROUGH;
            }
        };

        Ok(Self(value))
    }

    fn new_table(
        frame: FrameAddr,
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

    fn is_global(&self) -> bool {
        self.0 & Self::GLOBAL != 0
    }

    fn table_frame(&self, config: PagingConfig) -> Option<FrameAddr> {
        if !self.is_present() || self.is_huge() {
            return None;
        }

        FrameAddr::from_start_address(self.physical_address(config))
    }

    fn leaf_frame(&self, config: PagingConfig) -> Option<FrameAddr> {
        if !self.is_present() {
            return None;
        }

        FrameAddr::from_start_address(self.physical_address(config))
    }
}

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
struct EntryLocation {
    table: FrameAddr,
    index: usize,
}

#[derive(Debug)]
pub(crate) enum PageTableCreateError {
    PhysicalAddressTooLarge,
    OutOfFrames,
}

pub struct PageTable {
    pub direct_map: DirectMap,
    config: PagingConfig,
    frame: OwnedFrame,
}

impl PartialEq for PageTable {
    fn eq(&self, other: &Self) -> bool {
        self.frame.frame_address() == other.frame.frame_address()
    }
}

impl Eq for PageTable {}

impl PageTable {
    pub fn new(
        direct_map: DirectMap,
        config: PagingConfig,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, PageTableCreateError> {
        let frame = allocator.alloc().ok_or(PageTableCreateError::OutOfFrames)?;

        if frame.frame_address().start_address().as_usize() >= config.physical_address_limit() {
            unsafe { allocator.dealloc(frame) };

            return Err(PageTableCreateError::PhysicalAddressTooLarge);
        }

        Ok(Self {
            frame,
            direct_map,
            config,
        })
    }

    pub const fn config(&self) -> PagingConfig {
        self.config
    }

    fn is_active(&self) -> bool {
        let cr3 = unsafe { read_cr3(self.config) };

        cr3.start_address() == self.frame.frame_address().start_address()
    }

    fn table(&self, frame: FrameAddr) -> Option<&[PageTableEntry; PAGE_TABLE_ENTRIES]> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &*(virtual_addr.as_ptr::<[PageTableEntry; PAGE_TABLE_ENTRIES]>()) })
    }

    fn table_mut(&mut self, frame: FrameAddr) -> Option<&mut [PageTableEntry; PAGE_TABLE_ENTRIES]> {
        let virtual_addr = self.direct_map.translate(frame.start_address())?;

        Some(unsafe { &mut *(virtual_addr.as_mut_ptr::<[PageTableEntry; PAGE_TABLE_ENTRIES]>()) })
    }

    fn is_canonical(&self, addr: usize) -> bool {
        let bits = self.config.mode.virtual_address_bits();
        let shift = usize::BITS - bits;

        (((addr << shift) as isize >> shift) as usize) == addr
    }

    pub fn to_physical(&self, addr: VirtualAddr) -> Option<PhysicalAddr> {
        let address = addr.as_usize();

        if !self.is_canonical(address) {
            return None;
        }

        let mut table_frame = self.frame.frame_address();

        for &level in self.config.mode.intermediate_levels() {
            let table = self.table(table_frame)?;
            let entry = table[level.index(address)];

            if !entry.is_present() {
                return None;
            }

            if entry.is_huge() {
                return translate_huge_page(entry, address, level.large_page_size()?, self.config);
            }

            table_frame = entry.table_frame(self.config)?;
        }

        let page_table = self.table(table_frame)?;
        let entry = page_table[p1_index(address)];
        let physical_base = entry.leaf_frame(self.config)?.start_address().as_usize();

        physical_base
            .checked_add(page_offset(address))
            .map(PhysicalAddr::new)
    }

    pub fn to_virtual(&self, addr: PhysicalAddr) -> Option<VirtualAddr> {
        self.direct_map.translate(addr)
    }

    fn get_next_level(
        &self,
        parent: FrameAddr,
        index: usize,
        level: PageTableLevel,
    ) -> Result<FrameAddr, UnmapError> {
        let parent_table = self
            .table(parent)
            .ok_or(UnmapError::PageTableOutsideDirectMap)?;
        let entry = parent_table[index];

        if entry.is_huge() {
            return if level.large_page_size().is_some() {
                Err(UnmapError::HugePageConflict)
            } else {
                Err(UnmapError::InvalidPageTableEntry)
            };
        }

        entry
            .table_frame(self.config)
            .ok_or(UnmapError::PageNotMapped)
    }

    fn apply_user_upgrades(
        &mut self,
        upgrades: &[Option<EntryLocation>; MAX_INTERMEDIATE_LEVELS],
        count: usize,
    ) {
        for location in upgrades[..count].iter().flatten() {
            let entry = &mut self
                .table_mut(location.table)
                .expect("validated page table left the direct map")[location.index];
            entry.0 |= PageTableEntry::USER_ACCESSIBLE;
        }
    }

    pub fn map(
        &mut self,
        mapped_addr: VirtualAddr,
        frame: FrameAddr,
        permissions: PagePermissions,
        allocator: &mut FrameAllocator,
        cache_policy: CachePolicy,
        global: bool,
    ) -> Result<(), MapError> {
        let address = mapped_addr.as_usize();

        if !self.is_canonical(address) {
            return Err(MapError::InvalidVirtualAddress);
        }

        if address % PAGE_SIZE != 0 {
            return Err(MapError::VirtualAddressUnaligned);
        }

        let leaf_entry = PageTableEntry::new(
            frame.start_address(),
            permissions,
            cache_policy,
            self.config,
            global,
        )
        .map_err(|error| match error {
            PageTableEntryError::PhysicalAddressTooLarge => MapError::PhysicalAddressTooLarge,
            PageTableEntryError::NoExecuteUnsupported => MapError::NoExecuteUnsupported,
        })?;

        let levels = self.config.mode.intermediate_levels();
        let mut current_table_frame_addr = self.frame.frame_address();
        let mut first_missing = None;
        let mut user_upgrades: [Option<EntryLocation>; MAX_INTERMEDIATE_LEVELS] =
            [None; MAX_INTERMEDIATE_LEVELS];
        let mut user_upgrade_count = 0;

        for (depth, &level) in levels.iter().enumerate() {
            let index = level.index(address);
            let table = self
                .table(current_table_frame_addr)
                .ok_or(MapError::PageTableOutsideDirectMap)?;
            let entry = table[index];

            if !entry.is_present() {
                first_missing = Some((
                    depth,
                    EntryLocation {
                        table: current_table_frame_addr,
                        index,
                    },
                ));
                break;
            }

            if entry.is_huge() {
                return if level.large_page_size().is_some() {
                    Err(MapError::HugePageConflict)
                } else {
                    Err(MapError::InvalidPageTableEntry)
                };
            }

            if permissions.user_accessible && !entry.is_user_accessible() {
                user_upgrades[user_upgrade_count] = Some(EntryLocation {
                    table: current_table_frame_addr,
                    index,
                });
                user_upgrade_count += 1;
            }

            current_table_frame_addr = entry
                .table_frame(self.config)
                .ok_or(MapError::InvalidPageTableEntry)?;
        }

        if first_missing.is_none() {
            let pt = self
                .table(current_table_frame_addr)
                .ok_or(MapError::PageTableOutsideDirectMap)?;
            if pt[p1_index(address)].is_present() {
                return Err(MapError::PageAlreadyMapped);
            }

            self.apply_user_upgrades(&user_upgrades, user_upgrade_count);
            self.table_mut(current_table_frame_addr)
                .expect("validated page table left the direct map")[p1_index(address)] = leaf_entry;
            self.flush_tlb_if_active(mapped_addr);
            return Ok(());
        }

        let (missing_depth, publication_location) = first_missing.unwrap();
        let private_table_count = levels.len() - missing_depth;
        let mut private_tables: [Option<OwnedFrame>; MAX_INTERMEDIATE_LEVELS] =
            core::array::from_fn(|_| None);

        let prepare_result = (|| {
            for slot in &mut private_tables[..private_table_count] {
                let frame = allocator.alloc().ok_or(MapError::OutOfFrames)?;
                let address = frame.frame_address();
                // Track ownership before validation so every error uses the same cleanup.
                *slot = Some(frame);

                PageTableEntry::new_table(address, permissions.user_accessible, self.config)
                    .map_err(|_| MapError::PhysicalAddressTooLarge)?;
            }

            for private_index in 0..private_table_count {
                let private_frame = private_tables[private_index]
                    .as_ref()
                    .unwrap()
                    .frame_address();

                if private_index + 1 < private_table_count {
                    let child = private_tables[private_index + 1]
                        .as_ref()
                        .unwrap()
                        .frame_address();
                    let child_entry =
                        PageTableEntry::new_table(child, permissions.user_accessible, self.config)
                            .map_err(|_| MapError::PhysicalAddressTooLarge)?;
                    let child_index = levels[missing_depth + private_index + 1].index(address);
                    self.table_mut(private_frame)
                        .ok_or(MapError::PageTableOutsideDirectMap)?[child_index] = child_entry;
                } else {
                    self.table_mut(private_frame)
                        .ok_or(MapError::PageTableOutsideDirectMap)?[p1_index(address)] =
                        leaf_entry;
                }
            }

            PageTableEntry::new_table(
                private_tables[0].as_ref().unwrap().frame_address(),
                permissions.user_accessible,
                self.config,
            )
            .map_err(|_| MapError::PhysicalAddressTooLarge)
        })();

        let publication_entry = match prepare_result {
            Ok(entry) => entry,
            Err(error) => {
                for frame in private_tables.into_iter().rev().flatten() {
                    unsafe { allocator.dealloc(frame) };
                }
                return Err(error);
            }
        };

        self.apply_user_upgrades(&user_upgrades, user_upgrade_count);
        self.table_mut(publication_location.table)
            .expect("validated publication table left the direct map")
            [publication_location.index] = publication_entry;

        // The published page table now owns these frames.
        for frame in private_tables.into_iter().flatten() {
            let _ = frame.into_raw();
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
    ) -> Result<FrameAddr, UnmapError> {
        if !self.is_canonical(mapped_addr.as_usize()) {
            return Err(UnmapError::InvalidVirtualAddress);
        }

        if mapped_addr.as_usize() % PAGE_SIZE != 0 {
            return Err(UnmapError::VirtualAddressUnaligned);
        }

        let address = mapped_addr.as_usize();
        let levels = self.config.mode.intermediate_levels();
        let mut table_frames: [Option<FrameAddr>; MAX_INTERMEDIATE_LEVELS + 1] =
            core::array::from_fn(|_| None);
        table_frames[0] = Some(self.frame.frame_address());

        let mut current_table = self.frame.frame_address();
        for (depth, &level) in levels.iter().enumerate() {
            current_table = self.get_next_level(current_table, level.index(address), level)?;
            table_frames[depth + 1] = Some(current_table);
        }

        let config = self.config;
        let page_table = self
            .table_mut(current_table)
            .ok_or(UnmapError::PageTableOutsideDirectMap)?;
        let entry_index = p1_index(address);
        let entry = page_table[entry_index];
        let frame = entry.leaf_frame(config).ok_or(UnmapError::PageNotMapped)?;
        page_table[entry_index] = PageTableEntry::null();

        let mut child_is_empty = page_table.iter().all(|entry| !entry.is_present());
        let mut deallocatable_frames: [Option<FrameAddr>; MAX_INTERMEDIATE_LEVELS] =
            core::array::from_fn(|_| None);
        let mut deallocatable_count = 0;

        for depth in (0..levels.len()).rev() {
            if !child_is_empty {
                break;
            }

            let parent_frame_addr = table_frames[depth].unwrap();
            let child_frame = table_frames[depth + 1].unwrap();
            self.table_mut(parent_frame_addr)
                .expect("validated page table left the direct map")[levels[depth].index(address)] =
                PageTableEntry::null();

            deallocatable_frames[deallocatable_count] = Some(child_frame);
            deallocatable_count += 1;

            if depth > 0 {
                child_is_empty = self
                    .table(parent_frame_addr)
                    .expect("validated page table left the direct map")
                    .iter()
                    .all(|entry| !entry.is_present());
            }
        }

        self.flush_tlb_if_active(mapped_addr);

        for frame in deallocatable_frames[..deallocatable_count].iter().flatten() {
            unsafe { allocator.dealloc(OwnedFrame::from_raw(*frame)) };
        }

        Ok(frame)
    }

    pub fn copy_kernel_mappings_to(&self, destination: &mut PageTable) {
        let src = self
            .table(self.frame.frame_address())
            .expect("source page table outside direct map");
        let dest = destination
            .table_mut(destination.frame.frame_address())
            .expect("destination page table outside direct map");

        dest[256..512].copy_from_slice(&src[256..512]);
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
                in(reg) self.frame.frame_address().start_address().as_usize(),
                options(nostack, preserves_flags)
            );
        };
    }

    fn destroy_children(&mut self, table: FrameAddr, depth: usize, allocator: &mut FrameAllocator) {
        let levels = self.config.mode.intermediate_levels();

        if depth == levels.len() {
            return;
        }

        for idx in 0..PAGE_TABLE_ENTRIES {
            let entry = self.table(table).expect("page table outside direct map")[idx];

            if !entry.is_present() || entry.is_huge() {
                continue;
            }

            let child = entry
                .table_frame(self.config)
                .expect("invalid page table entry");
            self.destroy_children(child, depth + 1, allocator);

            self.table_mut(table)
                .expect("page table outside direct map")[idx] = PageTableEntry::null();

            unsafe { allocator.dealloc(OwnedFrame::from_raw(child)) };
        }
    }

    pub unsafe fn destroy(mut self, allocator: &mut FrameAllocator) {
        assert!(!self.is_active(), "attempted to destroy active page table");

        self.destroy_children(self.frame.frame_address(), 0, allocator);

        unsafe { allocator.dealloc(self.frame) };
    }

    pub unsafe fn destroy_user(mut self, allocator: &mut FrameAllocator) {
        assert!(!self.is_active(), "attempted to destroy active page table");

        let root_frame = self.frame.frame_address();

        for idx in 0..256 {
            let entry = self.table(root_frame).expect("table outside direct map")[idx];
            if !entry.is_present() || entry.is_huge() {
                continue;
            }

            let child = entry
                .table_frame(self.config)
                .expect("invalid page table entry");
            self.destroy_children(child, 1, allocator);

            self.table_mut(root_frame)
                .expect("table outside direct map")[idx] = PageTableEntry::null();
            unsafe { allocator.dealloc(OwnedFrame::from_raw(child)) };
        }

        unsafe { allocator.dealloc(self.frame) };
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

fn p1_index(addr: usize) -> usize {
    (addr >> 12) & 0x1FF
}

fn page_offset(addr: usize) -> usize {
    addr & 0xFFF
}

unsafe fn read_cr3(config: PagingConfig) -> FrameAddr {
    let value: usize;
    unsafe {
        asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    FrameAddr::from_start_address(PhysicalAddr::new(value & config.physical_address_mask()))
        .expect("CR3 contains an unaligned page-table address")
}
