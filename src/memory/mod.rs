mod frame;

pub use frame::{FRAME_SIZE, FrameAllocator, PhysicalFrame};

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
