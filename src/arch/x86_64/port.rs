use core::arch::asm;

#[inline(always)]
pub unsafe fn read_u8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

// #[inline(always)]
// pub unsafe fn read_u8_slice(port: u16, slice: &mut [u8]) {
//     unsafe {
//         asm!(
//             "rep insb",
//             in("dx") port,
//             inout("rdi") slice.as_mut_ptr() => _,
//             inout("rcx") slice.len() => _,
//             options(nostack, preserves_flags),
//         );
//     }
// }

// #[inline(always)]
// pub unsafe fn read_u16(port: u16) -> u16 {
//     let value: u16;
//     unsafe {
//         asm!(
//             "in ax, dx",
//             in("dx") port,
//             out("ax") value,
//             options(nomem, nostack, preserves_flags),
//         );
//     }
//     value
// }

// #[inline(always)]
// pub unsafe fn read_u32(port: u16) -> u32 {
//     let value: u32;
//     unsafe {
//         asm!(
//             "in eax, dx",
//             in("dx") port,
//             out("eax") value,
//             options(nomem, nostack, preserves_flags),
//         );
//     }
//     value
// }

#[inline(always)]
pub unsafe fn write_u8(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

// #[inline(always)]
// pub unsafe fn write_u8_slice(port: u16, slice: &[u8]) {
//     unsafe {
//         asm!(
//             "rep outsb",
//             in("dx") port,
//             inout("rsi") slice.as_ptr() => _,
//             inout("rcx") slice.len() => _,
//             options(nostack, preserves_flags),
//         );
//     }
// }

// #[inline(always)]
// pub unsafe fn write_u16(port: u16, value: u16) {
//     unsafe {
//         asm!(
//             "out dx, ax",
//             in("dx") port,
//             in("ax") value,
//             options(nomem, nostack, preserves_flags),
//         );
//     }
// }

// #[inline(always)]
// pub unsafe fn write_u32(port: u16, value: u32) {
//     unsafe {
//         asm!(
//             "out dx, eax",
//             in("dx") port,
//             in("eax") value,
//             options(nomem, nostack, preserves_flags),
//         );
//     }
// }
