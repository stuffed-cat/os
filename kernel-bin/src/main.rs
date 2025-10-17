#![cfg_attr(feature = "boot", no_std)]
#![cfg_attr(feature = "boot", no_main)]

#[cfg(feature = "boot")]
extern crate alloc;

#[cfg(feature = "boot")]
mod boot;

#[cfg(feature = "boot")]
use bootloader_api::BootInfo;

#[cfg(feature = "boot")]
use linked_list_allocator::LockedHeap;

#[cfg(feature = "boot")]
#[global_allocator]
static GLOBAL_ALLOCATOR: LockedHeap = LockedHeap::empty();

#[cfg(feature = "boot")]
bootloader_api::entry_point!(kernel_entry);

#[cfg(feature = "boot")]
fn kernel_entry(boot_info: &'static mut BootInfo) -> ! {
    boot::start(boot_info, &GLOBAL_ALLOCATOR)
}

#[cfg(feature = "boot")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    kernel::arch::x86_64::serial::write_str("kernel panic:\n");
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        kernel::arch::x86_64::serial::write_str(message);
        kernel::arch::x86_64::serial::write_str("\n");
    }
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(not(feature = "boot"))]
fn main() {
    println!("Run with `--features boot` to build the bootable kernel image.");
}
