#![no_std]
#![no_main]

mod cpio;
mod elf;

use dusk_sys::{
    Handle, SELF_AS, println, sys_as_create, sys_exit, sys_frame_alloc, sys_map, sys_task_create,
    sys_unmap, sys_yield,
};

// Mapped into the root task's address space by the kernel.
static INITRAMFS_START: usize = 0x4000_0000;
const SCRATCH_PAGE: usize = 0x8000_0000;
const STACK_TOP: usize = 0x0000_7FFF_FFFF_F000;
const STACK_PAGES: usize = 4;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    println!(r#"-----------------------------"#);
    println!(r#"   .d88888888b.   .d88888b.  "#);
    println!(r#"  d88P"    "Y88b 88P"  "Y88  "#);
    println!(r#"  888        888     .od88P  "#);
    println!(r#"  Y88b      d88P      "Y88b  "#);
    println!(r#"   "88bo  od88"  88b    d88  "#);
    println!(r#"  d88888  88888b  "Y88888P"  "#);
    println!(r#"----- Omega3 Dusk Root Server"#);

    let echo_bytes = cpio::find_file(INITRAMFS_START as *const u8, "echo.elf").unwrap();
    let echo_elf = elf::Elf::parse(echo_bytes).unwrap();
    let echo_as = sys_as_create().unwrap();
    let echo_entry = load_elf(&echo_elf, echo_as);
    map_stack(echo_as, STACK_TOP, STACK_PAGES);
    let _ = sys_task_create(echo_as, echo_entry, STACK_TOP).unwrap();

    let client_bytes = cpio::find_file(INITRAMFS_START as *const u8, "client.elf").unwrap();
    let client_elf = elf::Elf::parse(client_bytes).unwrap();
    let client_as = sys_as_create().unwrap();
    let client_entry = load_elf(&client_elf, client_as);
    map_stack(client_as, STACK_TOP, STACK_PAGES);
    let _ = sys_task_create(client_as, client_entry, STACK_TOP).unwrap();

    sys_exit(0);
}

fn load_elf(elf: &elf::Elf, target_as: Handle) -> usize {
    for header in elf.program_headers().unwrap() {
        let header = header.unwrap();
        if header.segment_type != elf::ProgramHeaderType::Load || header.memory_size == 0 {
            continue;
        }

        let mut perms = 0;
        if header.flags & 2 != 0 {
            perms |= 1 << 0;
        }
        if header.flags & 1 != 0 {
            perms |= 1 << 1;
        }

        let segment_start = header.virtual_address as usize;
        let segment_end = segment_start + header.memory_size as usize;
        let file_end = segment_start + header.file_size as usize;
        let page_start = segment_start & !0xFFF;

        for page in (page_start..segment_end).step_by(0x1000) {
            let frame = sys_frame_alloc().unwrap();

            sys_map(SELF_AS, frame, SCRATCH_PAGE, 0b01).unwrap();
            unsafe {
                core::ptr::write_bytes(SCRATCH_PAGE as *mut u8, 0, 0x1000);

                let copy_start = page.max(segment_start).min(page + 0x1000);
                let copy_end = (page + 0x1000).min(file_end).max(copy_start);
                if copy_end > copy_start {
                    let page_offset = copy_start - page;
                    let file_offset = header.file_offset as usize + (copy_start - segment_start);
                    let len = copy_end - copy_start;

                    core::ptr::copy_nonoverlapping(
                        elf.bytes().as_ptr().add(file_offset),
                        (SCRATCH_PAGE as *mut u8).add(page_offset),
                        len,
                    );
                }
            }
            sys_unmap(SELF_AS, SCRATCH_PAGE).unwrap();

            sys_map(target_as, frame, page, perms).unwrap();
        }
    }

    elf.entry()
}

fn map_stack(target_as: Handle, stack_top: usize, pages: usize) {
    for i in 1..=pages {
        let frame = sys_frame_alloc().unwrap();
        let page_addr = stack_top - i * 0x1000;
        sys_map(target_as, frame, page_addr, 0b01).unwrap();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    sys_exit(1);
}
