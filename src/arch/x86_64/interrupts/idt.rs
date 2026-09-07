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
        Self {
            offset_low: 0,
            code_selector: 0,
            ist: 0,
            attributes: 0,
            offset_middle: 0,
            offset_high: 0,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InterruptStackFrame {
    pub instruction_pointer: VirtualAddr,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: VirtualAddr,
    pub stack_segment: u64,
}

#[repr(C)]
#[derive(Debug)]
pub struct InterruptFrame {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub error_code: u64,
    pub stack_frame: InterruptStackFrame,
}

const INTERRUPT_GATE: u8 = 0b1110;
const PRESENT: u8 = 1 << 7;
const KERNEL_INTERRUPT_GATE: u8 = PRESENT | INTERRUPT_GATE;
const USER_DPL: u8 = 3 << 5;
const USER_INTERRUPT_GATE: u8 = PRESENT | USER_DPL | INTERRUPT_GATE;

pub(super) type RawHandler = unsafe extern "C" fn();

pub(super) struct Idt {
    entries: [IdtEntry; 256],
}

impl Idt {
    pub const fn new() -> Self {
        Self {
            entries: [IdtEntry::missing(); 256],
        }
    }

    pub(super) fn set_handler(&mut self, vector: u8, handler: RawHandler, ist: u8) {
        self.set_handler_address(vector, handler as usize, ist, KERNEL_INTERRUPT_GATE);
    }

    pub(super) fn set_user_handler(&mut self, vector: u8, handler: RawHandler, ist: u8) {
        self.set_handler_address(vector, handler as usize, ist, USER_INTERRUPT_GATE);
    }

    fn set_handler_address(&mut self, vector: u8, address: usize, ist: u8, attributes: u8) {
        self.entries[vector as usize] = IdtEntry {
            offset_low: address as u16,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            code_selector: KERNEL_CODE_SELECTOR,
            ist: ist & 0b111,
            attributes,
            reserved: 0,
        };
    }
}

static mut IDT: Idt = Idt::new();

const _: () = assert!(core::mem::size_of::<IdtEntry>() == 16);
const _: () = assert!(core::mem::size_of::<IdtPointer>() == 10);
const _: () = assert!(core::mem::size_of::<InterruptStackFrame>() == 40);
const _: () = assert!(core::mem::size_of::<InterruptFrame>() == 176);
const _: () = assert!(core::mem::offset_of!(InterruptFrame, stack_frame) == 136);

#[unsafe(naked)]
pub(super) unsafe extern "C" fn interrupt_common() {
    core::arch::naked_asm!(
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push rbp",
        "push rbx",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rax",

        // Check CS: bit 0 and 1 are CPL. If CPL != 0 (user mode), swapgs
        "test byte ptr [rsp + 144], 3",
        "jz 1f",
        "swapgs",
        "1:",

        "mov rdi, rsp",
        "cld",
        "call {dispatch}",

        // Check CS: if returning to user mode, swapgs
        "test byte ptr [rsp + 144], 3",
        "jz 2f",
        "swapgs",
        "2:",

        "pop rax",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop rbx",
        "pop rbp",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",

        "add rsp, 16",
        "iretq",

        dispatch = sym interrupt_dispatch,
    );
}

extern "C" fn interrupt_dispatch(frame: &mut InterruptFrame) {
    let vector = frame.vector as u8;
    match vector {
        0..=31 | 0x80 => exceptions::handle(frame),
        apic_vectors::PIT_CALIBRATION_VECTOR
        | apic_vectors::APIC_TIMER_VECTOR
        | apic_vectors::APIC_ERROR_VECTOR
        | apic_vectors::APIC_SPURIOUS_VECTOR => apic_vectors::handle(frame),
        _ => {
            crate::println!("Unhandled interrupt vector: {:#X}", vector);
        }
    }
}

macro_rules! stub_no_err {
    ($name:ident, $vec:literal) => {
        #[unsafe(naked)]
        pub(super) unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "push 0",
                concat!("push ", stringify!($vec)),
                "jmp {common}",
                common = sym $crate::arch::x86_64::interrupts::idt::interrupt_common,
            );
        }
    };
}

macro_rules! stub_err {
    ($name:ident, $vec:literal) => {
        #[unsafe(naked)]
        pub(super) unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                concat!("push ", stringify!($vec)),
                "jmp {common}",
                common = sym $crate::arch::x86_64::interrupts::idt::interrupt_common,
            );
        }
    };
}

pub(super) use stub_err;
pub(super) use stub_no_err;

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
