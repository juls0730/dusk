use crate::{
    format,
    memory::{
        self, AddressSpace, DirectMap, FRAME_SIZE, FrameAllocator, InitramfsImage, PagePermissions,
        UserStack, VirtualAddr,
    },
    task::{scheduler::TaskId, tcb::Tcb},
};

pub fn spawn(
    name: &str,
    initramfs: &InitramfsImage,
    kernel_as: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
) -> TaskId {
    let bytes = format::cpio::find_file(initramfs.data(), name)
        .unwrap_or_else(|| panic!("{name} missing from initramfs"));
    let kernel_stack = crate::task::scheduler::allocate_kernel_stack(kernel_as, allocator)
        .expect("kernel stack allocation failed");

    let initramfs_physical_addr = kernel_as
        .to_physical(initramfs.start)
        .expect("failed to translate initramfs start address");

    let mut address_space = kernel_as
        .new_user(allocator)
        .expect("address space allocation failed");
    address_space
        .map_range(
            initramfs_physical_addr,
            VirtualAddr::new(0x4000_0000),
            ((initramfs.length) + 0xFFF) & !0xFFF,
            PagePermissions::new(true, false, true),
            allocator,
            memory::CachePolicy::WriteBack,
        )
        .expect("failed to map initramfs");
    let user_stack =
        UserStack::allocate(&mut address_space, allocator).expect("user stack allocation failed");

    let entry = load_elf(bytes, &mut address_space, allocator, direct_map).expect("invalid ELF");

    let as_id = match crate::memory::insert_address_space(address_space) {
        Ok(id) => id,
        Err(_) => {
            panic!("address space table is full");
        }
    };

    let task = Tcb::new_user(as_id, kernel_stack, entry, user_stack.top());
    crate::task::scheduler::add_task(task).expect("scheduler is full")
}

#[derive(Debug)]
enum ElfLoadError {
    AddressTranslationFailed,
    FailedToMapSegment,
    OutOfMemory,
    InvalidElf,
}

fn load_elf(
    bytes: &[u8],
    user_address_space: &mut AddressSpace,
    allocator: &mut FrameAllocator,
    direct_map: DirectMap,
) -> Result<VirtualAddr, ElfLoadError> {
    let program = format::elf::Elf::parse(bytes).map_err(|_| ElfLoadError::InvalidElf)?;
    let mut executable_entry = false;

    for segment in program.segments() {
        let segment = segment.map_err(|_| ElfLoadError::InvalidElf)?;
        let end = segment
            .address
            .checked_add(segment.memory_size)
            .filter(|&end| end <= memory::USER_SPACE_END.as_usize())
            .ok_or(ElfLoadError::InvalidElf)?;
        if segment.memory_size == 0 {
            continue;
        }
        executable_entry |= segment.executable && (segment.address..end).contains(&program.entry);

        let page_start = segment.address & !(FRAME_SIZE - 1);
        let file_end = segment.address + segment.data.len();
        let permissions = PagePermissions::new(segment.writable, segment.executable, true);

        // Overlapping segment pages are rejected by map(), including stack/archive collisions.
        for page in (page_start..end).step_by(FRAME_SIZE) {
            let frame = allocator.alloc_nozero().ok_or(ElfLoadError::OutOfMemory)?;
            let physical = frame.frame_address().start_address();
            let Some(destination) = direct_map.translate(physical) else {
                unsafe { allocator.dealloc(frame) };
                return Err(ElfLoadError::AddressTranslationFailed);
            };

            let copy_start = page.max(segment.address).min(page + FRAME_SIZE);
            let copy_end = (page + FRAME_SIZE).min(file_end).max(copy_start);
            let prefix = copy_start - page;
            let copied = copy_end - copy_start;
            unsafe {
                let destination = destination.as_mut_ptr::<u8>();
                // Initialize padding and BSS, but don't zero bytes we're about to overwrite.
                core::ptr::write_bytes(destination, 0, prefix);
                if copied != 0 {
                    core::ptr::copy_nonoverlapping(
                        segment.data.as_ptr().add(copy_start - segment.address),
                        destination.add(prefix),
                        copied,
                    );
                }
                core::ptr::write_bytes(
                    destination.add(prefix + copied),
                    0,
                    FRAME_SIZE - prefix - copied,
                );
            }

            if user_address_space
                .map(
                    physical,
                    VirtualAddr::new(page),
                    permissions,
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
    }

    if !executable_entry {
        return Err(ElfLoadError::InvalidElf);
    }
    Ok(VirtualAddr::new(program.entry))
}
