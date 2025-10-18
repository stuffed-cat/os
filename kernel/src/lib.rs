#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "hardware", feature(abi_x86_interrupt))]
#![deny(missing_docs)]

//! Core hybrid-kernel primitives for the "os" project.
//!
//! This crate models the structure of a hybrid (micro + monolithic) kernel
//! targeting x86-64 hardware while exposing a POSIX-friendly surface for
//! userland components. The code focuses on the architectural scaffolding
//! necessary to evolve into a full operating system.

extern crate alloc;

mod core;
mod error;

pub mod arch;
pub mod elf;
pub mod fs;
pub mod hal;
pub mod ipc;
pub mod memory;
pub mod posix;
pub mod process;
pub mod scheduler;
pub mod services;
pub mod shell;
pub mod syscall;

pub use crate::core::{Kernel, KernelBuilder, KernelContext, KernelState, Subsystem, SubsystemId};
pub use crate::error::{KernelError, SubsystemError};

#[cfg(any(feature = "alloc", feature = "std"))]
pub use crate::hal::{Hal, HalConfig};

#[cfg(any(feature = "alloc", feature = "std"))]
pub use crate::memory::{BootFrameAllocator, FrameRange, MemoryManager};

#[cfg(test)]
mod tests;
