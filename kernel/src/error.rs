use core::fmt;

/// General kernel error type.
#[derive(Debug)]
pub enum KernelError {
    /// Wrapper around subsystem specific errors.
    Subsystem {
        /// Subsystem identifier.
        id: &'static str,
        /// Underlying cause.
        source: SubsystemError,
    },

    /// Placeholder for architecture specific faults.
    Arch(&'static str),

    /// Memory management failure.
    Memory(&'static str),

    /// Placeholder for unimplemented pieces of the kernel.
    Unimplemented(&'static str),
}

impl KernelError {
    /// Creates a subsystem error with explicit identifier.
    pub fn subsystem(id: &'static str, source: SubsystemError) -> Self {
        Self::Subsystem { id, source }
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Subsystem { id, source } => {
                write!(f, "subsystem {} failed: {}", id, source)
            }
            KernelError::Arch(msg) => write!(f, "architecture fault: {}", msg),
            KernelError::Memory(msg) => write!(f, "memory error: {}", msg),
            KernelError::Unimplemented(msg) => write!(f, "unimplemented: {}", msg),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for KernelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KernelError::Subsystem { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Subsystem level errors.
#[derive(Debug)]
pub enum SubsystemError {
    /// Initialization failure.
    Init(&'static str),

    /// Runtime failure.
    Runtime(&'static str),

    /// Resource exhaustion, including memory scarcity.
    Resource(&'static str),
}

impl fmt::Display for SubsystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubsystemError::Init(msg) => write!(f, "initialization failure: {}", msg),
            SubsystemError::Runtime(msg) => write!(f, "runtime failure: {}", msg),
            SubsystemError::Resource(msg) => write!(f, "resource exhaustion: {}", msg),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SubsystemError {}

impl From<SubsystemError> for KernelError {
    fn from(value: SubsystemError) -> Self {
        KernelError::Subsystem { id: "unknown", source: value }
    }
}

