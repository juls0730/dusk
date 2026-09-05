// CPIO newc

#[repr(C)]
struct Header {
    pub c_magic: [u8; 6],
    pub c_ino: [u8; 8],
    pub c_mode: [u8; 8],
    pub c_uid: [u8; 8],
    pub c_gid: [u8; 8],
    pub c_nlink: [u8; 8],
    pub c_mtime: [u8; 8],
    pub c_filesize: [u8; 8],
    pub c_devmajor: [u8; 8],
    pub c_devminor: [u8; 8],
    pub c_rdevmajor: [u8; 8],
    pub c_rdevminor: [u8; 8],
    pub c_namesize: [u8; 8],
    pub c_check: [u8; 8],
}

impl Header {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < core::mem::size_of::<Header>() {
            return None;
        }

        let header: Header = unsafe { core::ptr::read(bytes.as_ptr() as *const Header) };

        if header.c_magic != *b"070701" {
            return None;
        }

        Some(header)
    }
}

pub fn find_file<'a>(archive: &'a [u8], target: &str) -> Option<&'a [u8]> {
    let mut offset = 0;

    while offset + core::mem::size_of::<Header>() <= archive.len() {
        let header = Header::from_bytes(&archive[offset..])?;
        let header_start = offset;
        offset += core::mem::size_of::<Header>();

        let file_len =
            usize::from_str_radix(core::str::from_utf8(&header.c_filesize).ok()?, 16).ok()?;
        let name_len =
            usize::from_str_radix(core::str::from_utf8(&header.c_namesize).ok()?, 16).ok()?;

        if offset + name_len > archive.len() {
            return None;
        }

        let name_bytes = &archive[offset..offset + name_len];
        let name = core::str::from_utf8(name_bytes)
            .ok()?
            .trim_end_matches('\0');

        if name == "TRAILER!!!" {
            return None;
        }

        let data_start = header_start + ((core::mem::size_of::<Header>() + name_len + 3) & !3);
        if data_start + file_len > archive.len() {
            return None;
        }

        if name == target {
            return Some(&archive[data_start..data_start + file_len]);
        }

        offset = data_start + ((file_len + 3) & !3);
    }

    None
}
