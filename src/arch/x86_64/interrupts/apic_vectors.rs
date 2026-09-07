use super::idt::{self, InterruptFrame, stub_no_err};
use crate::arch::{apic, timer};

pub const PIT_CALIBRATION_VECTOR: u8 = 0xF1;
pub const APIC_TIMER_VECTOR: u8 = 0xFD;
pub const APIC_ERROR_VECTOR: u8 = 0xFE;
pub const APIC_SPURIOUS_VECTOR: u8 = 0xFF;

stub_no_err!(stub_pit_calibration, 0xF1);
stub_no_err!(stub_apic_timer, 0xFD);
stub_no_err!(stub_apic_error, 0xFE);
stub_no_err!(stub_apic_spurious, 0xFF);

pub(super) fn handle(frame: &mut InterruptFrame) {
    match frame.vector as u8 {
        PIT_CALIBRATION_VECTOR => {
            timer::record_pit_calibration();
            apic::end_of_interrupt();
        }
        APIC_TIMER_VECTOR => {
            apic::record_timer();
            apic::end_of_interrupt();
        }
        APIC_ERROR_VECTOR => {
            apic::record_error();
            apic::end_of_interrupt();
        }
        APIC_SPURIOUS_VECTOR => {
            // No EOI
        }
        _ => {}
    }
}

pub(super) fn install(idt: &mut idt::Idt) {
    idt.set_handler(PIT_CALIBRATION_VECTOR, stub_pit_calibration, 0);
    idt.set_handler(APIC_ERROR_VECTOR, stub_apic_error, 0);
    idt.set_handler(APIC_TIMER_VECTOR, stub_apic_timer, 0);
    idt.set_handler(APIC_SPURIOUS_VECTOR, stub_apic_spurious, 0);
}
