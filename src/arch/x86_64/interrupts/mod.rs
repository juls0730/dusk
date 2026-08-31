use core::arch::asm;

pub(super) mod apic_vectors;
mod exceptions;
mod idt;

pub use idt::idt_init as init;

#[inline(always)]
pub fn disable_interrupts_and_save() -> u64 {
    let flags: u64;

    unsafe {
        asm!("
            pushfq",
            "pop {flags}",
            "cli",
            flags = out(reg) flags,
        );
    }

    flags
}

#[inline(always)]
pub fn restore_interrupts(flags: u64) {
    unsafe {
        asm!(
            "push {flags}",
            "popfq",
            flags = in(reg) flags,
        );
    }
}

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
