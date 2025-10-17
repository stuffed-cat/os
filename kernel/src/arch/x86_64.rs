//! x86-64 specific hardware abstractions.

use bitflags::bitflags;
use log::trace;

use crate::error::{KernelError, SubsystemError};

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
    /// Initializes CPU level primitives.
    pub fn init_cpu_features() -> Result<(), KernelError> {
        trace!("Initializing CPU features for x86-64");
        // In a real kernel we would configure MSRs, GDT, IDT, etc.
        Ok(())
    }

    /// Verifies microkernel boundary support (VT-x/AMD-V).
    pub fn validate_virtualization() -> Result<(), KernelError> {
        trace!("Validating virtualization extensions");
        // Placeholder for CPUID and feature flag checks.
        Ok(())
    }
}

/// Handles low level interrupts and traps.
pub trait InterruptController {
    /// Enables interrupts.
    fn enable(&self);
    /// Disables interrupts.
    fn disable(&self);
    /// Acknowledges an interrupt.
    fn ack(&self, vector: u8);
}

/// Basic APIC backed interrupt controller stub.
pub struct XApicController;

impl InterruptController for XApicController {
    fn enable(&self) {
        trace!("APIC: enable interrupts");
    }

    fn disable(&self) {
        trace!("APIC: disable interrupts");
    }

    fn ack(&self, vector: u8) {
        trace!("APIC: ack vector {vector}");
    }
}

/// Kernel trap frame representing saved registers.
#[derive(Debug, Default, Clone)]
pub struct TrapFrame {
    /// Instruction pointer
    pub rip: u64,
    /// Stack pointer
    pub rsp: u64,
    /// Flags register
    pub rflags: u64,
}

impl TrapFrame {
    /// Create a trap frame for entering userland.
    pub fn userland_entry(entry: u64, stack: u64) -> Self {
        Self { rip: entry, rsp: stack, rflags: 0x202 }
    }
}

/// Wrapper around hardware result types.
pub type ArchResult<T> = Result<T, SubsystemError>;
