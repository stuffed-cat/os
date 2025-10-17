//! x86-64 specific hardware abstractions and initialization.

mod boot;
pub mod gdt;
#[cfg(feature = "hardware")]
pub mod idt;
#[cfg(not(feature = "hardware"))]
pub mod idt {
    //! Stub IDT module used when hardware features are disabled.
    /// Stub IDT initialization used for host testing without hardware features.
    pub fn init() {}

    /// Stub IDT load used for host testing without hardware features.
    pub fn load() {}
}
pub mod interrupts;
#[cfg(feature = "hardware")]
pub mod keyboard;
#[cfg(feature = "hardware")]
#[doc = "Framebuffer-backed text console for VGA output."]
pub mod framebuffer;
#[cfg(not(feature = "hardware"))]
pub mod framebuffer {
    use bootloader_api::info::FrameBuffer;

    /// Stub framebuffer init for host testing.
    pub fn init(_: &'static mut FrameBuffer) {}

    /// Stub framebuffer writer for host testing.
    pub fn write_str(_: &str) {}
}
pub mod serial;

pub use boot::ArchBootstrap;
pub use boot::Cr0Flags;
pub use interrupts::{InterruptController, Pic8259Controller};
