//! Hardware abstraction layer bridging architecture and services.

use alloc::sync::Arc;
use spin::Mutex;

use crate::{
    arch::x86_64::{ArchBootstrap, InterruptController, TrapFrame, XApicController},
    error::{KernelError, SubsystemError},
};

/// HAL facade for managing interrupt controllers and early CPU setup.
pub struct Hal {
    controller: Arc<dyn InterruptController + Send + Sync>,
}

impl Default for Hal {
    fn default() -> Self {
        Self { controller: Arc::new(XApicController) }
    }
}

impl Hal {
    /// Performs early CPU initialization.
    pub fn bootstrap() -> Result<Self, KernelError> {
        ArchBootstrap::init_cpu_features()?;
        ArchBootstrap::validate_virtualization()?;
        Ok(Self::default())
    }

    /// Enables interrupts globally.
    pub fn enable_interrupts(&self) {
        self.controller.enable();
    }

    /// Disables interrupts globally.
    pub fn disable_interrupts(&self) {
        self.controller.disable();
    }
}

/// Shared trap frame storage.
pub struct TrapStore {
    trap: Mutex<TrapFrame>,
}

impl TrapStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self { trap: Mutex::new(TrapFrame::default()) }
    }

    /// Saves a trap frame.
    pub fn save(&self, frame: TrapFrame) {
        *self.trap.lock() = frame;
    }

    /// Restores the trap frame.
    pub fn restore(&self) -> TrapFrame {
        self.trap.lock().clone()
    }
}

/// Result alias for HAL operations.
pub type HalResult<T> = Result<T, SubsystemError>;
