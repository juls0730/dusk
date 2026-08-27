use crate::arch::port::{read_u8, write_u8};

const COM1_BASE: u16 = 0x3F8;

mod register {
    pub const DATA_BUFFER: u16 = 0;

    pub const DIVISOR_LOW: u16 = 0;
    pub const INTERRUPT_ENABLE: u16 = 1;
    pub const DIVISOR_HIGH: u16 = 1;

    // pub const INTERRUPT_IDENTIFICATION: u16 = 2;
    pub const FIFO_CONTROL: u16 = 2;

    pub const LINE_CONTROL: u16 = 3;
    pub const MODEM_CONTROL: u16 = 4;
    pub const LINE_STATUS: u16 = 5;
    // pub const MODEM_STATUS: u16 = 6;
    pub const SCRATCH: u16 = 7;
}

mod fifo_control {
    pub const ENABLE: u8 = 1 << 0;
    pub const CLEAR_RECEIVE: u8 = 1 << 1;
    pub const CLEAR_TRANSMIT: u8 = 1 << 2;
    pub const TRIGGER_LEVEL_14: u8 = 0b11 << 6;
}

mod line_control {
    pub const DATA_BITS_8: u8 = 0b11;
    pub const DLAB: u8 = 1 << 7;
}

mod line_status {
    pub const TRANSMITTER_EMPTY: u8 = 1 << 5;
}

mod modem_control {
    pub const DATA_TERMINAL_READY: u8 = 1 << 0;
    pub const REQUEST_TO_SEND: u8 = 1 << 1;
    // commonly controls whether IRQs are enabled or disabled
    pub const OUT_1: u8 = 1 << 2;
    // unused in PC implementations
    pub const OUT_2: u8 = 1 << 3;
    pub const LOOPBACK: u8 = 1 << 4;
}

#[derive(Debug)]
pub enum SerialPortError {
    NotFound,
    Faulty,
}

struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub fn new(base: u16) -> Self {
        Self { base }
    }

    pub fn init(&self) -> Result<(), SerialPortError> {
        const SCRATCH_TEST_VALUE: u8 = 0x42;
        const LOOPBACK_TEST_VALUE: u8 = 0xAE;

        // The scratch register has no hardware-defined behavior, so it can be used to probe the port.
        self.write_register(register::SCRATCH, SCRATCH_TEST_VALUE);
        if self.read_register(register::SCRATCH) != SCRATCH_TEST_VALUE {
            return Err(SerialPortError::NotFound);
        }

        self.write_register(register::INTERRUPT_ENABLE, 0);

        // DLAB changes registers 0 and 1 into the low and high divisor registers so we can set the baud rate
        self.write_register(register::LINE_CONTROL, line_control::DLAB);
        // in this case we are setting the baud rate to 115200 baud
        self.write_register(register::DIVISOR_LOW, 1);
        self.write_register(register::DIVISOR_HIGH, 0);
        self.write_register(register::LINE_CONTROL, line_control::DATA_BITS_8);

        self.write_register(
            register::FIFO_CONTROL,
            fifo_control::ENABLE
                | fifo_control::CLEAR_RECEIVE
                | fifo_control::CLEAR_TRANSMIT
                | fifo_control::TRIGGER_LEVEL_14,
        );

        self.write_register(
            register::MODEM_CONTROL,
            modem_control::DATA_TERMINAL_READY
                | modem_control::REQUEST_TO_SEND
                | modem_control::OUT_2,
        );

        self.write_register(
            register::MODEM_CONTROL,
            modem_control::REQUEST_TO_SEND
                | modem_control::OUT_1
                | modem_control::OUT_2
                | modem_control::LOOPBACK,
        );

        self.write_byte(LOOPBACK_TEST_VALUE);
        if self.read_register(register::DATA_BUFFER) != LOOPBACK_TEST_VALUE {
            return Err(SerialPortError::Faulty);
        }

        self.write_register(
            register::MODEM_CONTROL,
            modem_control::DATA_TERMINAL_READY
                | modem_control::REQUEST_TO_SEND
                | modem_control::OUT_1
                | modem_control::OUT_2,
        );

        Ok(())
    }

    fn write_register(&self, register: u16, value: u8) {
        unsafe { write_u8(self.base + register, value) };
    }

    fn read_register(&self, register: u16) -> u8 {
        unsafe { read_u8(self.base + register) }
    }

    fn can_transfer(&self) -> bool {
        self.read_register(register::LINE_STATUS) & line_status::TRANSMITTER_EMPTY != 0
    }

    pub fn write_byte(&self, byte: u8) {
        if byte == b'\n' {
            self.write_byte(b'\r');
        }

        while !self.can_transfer() {
            core::hint::spin_loop();
        }

        self.write_register(register::DATA_BUFFER, byte);
    }
}

impl core::fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

fn com1() -> SerialPort {
    SerialPort::new(COM1_BASE)
}

pub fn init() -> Result<(), SerialPortError> {
    com1().init()
}

pub fn print(args: core::fmt::Arguments) {
    use core::fmt::Write;

    let mut serial = com1();
    let _ = serial.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::debug::serial::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::debug::serial::print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::debug::serial::print(format_args!("{}\n", format_args!($($arg)*))));
}
