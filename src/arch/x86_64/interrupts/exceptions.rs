use core::arch::asm;

use super::idt::{self, InterruptStackFrame};
use crate::{hcf, println};

macro_rules! fatal_without_error_code {
    ($handler:ident, $name:literal) => {
        extern "x86-interrupt" fn $handler(frame: InterruptStackFrame) {
            fatal_exception($name, &frame, None);
        }
    };
}

macro_rules! fatal_with_error_code {
    ($handler:ident, $name:literal) => {
        extern "x86-interrupt" fn $handler(frame: InterruptStackFrame, error_code: u64) {
            fatal_exception($name, &frame, Some(error_code));
        }
    };
}

extern "x86-interrupt" fn debug_handler(frame: InterruptStackFrame) {
    fatal_exception("DEBUG EXCEPTION", &frame, None);
}

extern "x86-interrupt" fn non_maskable_interrupt_handler(frame: InterruptStackFrame) {
    fatal_exception("NON-MASKABLE INTERRUPT", &frame, None);
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    report_exception("BREAKPOINT", &frame, None);
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    fatal_exception("DOUBLE FAULT", &frame, Some(error_code));
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    report_exception("PAGE FAULT", &frame, Some(error_code));
    println!("Faulting address: {:#X}", read_cr2());
    print_page_fault_error(error_code);
    hcf();
}

fatal_without_error_code!(divide_error_handler, "DIVIDE ERROR");
fatal_without_error_code!(invalid_opcode_handler, "INVALID OPCODE");
fatal_without_error_code!(device_not_available_handler, "DEVICE NOT AVAILABLE");
fatal_without_error_code!(x87_floating_point_handler, "X87 FLOATING-POINT EXCEPTION");
fatal_without_error_code!(machine_check_handler, "MACHINE CHECK");
fatal_without_error_code!(simd_floating_point_handler, "SIMD FLOATING-POINT EXCEPTION");

fatal_with_error_code!(invalid_tss_handler, "INVALID TSS");
fatal_with_error_code!(segment_not_present_handler, "SEGMENT NOT PRESENT");
fatal_with_error_code!(stack_segment_fault_handler, "STACK-SEGMENT FAULT");
fatal_with_error_code!(general_protection_handler, "GENERAL PROTECTION FAULT");
fatal_with_error_code!(alignment_check_handler, "ALIGNMENT CHECK");

pub(super) fn install(idt: &mut idt::Idt) {
    idt.set_handler(0, divide_error_handler, 0);
    idt.set_handler(1, debug_handler, 0);
    idt.set_handler(2, non_maskable_interrupt_handler, 0);
    idt.set_handler(3, breakpoint_handler, 0);
    idt.set_handler(6, invalid_opcode_handler, 0);
    idt.set_handler(7, device_not_available_handler, 0);
    idt.set_error_code_handler(8, double_fault_handler, 1);
    idt.set_error_code_handler(10, invalid_tss_handler, 0);
    idt.set_error_code_handler(11, segment_not_present_handler, 0);
    idt.set_error_code_handler(12, stack_segment_fault_handler, 0);
    idt.set_error_code_handler(13, general_protection_handler, 0);
    idt.set_error_code_handler(14, page_fault_handler, 0);
    idt.set_handler(16, x87_floating_point_handler, 0);
    idt.set_error_code_handler(17, alignment_check_handler, 0);
    idt.set_handler(18, machine_check_handler, 0);
    idt.set_handler(19, simd_floating_point_handler, 0);
}

fn read_cr2() -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "mov {}, cr2",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

fn print_page_fault_error(error_code: u64) {
    let present = error_code & (1 << 0) != 0;
    let write = error_code & (1 << 1) != 0;
    let user = error_code & (1 << 2) != 0;

    println!(
        "Cause: {}",
        if present {
            "protection violation"
        } else {
            "page not present"
        }
    );
    println!("Access: {}", if write { "write" } else { "read" });
    println!("Mode: {}", if user { "user" } else { "supervisor" });

    if error_code & (1 << 3) != 0 {
        println!("Reserved page-table bit was set");
    }

    if error_code & (1 << 4) != 0 {
        println!("Access was an instruction fetch");
    }

    if error_code & (1 << 5) != 0 {
        println!("Protection-key violation");
    }

    if error_code & (1 << 6) != 0 {
        println!("Shadow-stack access");
    }

    if error_code & (1 << 15) != 0 {
        println!("Software Guard Extensions violation");
    }
}

fn report_exception(name: &'static str, frame: &InterruptStackFrame, error_code: Option<u64>) {
    println!();
    println!("========= {name} =========");
    println!(
        "Origin: {}",
        if frame.code_segment & 0b11 == 3 {
            "user"
        } else {
            "kernel"
        }
    );
    println!("RIP: {:#X}", frame.instruction_pointer.as_u64());
    println!("CS: {:#X}", frame.code_segment);
    println!("FLAGS: {:#X}", frame.cpu_flags);
    println!("RSP: {:#X}", frame.stack_pointer.as_u64());
    println!("SS: {:#X}", frame.stack_segment);

    if let Some(error_code) = error_code {
        println!("ERROR: {:#X}", error_code);
    }
}

fn fatal_exception(name: &'static str, frame: &InterruptStackFrame, error_code: Option<u64>) -> ! {
    report_exception(name, frame, error_code);
    hcf()
}
