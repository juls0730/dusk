use crate::{
    memory::{
        FRAME_SIZE, MapError, PagePermissions, USER_SPACE_END, VirtualAddr, copy_from_user,
        copy_to_user, copy_val_to_user, validate_user_range,
    },
    println,
    task::{
        scheduler::TaskId,
        tcb::{BlockReason, Handle, KernelObject, MAX_MSG_SIZE, Message, Rights},
    },
};

use super::Status;

pub fn sys_yield() -> Result<(), Status> {
    crate::task::scheduler::yield_current();
    Ok(())
}

pub fn sys_exit(exit_code: usize) -> ! {
    crate::task::scheduler::exit_current(exit_code);
}

pub fn sys_write(fd: usize, buf_ptr: usize, len: usize, out_ptr: usize) -> Result<(), Status> {
    if fd != 1 && fd != 2 {
        return Err(Status::BadFileDescriptor);
    }

    crate::task::scheduler::with_task(crate::task::scheduler::current(), |current_task| {
        validate_user_range(current_task.as_id, VirtualAddr::new(buf_ptr), len, false)?;

        if out_ptr != 0 {
            if out_ptr % core::mem::align_of::<usize>() != 0 {
                return Err(Status::InvalidArgument);
            }
            validate_user_range(
                current_task.as_id,
                VirtualAddr::new(out_ptr),
                core::mem::size_of::<usize>(),
                true,
            )?;
        }

        Ok(())
    })
    .expect("failed to resolve self task")?;

    let mut chunk = [0u8; 128];
    let mut written = 0;
    while written < len {
        let n = (len - written).min(chunk.len());
        unsafe {
            copy_from_user(VirtualAddr::new(buf_ptr + written), &mut chunk[..n])?;
        }
        crate::debug::serial::write_bytes(&chunk[..n]);
        written += n;
    }

    if out_ptr != 0 {
        unsafe {
            copy_val_to_user(VirtualAddr::new(out_ptr), &written)?;
        }
    }

    Ok(())
}

pub fn sys_send(dest_task_id: usize, msg_ptr: usize, len: usize) -> Result<(), Status> {
    let dest_task_id = TaskId::new(dest_task_id);

    if len > MAX_MSG_SIZE {
        return Err(Status::InvalidArgument);
    }

    let sender = crate::task::scheduler::current();
    crate::task::scheduler::with_task(sender, |current_task| {
        validate_user_range(current_task.as_id, VirtualAddr::new(msg_ptr), len, false)
    })
    .expect("failed to resolve self task")?;

    let mut msg_buf = [0u8; MAX_MSG_SIZE];
    unsafe { copy_from_user(VirtualAddr::new(msg_ptr), &mut msg_buf[..len])? };

    let msg = Message {
        sender,
        length: len,
        data: msg_buf,
    };

    let should_unblock = crate::task::scheduler::with_task_mut(dest_task_id, |dest_task| {
        if !dest_task.mailbox.push(msg) {
            return Err(Status::OutOfMemory);
        }
        Ok(matches!(
            dest_task.state,
            crate::task::tcb::ThreadState::Blocked(BlockReason::Recv)
        ))
    })
    .ok_or(Status::NoSuchTask)??;

    if should_unblock {
        crate::task::scheduler::unblock(dest_task_id);
    }

    Ok(())
}

pub fn sys_recv(
    out_ptr: usize,
    max_len: usize,
    out_actual_len: usize,
    out_sender: usize,
) -> Result<(), Status> {
    if out_ptr == 0 {
        return Err(Status::InvalidArgument);
    }

    crate::task::scheduler::with_task(crate::task::scheduler::current(), |current_task| {
        if current_task.mailbox.len == 0 {
            crate::task::scheduler::block_current(BlockReason::Recv);
        }

        validate_user_range(current_task.as_id, VirtualAddr::new(out_ptr), max_len, true)?;

        if out_actual_len != 0 {
            if out_actual_len % core::mem::align_of::<usize>() != 0 {
                return Err(Status::InvalidArgument);
            }
            validate_user_range(
                current_task.as_id,
                VirtualAddr::new(out_actual_len),
                core::mem::size_of::<usize>(),
                true,
            )?;
        }

        if out_sender != 0 {
            if out_sender % core::mem::align_of::<usize>() != 0 {
                return Err(Status::InvalidArgument);
            }
            validate_user_range(
                current_task.as_id,
                VirtualAddr::new(out_sender),
                core::mem::size_of::<usize>(),
                true,
            )?;
        }

        Ok(())
    })
    .expect("failed to resolve self task")?;

    let msg =
        crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |current_task| {
            current_task.mailbox.pop().ok_or(Status::NoSuchTask)
        })
        .expect("failed to resolve self task")?;

    unsafe {
        copy_to_user(
            VirtualAddr::new(out_ptr),
            &msg.data[..msg.length.min(max_len)],
        )?;
        if out_actual_len != 0 {
            copy_val_to_user(VirtualAddr::new(out_actual_len), &msg.length)?;
        }
        if out_sender != 0 {
            copy_val_to_user(VirtualAddr::new(out_sender), &msg.sender)?;
        }
    }

    Ok(())
}

pub fn sys_frame_alloc(out_handle: usize) -> Result<(), Status> {
    if out_handle == 0 {
        return Err(Status::InvalidArgument);
    }

    if out_handle % core::mem::align_of::<usize>() != 0 {
        return Err(Status::InvalidArgument);
    }

    crate::task::scheduler::with_task(crate::task::scheduler::current(), |current_task| {
        validate_user_range(
            current_task.as_id,
            VirtualAddr::new(out_handle),
            core::mem::size_of::<usize>(),
            true,
        )
    })
    .expect("failed to resolve self task")?;

    let frame = crate::memory::alloc_frame().ok_or(Status::OutOfMemory)?;

    let handle = Handle {
        object: KernelObject::Frame(frame),
        rights: Rights::READ | Rights::WRITE | Rights::EXECUTE | Rights::MAP,
    };

    crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |current_task| {
        let handle_id = match current_task.handles.push(handle) {
            Ok(id) => id,
            Err(handle) => {
                let frame = match handle.object {
                    KernelObject::Frame(frame) => frame,
                    _ => unreachable!("pushed handle was not a frame"),
                };
                unsafe { crate::memory::dealloc_frame(frame) };
                return Err(Status::OutOfMemory);
            }
        };

        unsafe { copy_val_to_user(VirtualAddr::new(out_handle), &handle_id) }
    })
    .expect("failed to resolve self task")
}

pub fn sys_frame_dealloc(frame_handle_id: usize) -> Result<(), Status> {
    crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| {
        let frame_handle = task
            .handles
            .take(frame_handle_id)
            .ok_or(Status::BadHandle)?;
        match frame_handle.object {
            KernelObject::Frame(frame_addr) => {
                unsafe { crate::memory::dealloc_frame(frame_addr) };

                Ok(())
            }
            _ => {
                // Wrong-type operations must not consume the handle
                match task.handles.put(frame_handle_id, frame_handle) {
                    Ok(_) => {}
                    Err(_) => panic!("taken handle was unexpectedly occupied"),
                }

                return Err(Status::InvalidArgument);
            }
        }
    })
    .expect("failed to resolve self task")
}

pub fn sys_as_create(out_handle: usize) -> Result<(), Status> {
    if out_handle == 0 {
        return Err(Status::InvalidArgument);
    }

    if out_handle % core::mem::align_of::<usize>() != 0 {
        return Err(Status::InvalidArgument);
    }

    let new_as =
        crate::task::scheduler::with_task(crate::task::scheduler::current(), |current_task| {
            validate_user_range(
                current_task.as_id,
                VirtualAddr::new(out_handle),
                core::mem::size_of::<usize>(),
                true,
            )?;

            let new_as = match crate::memory::with_address_space(current_task.as_id, |caller_as| {
                crate::memory::with_allocator(|allocator| caller_as.new_user(allocator))
            }) {
                Some(Ok(as_space)) => as_space,
                _ => return Err(Status::OutOfMemory),
            };

            Ok(new_as)
        })
        .expect("failed to resolve self task")?;

    let as_id = match crate::memory::insert_address_space(new_as) {
        Ok(id) => id,
        Err(addr_space) => {
            crate::memory::with_allocator(|allocator| unsafe { addr_space.destroy(allocator) });
            return Err(Status::OutOfMemory);
        }
    };

    let handle = Handle {
        object: KernelObject::AddressSpace(as_id),
        rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
    };

    crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |current_task| {
        let handle_id = match current_task.handles.push(handle) {
            Ok(id) => id,
            Err(handle) => {
                let address_space = match handle.object {
                    KernelObject::AddressSpace(as_id) => crate::memory::remove_address_space(as_id)
                        .expect("address space was just inserted"),
                    _ => unreachable!("pushed handle was not an address space"),
                };
                crate::memory::with_allocator(|allocator| unsafe {
                    address_space.destroy(allocator)
                });
                return Err(Status::OutOfMemory);
            }
        };

        unsafe { copy_val_to_user(VirtualAddr::new(out_handle), &handle_id) }
    })
    .expect("failed to resolve self task")
}

pub fn sys_map(
    as_handle: usize,
    frame_handle: usize,
    virtual_addr: usize,
    permissions: usize,
    out_handle: usize,
) -> Result<(), Status> {
    if out_handle == 0 || out_handle % core::mem::align_of::<usize>() != 0 {
        return Err(Status::InvalidArgument);
    }

    if virtual_addr % FRAME_SIZE != 0 || permissions & !0b11 != 0 {
        return Err(Status::InvalidArgument);
    }

    let writable = permissions & (1 << 0) != 0;
    let executable = permissions & (1 << 1) != 0;

    let end = virtual_addr
        .checked_add(FRAME_SIZE)
        .ok_or(Status::InvalidArgument)?;
    if end > USER_SPACE_END.as_usize() {
        return Err(Status::InvalidArgument);
    }

    let current_task = crate::task::scheduler::current();
    let as_id = crate::task::scheduler::with_task(current_task, |task| {
        validate_user_range(
            task.as_id,
            VirtualAddr::new(out_handle),
            core::mem::size_of::<usize>(),
            true,
        )?;

        let as_handle = task.handles.get(as_handle).ok_or(Status::BadHandle)?;
        let as_id = match as_handle.object {
            KernelObject::AddressSpace(as_id) => as_id,
            _ => return Err(Status::InvalidArgument),
        };
        if as_handle.rights.0 & Rights::WRITE.0 == 0 {
            return Err(Status::InvalidArgument);
        }

        let frame_handle = task.handles.get(frame_handle).ok_or(Status::BadHandle)?;
        if !matches!(frame_handle.object, KernelObject::Frame(_)) {
            return Err(Status::InvalidArgument);
        }

        let mut required_rights = Rights::READ | Rights::MAP;
        if writable {
            required_rights = required_rights | Rights::WRITE;
        }
        if executable {
            required_rights = required_rights | Rights::EXECUTE;
        }
        if frame_handle.rights.0 & required_rights.0 != required_rights.0 {
            return Err(Status::InvalidArgument);
        }

        Ok(as_id)
    })
    .expect("failed to resolve self task")?;

    let handle = crate::task::scheduler::with_task_mut(current_task, |task| {
        task.handles.take(frame_handle).ok_or(Status::BadHandle)
    })
    .expect("failed to resolve self task")?;

    let Handle { object, rights } = handle;
    let KernelObject::Frame(frame) = object else {
        panic!("validated frame handle changed before it was taken");
    };

    let permissions = PagePermissions::new(writable, executable, true);
    let virtual_addr = VirtualAddr::new(virtual_addr);

    let map_result = crate::memory::with_address_space_mut(as_id, |target_as| {
        crate::memory::with_allocator(|allocator| {
            target_as.map(
                frame.frame_address().start_address(),
                virtual_addr,
                permissions,
                allocator,
                crate::memory::CachePolicy::WriteBack,
            )
        })
    })
    .expect("failed to resolve self address space");

    match map_result {
        Ok(_) => {
            crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| {
                match task.handles.put(
                    frame_handle,
                    Handle {
                        object: KernelObject::Mapping {
                            frame,
                            address_space: as_id,
                            virtual_addr,
                        },
                        rights,
                    },
                ) {
                    Ok(_) => {}
                    Err(_) => panic!("taken handle was unexpectedly occupied"),
                }
            });

            unsafe {
                copy_val_to_user(VirtualAddr::new(out_handle), &frame_handle)
                    .expect("out_handle has already been checked")
            }

            Ok(())
        }
        Err(err) => {
            crate::task::scheduler::with_task_mut(current_task, |task| {
                match task.handles.put(
                    frame_handle,
                    Handle {
                        object: KernelObject::Frame(frame),
                        rights,
                    },
                ) {
                    Ok(_) => {}
                    Err(_) => panic!("taken handle was unexpectedly occupied"),
                }
            })
            .expect("failed to resolve self task");

            match err {
                MapError::AlreadyMapped | MapError::UnsupportedPermissions => {
                    Err(Status::InvalidArgument)
                }
                MapError::OutOfMemory => Err(Status::OutOfMemory),
                MapError::InvalidVirtualAddress
                | MapError::VirtualAddressUnaligned
                | MapError::PhysicalAddressTooLarge
                | MapError::PhysicalAddressUnaligned
                | MapError::RangeLengthUnaligned
                | MapError::AddressOverflow
                | MapError::MappingConflict
                | MapError::PageTableUnavailable
                | MapError::CorruptedPageTable
                | MapError::InvalidUserAddress
                | MapError::InvalidUserMap => {
                    panic!("validated user mapping failed with an impossible error: {err:?}")
                }
            }
        }
    }
}

pub fn sys_unmap(mapping_handle: usize) -> Result<(), Status> {
    let handle = crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| {
        task.handles.take(mapping_handle).ok_or(Status::BadHandle)
    })
    .expect("failed to resolve self task")?;

    let Handle { object, rights } = handle;

    let KernelObject::Mapping {
        frame,
        address_space,
        virtual_addr,
    } = object
    else {
        // Wrong-type operations must not consume the handle
        crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| match task
            .handles
            .put(mapping_handle, Handle { object, rights })
        {
            Ok(_) => {}
            Err(_) => panic!("taken handle was unexpectedly occupied"),
        })
        .expect("failed to resolve self task");

        return Err(Status::InvalidArgument);
    };

    let unmap_result = crate::memory::with_address_space_mut(address_space, |target_as| {
        crate::memory::with_allocator(|allocator| unsafe {
            target_as.unmap(virtual_addr, allocator)
        })
    });

    match unmap_result {
        Some(Ok(unmapped_frame)) => {
            if unmapped_frame == frame.frame_address() {
                crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| {
                    match task.handles.put(
                        mapping_handle,
                        Handle {
                            object: KernelObject::Frame(frame),
                            rights,
                        },
                    ) {
                        Ok(_) => {}
                        Err(_) => panic!("taken handle was unexpectedly occupied"),
                    }
                });

                Ok(())
            } else {
                panic!("unmap resulted in a frame that was not the one we expected")
            }
        }
        Some(Err(err)) => {
            // every unmapping error should be impossible to occur
            panic!("failed to unmap: {err:?}");
        }
        None => {
            crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |task| {
                match task.handles.put(
                    mapping_handle,
                    Handle {
                        object: KernelObject::Mapping {
                            frame: frame,
                            address_space,
                            virtual_addr,
                        },
                        rights,
                    },
                ) {
                    Ok(_) => {}
                    Err(_) => panic!("taken handle was unexpectedly occupied"),
                }
            });

            Err(Status::BadHandle)
        }
    }
}

pub fn sys_task_create(
    as_handle: usize,
    entry: usize,
    user_stack: usize,
    out_task_handle: usize,
) -> Result<(), Status> {
    if entry == 0 || user_stack == 0 || out_task_handle == 0 {
        return Err(Status::InvalidArgument);
    }

    if entry >= USER_SPACE_END.as_usize() || user_stack > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    if out_task_handle % core::mem::align_of::<usize>() != 0 {
        return Err(Status::InvalidArgument);
    }

    let as_id =
        crate::task::scheduler::with_task(crate::task::scheduler::current(), |current_task| {
            validate_user_range(
                current_task.as_id,
                VirtualAddr::new(out_task_handle),
                core::mem::size_of::<usize>(),
                true,
            )?;

            let as_handle = current_task
                .handles
                .get(as_handle)
                .ok_or(Status::BadHandle)?;
            let as_id = match as_handle.object {
                KernelObject::AddressSpace(as_id) => as_id,
                _ => return Err(Status::InvalidArgument),
            };

            if as_handle.rights.0 & Rights::EXECUTE.0 == 0 {
                return Err(Status::InvalidArgument);
            }

            Ok(as_id)
        })
        .expect("failed to resolve self task")?;

    if crate::memory::with_address_space(as_id, |_| ()).is_none() {
        return Err(Status::BadHandle);
    }

    let stack_probe = user_stack.checked_sub(1).ok_or(Status::BadAddress)?;
    validate_user_range(as_id, VirtualAddr::new(stack_probe), 1, true)?;

    let entry_is_valid = crate::memory::with_address_space(as_id, |address_space| {
        address_space
            .mapping(VirtualAddr::new(entry))
            .is_some_and(|mapping| {
                mapping.permissions.user_accessible && mapping.permissions.executable
            })
    })
    .ok_or(Status::BadHandle)?;

    if !entry_is_valid {
        return Err(Status::BadAddress);
    }

    let kernel_stack = match crate::memory::with_kernel_address_space(|kernel_as| {
        crate::memory::with_allocator(|allocator| {
            crate::task::scheduler::allocate_kernel_stack(kernel_as, allocator)
        })
    }) {
        Ok(stack) => stack,
        Err(err) => {
            println!("Failed to allocate kernel stack: {:?}", err);
            return Err(Status::OutOfMemory);
        }
    };

    let new_tcb = crate::task::tcb::Tcb::new_user(
        as_id,
        kernel_stack,
        VirtualAddr::new(entry),
        VirtualAddr::new(user_stack),
    );

    let new_task_id = match crate::task::scheduler::add_task(new_tcb) {
        Ok(id) => id,
        Err(_) => return Err(Status::OutOfMemory),
    };

    let handle = Handle {
        object: KernelObject::Thread(new_task_id),
        rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
    };

    crate::task::scheduler::with_task_mut(crate::task::scheduler::current(), |current_task| {
        let handle_id = match current_task.handles.push(handle) {
            Ok(id) => id,
            Err(_) => {
                crate::task::scheduler::remove_task(new_task_id);
                return Err(Status::OutOfMemory);
            }
        };

        unsafe { copy_val_to_user(VirtualAddr::new(out_task_handle), &handle_id) }
    })
    .expect("failed to resolve self task")
}
