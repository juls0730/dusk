pub struct ElfError;

#[cfg(target_arch = "x86_64")]
const MACHINE: u16 = 62;
#[cfg(target_arch = "aarch64")]
const MACHINE: u16 = 183;
#[cfg(target_arch = "riscv64")]
const MACHINE: u16 = 243;

pub struct Elf<'a> {
    bytes: &'a [u8],
    headers: &'a [u8],
    pub entry: usize,
}

pub struct Segment<'a> {
    pub data: &'a [u8],
    pub address: usize,
    pub memory_size: usize,
    pub writable: bool,
    pub executable: bool,
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        let header = bytes.get(..64).ok_or(ElfError)?;
        // Bootstrap images are static ELF64 executables in the native ISA, always LE.
        if &header[..7] != b"\x7fELF\x02\x01\x01"
            || u16_at(header, 16) != 2
            || u16_at(header, 18) != MACHINE
            || u32_at(header, 20) != 1
            || u16_at(header, 52) != 64
            || u16_at(header, 54) != 56
        {
            return Err(ElfError);
        }

        let offset = usize_at(header, 32);
        let count = usize::from(u16_at(header, 56));
        let end = offset.checked_add(count * 56).ok_or(ElfError)?;
        let headers = bytes.get(offset..end).ok_or(ElfError)?;
        if headers
            .chunks_exact(56)
            .any(|h| matches!(u32_at(h, 0), 2 | 3))
        {
            // There is no dynamic linker or relocation processing during bootstrap.
            return Err(ElfError);
        }

        Ok(Self {
            bytes,
            headers,
            entry: usize_at(header, 24),
        })
    }

    pub fn segments(&self) -> impl Iterator<Item = Result<Segment<'a>, ElfError>> + '_ {
        self.headers
            .chunks_exact(56)
            .filter(|h| u32_at(h, 0) == 1)
            .map(|h| {
                let offset = usize_at(h, 8);
                let address = usize_at(h, 16);
                let file_size = usize_at(h, 32);
                let memory_size = usize_at(h, 40);
                let alignment = usize_at(h, 48);
                if file_size > memory_size
                    || (alignment > 1
                        && (!alignment.is_power_of_two()
                            || address % alignment != offset % alignment))
                {
                    return Err(ElfError);
                }
                let end = offset.checked_add(file_size).ok_or(ElfError)?;
                let data = self.bytes.get(offset..end).ok_or(ElfError)?;
                let flags = u32_at(h, 4);
                Ok(Segment {
                    data,
                    address,
                    memory_size,
                    writable: flags & 2 != 0,
                    executable: flags & 1 != 0,
                })
            })
    }
}

// Callers only read fixed offsets within already bounds-checked headers.
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    let mut value = [0; 2];
    value.copy_from_slice(&bytes[offset..offset + 2]);
    u16::from_le_bytes(value)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(value)
}

fn usize_at(bytes: &[u8], offset: usize) -> usize {
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(value) as usize
}
