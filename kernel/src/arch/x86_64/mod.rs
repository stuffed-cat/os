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
pub mod serial;

pub use boot::ArchBootstrap;
pub use boot::Cr0Flags;
pub use interrupts::{InterruptController, Pic8259Controller};
