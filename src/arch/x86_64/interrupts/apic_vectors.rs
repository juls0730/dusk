use crate::arch::{
    apic, timer,
    x86_64::interrupts::idt::{self, InterruptStackFrame},
};

pub const APIC_SELF_IPI_VECTOR: u8 = 0xF0;
pub const PIT_CALIBRATION_VECTOR: u8 = 0xF1;
pub const APIC_TIMER_VECTOR: u8 = 0xFD;
pub const APIC_ERROR_VECTOR: u8 = 0xFE;
pub const APIC_SPURIOUS_VECTOR: u8 = 0xFF;

extern "x86-interrupt" fn self_ipi_handler(_frame: InterruptStackFrame) {
    apic::record_self_ipi();
    apic::end_of_interrupt();
}

extern "x86-interrupt" fn error_handler(_frame: InterruptStackFrame) {
    apic::record_error();
    apic::end_of_interrupt();
}

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    apic::record_timer();
    apic::end_of_interrupt();
}

extern "x86-interrupt" fn pit_calibration_handler(_frame: InterruptStackFrame) {
    timer::record_pit_calibration();
    apic::end_of_interrupt();
}

extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {
    // No EOI
}

pub(super) fn install(idt: &mut idt::Idt) {
    idt.set_handler(APIC_SELF_IPI_VECTOR, self_ipi_handler, 0);
    idt.set_handler(PIT_CALIBRATION_VECTOR, pit_calibration_handler, 0);
    idt.set_handler(APIC_ERROR_VECTOR, error_handler, 0);
    idt.set_handler(APIC_TIMER_VECTOR, timer_handler, 0);
    idt.set_handler(APIC_SPURIOUS_VECTOR, spurious_handler, 0);
}
