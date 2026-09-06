use crate::{
    format,
    memory::{
        self, AddressSpace, DirectMap, FRAME_SIZE, FrameAllocator, PagePermissions, UserStack,
        VirtualAddr,
    },
    println,
    task::tcb::Tcb,
};

pub fn spawn(
    name: &str,
    initramfs: &[u8],
    kernel_as: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
    stacks: &mut crate::memory::KernelStackPool,
) -> usize {
    let bytes = format::cpio::find_file(initramfs, name)
        .unwrap_or_else(|| panic!("{name} missing from initramfs"));
    let kernel_stack = stacks
        .allocate(kernel_as, allocator)
        .expect("kernel stack allocation failed");

    let mut address_space = kernel_as
        .new_user(allocator)
        .expect("address space allocation failed");
    let user_stack =
        UserStack::allocate(&mut address_space, allocator).expect("user stack allocation failed");

    let entry = load_elf(bytes, &mut address_space, allocator, direct_map).expect("invalid ELF");

    let task = Tcb::new_user(0, address_space, kernel_stack, entry, user_stack.top());
    crate::task::scheduler::add_task(task).expect("scheduler is full")
}

pub fn root(
    initramfs: &[u8],
    kernel_as: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
    stacks: &mut crate::memory::KernelStackPool,
) {
    spawn(
        "omega3.elf",
        initramfs,
        kernel_as,
        allocator,
        direct_map,
        stacks,
    );
}

#[derive(Debug)]
enum ElfLoadError {
    AddressTranslationFailed,
    FailedToMapSegment,
    AddressOverflow,
    InvalidStack,
    OutOfMemory,
    InvalidElf,
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

fn load_elf(
    bytes: &[u8],
    user_address_space: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
) -> Result<VirtualAddr, ElfLoadError> {
    let program = format::elf::Elf::parse(bytes).map_err(|_| ElfLoadError::InvalidElf)?;
    if !is_loadable(&program) {
        return Err(ElfLoadError::InvalidElf);
    }

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

    Ok(VirtualAddr::new(program.entry()))
}
