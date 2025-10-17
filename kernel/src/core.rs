use core::fmt;
use alloc::vec::Vec;
use log::info;

use crate::{
    error::{KernelError, SubsystemError},
    services::ServiceRegistry,
};

/// Identifier for kernel subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubsystemId(pub &'static str);

impl fmt::Display for SubsystemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Trait implemented by all kernel subsystems.
pub trait Subsystem {
    /// Unique identifer for the subsystem.
    fn id(&self) -> SubsystemId;

    /// Initializes the subsystem with access to the kernel context.
    fn init(&mut self, ctx: &KernelContext) -> Result<(), SubsystemError>;

    /// Performs periodic housekeeping.
    fn tick(&mut self, ctx: &KernelContext) -> Result<(), SubsystemError>;
}

/// Kernel-wide state machine stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelState {
    /// Early bootstrap before services are available.
    Bootstrap,
    /// Core subsystems are initialized.
    Ready,
    /// Scheduler is active and userland can execute.
    Running,
    /// Kernel is shutting down.
    Shutdown,
}

/// Immutable context shared with subsystems.
pub struct KernelContext<'a> {
    state: KernelState,
    registry: &'a ServiceRegistry,
}

impl<'a> KernelContext<'a> {
    /// Creates a new kernel context.
    pub fn new(state: KernelState, registry: &'a ServiceRegistry) -> Self {
        Self { state, registry }
    }

    /// Returns the current kernel state.
    pub fn state(&self) -> KernelState {
        self.state
    }

    /// Returns the service registry reference.
    pub fn services(&self) -> &'a ServiceRegistry {
        self.registry
    }
}

/// Configures and builds a [`Kernel`].
pub struct KernelBuilder {
    services: ServiceRegistry,
    subsystems: Vec<Box<dyn Subsystem + Send>>, // hybrid architecture keeps dynamic dispatch manageable
}

impl Default for KernelBuilder {
    fn default() -> Self {
        Self { services: ServiceRegistry::default(), subsystems: Vec::new() }
    }
}

impl KernelBuilder {
    /// Adds a subsystem to the kernel.
    pub fn with_subsystem(mut self, subsystem: impl Subsystem + Send + 'static) -> Self {
        self.subsystems.push(Box::new(subsystem));
        self
    }

    /// Adds a service to the registry.
    pub fn with_service<S>(mut self, service: S) -> Self
    where
        S: crate::services::Service + Send + Sync + 'static,
    {
        self.services.register(service);
        self
    }

    /// Consumes the builder and returns a new kernel.
    pub fn build(self) -> Kernel {
        Kernel { state: KernelState::Bootstrap, services: self.services, subsystems: self.subsystems }
    }
}

/// Central kernel coordinator for the hybrid architecture.
pub struct Kernel {
    state: KernelState,
    services: ServiceRegistry,
    subsystems: Vec<Box<dyn Subsystem + Send>>,
}

impl Kernel {
    /// Initializes the kernel and all subsystems.
    pub fn init(&mut self) -> Result<(), KernelError> {
        let ctx = KernelContext::new(self.state, &self.services);
        for subsystem in self.subsystems.iter_mut() {
            let id = subsystem.id();
            info!("Initializing subsystem: {}", id);
            subsystem
                .init(&ctx)
                .map_err(|source| KernelError::Subsystem { id: id.0, source })?;
        }
        self.state = KernelState::Ready;
        Ok(())
    }

    /// Transition to the running state and invoke subsystem ticks.
    pub fn run(&mut self) -> Result<(), KernelError> {
        self.state = KernelState::Running;
        loop {
            let ctx = KernelContext::new(self.state, &self.services);
            for subsystem in self.subsystems.iter_mut() {
                let id = subsystem.id();
                subsystem
                    .tick(&ctx)
                    .map_err(|source| KernelError::Subsystem { id: id.0, source })?;
            }
            // In a real kernel this would yield to the scheduler and hardware interrupts.
            // For now we break to prevent a busy loop during unit tests.
            break;
        }
        Ok(())
    }

    /// Initiates kernel shutdown.
    pub fn shutdown(&mut self) -> Result<(), KernelError> {
        self.state = KernelState::Shutdown;
        Ok(())
    }
}
