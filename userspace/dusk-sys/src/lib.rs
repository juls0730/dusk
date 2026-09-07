#![no_std]

use core::arch::asm;

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    // Success = 0,
    InvalidArgument = 1,
    BadAddress = 2,
    BadFileDescriptor = 3,
    NoSuchTask = 4,
    OutOfMemory = 5,
}

// Opaque handle type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(usize);

// our own address space and thread handle are always given to us
pub const SELF_AS: Handle = Handle(0);
pub const SELF_THREAD: Handle = Handle(1);

impl From<usize> for Status {
    fn from(value: usize) -> Self {
        match value {
            1 => Self::InvalidArgument,
            2 => Self::BadAddress,
            3 => Self::BadFileDescriptor,
            4 => Self::NoSuchTask,
            5 => Self::OutOfMemory,
            _ => Self::InvalidArgument,
        }
    }
}

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

pub fn sys_yield() {
    unsafe {
        asm!(
            "syscall",
            in("rax") 1usize,
            lateout("rcx") _,
            lateout("r11") _,
        );
    }
}

fn debug_write(buf: &str) -> Result<(), Status> {
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") 1,
            in("rsi") buf.as_ptr(),
            in("rdx") buf.len(),
            in("r10") 0,
            inlateout("rax") SyscallNumber::Write as usize => status,
            lateout("rcx") _,
            lateout("r11") _
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(())
        }
    }
}

struct DebugWriter;

impl core::fmt::Write for DebugWriter {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        debug_write(value).map_err(|_| core::fmt::Error)
    }
}

#[doc(hidden)]
pub fn __print(arguments: core::fmt::Arguments<'_>) {
    use core::fmt::Write;

    let _ = DebugWriter.write_fmt(arguments);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::__print(core::format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    () => {{
        $crate::print!("\n");
    }};
    ($($arg:tt)*) => {{
        $crate::print!("{}\n", core::format_args!($($arg)*));
    }};
}

pub fn sys_exit(exit_code: usize) -> ! {
    unsafe {
        asm!(
            "syscall",
            in("rdi") exit_code,
            in("rax") SyscallNumber::Exit as usize,
            options(noreturn)
        );
    }
}

pub fn sys_send(dest_task_id: usize, msg_ptr: usize, len: usize) -> Result<(), Status> {
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") dest_task_id,
            in("rsi") msg_ptr,
            in("rdx") len,
            inlateout("rax") SyscallNumber::Send as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(())
        }
    }
}

pub fn sys_recv(buf_ptr: usize, max_len: usize) -> Result<(usize, usize), Status> {
    let mut actual_len: usize = 0;
    let mut sender: usize = 0;

    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") buf_ptr,
            in("rsi") max_len,
            in("rdx") &raw mut actual_len as usize,
            in("r10") &raw mut sender as usize,
            inlateout("rax") SyscallNumber::Recv as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok((actual_len, sender))
        }
    }
}

pub fn sys_frame_alloc() -> Result<Handle, Status> {
    let mut handle: usize = 0;
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") &raw mut handle as usize,
            inlateout("rax") SyscallNumber::FrameAlloc as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(Handle(handle))
        }
    }
}

pub fn sys_frame_dealloc(frame_handle: Handle) -> Result<(), Status> {
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") frame_handle.0,
            inlateout("rax") SyscallNumber::FrameDealloc as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(())
        }
    }
}

pub fn sys_as_create() -> Result<Handle, Status> {
    let mut handle: usize = 0;
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") &raw mut handle as usize,
            inlateout("rax") SyscallNumber::AsCreate as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(Handle(handle))
        }
    }
}

pub fn sys_map(
    as_handle: Handle,
    frame_handle: Handle,
    virtual_addr: usize,
    permissions: usize,
) -> Result<(), Status> {
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") as_handle.0,
            in("rsi") frame_handle.0,
            in("rdx") virtual_addr,
            in("r10") permissions,
            inlateout("rax") SyscallNumber::Map as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(())
        }
    }
}

pub fn sys_unmap(as_handle: Handle, virtual_addr: usize) -> Result<(), Status> {
    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") as_handle.0,
            in("rsi") virtual_addr,
            inlateout("rax") SyscallNumber::Unmap as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(())
        }
    }
}

pub fn sys_task_create(
    as_handle: Handle,
    entry: usize,
    user_stack: usize,
) -> Result<Handle, Status> {
    let mut handle: usize = 0;

    unsafe {
        let status: usize;

        asm!(
            "syscall",
            in("rdi") as_handle.0,
            in("rsi") entry,
            in("rdx") user_stack,
            in("r10") &raw mut handle as usize,
            inlateout("rax") SyscallNumber::TaskCreate as usize => status,
            lateout("rcx") _,
            lateout("r11") _,
        );

        if status != 0 {
            Err(status.into())
        } else {
            Ok(Handle(handle))
        }
    }
}
