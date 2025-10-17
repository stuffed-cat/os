//! Virtual memory and capability aware allocator scaffolding.

use alloc::collections::BTreeMap;
use core::ops::Range;
use spin::RwLock;

use crate::error::SubsystemError;

/// Physical frame identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame(u64);

/// Virtual memory region descriptor.
#[derive(Debug, Clone)]
pub struct VmRegion {
    /// Virtual address range.
    pub range: Range<u64>,
    /// Access permissions.
    pub flags: VmFlags,
}

bitflags::bitflags! {
    /// Access flags for virtual memory regions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct VmFlags: u8 {
        /// Region is readable.
        const READ = 1 << 0;
        /// Region is writable.
        const WRITE = 1 << 1;
        /// Region is executable.
        const EXEC = 1 << 2;
        /// Region is accessible from userland.
        const USER = 1 << 3;
        /// Region is restricted to kernel mode.
        const KERNEL = 1 << 4;
    }
}

/// Capability token for memory objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capability(u64);

/// Virtual memory manager bridging microkernel isolation with monolithic performance.
pub struct VirtualMemoryManager {
    regions: RwLock<BTreeMap<u64, VmRegion>>,
    capabilities: RwLock<BTreeMap<Capability, VmRegion>>,
    next_cap: core::sync::atomic::AtomicU64,
}

impl Default for VirtualMemoryManager {
    fn default() -> Self {
        Self {
            regions: RwLock::new(BTreeMap::new()),
            capabilities: RwLock::new(BTreeMap::new()),
            next_cap: core::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl VirtualMemoryManager {
    /// Creates a new VMM instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps a region with associated capability.
    pub fn map(&self, region: VmRegion) -> Result<Capability, SubsystemError> {
        let cap = Capability(self.next_cap.fetch_add(1, core::sync::atomic::Ordering::SeqCst));
        self.regions.write().insert(region.range.start, region.clone());
        self.capabilities.write().insert(cap, region);
        Ok(cap)
    }

    /// Looks up region metadata by address.
    pub fn region(&self, addr: u64) -> Option<VmRegion> {
        let guard = self.regions.read();
        guard.range(..=addr).next_back().and_then(|(_, region)| {
            if region.range.contains(&addr) {
                Some(region.clone())
            } else {
                None
            }
        })
    }

    /// Resolves capability to region metadata.
    pub fn lookup_capability(&self, cap: Capability) -> Option<VmRegion> {
        self.capabilities.read().get(&cap).cloned()
    }
}
