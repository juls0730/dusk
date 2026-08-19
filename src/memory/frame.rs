use core::ops::Range;

use crate::memory::{DirectMap, MemoryRegion, MemoryRegionKind, PhysicalAddr, VirtualAddr};

pub const FRAME_SIZE: usize = 4096;

pub fn align_up_to_frame(addr: usize) -> Option<usize> {
    addr.checked_add(FRAME_SIZE as usize - 1)
        .map(|addr| addr & !(FRAME_SIZE - 1))
}

pub fn align_down_to_frame(addr: usize) -> usize {
    addr & !(FRAME_SIZE - 1)
}

#[derive(Debug)]
struct Bitmap {
    start: VirtualAddr,
    bit_count: usize,
}

// 1 = available, 0 = unavailable
impl Bitmap {
    fn new(start: VirtualAddr, bit_count: usize) -> Self {
        Self { start, bit_count }
    }

    fn is_available(&self, idx: usize) -> bool {
        assert!(idx < self.bit_count, "index out of bounds");

        let byte = idx / 8;
        let bit = idx % 8;

        unsafe { self.start.as_mut_ptr::<u8>().add(byte).read() & (1 << bit) != 0 }
    }

    fn set_available(&mut self, idx: usize, available: bool) {
        assert!(idx < self.bit_count, "index out of bounds");

        let byte = idx / 8;
        let bit = idx % 8;

        let val = unsafe { self.start.as_mut_ptr::<u8>().add(byte).read() };
        unsafe {
            self.start.as_mut_ptr::<u8>().add(byte).write(if available {
                val | (1 << bit)
            } else {
                val & !(1 << bit)
            });
        }
    }

    // fn fill_available(&mut self, range: Range<usize>, available: bool) -> bool {
    //     if range.start > range.end {
    //         return false;
    //     }

    //     if range.end > self.bit_count {
    //         return false;
    //     }

    //     if range.is_empty() {
    //         return true;
    //     }

    //     let ptr = unsafe { self.start.as_mut_ptr::<u8>() };
    //     let first_byte = range.start / 8;
    //     let last_byte = (range.end - 1) / 8;
    //     let start_bit = range.start % 8;
    //     let end_bit = range.end % 8;

    //     unsafe {
    //         if first_byte == last_byte {
    //             let width = range.end - range.start;
    //             let mask = (((1u16 << width) - 1) << start_bit) as u8;
    //             let byte = ptr.add(first_byte).read();

    //             ptr.add(first_byte)
    //                 .write(if available { byte | mask } else { byte & !mask });

    //             return true;
    //         }

    //         let mut full_start = first_byte;

    //         if start_bit != 0 {
    //             let mask = u8::MAX << start_bit;
    //             let byte = ptr.add(first_byte).read();

    //             ptr.add(first_byte)
    //                 .write(if available { byte | mask } else { byte & !mask });

    //             full_start += 1;
    //         }

    //         let full_end = range.end / 8;

    //         ptr.add(full_start)
    //             .write_bytes(if available { u8::MAX } else { 0 }, full_end - full_start);

    //         if end_bit != 0 {
    //             let mask = (1u8 << end_bit) - 1;
    //             let byte = ptr.add(full_end).read();

    //             ptr.add(full_end)
    //                 .write(if available { byte | mask } else { byte & !mask });
    //         }
    //     }

    //     true
    // }
}

#[derive(Debug)]
pub enum FrameAllocatorInitError {
    AddressOverflow,
    NoUsableFrames,
    NoBitmapStorage,
    BitmapOutsideDirectMap,
}

// very very simple linked list frame/page allocator
#[derive(Debug)]
pub struct FrameAllocator {
    bitmap: Bitmap,
    bitmap_start_frame: usize,
    bitmap_frame_count: usize,
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
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            let range = Self::usable_frame_range(region)?;
            if range.is_empty() {
                continue;
            }

            highest_frame = Some(highest_frame.map_or(range.end, |current| current.max(range.end)));
        }

        let highest_frame = highest_frame.ok_or(FrameAllocatorInitError::NoUsableFrames)?;

        let bitmap_bytes = highest_frame.div_ceil(8);
        let bitmap_frame_count = bitmap_bytes.div_ceil(FRAME_SIZE);
        let bitmap_storage_bytes = bitmap_frame_count
            .checked_mul(FRAME_SIZE)
            .ok_or(FrameAllocatorInitError::AddressOverflow)?;

        let mut bitmap_start_frame: Option<usize> = None;

        for region in regions.clone() {
            if region.kind != MemoryRegionKind::Usable {
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
            if region.kind != MemoryRegionKind::Usable {
                continue;
            }

            for frame_idx in Self::usable_frame_range(region)? {
                let is_bitmap_storage =
                    frame_idx >= bitmap_start_frame && frame_idx < bitmap_end_frame;
                if is_bitmap_storage {
                    continue;
                }

                bitmap.set_available(frame_idx, true);
                allocatable_frames += 1;
                free_frames += 1;
            }
        }

        Ok(Self {
            bitmap,
            bitmap_start_frame: bitmap_start_frame,
            bitmap_frame_count,
            next_search: bitmap_start_frame + bitmap_frame_count,
            allocatable_frames,
            free_frames,
            direct_map,
        })
    }

    fn find_free_in(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).find(|&index| self.bitmap.is_available(index))
    }

    fn find_free_frame(&self) -> Option<usize> {
        self.find_free_in(self.next_search, self.bitmap.bit_count)
            .or_else(|| self.find_free_in(0, self.next_search))
    }

    pub fn alloc_nozero(&mut self) -> Option<PhysicalFrame> {
        if self.free_frames == 0 {
            return None;
        }

        let frame_idx = self.find_free_frame()?;

        self.bitmap.set_available(frame_idx, false);
        self.free_frames -= 1;
        self.next_search = frame_idx.saturating_add(1);

        Some(PhysicalFrame::from_index(frame_idx))
    }

    pub fn alloc(&mut self) -> Option<PhysicalFrame> {
        let frame = self.alloc_nozero()?;

        let start = self
            .direct_map
            .translate(frame.start_address())
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
    pub unsafe fn dealloc(&mut self, frame: PhysicalFrame) {
        let frame_idx = frame.index();

        assert!(
            frame_idx < self.bitmap.bit_count,
            "frame index out of bounds"
        );
        assert!(
            !self.is_bitmap_storage(frame_idx),
            "attempted to free frame allocator bitmap"
        );
        assert!(
            !self.bitmap.is_available(frame_idx),
            "frame is already free"
        );

        self.bitmap.set_available(frame_idx, true);
        self.free_frames += 1;
        self.next_search = self.next_search.min(frame_idx);
    }

    pub const fn free_frames(&self) -> usize {
        self.free_frames
    }

    pub const fn allocatable_frames(&self) -> usize {
        self.allocatable_frames
    }

    fn is_bitmap_storage(&self, index: usize) -> bool {
        index >= self.bitmap_start_frame
            && index < (self.bitmap_start_frame + self.bitmap_frame_count)
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
pub struct PhysicalFrame(PhysicalAddr);

impl PhysicalFrame {
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
