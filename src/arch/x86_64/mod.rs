mod cpu;
mod gdt;
mod interrupts;
mod paging;
pub mod port;

use core::arch::asm;

pub use interrupts::disable_interrupts;
pub(crate) use paging::{
    MapError as PageTableMapError, PageTableCreateError, UnmapError as PageTableUnmapError,
};
pub use paging::{PageTable, PagingConfig};

pub struct ArchState {
    pub paging: PagingConfig,
}

use crate::println;

pub fn init() -> ArchState {
    disable_interrupts();
    println!("Loading GDT...");
    gdt::init();
    println!("Loading IDT...");
    interrupts::init();
    println!("Detecting CPU features...");
    let cpu_features = cpu::detect_features_and_enable();
    let paging =
        PagingConfig::from_features(cpu_features.expect("required CPU features are not supported"));
    ArchState { paging }
}

pub fn halt() {
    unsafe {
        asm!("hlt");
    }
}
