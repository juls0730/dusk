use crate::arch::port::write_u8;

const PIT_CHANNEL_0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;

const CHANNEL_0: u8 = 0b00 << 6;
// access mode = lobyte/hibyte
const LOW_HIGH: u8 = 0b11 << 4;
// interrupt on terminal count
const MODE_0: u8 = 0b000 << 1;
const BINARY: u8 = 0;

pub const PIT_FREQUENCY: u64 = 1_193_182;
pub const PIT_CALIBRATION_COUNT: u16 = u16::MAX;

pub struct Pit;

impl Pit {
    pub fn start_one_shot(count: u16) {
        unsafe {
            write_u8(PIT_COMMAND, CHANNEL_0 | LOW_HIGH | MODE_0 | BINARY);

            write_u8(PIT_CHANNEL_0, count as u8);
            write_u8(PIT_CHANNEL_0, (count >> 8) as u8);
        }
    }
}
