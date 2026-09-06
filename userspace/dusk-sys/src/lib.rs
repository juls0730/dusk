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
            inlateout("rax") 3usize => status,
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
            in("rax") 2usize,
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
            inlateout("rax") 4usize => status,
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
            inlateout("rax") 5usize => status,
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
