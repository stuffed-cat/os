//! Power management helpers for reboot/shutdown sequences.

#[cfg(feature = "hardware")]
use x86_64::instructions::{hlt, port::Port};

/// Request the platform to power off.
#[cfg(feature = "hardware")]
pub fn shutdown() {
    unsafe {
        let mut port = Port::<u16>::new(0x604);
        port.write(0x2000);
    }
    loop {
        hlt();
    }
}

/// Request the platform to reboot.
#[cfg(feature = "hardware")]
pub fn reboot() {
    unsafe {
        let mut status = Port::<u8>::new(0x64);
        while status.read() & 0x02 != 0 {}
        status.write(0xFE);
    }
    loop {
        hlt();
    }
}

#[cfg(not(feature = "hardware"))]
/// Stub shutdown implementation used for host/testing builds.
pub fn shutdown() {
    crate::arch::x86_64::serial::write_str("shutdown requested (stub)\r\n");
}

#[cfg(not(feature = "hardware"))]
/// Stub reboot implementation used for host/testing builds.
pub fn reboot() {
    crate::arch::x86_64::serial::write_str("reboot requested (stub)\r\n");
}
