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
    BadHandle = 6,         // EBADH
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallNumber {
    Yield = 1,
    Exit = 2,
    Write = 3,
    Send = 4,
    Recv = 5,
    FrameAlloc = 6,
    FrameDealloc = 7,
    AsCreate = 8,
    Map = 9,
    Unmap = 10,
    TaskCreate = 11,
}

impl TryFrom<u64> for SyscallNumber {
    type Error = Status;
    fn try_from(val: u64) -> Result<Self, Self::Error> {
        match val {
            1 => Ok(Self::Yield),
            2 => Ok(Self::Exit),
            3 => Ok(Self::Write),
            4 => Ok(Self::Send),
            5 => Ok(Self::Recv),
            6 => Ok(Self::FrameAlloc),
            7 => Ok(Self::FrameDealloc),
            8 => Ok(Self::AsCreate),
            9 => Ok(Self::Map),
            10 => Ok(Self::Unmap),
            11 => Ok(Self::TaskCreate),
            _ => Err(Status::InvalidArgument),
        }
    }
}

pub fn handle(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, _arg5: u64) -> u64 {
    let result = (|| -> Result<(), Status> {
        let syscall = SyscallNumber::try_from(num)?;
        match syscall {
            SyscallNumber::Yield => sys_yield(),
            SyscallNumber::Exit => sys_exit(arg0 as usize),
            SyscallNumber::Write => {
                sys_write(arg0 as usize, arg1 as usize, arg2 as usize, arg3 as usize)
            }
            SyscallNumber::Send => sys_send(arg0 as usize, arg1 as usize, arg2 as usize),
            SyscallNumber::Recv => {
                sys_recv(arg0 as usize, arg1 as usize, arg2 as usize, arg3 as usize)
            }
            SyscallNumber::FrameAlloc => sys_frame_alloc(arg0 as usize),
            SyscallNumber::FrameDealloc => sys_frame_dealloc(arg0 as usize),
            SyscallNumber::AsCreate => sys_as_create(arg0 as usize),
            SyscallNumber::Map => sys_map(
                arg0 as usize,
                arg1 as usize,
                arg2 as usize,
                arg3 as usize,
                arg4 as usize,
            ),
            SyscallNumber::Unmap => sys_unmap(arg0 as usize),
            SyscallNumber::TaskCreate => {
                sys_task_create(arg0 as usize, arg1 as usize, arg2 as usize, arg3 as usize)
            }
        }
    })();

    match result {
        Ok(()) => 0,
        Err(err) => err as u64,
    }
}
