#![no_std]
#![no_main]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::fmt::Write;

fn sys_yield() {
    unsafe {
        asm!("mov rax, 1", "syscall");
    }
}

fn sys_write(fd: usize, buf: &str) -> Result<(), usize> {
    unsafe {
        let status;

        asm!(
            "mov rax, 3",
            "syscall",
            in("rdi") fd,
            in("rsi") buf.as_ptr(),
            in("rdx") buf.len(),
            in("r10") 0,
            lateout("rax") status,
        );

        if status != 0 { Err(status) } else { Ok(()) }
    }
}

struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        sys_write(1, s).map_err(|_| core::fmt::Error)?;
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => (let _ = $crate::Writer.write_fmt(format_args!($($arg)*)););
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn sys_exit(exit_code: usize) -> ! {
    unsafe {
        asm!(
            "mov rax, 2",
            "syscall",
            in("rdi") exit_code,
            options(noreturn)
        );
    }
}

struct Heap {
    pub data: UnsafeCell<[u8; 1024]>,
}

impl Heap {
    const fn new() -> Self {
        Self {
            data: UnsafeCell::new([0; 1024]),
        }
    }

    fn len(&self) -> usize {
        unsafe { (*self.data.get()).len() }
    }
}

impl core::ops::Deref for Heap {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { (*self.data.get()).as_ref() }
    }
}

unsafe impl Sync for Heap {}

static HEAP: Heap = Heap::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    (0..100).for_each(|i| {
        println!("Hello {}", i);
        sys_yield();
    });

    let len = HEAP.len();
    for i in 0..len {
        unsafe { HEAP.data.get().as_mut().unwrap()[i] = i as u8 };
    }

    (0..1024).for_each(|i| {
        println!("{}", unsafe { HEAP.data.get().as_ref().unwrap()[i] });
        sys_yield();
    });

    sys_exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    sys_exit(1);
}
