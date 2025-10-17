use kernel::{ipc::{IpcRouter, Message}, posix::PosixLayer, process::{Pid, ProcessTable, Tid}, scheduler::{RunQueueEntry, Scheduler, SchedulingClass}, syscall::SyscallId};

#[test]
fn ipc_message_roundtrip() {
    let router = IpcRouter::new();
    let (channel, _mailbox) = router.get_or_create(None);
    let sender = Pid::new(200);
    let payload = vec![1, 2, 3, 4];
    router.send(channel, Message::new(sender, sender, payload.clone())).expect("send succeeds");

    let message = router.recv(channel).expect("message available");
    assert_eq!(message.payload, payload);
}

#[test]
fn scheduler_round_robin_ordering() {
    let scheduler = Scheduler::new();
    scheduler.enqueue(RunQueueEntry { tid: Tid::new(1), class: SchedulingClass::User });
    scheduler.enqueue(RunQueueEntry { tid: Tid::new(2), class: SchedulingClass::Kernel });

    let first = scheduler.pick_next().expect("first task available");
    assert_eq!(first.tid.as_u64(), 1);

    let second = scheduler.pick_next().expect("second task available");
    assert_eq!(second.tid.as_u64(), 2);

    assert!(scheduler.pick_next().is_none());
}

#[test]
fn posix_fork_exec_open_read_flow() {
    let table = ProcessTable::new();
    let parent = table.spawn();
    let layer = PosixLayer::new(&table);
    layer.register_program_handle(1, "/bin/init".to_string());
    layer.register_path_handle(2, "/tmp/data".to_string());

    let child_pid = layer.dispatch(parent.pid(), SyscallId::Fork, &[]).expect("fork returns child");
    assert_ne!(child_pid, parent.pid().as_u64());
    assert!(table.lookup(Pid::new(child_pid)).is_some());

    layer.dispatch(parent.pid(), SyscallId::Exec, &[1]).expect("exec succeeds");

    let fd = layer.dispatch(parent.pid(), SyscallId::Open, &[2, 0]).expect("open returns fd");
    assert!(fd >= 4);

    let read_len = layer.dispatch(parent.pid(), SyscallId::Read, &[fd, 64]).expect("read returns count");
    assert_eq!(read_len, 64);
}
