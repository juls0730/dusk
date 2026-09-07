use core::arch::asm;

use super::idt::{self, InterruptFrame, InterruptStackFrame, stub_err, stub_no_err};
use crate::{hcf, println};

stub_no_err!(stub_divide_error, 0);
stub_no_err!(stub_debug, 1);
stub_no_err!(stub_non_maskable_interrupt, 2);
stub_no_err!(stub_breakpoint, 3);
stub_no_err!(stub_invalid_opcode, 6);
stub_no_err!(stub_device_not_available, 7);
stub_err!(stub_double_fault, 8);
stub_err!(stub_invalid_tss, 10);
stub_err!(stub_segment_not_present, 11);
stub_err!(stub_stack_segment_fault, 12);
stub_err!(stub_general_protection, 13);
stub_err!(stub_page_fault, 14);
stub_no_err!(stub_x87_floating_point, 16);
stub_err!(stub_alignment_check, 17);
stub_no_err!(stub_machine_check, 18);
stub_no_err!(stub_simd_floating_point, 19);
stub_no_err!(stub_user_test_exit, 0x80);

pub(super) fn handle(frame: &mut InterruptFrame) {
    match frame.vector as u8 {
        0 => fatal_exception("DIVIDE ERROR", &frame.stack_frame, None),
        1 => fatal_exception("DEBUG EXCEPTION", &frame.stack_frame, None),
        2 => fatal_exception("NON-MASKABLE INTERRUPT", &frame.stack_frame, None),
        3 => report_exception("BREAKPOINT", &frame.stack_frame, None),
        6 => fatal_exception("INVALID OPCODE", &frame.stack_frame, None),
        7 => fatal_exception("DEVICE NOT AVAILABLE", &frame.stack_frame, None),
        8 => fatal_exception("DOUBLE FAULT", &frame.stack_frame, Some(frame.error_code)),
        10 => fatal_exception("INVALID TSS", &frame.stack_frame, Some(frame.error_code)),
        11 => fatal_exception("SEGMENT NOT PRESENT", &frame.stack_frame, Some(frame.error_code)),
        12 => fatal_exception("STACK-SEGMENT FAULT", &frame.stack_frame, Some(frame.error_code)),
        13 => fatal_exception(
            "GENERAL PROTECTION FAULT",
            &frame.stack_frame,
            Some(frame.error_code),
        ),
        14 => {
            report_exception("PAGE FAULT", &frame.stack_frame, Some(frame.error_code));
            println!("Faulting address: {:#X}", read_cr2());
            print_page_fault_error(frame.error_code);
            hcf();
        }
        16 => fatal_exception("X87 FLOATING-POINT EXCEPTION", &frame.stack_frame, None),
        17 => fatal_exception("ALIGNMENT CHECK", &frame.stack_frame, Some(frame.error_code)),
        18 => fatal_exception("MACHINE CHECK", &frame.stack_frame, None),
        19 => fatal_exception("SIMD FLOATING-POINT EXCEPTION", &frame.stack_frame, None),
        0x80 => {
            if frame.stack_frame.code_segment & 0b11 != 3 {
                panic!("user_test_exit_handler called from kernel");
            }

            println!("User test exit");
            hcf();
        }
        _ => {
            println!("Unhandled exception vector: {:#X}", frame.vector);
            hcf();
        }
    }
}

pub(super) fn install(idt: &mut idt::Idt) {
    idt.set_handler(0, stub_divide_error, 0);
    idt.set_handler(1, stub_debug, 0);
    idt.set_handler(2, stub_non_maskable_interrupt, 0);
    idt.set_user_handler(3, stub_breakpoint, 0);
    idt.set_handler(6, stub_invalid_opcode, 0);
    idt.set_handler(7, stub_device_not_available, 0);
    idt.set_handler(8, stub_double_fault, 1);
    idt.set_handler(10, stub_invalid_tss, 0);
    idt.set_handler(11, stub_segment_not_present, 0);
    idt.set_handler(12, stub_stack_segment_fault, 0);
    idt.set_handler(13, stub_general_protection, 0);
    idt.set_handler(14, stub_page_fault, 0);
    idt.set_handler(16, stub_x87_floating_point, 0);
    idt.set_handler(17, stub_alignment_check, 0);
    idt.set_handler(18, stub_machine_check, 0);
    idt.set_handler(19, stub_simd_floating_point, 0);

    idt.set_user_handler(0x80, stub_user_test_exit, 0);
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
    println!("RIP: {:#X}", frame.instruction_pointer.as_usize());
    println!("CS: {:#X}", frame.code_segment);
    println!("FLAGS: {:#X}", frame.cpu_flags);
    println!("RSP: {:#X}", frame.stack_pointer.as_usize());
    println!("SS: {:#X}", frame.stack_segment);

    if let Some(error_code) = error_code {
        println!("ERROR: {:#X}", error_code);
    }
}

fn fatal_exception(name: &'static str, frame: &InterruptStackFrame, error_code: Option<u64>) -> ! {
    report_exception(name, frame, error_code);
    hcf()
}
