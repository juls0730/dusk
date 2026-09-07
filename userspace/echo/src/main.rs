#![no_std]
#![no_main]

use dusk_sys::{println, sys_exit, sys_recv, sys_send};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let out = [0u8; 128];
    loop {
        let (actual_len, sender) = sys_recv(out.as_ptr() as usize, out.len()).unwrap();
        println!(
            "[echo] Received: {}",
            core::str::from_utf8(&out[..actual_len]).unwrap()
        );
        sys_send(sender, out.as_ptr() as usize, actual_len).unwrap();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    sys_exit(1);
}
