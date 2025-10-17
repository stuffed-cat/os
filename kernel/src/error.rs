use thiserror::Error;

/// General kernel error type.
#[derive(Debug, Error)]
pub enum KernelError {
    /// Wrapper around subsystem specific errors.
    #[error("subsystem {id} failed: {source}")]
    Subsystem {
        /// Subsystem identifier.
        id: &'static str,
        /// Underlying cause.
        #[source]
        source: SubsystemError,
    },

    /// Placeholder for architecture specific faults.
    #[error("architecture fault: {0}")]
    Arch(&'static str),

    /// Placeholder for unimplemented pieces of the kernel.
    #[error("unimplemented: {0}")]
    Unimplemented(&'static str),
}

/// Subsystem level errors.
#[derive(Debug, Error)]
pub enum SubsystemError {
    /// Initialization failure.
    #[error("initialization failure: {0}")]
    Init(&'static str),

    /// Runtime failure.
    #[error("runtime failure: {0}")]
    Runtime(&'static str),
}

impl From<SubsystemError> for KernelError {
    fn from(value: SubsystemError) -> Self {
        KernelError::Subsystem { id: "unknown", source: value }
    }
}

