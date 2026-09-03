use core::arch::{asm, naked_asm};

use crate::arch::x86_64::{
    cpu::{CpuLocal, write_msr},
    gdt::{KERNEL_CODE_SELECTOR, KERNEL_DATA_SELECTOR},
};

const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_CSTAR: u32 = 0xC000_0083;
const IA32_FMASK: u32 = 0xC000_0084;
const IA32_GS_BASE: u32 = 0xC000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

const RFLAGS_MASK: u64 = 0x257FD5; // Clear IF, TF, DF, IOPL, NT, AC

#[repr(C)]
#[derive(Debug)]
struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r9: u64,          // arg5
    pub r8: u64,          // arg4
    pub r10: u64,         // arg3
    pub rdx: u64,         // arg2
    pub rsi: u64,         // arg1
    pub rdi: u64,         // arg0
    pub rax: u64,         // syscall number on entry / return value on exit
    pub user_rip: u64,    // rcx
    pub user_rflags: u64, // r11
    pub user_rsp: u64,
}

pub fn init(cpu_local: *const CpuLocal) {
    unsafe {
        let star = ((KERNEL_DATA_SELECTOR as u64) << 48) | ((KERNEL_CODE_SELECTOR as u64) << 32);
        write_msr(IA32_STAR, star);
        write_msr(IA32_LSTAR, syscall_entry as *const () as u64);
        write_msr(IA32_CSTAR, 0);
        write_msr(IA32_FMASK, RFLAGS_MASK);
        write_msr(IA32_GS_BASE, 0);
        write_msr(IA32_KERNEL_GS_BASE, cpu_local as u64);

        // Kernel code always runs with GS pointing at CpuLocal. User entry
        // swaps this into IA32_KERNEL_GS_BASE before transitioning to ring 3.
        asm!("swapgs", options(nostack, preserves_flags));
    }
}

#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() {
    naked_asm!(
        "swapgs",
        "mov gs:[8], rsp", // user_rsp_scratch
        "mov rsp, gs:[0]", // kernel_stack_top
        "",
        // build the syscall frame
        "push qword ptr gs:[8]", // user_rsp
        "push r11",              // user_rflags
        "push rcx",              // user_rip
        "push rax",
        "push rdi",
        "push rsi",
        "push rdx",
        "push r10",
        "push r8",
        "push r9",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "",
        // Syscall calling convention:
        // Syscall number in rax, args in rdi, rsi, rdx, r10, r8, r9
        "mov rdi, rsp",
        "call {dispatch}",
        "",
        // restore the syscall frame
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "pop r9",
        "pop r8",
        "pop r10",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rax",              // return value
        "pop rcx",              // user_rip for sysret
        "pop r11",              // user_rflags for sysret
        "pop qword ptr gs:[8]", // user_rsp
        "",
        "mov rsp, gs:[8]", // switch to user stack
        "swapgs",
        "sysretq",
        dispatch = sym syscall_dispatch,
    );
}

extern "C" fn syscall_dispatch(frame: &mut SyscallFrame) {
    let ret = crate::syscall::handle(
        frame.rax, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
    );

    frame.rax = ret;
}
