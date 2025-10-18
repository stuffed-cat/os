//! Shared userland utilities for the hybrid kernel prototypes.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(any(feature = "baremetal", not(feature = "std")))]
extern crate alloc;

#[cfg(feature = "std")]
pub mod shell;
#[cfg(feature = "std")]
pub mod syscall;

#[cfg(feature = "std")]
pub use shell::Shell;
#[cfg(feature = "std")]
pub use syscall::{to_hex, Runtime, SyscallRequest};

#[cfg(feature = "baremetal")]
pub mod bare_shell;
#[cfg(feature = "baremetal")]
pub use bare_shell::{
    BareShell, DirEntry, EntryKind, FsError, ShellFs, ShellIo, ShellSystem, SystemError,
};
