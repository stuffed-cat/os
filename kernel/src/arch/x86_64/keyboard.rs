//! Basic PS/2 keyboard helpers for bare-metal builds.

use x86_64::instructions::port::Port;

const DATA_PORT: u16 = 0x60;

/// Reads a single scancode from the keyboard data port.
pub fn read_scancode() -> u8 {
	unsafe {
		let mut port: Port<u8> = Port::new(DATA_PORT);
		port.read()
	}
}
