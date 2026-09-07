#![no_std]
#![no_main]

use dusk_sys::{println, sys_exit, sys_recv, sys_send};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let msg = "Hello from client!";
    println!("[client] Sent: {}", msg);
    // TODO: we assume the echo server is task 1 (spawned by omega3)
    sys_send(1, msg.as_bytes()).unwrap();

    let mut out = [0u8; 128];
    let (actual_len, _) = sys_recv(&mut out).unwrap();
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
