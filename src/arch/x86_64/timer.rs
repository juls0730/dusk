use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::{
    arch::{
        apic::{self, LocalApic},
        disable_interrupts,
        io_apic::IoApic,
        x86_64::{
            interrupts::enable_interrupts,
            pit::{PIT_CALIBRATION_COUNT, PIT_FREQUENCY, Pit},
        },
    },
    platform::acpi::IsaIrqRoute,
};

static PIT_FIRED: AtomicBool = AtomicBool::new(false);
static LAPIC_COUNT_AT_PIT: AtomicU32 = AtomicU32::new(0);

#[derive(Debug)]
pub enum TimerCalibrationError {
    PitTimeout,
    IoApicNotHandled,
    FrequencyOverflow,
    InvalidTimerCount,
}

pub fn calibrate_local_apic(
    local_apic: &mut LocalApic,
    io_apic: &mut IoApic,
    pit_route: IsaIrqRoute,
) -> Result<u64, TimerCalibrationError> {
    PIT_FIRED.store(false, Ordering::SeqCst);
    LAPIC_COUNT_AT_PIT.store(0, Ordering::SeqCst);

    io_apic
        .unmask(pit_route.gsi)
        .map_err(|_| TimerCalibrationError::IoApicNotHandled)?;
    local_apic.start_calibration_counter();

    Pit::start_one_shot(PIT_CALIBRATION_COUNT);

    enable_interrupts();

    while !PIT_FIRED.load(Ordering::SeqCst) {
        if apic::current_timer_count() == 0 {
            disable_interrupts();
            let _ = io_apic.mask(pit_route.gsi);
            local_apic.stop_timer();

            return Err(TimerCalibrationError::PitTimeout);
        }

        core::hint::spin_loop();
    }

    disable_interrupts();

    io_apic
        .mask(pit_route.gsi)
        .map_err(|_| TimerCalibrationError::IoApicNotHandled)?;
    local_apic.stop_timer();

    let elapsed = u32::MAX - LAPIC_COUNT_AT_PIT.load(Ordering::SeqCst);

    if elapsed == 0 {
        return Err(TimerCalibrationError::InvalidTimerCount);
    }

    let ticks_per_second = (elapsed as u64)
        .checked_mul(PIT_FREQUENCY)
        .ok_or(TimerCalibrationError::FrequencyOverflow)?
        / PIT_CALIBRATION_COUNT as u64;

    Ok(ticks_per_second)
}

pub fn record_pit_calibration() {
    let current = apic::current_timer_count();

    LAPIC_COUNT_AT_PIT.store(current, Ordering::SeqCst);
    PIT_FIRED.store(true, Ordering::SeqCst);
}
