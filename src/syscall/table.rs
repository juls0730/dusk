use crate::memory::{VirtualAddr, copy_from_user, copy_val_to_user};

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
