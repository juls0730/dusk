use core::arch::asm;

pub(super) mod apic_vectors;
mod exceptions;
mod idt;

pub use idt::idt_init as init;

#[inline(always)]
pub fn disable_interrupts() {
    unsafe {
        asm!("cli", options(nostack));
    }
}

#[inline(always)]
pub fn enable_interrupts() {
    unsafe {
        asm!("sti", options(nostack));
    }
}
