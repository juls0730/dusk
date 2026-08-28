use core::arch::asm;

use crate::memory::VirtualAddr;

#[repr(C, align(8))]
struct Gdt {
    entries: [u64; 7],
}

impl Gdt {
    pub const fn new() -> Self {
        Self { entries: [0; 7] }
    }
}

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct TaskStateSegment {
    reserved_0: u32,
    privilege_stacks: [u64; 3],
    reserved_1: u64,
    interrupt_stacks: [u64; 7],
    reserved_2: u64,
    reserved_3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        Self {
            reserved_0: 0,
            privilege_stacks: [0; 3],
            reserved_1: 0,
            interrupt_stacks: [0; 7],
            reserved_2: 0,
            reserved_3: 0,
            iomap_base: core::mem::size_of::<TaskStateSegment>() as u16,
        }
    }
}

// 16 KiB
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

pub(super) const KERNEL_CODE_SELECTOR: u16 = 1 * 8;
pub(super) const KERNEL_DATA_SELECTOR: u16 = 2 * 8;
pub(super) const USER_CODE_SELECTOR: u16 = (3 * 8) | 3;
pub(super) const USER_DATA_SELECTOR: u16 = (4 * 8) | 3;
pub(super) const TSS_SELECTOR: u16 = 5 * 8;

#[repr(align(16))]
#[allow(dead_code)] // field 0 is read, rust just cant tell
struct ExceptionStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut GDT: Gdt = Gdt::new();
static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut DOUBLE_FAULT_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);

const _: () = assert!(core::mem::size_of::<TaskStateSegment>() == 104);
const _: () = assert!(core::mem::size_of::<GdtPointer>() == 10);

pub fn init() {
    unsafe {
        let stack_bottom = core::ptr::addr_of_mut!(DOUBLE_FAULT_STACK) as u64;
        let stack_top = stack_bottom + DOUBLE_FAULT_STACK_SIZE as u64;

        TSS.interrupt_stacks[0] = stack_top;

        let [tss_low, tss_high] = tss_descriptor(core::ptr::addr_of!(TSS));

        let gdt = Gdt {
            entries: [
                0,
                kernel_code_descriptor(),
                kernel_data_descriptor(),
                user_code_descriptor(),
                user_data_descriptor(),
                tss_low,
                tss_high,
            ],
        };
        core::ptr::addr_of_mut!(GDT).write(gdt);

        gdt_reload();
    }
}

pub fn set_kernel_stack(stack_top: VirtualAddr) {
    unsafe {
        TSS.privilege_stacks[0] = stack_top.as_usize() as u64;
    }
}

fn gdt_reload() {
    unsafe {
        asm!(
            "lgdt [{gdt_pointer}]",

            "push {code_selector}",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",

            "mov ax, {data_selector}",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",

            "mov ax, {tss_selector}",
            "ltr ax",

            gdt_pointer = in(reg) &GdtPointer {
                limit: (core::mem::size_of::<Gdt>() - 1) as u16,
                base: core::ptr::addr_of!(GDT) as u64,
            },
            code_selector = const KERNEL_CODE_SELECTOR,
            data_selector = const KERNEL_DATA_SELECTOR,
            tss_selector = const TSS_SELECTOR,
            lateout("rax") _,
            options(preserves_flags)
        )
    }
}

const PRESENT: u64 = 1 << 47;
const CODE_DATA_DESCRIPTOR: u64 = 1 << 44;
const USER_PRIVILEGE: u64 = 3 << 45;
const EXECUTABLE: u64 = 1 << 43;
const READ_WRITE: u64 = 1 << 41;
const GRANULARITY: u64 = 1 << 55;
const SIZE: u64 = 1 << 54;
const LONG_MODE: u64 = 1 << 53;

fn kernel_code_descriptor() -> u64 {
    PRESENT | CODE_DATA_DESCRIPTOR | EXECUTABLE | READ_WRITE | LONG_MODE | GRANULARITY
}

fn kernel_data_descriptor() -> u64 {
    PRESENT | CODE_DATA_DESCRIPTOR | READ_WRITE | SIZE | GRANULARITY
}

fn user_code_descriptor() -> u64 {
    PRESENT
        | CODE_DATA_DESCRIPTOR
        | USER_PRIVILEGE
        | EXECUTABLE
        | READ_WRITE
        | LONG_MODE
        | GRANULARITY
}

fn user_data_descriptor() -> u64 {
    PRESENT | CODE_DATA_DESCRIPTOR | USER_PRIVILEGE | READ_WRITE | SIZE | GRANULARITY
}

fn tss_descriptor(tss: *const TaskStateSegment) -> [u64; 2] {
    let base = tss as u64;
    let limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;

    let low = (limit & 0xFFFF)
        | ((base & 0x00FF_FFFF) << 16)
        | (0x09 << 40)
        | (1 << 47)
        | (((limit >> 16) & 0x0F) << 48)
        | (((base >> 24) & 0xFF) << 56);

    let high = base >> 32;
    [low, high]
}
