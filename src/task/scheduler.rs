use core::cell::UnsafeCell;

use crate::{
    arch::ThreadContext,
    memory::{AddressSpace, VirtualAddr},
    println,
    task::tcb::{BlockReason, ExitReason, Tcb, ThreadState},
};

const MAX_TASKS: usize = 32;

type TaskId = usize;

struct Scheduler {
    current: Option<TaskId>,
    tasks: [Option<Tcb>; MAX_TASKS],
    ready: ReadyQueue,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            current: None,
            tasks: [const { None }; MAX_TASKS],
            ready: ReadyQueue::new(),
        }
    }

    fn make_switch(&mut self, current_id: TaskId, next_id: TaskId) -> Switch {
        assert_ne!(current_id, next_id);

        let current = self.tasks[current_id].as_mut().unwrap();
        let prev_ctx = &mut current.context as *mut ThreadContext;
        let prev_addr_space = &current.address_space as *const AddressSpace;

        let next = self.tasks[next_id].as_ref().unwrap();
        let next_ctx = &next.context as *const ThreadContext;
        let next_addr_space = &next.address_space as *const AddressSpace;
        let next_kernel_stack = next.kernel_stack.top();

        Switch {
            previous_context: prev_ctx,
            next_context: next_ctx,
            next_address_space: next_addr_space,
            next_kernel_stack,
            activate_address_space: unsafe { *next_addr_space != *prev_addr_space },
        }
    }
}

struct Switch {
    previous_context: *mut ThreadContext,
    next_context: *const ThreadContext,
    next_address_space: *const AddressSpace,
    next_kernel_stack: VirtualAddr,
    activate_address_space: bool,
}

impl Switch {
    unsafe fn perform(self) {
        if self.activate_address_space {
            unsafe {
                (&*self.next_address_space).activate();
            }
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
            entries: [0; MAX_TASKS],
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
                task.id = id;
                task.state = ThreadState::Ready;
                scheduler.tasks[id] = Some(task);
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

        let next = scheduler.tasks[next_id]
            .as_mut()
            .expect("ready task is missing");

        next.state = ThreadState::Running;
        scheduler.current = Some(next_id);

        Switch {
            previous_context: &mut bootstrap_context,
            next_context: &next.context,
            next_address_space: &next.address_space,
            next_kernel_stack: next.kernel_stack.top(),
            activate_address_space: true,
        }
    };

    unsafe {
        switch.perform();
    }

    panic!("scheduler returned to bootstrap context");
}

pub fn get_task(id: TaskId) -> Option<&'static Tcb> {
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    scheduler.tasks.get(id).and_then(Option::as_ref)
}

pub fn get_task_mut(id: TaskId) -> Option<&'static mut Tcb> {
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    scheduler.tasks.get_mut(id).and_then(Option::as_mut)
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

        scheduler.tasks[current_id].as_mut().unwrap().state = ThreadState::Blocked(reason);
        // explicitly do NOT push back the current task, because it is not ready

        scheduler.tasks[next_id].as_mut().unwrap().state = ThreadState::Running;
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
    if let Some(task) = scheduler.tasks[id].as_mut() {
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

        scheduler.tasks[current_id].as_mut().unwrap().state = ThreadState::Ready;
        assert!(scheduler.ready.push_back(current_id));

        scheduler.tasks[next_id].as_mut().unwrap().state = ThreadState::Running;
        scheduler.current = Some(next_id);

        scheduler.make_switch(current_id, next_id)
    };

    unsafe {
        switch.perform();
    }

    // this runs when this task is selected to run again
    crate::arch::restore_interrupts(interrupt_state);
}

pub fn exit_current(exit_code: usize) -> ! {
    crate::arch::disable_interrupts();

    let switch = {
        let scheduler = unsafe { &mut *SCHEDULER.0.get() };
        let current_id = scheduler.current.expect("no current task");
        let Some(next_id) = scheduler.ready.pop_front() else {
            println!("All tasks exited");
            crate::hcf();
        };

        let current = scheduler.tasks[current_id].as_mut().unwrap();
        current.state = ThreadState::Dead(ExitReason::Exited(exit_code));

        scheduler.tasks[next_id].as_mut().unwrap().state = ThreadState::Running;
        scheduler.current = Some(next_id);

        scheduler.make_switch(current_id, next_id)
    };

    unsafe {
        switch.perform();
    }

    panic!("dead task was scheduled again");
}
