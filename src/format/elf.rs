#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfIsa {
    None,
    Sparc,
    X86,
    Mips,
    Ppc,
    Arm,
    SuperH,
    Ia64,
    Amd64,
    AArch64,
    Riscv,
}

impl ElfIsa {
    fn from_u16(value: u16) -> Result<Self, ElfError> {
        match value {
            0x00 => Ok(Self::None),
            0x02 => Ok(Self::Sparc),
            0x03 => Ok(Self::X86),
            0x08 => Ok(Self::Mips),
            0x14 => Ok(Self::Ppc),
            0x28 => Ok(Self::Arm),
            0x2A => Ok(Self::SuperH),
            0x32 => Ok(Self::Ia64),
            0x3E => Ok(Self::Amd64),
            0xB7 => Ok(Self::AArch64),
            0xF3 => Ok(Self::Riscv),
            _ => Err(ElfError::InvalidElf),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfClass {
    Elf32,
    Elf64,
}

impl ElfClass {
    fn from_u8(value: u8) -> Result<Self, ElfError> {
        match value {
            1 => Ok(Self::Elf32),
            2 => Ok(Self::Elf64),
            _ => Err(ElfError::InvalidElf),
        }
    }

    const fn header_size(self) -> u16 {
        match self {
            Self::Elf32 => 52,
            Self::Elf64 => 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endianness {
    Little,
    Big,
}

impl Endianness {
    fn from_u8(value: u8) -> Result<Self, ElfError> {
        match value {
            1 => Ok(Self::Little),
            2 => Ok(Self::Big),
            _ => Err(ElfError::InvalidElf),
        }
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfType {
    Relocatable = 1,
    Executable = 2,
    SharedObject = 3,
    Core = 4,
}

impl ElfType {
    fn from_u16(value: u16) -> Result<Self, ElfError> {
        match value {
            1 => Ok(Self::Relocatable),
            2 => Ok(Self::Executable),
            3 => Ok(Self::SharedObject),
            4 => Ok(Self::Core),
            _ => Err(ElfError::InvalidElf),
        }
    }
}

#[derive(Debug)]
#[allow(unused)]
pub struct ElfHeader {
    magic: [u8; 4],
    pub class: ElfClass,
    endianness: Endianness,
    version: u8,
    os_abi: u8,
    _reserved: [u8; 8],
    pub object_type: ElfType,
    pub machine: ElfIsa,
    version_1: u32,
    entry: u64,                     // 2115136
    program_header_offset: u64,     // 64
    section_header_offset: u64,     // 2759752
    flags: u32,                     // 0
    header_size: u16,               // 64
    program_header_entry_size: u16, // 56
    program_header_count: u16,      // 6
    section_header_entry_size: u16, // 64
    section_header_count: u16,      // 17
    section_name_index: u16,        // 15
}

#[derive(Debug)]
pub enum ElfError {
    InvalidElf,
}

impl ElfHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self, ElfError> {
        let mut reader = Reader::new(bytes);

        let magic = reader.read_array()?;
        if magic != *b"\x7fELF" {
            return Err(ElfError::InvalidElf);
        }

        let class = ElfClass::from_u8(reader.read_u8()?)?;
        let endianness = Endianness::from_u8(reader.read_u8()?)?;
        reader.set_endianness(endianness);

        let version = reader.read_u8()?;
        let os_abi = reader.read_u8()?;
        let reserved = reader.read_array()?;
        let object_type = ElfType::from_u16(reader.read_u16()?)?;
        let machine = ElfIsa::from_u16(reader.read_u16()?)?;
        let version_1 = reader.read_u32()?;
        let entry = reader.read_word(class)?;
        let program_header_offset = reader.read_word(class)?;
        let section_header_offset = reader.read_word(class)?;
        let flags = reader.read_u32()?;
        let header_size = reader.read_u16()?;
        let program_header_entry_size = reader.read_u16()?;
        let program_header_count = reader.read_u16()?;
        let section_header_entry_size = reader.read_u16()?;
        let section_header_count = reader.read_u16()?;
        let section_name_index = reader.read_u16()?;

        if header_size != class.header_size() {
            return Err(ElfError::InvalidElf);
        }

        Ok(Self {
            magic,
            class,
            endianness,
            version,
            os_abi,
            _reserved: reserved,
            object_type,
            machine,
            version_1,
            entry,
            program_header_offset,
            section_header_offset,
            flags,
            header_size,
            program_header_entry_size,
            program_header_count,
            section_header_entry_size,
            section_header_count,
            section_name_index,
        })
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramHeaderType {
    Null = 0,
    Load = 1,
    Dynamic = 2,
    Interpreter = 3,
    Note = 4,
    Shlib = 5,
    Phdr = 6,
    GnuStack = 0x6474e551,
    Relro = 0x6474e552,
    Other(u32),
}

impl ProgramHeaderType {
    fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Null,
            1 => Self::Load,
            2 => Self::Dynamic,
            3 => Self::Interpreter,
            4 => Self::Note,
            5 => Self::Shlib,
            6 => Self::Phdr,
            0x6474e551 => Self::GnuStack,
            0x6474e552 => Self::Relro,
            _ => Self::Other(value),
        }
    }
}

#[derive(Debug)]
pub struct ProgramHeader {
    pub segment_type: ProgramHeaderType,
    pub flags: u32,
    pub file_offset: u64,
    pub virtual_address: u64,
    _physical_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub alignment: u64,
}

impl ProgramHeader {
    pub fn parse(bytes: &[u8], class: ElfClass, endianness: Endianness) -> Result<Self, ElfError> {
        let mut reader = Reader::new(bytes);
        reader.set_endianness(endianness);

        let segment_type = ProgramHeaderType::from_u32(reader.read_u32()?);

        let flags = if class == ElfClass::Elf64 {
            reader.read_u32()?
        } else {
            0
        };

        let file_offset = reader.read_word(class)?;
        let virtual_address = reader.read_word(class)?;
        let physical_address = reader.read_word(class)?;
        let file_size = reader.read_word(class)?;
        let memory_size = reader.read_word(class)?;

        let flags = if class == ElfClass::Elf32 {
            reader.read_u32()?
        } else {
            flags
        };

        let alignment = reader.read_word(class)?;

        Ok(Self {
            segment_type,
            flags,
            file_offset,
            virtual_address,
            _physical_address: physical_address,
            file_size,
            memory_size,
            alignment,
        })
    }
}

pub struct Elf<'a> {
    bytes: &'a [u8],
    header: ElfHeader,
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        let header = ElfHeader::parse(bytes)?;

        Ok(Self { bytes, header })
    }

    pub fn program_headers(&self) -> Result<ProgramHeaders<'_>, ElfError> {
        let offset =
            usize::try_from(self.header.program_header_offset).map_err(|_| ElfError::InvalidElf)?;
        let entry_size = usize::from(self.header.program_header_entry_size);
        let count = usize::from(self.header.program_header_count);

        let expected_entry_size = match self.header.class {
            ElfClass::Elf32 => 32,
            ElfClass::Elf64 => 56,
        };

        if entry_size != expected_entry_size {
            return Err(ElfError::InvalidElf);
        }

        let table_size = entry_size.checked_mul(count).ok_or(ElfError::InvalidElf)?;
        let table_end = offset.checked_add(table_size).ok_or(ElfError::InvalidElf)?;
        let bytes = self
            .bytes
            .get(offset..table_end)
            .ok_or(ElfError::InvalidElf)?;

        Ok(ProgramHeaders {
            bytes,
            class: self.header.class,
            endianness: self.header.endianness,
            entry_size,
            remaining: count,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    pub fn machine(&self) -> ElfIsa {
        self.header.machine
    }

    pub fn entry(&self) -> usize {
        self.header.entry as usize
    }
}

pub struct ProgramHeaders<'a> {
    bytes: &'a [u8],
    class: ElfClass,
    endianness: Endianness,
    entry_size: usize,
    remaining: usize,
}

impl<'a> Iterator for ProgramHeaders<'a> {
    type Item = Result<ProgramHeader, ElfError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let entry = match self.bytes.get(..self.entry_size) {
            Some(entry) => entry,
            None => {
                self.remaining = 0;
                return None;
            }
        };

        self.bytes = &self.bytes[self.entry_size..];
        self.remaining -= 1;

        Some(ProgramHeader::parse(entry, self.class, self.endianness))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ProgramHeaders<'_> {}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    endianness: Endianness,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            endianness: Endianness::Little,
        }
    }

    fn set_endianness(&mut self, endianness: Endianness) {
        self.endianness = endianness;
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ElfError> {
        let end = self.offset.checked_add(N).ok_or(ElfError::InvalidElf)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ElfError::InvalidElf)?;
        self.offset = end;

        bytes.try_into().map_err(|_| ElfError::InvalidElf)
    }

    fn read_u8(&mut self) -> Result<u8, ElfError> {
        Ok(self.read_array::<1>()?[0])
    }

    fn read_u16(&mut self) -> Result<u16, ElfError> {
        let bytes = self.read_array()?;
        Ok(match self.endianness {
            Endianness::Little => u16::from_le_bytes(bytes),
            Endianness::Big => u16::from_be_bytes(bytes),
        })
    }

    fn read_u32(&mut self) -> Result<u32, ElfError> {
        let bytes = self.read_array()?;
        Ok(match self.endianness {
            Endianness::Little => u32::from_le_bytes(bytes),
            Endianness::Big => u32::from_be_bytes(bytes),
        })
    }

    fn read_u64(&mut self) -> Result<u64, ElfError> {
        let bytes = self.read_array()?;
        Ok(match self.endianness {
            Endianness::Little => u64::from_le_bytes(bytes),
            Endianness::Big => u64::from_be_bytes(bytes),
        })
    }

    fn read_word(&mut self, class: ElfClass) -> Result<u64, ElfError> {
        match class {
            ElfClass::Elf32 => Ok(u64::from(self.read_u32()?)),
            ElfClass::Elf64 => self.read_u64(),
        }
    }
}
