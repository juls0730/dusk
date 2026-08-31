#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64::{
    PageTableCreateError, PageTableMapError, PageTableUnmapError, ThreadContext, set_kernel_stack,
    switch_context,
};
