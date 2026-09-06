use crate::{
    arch::ThreadContext,
    memory::{AddressSpace, KernelStack, VirtualAddr},
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

#[derive(Clone, Copy, Debug)]
pub struct Message {
    pub sender: usize,
    pub length: usize,
    pub data: [u8; MAX_MSG_SIZE],
}

#[derive(Debug)]
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

#[derive(Debug)]
pub struct Tcb {
    pub id: usize,
    pub state: ThreadState,
    pub kernel_stack: KernelStack,
    pub context: ThreadContext,
    pub address_space: AddressSpace,
    pub mailbox: Mailbox,
}

impl Tcb {
    pub fn new_user(
        id: usize,
        address_space: AddressSpace,
        kernel_stack: KernelStack,
        entry: VirtualAddr,
        user_stack: VirtualAddr,
    ) -> Self {
        let context = ThreadContext::new(entry, user_stack, kernel_stack.top());

        Self {
            id,
            state: ThreadState::Ready,
            kernel_stack,
            context,
            address_space,
            mailbox: Mailbox::new(),
        }
    }
}
