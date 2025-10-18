use ::core::sync::atomic::{AtomicBool, Ordering};
use alloc::sync::Arc;

use super::*;
use crate::process::{Pid, Process};

struct DummySubsystem {
    id: SubsystemId,
    init_called: Arc<AtomicBool>,
    tick_called: Arc<AtomicBool>,
}

impl DummySubsystem {
    fn new(name: &'static str) -> (Self, Arc<AtomicBool>, Arc<AtomicBool>) {
        let init_called = Arc::new(AtomicBool::new(false));
        let tick_called = Arc::new(AtomicBool::new(false));
        (
            Self {
                id: SubsystemId(name),
                init_called: init_called.clone(),
                tick_called: tick_called.clone(),
            },
            init_called,
            tick_called,
        )
    }
}

impl Subsystem for DummySubsystem {
    fn id(&self) -> SubsystemId {
        self.id
    }

    fn init(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        self.init_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn tick(&mut self, _ctx: &KernelContext) -> Result<(), SubsystemError> {
        self.tick_called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn kernel_initializes_and_runs_subsystems() {
    let (subsystem, init_called, tick_called) = DummySubsystem::new("dummy");

    let mut kernel = KernelBuilder::default().with_subsystem(subsystem).build();

    kernel.init().expect("init succeeds");
    assert!(init_called.load(Ordering::SeqCst));

    kernel.run().expect("run succeeds");
    assert!(tick_called.load(Ordering::SeqCst));
}

#[test]
fn process_allocates_monotonic_thread_ids() {
    let process = Process::new(Pid::new(1));
    let first = process.allocate_tid();
    let second = process.allocate_tid();
    assert!(second.as_u64() > first.as_u64());
}
