use crate::{
    memory::{
        FRAME_SIZE, OwnedFrame, PagePermissions, USER_SPACE_END, VirtualAddr, copy_from_user,
        copy_to_user, copy_val_to_user,
    },
    println,
    task::tcb::{BlockReason, Handle, KernelObject, MAX_MSG_SIZE, Message, Rights},
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

    let mut chunk = [0u8; 128];
    let mut written = 0;
    while written < len {
        let n = (len - written).min(chunk.len());
        copy_from_user(VirtualAddr::new(buf_ptr + written), &mut chunk[..n])?;
        crate::debug::serial::write_bytes(&chunk[..n]);
        written += n;
    }

    if out_ptr != 0 {
        copy_val_to_user(VirtualAddr::new(out_ptr), &written)?;
    }

    Ok(())
}

pub fn sys_send(dest_task_id: usize, msg_ptr: usize, len: usize) -> Result<(), Status> {
    if len > MAX_MSG_SIZE {
        return Err(Status::InvalidArgument);
    }

    let mut msg_buf = [0u8; MAX_MSG_SIZE];
    copy_from_user(VirtualAddr::new(msg_ptr), &mut msg_buf[..len])?;

    let dest_task = crate::task::scheduler::get_task_mut(dest_task_id).ok_or(Status::NoSuchTask)?;
    let sender = crate::task::scheduler::current();

    let msg = Message {
        sender,
        length: len,
        data: msg_buf,
    };

    if !dest_task.mailbox.push(msg) {
        return Err(Status::OutOfMemory);
    }

    if matches!(
        dest_task.state,
        crate::task::tcb::ThreadState::Blocked(BlockReason::Recv { .. })
    ) {
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

    let current_task = crate::task::scheduler::get_task_mut(crate::task::scheduler::current())
        .ok_or(Status::NoSuchTask)?;

    if current_task.mailbox.len == 0 {
        crate::task::scheduler::block_current(BlockReason::Recv { ep: 0 });
    }

    // if we blocked, we will wake up when the mailbox is non-empty

    let current_task = crate::task::scheduler::get_task_mut(crate::task::scheduler::current())
        .ok_or(Status::NoSuchTask)?;
    let msg = current_task.mailbox.pop().ok_or(Status::NoSuchTask)?;

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

    Ok(())
}

pub fn sys_frame_alloc(out_handle: usize) -> Result<(), Status> {
    if out_handle == 0 {
        return Err(Status::InvalidArgument);
    }

    let frame = crate::memory::alloc_frame().ok_or(Status::OutOfMemory)?;

    let task_id = crate::task::scheduler::current();
    let task = match crate::task::scheduler::get_task_mut(task_id) {
        Some(task) => task,
        None => {
            unsafe { crate::memory::dealloc_frame(frame) };
            return Err(Status::NoSuchTask);
        }
    };

    let frame_addr = frame.into_raw();
    let handle = Handle {
        object: KernelObject::Frame(frame_addr),
        rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
    };

    let handle_id = match task.handles.push(handle) {
        Some(id) => id,
        None => {
            unsafe { crate::memory::dealloc_frame(OwnedFrame::from_raw(frame_addr)) };
            return Err(Status::OutOfMemory);
        }
    };

    copy_val_to_user(VirtualAddr::new(out_handle), &handle_id)?;

    Ok(())
}

pub fn sys_frame_dealloc(frame_handle: usize) -> Result<(), Status> {
    let task = crate::task::scheduler::current();
    let task = crate::task::scheduler::get_task_mut(task).ok_or(Status::NoSuchTask)?;

    let frame_handle = task.handles.get(frame_handle).ok_or(Status::BadHandle)?;
    let frame = match frame_handle.object {
        KernelObject::Frame(frame_addr) => frame_addr,
        _ => return Err(Status::InvalidArgument),
    };

    unsafe { crate::memory::dealloc_frame(OwnedFrame::from_raw(frame)) };

    Ok(())
}

pub fn sys_as_create(out_handle: usize) -> Result<(), Status> {
    if out_handle == 0 {
        return Err(Status::InvalidArgument);
    }

    let task_id = crate::task::scheduler::current();
    let task = crate::task::scheduler::get_task_mut(task_id).ok_or(Status::NoSuchTask)?;

    let new_as = match crate::memory::with_address_space(task.as_id, |caller_as| {
        crate::memory::with_allocator(|allocator| caller_as.new_user(allocator))
    }) {
        Some(Ok(as_space)) => as_space,
        _ => return Err(Status::OutOfMemory),
    };

    let as_id = crate::memory::insert_address_space(new_as).ok_or(Status::OutOfMemory)?;

    let handle = Handle {
        object: KernelObject::AddressSpace(as_id),
        rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
    };

    let handle_id = match task.handles.push(handle) {
        Some(id) => id,
        None => {
            crate::memory::remove_address_space(as_id);
            return Err(Status::OutOfMemory);
        }
    };

    copy_val_to_user(VirtualAddr::new(out_handle), &handle_id)?;

    Ok(())
}

pub fn sys_map(
    as_handle: usize,
    frame_handle: usize,
    virtual_addr: usize,
    permissions: usize,
) -> Result<(), Status> {
    if virtual_addr % FRAME_SIZE != 0 {
        return Err(Status::InvalidArgument);
    }

    if virtual_addr >= USER_SPACE_END.as_usize() {
        return Err(Status::InvalidArgument);
    }

    let task = crate::task::scheduler::current();
    let task = crate::task::scheduler::get_task_mut(task).ok_or(Status::NoSuchTask)?;

    let as_handle = task.handles.get(as_handle).ok_or(Status::BadHandle)?;
    let as_id = match as_handle.object {
        KernelObject::AddressSpace(as_id) => as_id,
        _ => return Err(Status::InvalidArgument),
    };

    let frame_handle = task.handles.get(frame_handle).ok_or(Status::BadHandle)?;
    let frame = match frame_handle.object {
        KernelObject::Frame(frame_addr) => frame_addr,
        _ => return Err(Status::InvalidArgument),
    };

    let virtual_addr = VirtualAddr::new(virtual_addr);
    let permissions = PagePermissions::new(
        permissions & (1 << 0) != 0,
        permissions & (1 << 1) != 0,
        true,
    );

    let map_result = crate::memory::with_address_space_mut(as_id, |target_as| {
        crate::memory::with_allocator(|allocator| {
            target_as.map(
                frame.start_address(),
                virtual_addr,
                permissions,
                allocator,
                crate::memory::CachePolicy::WriteBack,
            )
        })
    })
    .ok_or(Status::BadHandle)?;

    map_result.map_err(|_| Status::OutOfMemory)?;

    Ok(())
}

pub fn sys_unmap(as_handle: usize, virtual_addr: usize) -> Result<(), Status> {
    if virtual_addr % FRAME_SIZE != 0 {
        return Err(Status::InvalidArgument);
    }

    if virtual_addr >= USER_SPACE_END.as_usize() {
        return Err(Status::InvalidArgument);
    }

    let task = crate::task::scheduler::current();
    let task = crate::task::scheduler::get_task_mut(task).ok_or(Status::NoSuchTask)?;

    let as_handle = task.handles.get(as_handle).ok_or(Status::BadHandle)?;
    let as_id = match as_handle.object {
        KernelObject::AddressSpace(as_id) => as_id,
        _ => return Err(Status::InvalidArgument),
    };

    let virtual_addr = VirtualAddr::new(virtual_addr);

    let map_result = crate::memory::with_address_space_mut(as_id, |target_as| {
        if target_as.to_physical(virtual_addr).is_none() {
            return Err(Status::InvalidArgument);
        }

        crate::memory::with_allocator(|allocator| unsafe {
            target_as.unmap(virtual_addr, allocator)
        })
        .map_err(|_| Status::BadAddress)
    })
    .ok_or(Status::BadHandle)?;

    map_result.map_err(|_| Status::OutOfMemory)?;

    Ok(())
}

pub fn sys_task_create(
    as_handle: usize,
    entry: usize,
    user_stack: usize,
    out_task_handle: usize,
) -> Result<(), Status> {
    if out_task_handle == 0 || entry == 0 || user_stack == 0 {
        return Err(Status::InvalidArgument);
    }

    if entry >= 0x0000_8000_0000_0000 || user_stack >= 0x0000_8000_0000_0000 {
        return Err(Status::BadAddress);
    }

    let task_id = crate::task::scheduler::current();
    let task = crate::task::scheduler::get_task_mut(task_id).ok_or(Status::NoSuchTask)?;

    let as_handle = task.handles.get(as_handle).ok_or(Status::BadHandle)?;
    let as_id = match as_handle.object {
        KernelObject::AddressSpace(as_id) => as_id,
        _ => return Err(Status::InvalidArgument),
    };

    if as_handle.rights.0 & Rights::EXECUTE.0 == 0 {
        return Err(Status::InvalidArgument);
    }

    if crate::memory::with_address_space(as_id, |_| ()).is_none() {
        return Err(Status::BadHandle);
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
        0, // assigned by scheduler::add_task
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

    let handle_id = match task.handles.push(handle) {
        Some(id) => id,
        None => {
            crate::task::scheduler::remove_task(new_task_id);
            return Err(Status::OutOfMemory);
        }
    };

    copy_val_to_user(VirtualAddr::new(out_task_handle), &handle_id)?;

    Ok(())
}
