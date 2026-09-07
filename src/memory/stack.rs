use crate::memory::{
    AddressSpace, CachePolicy, FRAME_SIZE, FrameAllocator, MapError, OwnedFrame, PagePermissions,
    VirtualAddr,
};

const GUARD_PAGES: usize = 1; // 4KiB
const KERNEL_STACK_PAGES: usize = 8; // 32KiB
const USER_STACK_PAGES: usize = 16; // 64KiB
const KERNEL_STACK_SIZE: usize = KERNEL_STACK_PAGES * FRAME_SIZE;
const USER_STACK_SIZE: usize = USER_STACK_PAGES * FRAME_SIZE;

const KERNEL_SLOT_SIZE: usize = (KERNEL_STACK_PAGES + GUARD_PAGES) * FRAME_SIZE;
const MAX_KERNEL_STACKS: usize = 64;
const KERNEL_STACK_BASE: usize = 0xFFFF_FFFE_0000_0000;

const USER_STACK_TOP: VirtualAddr = VirtualAddr::new(0x0000_7FFF_FFFF_F000);

#[derive(Debug)]
pub enum StackCreateError {
    AddressOverflow,
    UnalignedStackTop,
    OutOfFrames,
    OutOfStacks,
    GuardPageMapped,
    Map(MapError),
}

#[derive(Debug)]
struct StackMapping {
    guard_page: VirtualAddr,
    mapped_start: VirtualAddr,
    stack_size: usize,
    top: VirtualAddr,
}

impl StackMapping {
    fn allocate(
        address_space: &mut AddressSpace,
        allocator: &mut FrameAllocator,
        stack_size: usize,
        top: VirtualAddr,
        permissions: PagePermissions,
    ) -> Result<Self, StackCreateError> {
        if top.as_usize() % FRAME_SIZE != 0 {
            return Err(StackCreateError::UnalignedStackTop);
        }

        let mapped_start = VirtualAddr::new(
            top.as_usize()
                .checked_sub(stack_size)
                .ok_or(StackCreateError::AddressOverflow)?,
        );
        let guard_page = VirtualAddr::new(
            mapped_start
                .as_usize()
                .checked_sub(FRAME_SIZE)
                .ok_or(StackCreateError::AddressOverflow)?,
        );

        if address_space.to_physical(guard_page).is_some() {
            return Err(StackCreateError::GuardPageMapped);
        }

        let mut mapped_pages = 0;

        while mapped_pages < stack_size / FRAME_SIZE {
            let virtual_address = VirtualAddr::new(
                mapped_start
                    .as_usize()
                    .checked_add(mapped_pages * FRAME_SIZE)
                    .ok_or(StackCreateError::AddressOverflow)?,
            );
            let frame = match allocator.alloc() {
                Some(frame) => frame,
                None => {
                    Self::rollback(address_space, allocator, mapped_start, mapped_pages);
                    return Err(StackCreateError::OutOfFrames);
                }
            };

            if let Err(error) = address_space.map(
                frame.frame_address().start_address(),
                virtual_address,
                permissions,
                allocator,
                CachePolicy::WriteBack,
            ) {
                unsafe { allocator.dealloc(frame) };
                Self::rollback(address_space, allocator, mapped_start, mapped_pages);
                return Err(StackCreateError::Map(error));
            }

            let _ = frame.into_raw();
            mapped_pages += 1;
        }

        debug_assert!(address_space.to_physical(guard_page).is_none());

        Ok(Self {
            guard_page,
            mapped_start,
            stack_size,
            top,
        })
    }

    fn rollback(
        address_space: &mut AddressSpace,
        allocator: &mut FrameAllocator,
        mapped_start: VirtualAddr,
        mapped_pages: usize,
    ) {
        for page in (0..mapped_pages).rev() {
            let virtual_address = VirtualAddr::new(mapped_start.as_usize() + page * FRAME_SIZE);
            let frame = unsafe {
                address_space
                    .unmap(virtual_address, allocator)
                    .expect("failed to roll back stack mapping")
            };
            unsafe { allocator.dealloc(OwnedFrame::from_raw(frame)) };
        }
    }

    /// # Safety
    ///
    /// The caller must ensure this stack is not active on any CPU and cannot be accessed by any
    /// kernel operation while it is being destroyed.
    unsafe fn destroy(self, address_space: &mut AddressSpace, allocator: &mut FrameAllocator) {
        for page in (0..self.stack_size / FRAME_SIZE).rev() {
            let virtual_address =
                VirtualAddr::new(self.mapped_start.as_usize() + page * FRAME_SIZE);
            let frame = unsafe {
                address_space
                    .unmap(virtual_address, allocator)
                    .expect("stack mapping was unexpectedly missing")
            };
            unsafe { allocator.dealloc(OwnedFrame::from_raw(frame)) };
        }

        debug_assert!(address_space.to_physical(self.guard_page).is_none());
    }
}

#[derive(Debug)]
pub struct KernelStack {
    mapping: StackMapping,
}

impl KernelStack {
    pub fn allocate(
        address_space: &mut AddressSpace,
        top: usize,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, StackCreateError> {
        let mapping = StackMapping::allocate(
            address_space,
            allocator,
            KERNEL_STACK_SIZE,
            VirtualAddr::new(top),
            PagePermissions::new(true, false, false),
        )?;

        Ok(Self { mapping })
    }

    pub const fn top(&self) -> VirtualAddr {
        self.mapping.top
    }

    /// # Safety
    ///
    /// The caller must ensure this stack is not active on any CPU and cannot be accessed by any
    /// kernel operation while it is being destroyed.
    pub unsafe fn destroy(self, address_space: &mut AddressSpace, allocator: &mut FrameAllocator) {
        unsafe { self.mapping.destroy(address_space, allocator) };
    }
}

pub struct UserStack {
    mapping: StackMapping,
}

impl UserStack {
    pub fn allocate(
        address_space: &mut AddressSpace,
        allocator: &mut FrameAllocator,
    ) -> Result<Self, StackCreateError> {
        let top = VirtualAddr::new(USER_STACK_TOP.as_usize());
        let mapping = StackMapping::allocate(
            address_space,
            allocator,
            USER_STACK_SIZE,
            top,
            PagePermissions::new(true, false, true),
        )?;

        Ok(Self { mapping })
    }

    pub const fn top(&self) -> VirtualAddr {
        self.mapping.top
    }

    /// # Safety
    ///
    /// The caller must ensure this stack is not active in any thread and cannot be accessed while
    /// it is being destroyed.
    pub unsafe fn destroy(self, address_space: &mut AddressSpace, allocator: &mut FrameAllocator) {
        unsafe { self.mapping.destroy(address_space, allocator) };
    }
}

pub struct KernelStackPool {
    free_slots: u64, // bitmap
}

impl KernelStackPool {
    pub const fn new() -> Self {
        Self { free_slots: 0 }
    }

    pub fn allocate(
        &mut self,
        address_space: &mut AddressSpace,
        allocator: &mut FrameAllocator,
    ) -> Result<KernelStack, StackCreateError> {
        let mut slot = 0;
        while slot < MAX_KERNEL_STACKS {
            if self.free_slots & (1 << slot) == 0 {
                self.free_slots |= 1 << slot;
                let top = KERNEL_STACK_BASE + ((slot + 1) * KERNEL_SLOT_SIZE);
                return KernelStack::allocate(address_space, top, allocator);
            }
            slot += 1;
        }

        Err(StackCreateError::OutOfStacks)
    }

    pub fn free(&mut self, stack: KernelStack) {
        let slot = (stack.top().as_usize() - KERNEL_STACK_BASE) / KERNEL_SLOT_SIZE - 1;
        crate::memory::with_kernel_address_space(|kernel_as| {
            crate::memory::with_allocator(|allocator| unsafe {
                stack.destroy(kernel_as, allocator)
            });
        });
        self.free_slots &= !(1 << slot);
    }
}
