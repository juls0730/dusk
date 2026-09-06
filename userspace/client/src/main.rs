#![no_std]
#![no_main]

use dusk_sys::{println, sys_exit, sys_recv, sys_send};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = "Hello from client!";
    println!("[client] Sent: {}", msg);
    sys_send(0, msg.as_ptr() as usize, msg.len()).unwrap();
    let out = [0u8; 128];
    let out_ptr = out.as_ptr() as usize;
    let max_len = out.len();
    let (actual_len, sender) = sys_recv(out_ptr, max_len).unwrap();
    println!(
        "[client] Received: {}",
        core::str::from_utf8(&out[..actual_len]).unwrap()
    );
    sys_exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    sys_exit(1);
}
