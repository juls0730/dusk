use crate::{
    memory::{FRAME_SIZE, VirtualAddr, address_space::AddressSpaceId, with_address_space},
    syscall::Status,
};

pub const USER_SPACE_END: VirtualAddr = VirtualAddr::new(0x0000_8000_0000_0000);

pub fn validate_user_range(
    as_id: AddressSpaceId,
    start: VirtualAddr,
    len: usize,
    writable: bool,
) -> Result<(), Status> {
    let start_addr = start.as_usize();
    let end_addr = start_addr.checked_add(len).ok_or(Status::BadAddress)?;

    if start_addr >= USER_SPACE_END.as_usize() || end_addr > USER_SPACE_END.as_usize() {
        return Err(Status::BadAddress);
    }

    if len == 0 {
        return Ok(());
    }

    let page_start = start_addr & !(FRAME_SIZE - 1);
    for page in (page_start..end_addr).step_by(FRAME_SIZE) {
        let is_valid = with_address_space(as_id, |address_space| {
            address_space
                .mapping(VirtualAddr::new(page))
                .is_some_and(|mapping| {
                    mapping.permissions.user_accessible
                        && (!writable || mapping.permissions.writable)
                })
        })
        .ok_or(Status::BadAddress)?;

        if !is_valid {
            return Err(Status::BadAddress);
        }
    }

    Ok(())
}

/// # Safety
///
/// The caller must ensure that the user address range is valid, mapped, and user-accessible (e.g. via [`validate_user_range`]).
pub unsafe fn copy_from_user(src: VirtualAddr, dst: &mut [u8]) -> Result<(), Status> {
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

/// # Safety
///
/// The caller must ensure that the user address range is valid, mapped, user-accessible, and writable (e.g. via [`validate_user_range`]).
pub unsafe fn copy_to_user(dst: VirtualAddr, src: &[u8]) -> Result<(), Status> {
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

/// # Safety
///
/// The caller must ensure that the user address is valid, mapped, user-accessible, and writable (e.g. via [`validate_user_range`]).
pub unsafe fn copy_val_to_user<T: Copy>(dst: VirtualAddr, val: &T) -> Result<(), Status> {
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

/// # Safety
///
/// The caller must ensure that the user address is valid, mapped, and user-accessible (e.g. via [`validate_user_range`]).
#[allow(unused)]
pub unsafe fn copy_val_from_user<T: Copy>(src: VirtualAddr) -> Result<T, Status> {
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
