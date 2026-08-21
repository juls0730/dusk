mod address_space;
mod frame;

pub use address_space::{AddressSpace, AddressSpaceCreateError, MapError, UnmapError};
pub use frame::{FRAME_SIZE, FrameAllocator, PhysicalFrame};

pub struct KernelSegment {
    pub physical_base: PhysicalAddr,
    pub virtual_base: VirtualAddr,
    pub length: usize,
    pub permissions: PagePermissions,
}

pub struct KernelMemoryLayout {
    pub segments: [KernelSegment; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub fn new(addr: usize) -> Self {
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
    pub fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub unsafe fn as_mut_ptr<T>(self) -> *mut T {
        self.as_usize() as *mut T
    }

    pub unsafe fn as_ptr<T>(self) -> *const T {
        self.as_usize() as *const T
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: PhysicalAddr,
    pub length: usize,
    pub kind: MemoryRegionKind,
}

#[derive(Debug, Clone, Copy)]
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
