//! Architecture specific code for the hybrid kernel.
//!
//! The kernel targets the x86-64 architecture with a split between
//! microkernel-style isolation (explicit capability boundaries) and
//! monolithic fast paths for performance critical subsystems like the
//! scheduler and virtual memory manager.

pub mod x86_64;
