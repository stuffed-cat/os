//! Boot-time architectural dispatcher for x86-64.

use bitflags::bitflags;
use log::trace;

use crate::error::KernelError;

use super::{gdt, idt, serial};

bitflags! {
    /// x86-64 control register flags captured for virtualization and
    /// privilege separation between kernel and userland.
    pub struct Cr0Flags: u64 {
        /// Enables protected mode.
        const PROTECTED_MODE = 1 << 0;
        /// Restricts kernel write access to read-only pages.
        const WRITE_PROTECT = 1 << 16;
        /// Activates paging hardware.
        const PAGING = 1 << 31;
    }
}

/// Boot-time architectural dispatcher.
pub struct ArchBootstrap;

impl ArchBootstrap {
    /// Initializes CPU level primitives such as the logger and descriptor tables.
    pub fn init_cpu_features() -> Result<(), KernelError> {
        trace!("Initializing CPU features for x86-64");
        serial::init();
        gdt::init();
        idt::init();
        Ok(())
    }

    /// Configures interrupt descriptors and unmasks the PIC.
    pub fn init_interrupts() -> Result<(), KernelError> {
        trace!("Configuring interrupt controller");
        idt::load();
        Ok(())
    }

    /// Verifies virtualization support (VT-x/AMD-V). Currently a stub.
    pub fn validate_virtualization() -> Result<(), KernelError> {
        trace!("Validating virtualization extensions");
        // Placeholder for CPUID feature checks.
        Ok(())
    }
}
