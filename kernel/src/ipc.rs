//! Inter-process communication primitives.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::{
    error::SubsystemError,
    process::Pid,
};

/// Unique channel identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(u64);

/// Message payload placeholder.
#[derive(Debug, Clone)]
pub struct Message {
    /// Identifier of the sending process.
    pub from: Pid,
    /// Identifier of the recipient process.
    pub to: Pid,
    /// Raw bytes that would encode POSIX messages.
    pub payload: alloc::vec::Vec<u8>,
}

impl Message {
    /// Creates a new message.
    pub fn new(from: Pid, to: Pid, payload: alloc::vec::Vec<u8>) -> Self {
        Self { from, to, payload }
    }
}

/// Mailbox bridging microkernel message passing with monolithic fast path.
pub struct Mailbox {
    messages: Mutex<VecDeque<Message>>,
}

impl Mailbox {
    /// Creates an empty mailbox.
    pub fn new() -> Self {
        Self { messages: Mutex::new(VecDeque::new()) }
    }

    /// Enqueues a message.
    pub fn enqueue(&self, message: Message) {
        self.messages.lock().push_back(message);
    }

    /// Dequeues a message.
    pub fn dequeue(&self) -> Option<Message> {
        self.messages.lock().pop_front()
    }
}

/// High level IPC fabric.
pub struct IpcRouter {
    next_id: AtomicU64,
    mailboxes: Mutex<BTreeMap<ChannelId, Arc<Mailbox>>>,
}

impl Default for IpcRouter {
    fn default() -> Self {
        Self { next_id: AtomicU64::new(1), mailboxes: Mutex::new(BTreeMap::new()) }
    }
}

impl IpcRouter {
    /// Creates a new IPC router.
    pub fn new() -> Self {
        Self::default()
    }

    fn allocate_channel(&self) -> ChannelId {
        ChannelId(self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Returns an existing mailbox or creates a new one.
    pub fn get_or_create(&self, channel: Option<ChannelId>) -> (ChannelId, Arc<Mailbox>) {
        let mut guard = self.mailboxes.lock();
        if let Some(id) = channel {
            guard.get(&id).cloned().map(|mailbox| (id, mailbox)).unwrap_or_else(|| {
                let mb = Arc::new(Mailbox::new());
                guard.insert(id, mb.clone());
                (id, mb)
            })
        } else {
            let id = self.allocate_channel();
            let mb = Arc::new(Mailbox::new());
            guard.insert(id, mb.clone());
            (id, mb)
        }
    }

    /// Sends a message to the destination channel.
    pub fn send(&self, channel: ChannelId, message: Message) -> Result<(), SubsystemError> {
        let guard = self.mailboxes.lock();
        let mailbox = guard.get(&channel).ok_or(SubsystemError::Runtime("channel not found"))?;
        mailbox.enqueue(message);
        Ok(())
    }

    /// Receives a message if one is pending.
    pub fn recv(&self, channel: ChannelId) -> Option<Message> {
        self.mailboxes.lock().get(&channel).and_then(|mailbox| mailbox.dequeue())
    }
}
