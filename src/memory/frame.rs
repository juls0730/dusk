use core::cell::UnsafeCell;

use crate::memory::{DirectMap, MemoryRegion, MemoryRegionKind, PhysicalAddr, VirtualAddr};

pub const FRAME_SIZE: usize = 4096;

pub fn align_up_to_frame(addr: usize) -> Option<usize> {
    addr.checked_add(FRAME_SIZE as usize - 1)
        .map(|addr| addr & !(FRAME_SIZE - 1))
}

pub fn align_down_to_frame(addr: usize) -> usize {
    addr & !(FRAME_SIZE - 1)
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Reserved = 0b00,
    Free = 0b01,
    Allocated = 0b10,
}

struct GlobalFrameAllocator(UnsafeCell<Option<FrameAllocator>>);
unsafe impl Sync for GlobalFrameAllocator {}

static FRAME_ALLOCATOR: GlobalFrameAllocator = GlobalFrameAllocator(UnsafeCell::new(None));

pub fn init_global(allocator: FrameAllocator) {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    unsafe {
        *FRAME_ALLOCATOR.0.get() = Some(allocator);
    }
    crate::arch::restore_interrupts(interrupt_state);
}

pub fn alloc_frame() -> Option<OwnedFrame> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let allocator = unsafe { &mut *FRAME_ALLOCATOR.0.get() };
    let frame = allocator.as_mut().and_then(|a| a.alloc());
    crate::arch::restore_interrupts(interrupt_state);
    frame
}

pub unsafe fn dealloc_frame(frame: OwnedFrame) {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let allocator = unsafe { &mut *FRAME_ALLOCATOR.0.get() };
    if let Some(a) = allocator.as_mut() {
        unsafe { a.dealloc(frame) };
    }
    crate::arch::restore_interrupts(interrupt_state);
}

#[allow(unused)]
pub fn with_allocator<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> R {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let allocator = unsafe {
        (&mut *FRAME_ALLOCATOR.0.get())
            .as_mut()
            .expect("frame allocator not initialized")
    };
    let result = f(allocator);
    crate::arch::restore_interrupts(interrupt_state);
    result
}

// 64 KiB per GiB
struct Bitmap {
    start: VirtualAddr,
    frame_count: usize,
}

impl Bitmap {
    fn new(start: VirtualAddr, frame_count: usize) -> Self {
        Self { start, frame_count }
    }

    fn state(&self, frame_idx: usize) -> FrameState {
        assert!(frame_idx < self.frame_count, "frame index out of bounds");

        let byte_idx = frame_idx / 4;
        let shift = (frame_idx % 4) * 2;
        let byte = unsafe { self.start.as_ptr::<u8>().add(byte_idx).read() };

        match (byte >> shift) & 0b11 {
            0b00 => FrameState::Reserved,
            0b01 => FrameState::Free,
            0b10 => FrameState::Allocated,
            _ => panic!("invalid frame state"),
        }
    }

    fn set_state(&mut self, frame_idx: usize, state: FrameState) {
        assert!(frame_idx < self.frame_count, "frame index out of bounds");

        let byte_idx = frame_idx / 4;
        let shift = (frame_idx % 4) * 2;
        let ptr = unsafe { self.start.as_mut_ptr::<u8>().add(byte_idx) };
        let byte = unsafe { ptr.read() };
        let mask = 0b11 << shift;

        unsafe {
            ptr.write((byte & !mask) | ((state as u8) << shift));
        }
    }
}

#[derive(Debug)]
pub enum FrameAllocatorInitError {
    AddressOverflow,
    NoUsableFrames,
    NoBitmapStorage,
    BitmapOutsideDirectMap,
}

// very very simple bitmap frame/page allocator
pub struct FrameAllocator {
    bitmap: Bitmap,
    next_search: usize,
    allocatable_frames: usize,
    free_frames: usize,
    direct_map: DirectMap,
}

impl FrameAllocator {
    pub fn new<I>(regions: I, direct_map: DirectMap) -> Result<Self, FrameAllocatorInitError>
    where
        I: Iterator<Item = MemoryRegion> + Clone,
    {
        let mut highest_frame: Option<usize> = None;

        for region in regions.clone() {
            if !Self::should_track(region.kind) {
                continue;
            }

            let range = Self::usable_frame_range(region)?;
            if range.is_empty() {
                continue;
            }

            highest_frame = Some(highest_frame.map_or(range.end, |current| current.max(range.end)));
        }

        let highest_frame = highest_frame.ok_or(FrameAllocatorInitError::NoUsableFrames)?;

        let bitmap_bytes = highest_frame.div_ceil(4);
        let bitmap_frame_count = bitmap_bytes.div_ceil(FRAME_SIZE);
        let bitmap_storage_bytes = bitmap_frame_count
            .checked_mul(FRAME_SIZE)
            .ok_or(FrameAllocatorInitError::AddressOverflow)?;

        let mut bitmap_start_frame: Option<usize> = None;

        for region in regions.clone() {
            if !Self::can_store_bitmap(region.kind) {
                continue;
            }

            let range = Self::usable_frame_range(region)?;
            if range.is_empty() {
                continue;
            }

            if range.len() < bitmap_frame_count {
                continue;
            }

            bitmap_start_frame = Some(range.start);
            break;
        }

        if bitmap_start_frame.is_none() {
            return Err(FrameAllocatorInitError::NoBitmapStorage);
        }

        let bitmap_start_frame = bitmap_start_frame.unwrap();

        let bitmap_physical_addr = PhysicalAddr::new(bitmap_start_frame * FRAME_SIZE);
        let bitmap_virtual = direct_map
            .translate(bitmap_physical_addr)
            .ok_or(FrameAllocatorInitError::BitmapOutsideDirectMap)?;

        unsafe {
            // set everyting to unavailable
            core::ptr::write_bytes(bitmap_virtual.as_mut_ptr::<u8>(), 0, bitmap_storage_bytes);
        }

        let mut allocatable_frames = 0;
        let mut free_frames = 0;
        let mut bitmap = Bitmap::new(bitmap_virtual, highest_frame);

        let bitmap_end_frame = bitmap_start_frame
            .checked_add(bitmap_frame_count)
            .ok_or(FrameAllocatorInitError::AddressOverflow)?;

        for region in regions {
            if !Self::is_initially_free(region.kind) {
                continue;
            }

            for frame_idx in Self::usable_frame_range(region)? {
                let is_bitmap_storage =
                    frame_idx >= bitmap_start_frame && frame_idx < bitmap_end_frame;
                if is_bitmap_storage {
                    continue;
                }

                bitmap.set_state(frame_idx, FrameState::Free);
                allocatable_frames += 1;
                free_frames += 1;
            }
        }

        Ok(Self {
            bitmap,
            next_search: bitmap_start_frame + bitmap_frame_count,
            allocatable_frames,
            free_frames,
            direct_map,
        })
    }

    fn should_track(kind: MemoryRegionKind) -> bool {
        matches!(
            kind,
            MemoryRegionKind::Usable
                | MemoryRegionKind::BootloaderReclaimable
                | MemoryRegionKind::AcpiReclaimable
        )
    }

    fn can_store_bitmap(kind: MemoryRegionKind) -> bool {
        kind == MemoryRegionKind::Usable
    }

    fn is_initially_free(kind: MemoryRegionKind) -> bool {
        kind == MemoryRegionKind::Usable
    }

    fn find_free_in(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).find(|&index| self.bitmap.state(index) == FrameState::Free)
    }

    fn find_free_frame(&self) -> Option<usize> {
        self.find_free_in(self.next_search, self.bitmap.frame_count)
            .or_else(|| self.find_free_in(0, self.next_search))
    }

    pub fn reclaim_regions<I: Iterator<Item = MemoryRegion> + Clone>(
        &mut self,
        memory_map: I,
        region_kind: MemoryRegionKind,
    ) {
        if !matches!(
            region_kind,
            MemoryRegionKind::BootloaderReclaimable | MemoryRegionKind::AcpiReclaimable
        ) {
            return;
        }

        for region in memory_map {
            if region.kind == region_kind {
                for frame_idx in Self::usable_frame_range(region).expect("invalid memory region") {
                    if self.bitmap.state(frame_idx) != FrameState::Reserved {
                        continue;
                    }

                    self.bitmap.set_state(frame_idx, FrameState::Free);
                    self.allocatable_frames += 1;
                    self.free_frames += 1;
                    self.next_search = self.next_search.min(frame_idx);
                }
            }
        }
    }

    pub fn alloc_nozero(&mut self) -> Option<OwnedFrame> {
        if self.free_frames == 0 {
            return None;
        }

        let frame_idx = self.find_free_frame()?;

        self.bitmap.set_state(frame_idx, FrameState::Allocated);
        self.free_frames -= 1;
        self.next_search = frame_idx.saturating_add(1);

        Some(OwnedFrame::new(FrameAddr::from_index(frame_idx)))
    }

    pub fn alloc(&mut self) -> Option<OwnedFrame> {
        let frame = self.alloc_nozero()?;

        let start = self
            .direct_map
            .translate(frame.frame_address().start_address())
            .expect("frame is outside the direct map");

        unsafe {
            core::ptr::write_bytes(start.as_mut_ptr::<u8>(), 0, FRAME_SIZE);
        }

        Some(frame)
    }

    /// # Safety
    ///
    /// The caller must ensure:
    /// - The frame is currently owned by the caller
    /// - it is not currently in use
    /// - it has not been freed
    pub unsafe fn dealloc(&mut self, frame: OwnedFrame) {
        let frame_idx = frame.index();

        match self.bitmap.state(frame_idx) {
            FrameState::Allocated => {
                self.bitmap.set_state(frame_idx, FrameState::Free);
                self.free_frames += 1;
                self.next_search = self.next_search.min(frame_idx);
            }
            FrameState::Free => panic!("attempted to free free frame"),
            FrameState::Reserved => {
                panic!("attempted to free reserved frame");
            }
        };
    }

    fn usable_frame_range(
        region: MemoryRegion,
    ) -> Result<core::ops::Range<usize>, FrameAllocatorInitError> {
        let region_end = region
            .start
            .as_usize()
            .checked_add(region.length)
            .ok_or(FrameAllocatorInitError::AddressOverflow)?;

        let start = align_up_to_frame(region.start.as_usize())
            .ok_or(FrameAllocatorInitError::AddressOverflow)?;
        let end = align_down_to_frame(region_end);

        Ok((start / FRAME_SIZE)..(end / FRAME_SIZE))
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameAddr(PhysicalAddr);

impl FrameAddr {
    pub fn from_start_address(address: PhysicalAddr) -> Option<Self> {
        if address.as_usize() % FRAME_SIZE != 0 {
            return None;
        }

        Some(Self(address))
    }

    pub fn start_address(&self) -> PhysicalAddr {
        self.0
    }

    fn from_index(index: usize) -> Self {
        Self(PhysicalAddr::new(index * FRAME_SIZE))
    }

    fn index(&self) -> usize {
        self.0.as_usize() / FRAME_SIZE
    }
}

// specifically not Clone or Copy
pub struct OwnedFrame {
    frame: FrameAddr,
}

impl OwnedFrame {
    fn new(frame: FrameAddr) -> Self {
        Self { frame }
    }

    pub fn into_raw(self) -> FrameAddr {
        self.frame
    }

    pub unsafe fn from_raw(frame: FrameAddr) -> Self {
        Self { frame }
    }

    pub fn frame_address(&self) -> FrameAddr {
        self.frame
    }

    pub fn index(&self) -> usize {
        self.frame.index()
    }
}
