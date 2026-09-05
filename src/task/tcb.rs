use crate::{
    arch::ThreadContext,
    memory::{AddressSpace, KernelStack, VirtualAddr},
    task::loader::LoadedImage,
};

// Thread Control Block
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Exited(usize),
    Killed,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Dead(ExitReason),
}

#[derive(Debug)]
pub struct Tcb {
    pub id: usize,
    pub state: ThreadState,
    pub kernel_stack: KernelStack,
    pub context: ThreadContext,
    pub address_space: AddressSpace,
    pub image: LoadedImage,
}

impl Tcb {
    pub fn new_user(
        id: usize,
        address_space: AddressSpace,
        kernel_stack: KernelStack,
        image: LoadedImage,
        user_stack: VirtualAddr,
    ) -> Self {
        let context = ThreadContext::new(image.entry, user_stack, kernel_stack.top());

        Self {
            id,
            state: ThreadState::Ready,
            kernel_stack,
            context,
            address_space,
            image,
        }
    }
}
