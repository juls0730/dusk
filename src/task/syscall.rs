use crate::{hcf, println};

pub fn handle(
    syscall_num: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> u64 {
    println!(
        "Syscall nr={:#X} args=({:#X}, {:#X}, {:#X}, {:#X}, {:#X}, {:#X})",
        syscall_num, arg0, arg1, arg2, arg3, arg4, arg5
    );
    hcf();
    0
}
