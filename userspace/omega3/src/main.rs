#![no_std]
#![no_main]

use dusk_sys::{println, sys_exit, sys_recv, sys_send};

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

    let buf = [0u8; 128];
    let buf_ptr = buf.as_ptr() as usize;
    let max_len = buf.len();
    loop {
        let (actual_len, sender) = sys_recv(buf_ptr, max_len).unwrap();
        println!(
            "[server] Received: {}",
            core::str::from_utf8(&buf[..actual_len]).unwrap()
        );
        sys_send(sender, buf_ptr, actual_len).unwrap();
    }

    sys_exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    sys_exit(1);
}
