use ::limine as limine_api;

use limine_api::request::{ExecutableAddressRequest, HhdmRequest, MemmapRequest};
use limine_api::{BaseRevision, RequestsEndMarker, RequestsStartMarker};

/// Sets the base revision to the latest revision supported by the crate.
/// See specification for further info.
/// Be sure to mark all limine requests with #[used], otherwise they may be removed by the compiler.
#[used]
// The .requests section allows limine to find the requests faster and more safely.
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static KERNEL_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

/// Define the stand and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootInfo {
    pub kernel_address: crate::memory::PhysicalAddr,
    pub hhdm_offset: u64,
    entries: &'static [&'static limine_api::memmap::Entry],
}

impl BootInfo {
    pub fn memory_regions(&self) -> impl Iterator<Item = crate::memory::MemoryRegion> + '_ {
        use crate::memory::{MemoryRegion, MemoryRegionKind};
        self.entries.iter().map(|&entry| MemoryRegion {
            start: crate::memory::PhysicalAddr::new(entry.base),
            length: entry.length,
            kind: match entry.type_ {
                limine_api::memmap::MEMMAP_USABLE => MemoryRegionKind::Usable,
                limine_api::memmap::MEMMAP_RESERVED => MemoryRegionKind::Reserved,
                limine_api::memmap::MEMMAP_ACPI_RECLAIMABLE => MemoryRegionKind::AcpiReclaimable,
                limine_api::memmap::MEMMAP_ACPI_NVS => MemoryRegionKind::AcpiNvs,
                limine_api::memmap::MEMMAP_BAD_MEMORY => MemoryRegionKind::BadMemory,
                limine_api::memmap::MEMMAP_BOOTLOADER_RECLAIMABLE => {
                    MemoryRegionKind::BootloaderReclaimable
                }
                limine_api::memmap::MEMMAP_EXECUTABLE_AND_MODULES => {
                    MemoryRegionKind::KernelAndModules
                }
                limine_api::memmap::MEMMAP_FRAMEBUFFER => MemoryRegionKind::Framebuffer,
                limine_api::memmap::MEMMAP_MAPPED_RESERVED => MemoryRegionKind::MappedReserved,
                _ => MemoryRegionKind::Reserved,
            },
        })
    }
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedBaseRevision,
    FailedToGetKernelAddress,
    FailedToGetHHDMAddress,
    FailedToGetMemmap,
}

pub fn load_boot_info() -> Result<BootInfo, BootError> {
    if !BASE_REVISION.is_supported() {
        return Err(BootError::UnsupportedBaseRevision);
    }

    let kernel_address = KERNEL_ADDRESS_REQUEST
        .response()
        .ok_or(BootError::FailedToGetKernelAddress)?
        .physical_base;
    let hhdm_offset = HHDM_REQUEST
        .response()
        .ok_or(BootError::FailedToGetHHDMAddress)?
        .offset;

    let memmap = MEMMAP_REQUEST
        .response()
        .ok_or(BootError::FailedToGetMemmap)?
        .entries();

    Ok(BootInfo {
        kernel_address: crate::memory::PhysicalAddr::new(kernel_address),
        hhdm_offset,
        entries: memmap,
    })
}
