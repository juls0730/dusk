use core::ops::BitOr;

use crate::{
    arch::ThreadContext,
    memory::{FrameAddr, KernelStack, VirtualAddr},
};

// Thread Control Block
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Exited(usize),
    Killed,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    Send { ep: usize },
    Recv { ep: usize },
    Reply { client: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked(BlockReason),
    Dead(ExitReason),
}

pub const MAX_MSG_SIZE: usize = 128;
pub const MAILBOX_CAPACITY: usize = 4;

#[derive(Clone, Copy)]
pub struct Message {
    pub sender: usize,
    pub length: usize,
    pub data: [u8; MAX_MSG_SIZE],
}

pub struct Mailbox {
    pub messages: [Option<Message>; MAILBOX_CAPACITY],
    pub head: usize,
    pub len: usize,
}

impl Mailbox {
    const fn new() -> Self {
        Self {
            messages: [None; MAILBOX_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    pub fn pop(&mut self) -> Option<Message> {
        if self.len == 0 {
            return None;
        }

        let msg = self.messages[self.head];
        self.head = (self.head + 1) % MAILBOX_CAPACITY;
        self.len -= 1;
        msg
    }

    pub fn push(&mut self, msg: Message) -> bool {
        if self.len == MAILBOX_CAPACITY {
            return false;
        }

        let tail = (self.head + self.len) % MAILBOX_CAPACITY;
        self.messages[tail] = Some(msg);
        self.len += 1;
        true
    }
}

const MAX_HANDLES: usize = 32;

pub enum KernelObject {
    AddressSpace(usize),
    Frame(FrameAddr),
    Thread(usize),
}

pub struct Handle {
    pub object: KernelObject,
    pub rights: Rights,
}

#[derive(Clone, Copy)]
pub struct Rights(pub u32);

impl Rights {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const MAP: Self = Self(1 << 3);
}

impl BitOr for Rights {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

pub struct HandleTable {
    handles: [Option<Handle>; MAX_HANDLES],
}

impl HandleTable {
    pub const fn new() -> Self {
        Self {
            handles: [const { None }; MAX_HANDLES],
        }
    }

    pub fn push(&mut self, handle: Handle) -> Option<usize> {
        for (i, slot) in self.handles.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(handle);
                return Some(i);
            }
        }

        None
    }

    pub fn get(&self, id: usize) -> Option<&Handle> {
        self.handles.get(id).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Handle> {
        self.handles.get_mut(id).and_then(Option::as_mut)
    }
}

pub struct Tcb {
    pub id: usize,
    pub as_id: usize,
    pub state: ThreadState,
    pub kernel_stack: KernelStack,
    pub context: ThreadContext,
    pub mailbox: Mailbox,
    pub handles: HandleTable,
}

impl core::fmt::Debug for Tcb {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Tcb")
            .field("id", &self.id)
            .field("as_id", &self.as_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Tcb {
    pub fn new_user(
        id: usize,
        as_id: usize,
        kernel_stack: KernelStack,
        entry: VirtualAddr,
        user_stack: VirtualAddr,
    ) -> Self {
        let context = ThreadContext::new(entry, user_stack, kernel_stack.top());
        let mut handles = HandleTable::new();
        handles.push(Handle {
            object: KernelObject::AddressSpace(as_id),
            rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
        });

        Self {
            id,
            as_id,
            state: ThreadState::Ready,
            kernel_stack,
            context,
            mailbox: Mailbox::new(),
            handles,
        }
    }
}
