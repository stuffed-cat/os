use kernel::{
    elf::{ExecutableImage, ExecutableSegment, SegmentFlags},
    ipc::{IpcRouter, Message},
    posix::{Errno, PosixLayer},
    process::{Pid, ProcessTable, Tid},
    scheduler::{RunQueueEntry, Scheduler},
    syscall::{SyscallDispatcher, SyscallId},
    user::TrapFrame,
};

#[test]
fn ipc_message_roundtrip() {
    let router = IpcRouter::new();
    let (channel, _mailbox) = router.get_or_create(None);
    let sender = Pid::new(200);
    let payload = vec![1, 2, 3, 4];
    router
        .send(channel, Message::new(sender, sender, payload.clone()))
        .expect("send succeeds");

    let message = router.recv(channel).expect("message available");
    assert_eq!(message.payload, payload);
}

#[test]
fn scheduler_round_robin_ordering() {
    let scheduler = Scheduler::new();
    scheduler.enqueue(RunQueueEntry::user(Pid::new(1), Tid::new(1)));
    scheduler.enqueue(RunQueueEntry::kernel(Pid::new(1), Tid::new(2)));

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
    layer.register_virtual_file("/tmp/data".to_string(), vec![0u8; 128]);

    let stub_segment = ExecutableSegment {
        virtual_addr: 0x4000_0000,
        data: vec![0xC3], // ret
        flags: SegmentFlags {
            readable: true,
            writable: false,
            executable: true,
        },
    };
    let stub_image = ExecutableImage::from_parts(0x4000_0000, vec![stub_segment])
        .expect("stub executable valid");
    let stub_entry = stub_image.entry_point();
    table.register_exec_override("/bin/init".to_string(), stub_image);

    let child_pid = layer
        .dispatch(parent.pid(), SyscallId::Fork, &[])
        .expect("fork returns child");
    assert_ne!(child_pid, parent.pid().as_u64());
    assert!(table.lookup(Pid::new(child_pid)).is_some());

    layer
        .dispatch(parent.pid(), SyscallId::Exec, &[1])
        .expect("exec succeeds");

    let exec_proc = table.lookup(parent.pid()).expect("process still present");
    let address_space = exec_proc
        .address_space()
        .expect("address space present after exec");
    assert_eq!(address_space.entry_point(), stub_entry);
    assert_eq!(address_space.segments().len(), 1);
    let context = exec_proc
        .user_context()
        .expect("user context present after exec");
    assert_eq!(context.frame().rip, stub_entry);
    assert_eq!(context.frame().rsp, address_space.stack().top());

    let fd = layer
        .dispatch(parent.pid(), SyscallId::Open, &[2, 0])
        .expect("open returns fd");
    assert!(fd >= 3);

    let mut buf = [0u8; 64];
    let read_len = layer
        .dispatch(
            parent.pid(),
            SyscallId::Read,
            &[fd, buf.as_mut_ptr() as u64, buf.len() as u64],
        )
        .expect("read returns count");
    assert_eq!(read_len, 64);

    // getpid should return the caller's PID.
    let reported_pid = layer
        .dispatch(parent.pid(), SyscallId::GetPid, &[])
        .expect("getpid works");
    assert_eq!(reported_pid, parent.pid().as_u64());

    // close should succeed once and fail with EBADF on repeated calls.
    assert_eq!(
        layer
            .dispatch(parent.pid(), SyscallId::Close, &[fd])
            .expect("close succeeds"),
        0
    );
    let close_err = layer
        .dispatch(parent.pid(), SyscallId::Close, &[fd])
        .unwrap_err();
    assert_eq!(close_err, Errno::Badf);

    // waitpid should return EAGAIN while the child is still running.
    let wait_err = layer
        .dispatch(parent.pid(), SyscallId::WaitPid, &[child_pid, 0, 0])
        .unwrap_err();
    assert_eq!(wait_err, Errno::Again);

    // Simulate child exit and ensure waitpid reaps it.
    layer
        .dispatch(Pid::new(child_pid), SyscallId::Exit, &[42])
        .expect("child exit succeeds");
    let wait_result = layer
        .dispatch(parent.pid(), SyscallId::WaitPid, &[child_pid, 0, 0])
        .expect("waitpid succeeds");
    assert_eq!(wait_result & 0xFFFF_FFFF, child_pid);
    assert_eq!((wait_result >> 32) as i32, 42);

    // Subsequent wait should report no remaining children.
    let no_child = layer
        .dispatch(parent.pid(), SyscallId::WaitPid, &[child_pid, 0, 0])
        .unwrap_err();
    assert_eq!(no_child, Errno::Child);
}

#[test]
fn syscall_dispatcher_handles_trap_frame() {
    let table = ProcessTable::new();
    let process = table.spawn();
    let dispatcher = SyscallDispatcher::new(&table);
    let mut frame = TrapFrame::default();
    frame.rax = 39; // getpid syscall number
    dispatcher
        .handle_trap(process.pid(), &mut frame)
        .expect("syscall trap handled");
    assert_eq!(frame.rax, process.pid().as_u64());
}
