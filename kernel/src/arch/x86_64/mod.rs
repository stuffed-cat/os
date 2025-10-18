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
#[cfg(feature = "hardware")]
#[doc = "Framebuffer-backed text console for VGA output."]
pub mod framebuffer;
pub mod interrupts;
#[cfg(feature = "hardware")]
pub mod keyboard;
#[cfg(not(feature = "hardware"))]
pub mod framebuffer {
    //! Stub framebuffer interface for host-side testing when hardware support is disabled.
    use bootloader_api::info::FrameBuffer;

    /// Stub framebuffer init for host testing.
    pub fn init(_: &'static mut FrameBuffer) {}

    /// Stub framebuffer writer for host testing.
    pub fn write_str(_: &str) {}
}
pub mod power;
pub mod serial;

pub use boot::ArchBootstrap;
pub use boot::Cr0Flags;
pub use interrupts::{InterruptController, Pic8259Controller};
