mod gdt;
mod interrupts;
pub mod port;

use core::arch::asm;

pub use interrupts::disable_interrupts;

use crate::println;

pub fn init() {
    disable_interrupts();
    println!("Loading GDT...");
    gdt::init();
    println!("Loading IDT...");
    interrupts::init();
}

pub fn halt() {
    unsafe {
        asm!("hlt");
    }
}
