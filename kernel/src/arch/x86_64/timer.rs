//! x86-64 PIT timer programming utilities for preemptive scheduling.
#![cfg(feature = "hardware")]

use spin::Mutex;
use x86_64::instructions::port::Port;

const PIT_BASE_FREQUENCY_HZ: u32 = 1_193_182;

static PROGRAMMED_FREQUENCY: Mutex<Option<u32>> = Mutex::new(None);

/// Programs the legacy PIT (channel 0) for a periodic interrupt at the requested frequency.
pub fn init(frequency_hz: u32) {
    let requested = frequency_hz.max(1);
    let divisor = core::cmp::max(PIT_BASE_FREQUENCY_HZ / requested, 1).min(u16::MAX as u32);

    unsafe {
        let mut command = Port::new(0x43);
        command.write(0x36u8);
        let mut data = Port::new(0x40);
        data.write((divisor & 0xFF) as u8);
        data.write(((divisor >> 8) & 0xFF) as u8);
    }

    *PROGRAMMED_FREQUENCY.lock() = Some(requested);
}

/// Returns the last frequency programmed into the PIT, if any.
pub fn current_frequency() -> Option<u32> {
    *PROGRAMMED_FREQUENCY.lock()
}
