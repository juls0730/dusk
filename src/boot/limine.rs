use ::limine as limine_api;
use limine::paging::PagingMode;
use limine::request::{ExecutableCmdlineRequest, ModulesRequest, PagingModeRequest, RsdpRequest};

use limine_api::request::{ExecutableAddressRequest, HhdmRequest, MemmapRequest};
use limine_api::{BaseRevision, RequestsEndMarker, RequestsStartMarker};

use crate::memory::{
    BootString, InitramfsImage, KernelMemoryLayout, KernelSegment, MemoryMap, MemoryRegion,
    MemoryRegionKind, PagePermissions, PhysicalAddr, VirtualAddr,
};

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
static KERNEL_CMDLINE_REQUEST: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static KERNEL_MODULE_REQUEST: ModulesRequest = ModulesRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static PAGING_REQUEST: PagingModeRequest =
    PagingModeRequest::new(PagingMode::MAX, PagingMode::MAX, PagingMode::MIN);

#[used]
#[unsafe(link_section = ".requests")]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

/// Define the stand and end markers for Limine requests.
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

unsafe extern "C" {
    static __text_start: u64;
    static __text_end: u64;

    static __rodata_start: u64;
    static __rodata_end: u64;

    static __data_start: u64;
    static __data_end: u64;
}

const MAX_COMMAND_LINE_LENGTH: usize = 512;

pub struct BootInfo {
    pub kernel_layout: KernelMemoryLayout,
    pub command_line: BootString<MAX_COMMAND_LINE_LENGTH>,
    pub initramfs: InitramfsImage,
    pub hhdm_offset: usize,
    pub memory_map: MemoryMap,
    pub rsdp: VirtualAddr,
}

impl BootInfo {
    pub fn memory_regions(&self) -> impl Iterator<Item = MemoryRegion> + Clone + '_ {
        self.memory_map.iter()
    }
}

#[derive(Debug)]
pub enum BootError {
    UnsupportedBaseRevision,
    FailedToGetKernelAddress,
    FailedToGetKernelCmdline,
    FailedToGetModules,
    FailedToGetInitramfs,
    FailedToGetHHDMAddress,
    FailedToGetMemmap,
    TooManyMemoryRegions,
    FailedToLocateKernel,
    FailedToGetRsdp,
}

pub fn load_boot_info() -> Result<BootInfo, BootError> {
    if !BASE_REVISION.is_supported() {
        return Err(BootError::UnsupportedBaseRevision);
    }

    let kernel_address = KERNEL_ADDRESS_REQUEST
        .response()
        .ok_or(BootError::FailedToGetKernelAddress)?;
    let kernel_cmdline = KERNEL_CMDLINE_REQUEST
        .response()
        .ok_or(BootError::FailedToGetKernelCmdline)?;
    let command_line = BootString::from_bytes(kernel_cmdline.cmdline().as_bytes());

    let modules = KERNEL_MODULE_REQUEST
        .response()
        .ok_or(BootError::FailedToGetModules)?;
    let initramfs = modules
        .modules()
        .get(0)
        .map(|module| {
            let start = VirtualAddr::new(module.data().as_ptr() as usize);
            let length = module.data().len();
            InitramfsImage { start, length }
        })
        .ok_or(BootError::FailedToGetInitramfs)?;

    let hhdm_offset = HHDM_REQUEST
        .response()
        .ok_or(BootError::FailedToGetHHDMAddress)?
        .offset as usize;

    let rsdp = RSDP_REQUEST.response().ok_or(BootError::FailedToGetRsdp)?;

    let memmap = MEMMAP_REQUEST
        .response()
        .ok_or(BootError::FailedToGetMemmap)?
        .entries();

    let mut memory_map = MemoryMap::new();
    for entry in memmap.iter() {
        memory_map
            .push(MemoryRegion {
                start: PhysicalAddr::new(entry.base as usize),
                length: entry.length as usize,
                kind: match entry.type_ {
                    limine_api::memmap::MEMMAP_USABLE => MemoryRegionKind::Usable,
                    limine_api::memmap::MEMMAP_RESERVED => MemoryRegionKind::Reserved,
                    limine_api::memmap::MEMMAP_ACPI_RECLAIMABLE => {
                        MemoryRegionKind::AcpiReclaimable
                    }
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
            .map_err(|_| BootError::TooManyMemoryRegions)?;
    }

    let mut segment_physical = kernel_address.physical_base as usize;
    let mut segment_virtual = core::ptr::addr_of!(__text_start) as usize;

    let mut segment_length = 0;

    let kernel_text_segment = KernelSegment {
        physical_base: PhysicalAddr::new(segment_physical),
        virtual_base: VirtualAddr::new(segment_virtual),
        length: (core::ptr::addr_of!(__text_end) as usize)
            - (core::ptr::addr_of!(__text_start) as usize),
        permissions: PagePermissions::new(false, true, false),
    };

    segment_length += kernel_text_segment.length;

    segment_virtual = core::ptr::addr_of!(__rodata_start) as usize;
    segment_physical = kernel_address.physical_base as usize
        + (segment_virtual - kernel_address.virtual_base as usize);

    let kernel_rodata_segment = KernelSegment {
        physical_base: PhysicalAddr::new(segment_physical),
        virtual_base: VirtualAddr::new(segment_virtual),
        length: (core::ptr::addr_of!(__rodata_end) as usize)
            - (core::ptr::addr_of!(__rodata_start) as usize),
        permissions: PagePermissions::new(false, false, false),
    };

    segment_length += kernel_rodata_segment.length;

    segment_virtual = core::ptr::addr_of!(__data_start) as usize;
    segment_physical = kernel_address.physical_base as usize
        + (segment_virtual - kernel_address.virtual_base as usize);

    let kernel_data_segment = KernelSegment {
        physical_base: PhysicalAddr::new(segment_physical),
        virtual_base: VirtualAddr::new(segment_virtual),
        length: (core::ptr::addr_of!(__data_end) as usize)
            - (core::ptr::addr_of!(__data_start) as usize),
        permissions: PagePermissions::new(true, false, false),
    };

    segment_length += kernel_data_segment.length;

    #[cfg(debug_assertions)]
    {
        let mut kernel_length = None;
        for &entry in memmap.iter() {
            if entry.type_ != limine_api::memmap::MEMMAP_EXECUTABLE_AND_MODULES {
                continue;
            }

            if entry.base != kernel_address.physical_base {
                continue;
            }

            kernel_length = Some(entry.length as usize);
            break;
        }

        if kernel_length.is_none() {
            return Err(BootError::FailedToLocateKernel);
        }

        debug_assert_eq!(segment_length, kernel_length.unwrap());
    }

    Ok(BootInfo {
        kernel_layout: KernelMemoryLayout {
            segments: [
                kernel_text_segment,
                kernel_rodata_segment,
                kernel_data_segment,
            ],
        },
        command_line,
        initramfs,
        hhdm_offset,
        memory_map,
        rsdp: VirtualAddr::new(rsdp.address as usize),
    })
}
