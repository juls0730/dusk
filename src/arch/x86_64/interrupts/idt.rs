use super::exceptions;
use crate::{
    arch::x86_64::{gdt::KERNEL_CODE_SELECTOR, interrupts::apic_vectors},
    memory::VirtualAddr,
};

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    code_selector: u16,
    ist: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        return Self {
            offset_low: 0,
            code_selector: 0x08,
            ist: 0,
            attributes: 0,
            offset_middle: 0,
            offset_high: 0,
            reserved: 0,
        };
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct InterruptStackFrame {
    pub instruction_pointer: VirtualAddr,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: VirtualAddr,
    pub stack_segment: u64,
}

const INTERRUPT_GATE: u8 = 0b1110;
const PRESENT: u8 = 1 << 7;
const KERNEL_INTERRUPT_GATE: u8 = PRESENT | INTERRUPT_GATE;

pub(super) type Handler = extern "x86-interrupt" fn(InterruptStackFrame);
pub(super) type ErrorCodeHandler = extern "x86-interrupt" fn(InterruptStackFrame, u64);

pub(super) struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    pub const fn new() -> Self {
        Self {
            entries: [IdtEntry::missing(); 256],
        }
    }

    pub(super) fn set_handler(&mut self, vector: u8, handler: Handler, ist: u8) {
        self.set_handler_address(vector, handler as usize, ist);
    }

    pub(super) fn set_error_code_handler(
        &mut self,
        vector: u8,
        handler: ErrorCodeHandler,
        ist: u8,
    ) {
        self.set_handler_address(vector, handler as usize, ist);
    }

    fn set_handler_address(&mut self, vector: u8, address: usize, ist: u8) {
        self.entries[vector as usize] = IdtEntry {
            offset_low: address as u16,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            code_selector: KERNEL_CODE_SELECTOR,
            ist: ist & 0b111,
            attributes: KERNEL_INTERRUPT_GATE,
            reserved: 0,
        };
    }
}

static mut IDT: Idt = Idt::new();

const _: () = assert!(core::mem::size_of::<IdtEntry>() == 16);
const _: () = assert!(core::mem::size_of::<IdtPointer>() == 10);
const _: () = assert!(core::mem::size_of::<InterruptStackFrame>() == 40);

pub fn idt_init() {
    let mut idt = Idt::new();

    exceptions::install(&mut idt);
    apic_vectors::install(&mut idt);

    unsafe {
        core::ptr::addr_of_mut!(IDT).write(idt);

        let pointer = IdtPointer {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };

        core::arch::asm!(
            "lidt [{}]",
            in(reg) core::ptr::addr_of!(pointer),
            options(readonly, nostack, preserves_flags),
        );
    }
}
