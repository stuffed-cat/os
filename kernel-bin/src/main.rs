#![cfg_attr(feature = "boot", no_std)]
#![cfg_attr(feature = "boot", no_main)]
#![cfg_attr(feature = "boot", feature(lang_items, global_asm))]

#[cfg(feature = "boot")]
mod boot;

#[cfg(feature = "boot")]
extern crate alloc;

#[cfg(feature = "boot")]
use linked_list_allocator::LockedHeap;

#[cfg(feature = "boot")]
#[global_allocator]
static GLOBAL_ALLOCATOR: LockedHeap = LockedHeap::empty();

#[cfg(feature = "boot")]
use kernel::arch::x86_64::serial;
#[cfg(feature = "boot")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    serial::write_str("kernel panic!\r\n");
    if let Some(location) = info.location() {
        serial::write_str(" at ");
        serial::write_str(location.file());
        serial::write_str(":");
        let mut buffer = itoa::Buffer::new();
        let line = location.line();
        serial::write_str(buffer.format(line));
        serial::write_str("\r\n");
    }
    let message = info.message();
    serial::write_str(" message: ");
    serial::write_fmt(format_args!("{}", message));
    serial::write_str("\r\n");
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(feature = "boot")]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

#[cfg(not(feature = "boot"))]
fn main() {
    println!("Run with `--features boot` to build the bootable kernel image.");
}
