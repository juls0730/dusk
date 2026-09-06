use crate::{
    memory::{VirtualAddr, copy_from_user, copy_to_user, copy_val_to_user},
    task::tcb::{BlockReason, MAX_MSG_SIZE, Message},
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

    let mut interrupt_state = crate::arch::disable_interrupts_and_save();

    let current_task = crate::task::scheduler::get_task_mut(crate::task::scheduler::current())
        .ok_or(Status::NoSuchTask)?;

    if current_task.mailbox.len == 0 {
        crate::arch::restore_interrupts(interrupt_state);
        crate::task::scheduler::block_current(BlockReason::Recv { ep: 0 });
        interrupt_state = crate::arch::disable_interrupts_and_save();
    }

    // if we blocked, we will wake up when the mailbox is non-empty

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

    crate::arch::restore_interrupts(interrupt_state);

    Ok(())
}
