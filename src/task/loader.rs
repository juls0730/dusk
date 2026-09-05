use crate::{
    format,
    memory::{
        self, AddressSpace, DirectMap, FRAME_SIZE, FrameAllocator, OwnedFrame, PagePermissions,
        VirtualAddr,
    },
    println,
};

const MAX_LOAD_SEGMENTS: usize = 32;

#[derive(Debug)]
struct LoadedRegion {
    start: VirtualAddr,
    mapped_pages: usize,
}

#[derive(Debug)]
pub struct LoadedImage {
    pub entry: VirtualAddr,
    regions: [LoadedRegion; MAX_LOAD_SEGMENTS],
    region_count: usize,
}

impl LoadedImage {
    /// # Safety
    /// The supplied address space must contain this image's original mappings.
    /// Its frames must be exclusively owned by this image and no longer in use.
    pub unsafe fn destroy(self, address_space: &mut AddressSpace, allocator: &mut FrameAllocator) {
        for region in self.regions[..self.region_count].iter().rev() {
            for page in (0..region.mapped_pages).rev() {
                let address = VirtualAddr::new(region.start.as_usize() + page * FRAME_SIZE);
                let frame = unsafe {
                    address_space
                        .unmap(address, allocator)
                        .expect("loaded image mapping was unexpectedly missing")
                };
                unsafe { allocator.dealloc(OwnedFrame::from_raw(frame)) };
            }
        }
    }
}

#[derive(Debug)]
pub enum ElfLoadError {
    AddressTranslationFailed,
    FailedToMapSegment,
    AddressOverflow,
    InvalidStack,
    OutOfMemory,
    InvalidElf,
    TooManyLoadSegments,
}

#[cfg(target_arch = "x86_64")]
fn is_loadable(elf: &format::elf::Elf) -> bool {
    // on x86_64, we only support ELFs that are either 32 bit x86 or 64 bit x86
    matches!(
        elf.machine(),
        format::elf::ElfIsa::X86 | format::elf::ElfIsa::Amd64
    )
}

#[cfg(not(target_arch = "x86_64"))]
fn is_loadable(elf: &format::elf::Elf) -> bool {
    false
}

pub fn load_elf(
    bytes: &[u8],
    user_address_space: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
) -> Result<LoadedImage, ElfLoadError> {
    let program = format::elf::Elf::parse(bytes).map_err(|_| ElfLoadError::InvalidElf)?;
    if !is_loadable(&program) {
        return Err(ElfLoadError::InvalidElf);
    }

    let mut image = LoadedImage {
        entry: VirtualAddr::new(program.entry()),
        regions: core::array::from_fn(|_| LoadedRegion {
            start: VirtualAddr::new(0),
            mapped_pages: 0,
        }),
        region_count: 0,
    };

    let result = (|| {
        for header in program
            .program_headers()
            .map_err(|_| ElfLoadError::InvalidElf)?
        {
            println!("Processing program header: {:?}", header);
            let header = header.map_err(|_| ElfLoadError::InvalidElf)?;

            if header.file_size > header.memory_size {
                return Err(ElfLoadError::InvalidElf);
            }

            match header.segment_type {
                format::elf::ProgramHeaderType::Load => {
                    if image.region_count == MAX_LOAD_SEGMENTS {
                        return Err(ElfLoadError::TooManyLoadSegments);
                    }
                    // TODO: give a fuck about alignment
                    // TODO: handle program segments that overlap
                    let segment_start = header.virtual_address as usize;
                    if (program.entry() >= segment_start
                        && program.entry() < segment_start + header.memory_size as usize)
                        && header.flags & 0x01 == 0
                    {
                        // entry is within NX segment
                        return Err(ElfLoadError::InvalidElf);
                    }

                    let page_start = segment_start & !(FRAME_SIZE - 1);
                    let page_offset = segment_start - page_start;

                    let mapped_length = page_offset
                        .checked_add(header.memory_size as usize)
                        .ok_or(ElfLoadError::AddressOverflow)?
                        .div_ceil(FRAME_SIZE)
                        * FRAME_SIZE;

                    let frame_count = mapped_length / FRAME_SIZE;

                    let executable = header.flags & 0x01 != 0;
                    let writable = header.flags & 0x02 != 0;
                    // TODO: support only-executable segments
                    // let readable = header.flags & 0x04 != 0;

                    let region = &mut image.regions[image.region_count];
                    region.start = VirtualAddr::new(page_start);
                    image.region_count += 1;

                    for i in 0..frame_count {
                        let frame = allocator.alloc().ok_or(ElfLoadError::OutOfMemory)?;

                        println!(
                            "Mapping code frame: {:X?} to {:X?}",
                            frame,
                            page_start + i * FRAME_SIZE
                        );

                        if user_address_space
                            .map(
                                frame.frame_address().start_address(),
                                VirtualAddr::new(page_start + i * FRAME_SIZE),
                                PagePermissions::new(writable, executable, true),
                                allocator,
                                memory::CachePolicy::WriteBack,
                            )
                            .is_err()
                        {
                            unsafe { allocator.dealloc(frame) };
                            return Err(ElfLoadError::FailedToMapSegment);
                        }

                        let _ = frame.into_raw();
                        region.mapped_pages += 1;
                    }

                    let mut copied = 0;

                    while copied < header.file_size as usize {
                        let destination = VirtualAddr::new(segment_start + copied);
                        let physical = user_address_space
                            .to_physical(destination)
                            .ok_or(ElfLoadError::AddressTranslationFailed)?;
                        let direct_mapped = direct_map
                            .translate(physical)
                            .ok_or(ElfLoadError::AddressTranslationFailed)?;

                        let page_remaining = FRAME_SIZE - destination.as_usize() % FRAME_SIZE;
                        let copy_length = page_remaining.min(header.file_size as usize - copied);

                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                program
                                    .bytes()
                                    .as_ptr()
                                    .add(header.file_offset as usize + copied),
                                direct_mapped.as_mut_ptr(),
                                copy_length,
                            );
                        }

                        copied += copy_length;
                    }
                }
                format::elf::ProgramHeaderType::GnuStack => {
                    // if the stack is NOT R/W NX, we refuse to map it
                    if header.flags != 6 {
                        return Err(ElfLoadError::InvalidStack);
                    }
                }
                _ => {}
            }
        }

        Ok(())
    })();

    if let Err(error) = result {
        // Only pages created by this load are recorded; none have been handed to a task.
        unsafe { image.destroy(user_address_space, allocator) };
        return Err(error);
    }

    Ok(image)
}
