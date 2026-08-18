use core::arch::asm;

mod exceptions;
mod idt;

pub use idt::idt_init as init;

pub fn disable_interrupts() {
    unsafe {
        asm!("cli");
    }
}
