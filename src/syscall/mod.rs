mod table;

use table::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Status {
    // Status::Success = 0
    InvalidArgument = 1,   // EINVAL
    BadAddress = 2,        // EFAULT
    BadFileDescriptor = 3, // EBADF
    NoSuchTask = 4,        // ESRCH
    OutOfMemory = 5,       // ENOMEM
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallNumber {
    Yield = 1,
    Exit = 2,
    Write = 3,
}

impl TryFrom<u64> for SyscallNumber {
    type Error = Status;
    fn try_from(val: u64) -> Result<Self, Self::Error> {
        match val {
            1 => Ok(Self::Yield),
            2 => Ok(Self::Exit),
            3 => Ok(Self::Write),
            _ => Err(Status::InvalidArgument),
        }
    }
}

pub fn handle(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, _arg4: u64, _arg5: u64) -> u64 {
    let result = (|| -> Result<(), Status> {
        let syscall = SyscallNumber::try_from(num)?;
        match syscall {
            SyscallNumber::Yield => sys_yield(),
            SyscallNumber::Exit => sys_exit(arg0 as usize),
            SyscallNumber::Write => {
                sys_write(arg0 as usize, arg1 as usize, arg2 as usize, arg3 as usize)
            }
        }
    })();

    match result {
        Ok(()) => 0,
        Err(err) => err as u64,
    }
}
