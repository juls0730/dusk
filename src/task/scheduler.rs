use core::cell::UnsafeCell;

use crate::{
    arch::ThreadContext,
    memory::{
        AddressSpace, AddressSpaceId, FrameAllocator, KernelStack, KernelStackPool,
        StackCreateError, VirtualAddr,
    },
    println,
    task::tcb::{BlockReason, ExitReason, Handle, KernelObject, Rights, Tcb, ThreadState},
};

const MAX_TASKS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct TaskId(usize);

impl TaskId {
    pub const fn new(id: usize) -> Self {
        Self(id)
    }
}

struct Scheduler {
    current: Option<TaskId>,
    tasks: [Option<Tcb>; MAX_TASKS],
    ready: ReadyQueue,
    stacks: KernelStackPool,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            current: None,
            tasks: [const { None }; MAX_TASKS],
            ready: ReadyQueue::new(),
            stacks: KernelStackPool::new(),
        }
    }

    fn make_switch(&mut self, current_id: TaskId, next_id: TaskId) -> Switch {
        assert_ne!(current_id, next_id);

        let current = self.tasks[current_id.0].as_mut().unwrap();
        let prev_ctx = &mut current.context as *mut ThreadContext;
        let prev_as_id = current.as_id;

        let next = self.tasks[next_id.0].as_ref().unwrap();
        let next_ctx = &next.context as *const ThreadContext;
        let next_as_id = next.as_id;
        let next_kernel_stack = next.kernel_stack.top();

        Switch {
            previous_context: prev_ctx,
            next_context: next_ctx,
            next_as_id,
            next_kernel_stack,
            activate_address_space: next_as_id != prev_as_id,
        }
    }
}

struct Switch {
    previous_context: *mut ThreadContext,
    next_context: *const ThreadContext,
    next_as_id: AddressSpaceId,
    next_kernel_stack: VirtualAddr,
    activate_address_space: bool,
}

impl Switch {
    unsafe fn perform(self) {
        if self.activate_address_space {
            crate::memory::with_address_space(self.next_as_id, |as_ref| unsafe {
                as_ref.activate();
            });
        }

        crate::arch::set_kernel_stack(self.next_kernel_stack);

        unsafe {
            crate::arch::switch_context(self.previous_context, self.next_context);
        }
    }
}

struct ReadyQueue {
    entries: [TaskId; MAX_TASKS],
    head: usize,
    len: usize,
}

impl ReadyQueue {
    pub const fn new() -> Self {
        Self {
            entries: [TaskId(0); MAX_TASKS],
            head: 0,
            len: 0,
        }
    }

    pub fn push_back(&mut self, task: TaskId) -> bool {
        if self.len == MAX_TASKS {
            return false;
        }

        let tail = (self.head + self.len) % MAX_TASKS;
        self.entries[tail] = task;
        self.len += 1;

        true
    }

    pub fn pop_front(&mut self) -> Option<TaskId> {
        if self.len == 0 {
            return None;
        }

        let task = self.entries[self.head];
        self.head = (self.head + 1) % MAX_TASKS;
        self.len -= 1;

        Some(task)
    }

    pub fn remove(&mut self, task: TaskId) -> bool {
        for i in 0..self.len {
            let idx = (self.head + i) % MAX_TASKS;
            if self.entries[idx] == task {
                for j in i..(self.len - 1) {
                    let from = (self.head + j + 1) % MAX_TASKS;
                    let to = (self.head + j) % MAX_TASKS;
                    self.entries[to] = self.entries[from];
                }
                self.len -= 1;
                return true;
            }
        }

        false
    }
}

struct GlobalScheduler(UnsafeCell<Scheduler>);

unsafe impl Sync for GlobalScheduler {}

static SCHEDULER: GlobalScheduler = GlobalScheduler(UnsafeCell::new(Scheduler::new()));

pub fn add_task(mut task: Tcb) -> Result<TaskId, Tcb> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();

    let result = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };

        match scheduler.tasks.iter().position(Option::is_none) {
            Some(id) => {
                let id = TaskId(id);

                task.id = id;

                match task.handles.push(Handle {
                    object: KernelObject::Thread(id),
                    rights: Rights::READ | Rights::WRITE | Rights::EXECUTE,
                }) {
                    Ok(_) => {}
                    Err(_) => unreachable!("Cant push root thread handle"),
                }

                task.state = ThreadState::Ready;
                scheduler.tasks[id.0] = Some(task);
                assert!(scheduler.ready.push_back(id));
                Ok(id)
            }
            None => Err(task),
        }
    };

    crate::arch::restore_interrupts(interrupt_state);
    result
}

pub fn start() -> ! {
    crate::arch::disable_interrupts();

    let mut bootstrap_context = ThreadContext::empty();

    let switch = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };
        let next_id = scheduler.ready.pop_front().expect("no tasks to run");

        let next = scheduler.tasks[next_id.0]
            .as_mut()
            .expect("ready task is missing");

        next.state = ThreadState::Running;
        scheduler.current = Some(next_id);

        Switch {
            previous_context: &mut bootstrap_context,
            next_context: &next.context,
            next_as_id: next.as_id,
            next_kernel_stack: next.kernel_stack.top(),
            activate_address_space: true,
        }
    };

    unsafe {
        switch.perform();
    }

    panic!("scheduler returned to bootstrap context");
}

pub fn allocate_kernel_stack(
    address_space: &mut AddressSpace,
    allocator: &mut FrameAllocator,
) -> Result<KernelStack, StackCreateError> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let result = scheduler.stacks.allocate(address_space, allocator);
    crate::arch::restore_interrupts(interrupt_state);
    result
}

pub fn remove_task(id: TaskId) -> bool {
    let interrupt_state = crate::arch::disable_interrupts_and_save();

    let result = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };

        if scheduler.current == Some(id) {
            false
        } else if let Some(task) = scheduler.tasks.get_mut(id.0).and_then(Option::take) {
            scheduler.ready.remove(id);
            scheduler.stacks.free(task.kernel_stack);
            true
        } else {
            false
        }
    };

    crate::arch::restore_interrupts(interrupt_state);
    result
}

pub fn with_task<R>(id: TaskId, f: impl FnOnce(&Tcb) -> R) -> Option<R> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    let res = scheduler.tasks.get(id.0).and_then(Option::as_ref).map(f);
    crate::arch::restore_interrupts(interrupt_state);
    res
}

pub fn with_task_mut<R>(id: TaskId, f: impl FnOnce(&mut Tcb) -> R) -> Option<R> {
    let interrupt_state = crate::arch::disable_interrupts_and_save();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let res = scheduler
        .tasks
        .get_mut(id.0)
        .and_then(Option::as_mut)
        .map(f);
    crate::arch::restore_interrupts(interrupt_state);
    res
}

pub fn current() -> TaskId {
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    scheduler.current.expect("no current task")
}

pub fn block_current(reason: BlockReason) {
    let interrupt_state = crate::arch::disable_interrupts_and_save();

    let switch = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };

        let Some(next_id) = scheduler.ready.pop_front() else {
            println!("Deadlock: all tasks blocked");
            crate::hcf();
        };

        let current_id = scheduler.current.expect("no current task");

        scheduler.tasks[current_id.0].as_mut().unwrap().state = ThreadState::Blocked(reason);
        // explicitly do NOT push back the current task, because it is not ready

        scheduler.tasks[next_id.0].as_mut().unwrap().state = ThreadState::Running;
        scheduler.current = Some(next_id);

        scheduler.make_switch(current_id, next_id)
    };

    unsafe {
        switch.perform();
    }

    // this runs when this task is selected to run again
    crate::arch::restore_interrupts(interrupt_state);
}

pub fn unblock(id: TaskId) {
    let interrupt_state = crate::arch::disable_interrupts_and_save();

    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    if let Some(task) = scheduler.tasks[id.0].as_mut() {
        if matches!(task.state, ThreadState::Blocked(_)) {
            task.state = ThreadState::Ready;
            assert!(scheduler.ready.push_back(id));
        }
    }

    crate::arch::restore_interrupts(interrupt_state);
}

pub fn yield_current() {
    let interrupt_state = crate::arch::disable_interrupts_and_save();

    let switch = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };

        let Some(next_id) = scheduler.ready.pop_front() else {
            crate::arch::restore_interrupts(interrupt_state);
            return;
        };

        let current_id = scheduler.current.expect("no current task");

        scheduler.tasks[current_id.0].as_mut().unwrap().state = ThreadState::Ready;
        assert!(scheduler.ready.push_back(current_id));

        scheduler.tasks[next_id.0].as_mut().unwrap().state = ThreadState::Running;
        scheduler.current = Some(next_id);

        scheduler.make_switch(current_id, next_id)
    };

    unsafe {
        switch.perform();
    }

    // this runs when this task is selected to run again
    crate::arch::restore_interrupts(interrupt_state);
}

pub fn exit_current(reason: ExitReason) -> ! {
    crate::arch::disable_interrupts();

    let switch = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };
        let current_id = scheduler.current.expect("no current task");
        let Some(next_id) = scheduler.ready.pop_front() else {
            println!("All tasks exited");
            crate::hcf();
        };

        let current = scheduler.tasks[current_id.0].as_mut().unwrap();
        current.state = ThreadState::Dead(reason);

        scheduler.tasks[next_id.0].as_mut().unwrap().state = ThreadState::Running;
        scheduler.current = Some(next_id);

        scheduler.make_switch(current_id, next_id)
    };

    unsafe {
        switch.perform();
    }

    panic!("dead task was scheduled again");
}
