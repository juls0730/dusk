use crate::{
    arch::ThreadContext,
    memory::{AddressSpace, KernelStack, VirtualAddr},
    println,
};

// Thread Control Block
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Dead,
}

#[derive(Debug)]
pub struct Tcb {
    pub id: usize,
    pub state: ThreadState,
    pub kernel_stack: KernelStack,
    pub context: ThreadContext,
    pub address_space: AddressSpace,
}

impl Tcb {
    pub fn new_user(
        id: usize,
        address_space: AddressSpace,
        kernel_stack: KernelStack,
        user_entry: VirtualAddr,
        user_stack: VirtualAddr,
    ) -> Self {
        let context = ThreadContext::new(user_entry, user_stack, kernel_stack.top());

        Self {
            id,
            state: ThreadState::Ready,
            kernel_stack,
            context,
            address_space,
        }
    }
}

pub unsafe fn run_first_user(user_tcb: &Tcb) {
    let previous_interrupts = crate::arch::disable_interrupts_and_save();

    let mut boot_thread_ctx = ThreadContext::empty();
    unsafe {
        user_tcb.address_space.activate();
        crate::arch::set_kernel_stack(user_tcb.kernel_stack.top());
        crate::arch::switch_context(&mut boot_thread_ctx, &user_tcb.context);
    }

    crate::arch::restore_interrupts(previous_interrupts);
}

pub unsafe fn switch(prev: &mut Tcb, next: &Tcb) {
    let previous_interrupts = crate::arch::disable_interrupts_and_save();

    if prev.address_space != next.address_space {
        unsafe { next.address_space.activate() };
    }

    crate::arch::set_kernel_stack(next.kernel_stack.top());

    unsafe {
        crate::arch::switch_context(&mut prev.context, &next.context);
    }

    crate::arch::restore_interrupts(previous_interrupts);
}
