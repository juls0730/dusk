mod address_space;
mod frame;
mod stack;
mod user;

use core::ops::Add;

#[allow(unused)]
pub use address_space::{
    AddressSpace, AddressSpaceCreateError, AddressSpaceId, MapError, PageTableMapping, UnmapError,
    init_kernel_address_space, insert_address_space, remove_address_space, with_address_space,
    with_address_space_mut, with_kernel_address_space,
};
pub use frame::{
    FRAME_SIZE, FrameAddr, FrameAllocator, OwnedFrame, alloc_frame, dealloc_frame,
    init_global as init_frame_allocator, with_allocator,
};
#[allow(unused)]
pub use stack::{KernelStack, KernelStackPool, StackCreateError, UserStack};
#[allow(unused)]
pub use user::*;

pub struct BootString<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> BootString<N> {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let len = bytes.len();
        let mut boot_string = Self { bytes: [0; N], len };
        boot_string.bytes[..len].copy_from_slice(bytes);
        boot_string
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
}

pub struct InitramfsImage {
    pub start: VirtualAddr,
    pub length: usize,
}

impl InitramfsImage {
    pub fn data(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.start.as_ptr(), self.length) }
    }
}

pub struct KernelSegment {
    pub physical_base: PhysicalAddr,
    pub virtual_base: VirtualAddr,
    pub length: usize,
    pub permissions: PagePermissions,
}

pub struct KernelMemoryLayout {
    pub segments: [KernelSegment; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PagePermissions {
    pub writable: bool,
    pub executable: bool,
    pub user_accessible: bool,
}

impl PagePermissions {
    pub const fn new(writable: bool, executable: bool, user_accessible: bool) -> Self {
        Self {
            writable,
            executable,
            user_accessible,
        }
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalAddr(usize);

impl PhysicalAddr {
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub const fn as_usize(self) -> usize {
        self.0
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualAddr(usize);

impl VirtualAddr {
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub const unsafe fn as_mut_ptr<T>(self) -> *mut T {
        self.as_usize() as *mut T
    }

    pub const unsafe fn as_ptr<T>(self) -> *const T {
        self.as_usize() as *const T
    }
}

impl Add<usize> for VirtualAddr {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionKind {
    Usable,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    BadMemory,
    BootloaderReclaimable,
    KernelAndModules,
    Framebuffer,
    MappedReserved,
}

#[derive(Clone, Copy)]
pub enum CachePolicy {
    Uncacheable,
    WriteBack,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: PhysicalAddr,
    pub length: usize,
    pub kind: MemoryRegionKind,
}

#[derive(Clone, Copy)]
pub struct DirectMap {
    offset: usize,
}

impl DirectMap {
    pub fn new(offset: usize) -> Self {
        Self { offset }
    }

    pub fn translate(self, addr: PhysicalAddr) -> Option<VirtualAddr> {
        addr.as_usize()
            .checked_add(self.offset)
            .map(VirtualAddr::new)
    }
}

const EMPTY_MEMORY_REGION: MemoryRegion = MemoryRegion {
    start: PhysicalAddr::new(0),
    length: 0,
    kind: MemoryRegionKind::Reserved,
};
const MAX_MEMORY_REGIONS: usize = 128;

pub enum MemoryMapError {
    TooManyRegions,
}

#[derive(Clone, Copy)]
pub struct MemoryMap {
    pub entries: [MemoryRegion; MAX_MEMORY_REGIONS],
    pub len: usize,
}

impl MemoryMap {
    pub const fn new() -> Self {
        Self {
            entries: [EMPTY_MEMORY_REGION; MAX_MEMORY_REGIONS],
            len: 0,
        }
    }

    pub fn push(&mut self, entry: MemoryRegion) -> Result<(), MemoryMapError> {
        if self.len >= MAX_MEMORY_REGIONS {
            return Err(MemoryMapError::TooManyRegions);
        }

        self.entries[self.len] = entry;
        self.len += 1;

        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = MemoryRegion> + Clone + '_ {
        self.entries[..self.len].iter().copied()
    }
}
