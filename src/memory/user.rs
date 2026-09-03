use crate::{memory::VirtualAddr, syscall::Status};

pub const USER_SPACE_END: VirtualAddr = VirtualAddr::new(0x0000_8000_0000_0000);

pub fn copy_from_user(src: VirtualAddr, dst: &mut [u8]) -> Result<(), Status> {
    // TODO: guard against unmapped pages
    let end = src
        .as_usize()
        .checked_add(dst.len())
        .ok_or(Status::BadAddress)?;
    if end > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(())
}

pub fn copy_to_user(dst: VirtualAddr, src: &[u8]) -> Result<(), Status> {
    let end = dst
        .as_usize()
        .checked_add(src.len())
        .ok_or(Status::BadAddress)?;
    if end > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr::<u8>(), src.len());
    }

    Ok(())
}

pub fn copy_val_to_user<T: Copy>(dst: VirtualAddr, val: &T) -> Result<(), Status> {
    if dst.as_usize() % core::mem::align_of::<T>() != 0 {
        return Err(Status::InvalidArgument);
    }
    let end = dst
        .as_usize()
        .checked_add(core::mem::size_of::<T>())
        .ok_or(Status::BadAddress)?;
    if end > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    unsafe {
        (dst.as_mut_ptr::<T>()).write(*val);
    }

    Ok(())
}

pub fn copy_val_from_user<T: Copy>(src: VirtualAddr) -> Result<T, Status> {
    if src.as_usize() % core::mem::align_of::<T>() != 0 {
        return Err(Status::InvalidArgument);
    }
    let end = src
        .as_usize()
        .checked_add(core::mem::size_of::<T>())
        .ok_or(Status::BadAddress)?;
    if end > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    let val = unsafe { src.as_ptr::<T>().read() };
    Ok(val)
}
