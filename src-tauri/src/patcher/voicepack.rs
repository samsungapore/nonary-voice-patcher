use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"NVPACK01";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 0x70;
const ENTRY_SIZE: usize = 40;
const MAX_VOICES: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Japanese,
    English,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::Japanese => "jp",
            Self::English => "en",
        }
    }

    fn parse(raw: &[u8]) -> Result<Self, VoicePackError> {
        match raw {
            [b'j', b'p', 0, 0] => Ok(Self::Japanese),
            [b'e', b'n', 0, 0] => Ok(Self::English),
            _ => Err(VoicePackError::InvalidLanguage(
                String::from_utf8_lossy(raw).into_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoiceEntry {
    pub size: u32,
    pub sha256: [u8; 32],
    offset: u64,
}

#[derive(Debug)]
pub struct VoicePack {
    path: PathBuf,
    language: Language,
    entries: Vec<VoiceEntry>,
    payload_offset: u64,
    payload_size: u64,
    payload_sha256: [u8; 32],
}

#[derive(Debug, Error)]
pub enum VoicePackError {
    #[error("cannot read voice pack {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("truncated voice pack")]
    Truncated,
    #[error("this file is not a Nonary voice pack")]
    InvalidMagic,
    #[error("unsupported voice pack version: {0}")]
    InvalidVersion(u32),
    #[error("invalid voice pack language: {0:?}")]
    InvalidLanguage(String),
    #[error("inconsistent voice pack: {0}")]
    InvalidLayout(String),
    #[error("voice pack index is corrupt")]
    IndexHashMismatch,
    #[error("voice entry {index} is corrupt")]
    EntryHashMismatch { index: usize },
    #[error("voice pack payload is corrupt")]
    PayloadHashMismatch,
    #[error("voice index out of bounds: {0}")]
    InvalidEntry(usize),
}

fn io_error(path: &Path, source: io::Error) -> VoicePackError {
    VoicePackError::Io {
        path: path.to_owned(),
        source,
    }
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, VoicePackError> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or(VoicePackError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64, VoicePackError> {
    Ok(u64::from_le_bytes(
        data.get(offset..offset + 8)
            .ok_or(VoicePackError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn digest_at(data: &[u8], offset: usize) -> Result<[u8; 32], VoicePackError> {
    Ok(data
        .get(offset..offset + 32)
        .ok_or(VoicePackError::Truncated)?
        .try_into()
        .unwrap())
}

impl VoicePack {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VoicePackError> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|error| io_error(path, error))?;
        let file_len = file
            .metadata()
            .map_err(|error| io_error(path, error))?
            .len();
        let mut header = [0_u8; HEADER_SIZE];
        file.read_exact(&mut header)
            .map_err(|error| io_error(path, error))?;
        if &header[..8] != MAGIC {
            return Err(VoicePackError::InvalidMagic);
        }
        let version = u32_at(&header, 8)?;
        if version != VERSION {
            return Err(VoicePackError::InvalidVersion(version));
        }
        let count = u32_at(&header, 12)? as usize;
        if count == 0 || count > MAX_VOICES {
            return Err(VoicePackError::InvalidLayout(format!(
                "invalid voice count: {count}"
            )));
        }
        let language = Language::parse(&header[16..20])?;
        if u32_at(&header, 20)? != 0 {
            return Err(VoicePackError::InvalidLayout(
                "reserved flags are nonzero".to_owned(),
            ));
        }
        let index_offset = u64_at(&header, 24)?;
        let payload_offset = u64_at(&header, 32)?;
        let payload_size = u64_at(&header, 40)?;
        let index_sha256 = digest_at(&header, 48)?;
        let payload_sha256 = digest_at(&header, 80)?;
        let index_len = count
            .checked_mul(ENTRY_SIZE)
            .ok_or_else(|| VoicePackError::InvalidLayout("index is too large".to_owned()))?;
        let expected_payload_min = HEADER_SIZE as u64 + index_len as u64;
        if index_offset != HEADER_SIZE as u64
            || payload_offset < expected_payload_min
            || payload_offset % 16 != 0
            || payload_offset.checked_add(payload_size) != Some(file_len)
        {
            return Err(VoicePackError::InvalidLayout(format!(
                "invalid index/payload offsets ({index_offset:#x}, {payload_offset:#x}, {payload_size:#x}, file {file_len:#x})"
            )));
        }

        let mut index = vec![0_u8; index_len];
        file.seek(SeekFrom::Start(index_offset))
            .and_then(|_| file.read_exact(&mut index))
            .map_err(|error| io_error(path, error))?;
        if Sha256::digest(&index).as_slice() != index_sha256 {
            return Err(VoicePackError::IndexHashMismatch);
        }
        let mut entries = Vec::with_capacity(count);
        let mut offset = payload_offset;
        for raw in index.chunks_exact(ENTRY_SIZE) {
            let size = u32_at(raw, 0)?;
            if size == 0 || u32_at(raw, 4)? != 0 {
                return Err(VoicePackError::InvalidLayout(
                    "empty entry or nonzero reserved fields".to_owned(),
                ));
            }
            let sha256 = digest_at(raw, 8)?;
            entries.push(VoiceEntry {
                size,
                sha256,
                offset,
            });
            offset = offset
                .checked_add(u64::from(size))
                .ok_or_else(|| VoicePackError::InvalidLayout("payload size overflow".to_owned()))?;
        }
        if offset != payload_offset + payload_size {
            return Err(VoicePackError::InvalidLayout(format!(
                "voice entries total {} bytes, expected {payload_size}",
                offset - payload_offset
            )));
        }

        Ok(Self {
            path: path.to_owned(),
            language,
            entries,
            payload_offset,
            payload_size,
            payload_sha256,
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn entries(&self) -> &[VoiceEntry] {
        &self.entries
    }

    pub fn payload_size(&self) -> u64 {
        self.payload_size
    }

    pub fn copy_entry_to(
        &self,
        index: usize,
        writer: &mut impl Write,
    ) -> Result<u64, VoicePackError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(VoicePackError::InvalidEntry(index))?;
        let mut file = File::open(&self.path).map_err(|error| io_error(&self.path, error))?;
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(|error| io_error(&self.path, error))?;
        // Entries are streamed and authenticated individually so large packs
        // do not need to reside in memory and corruption is caught before use.
        let mut limited = file.take(u64::from(entry.size));
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 1024 * 1024];
        let mut written = 0_u64;
        loop {
            let read = limited
                .read(&mut buffer)
                .map_err(|error| io_error(&self.path, error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            writer
                .write_all(&buffer[..read])
                .map_err(|error| io_error(&self.path, error))?;
            written += read as u64;
        }
        if written != u64::from(entry.size) || hasher.finalize().as_slice() != entry.sha256 {
            return Err(VoicePackError::EntryHashMismatch { index });
        }
        Ok(written)
    }

    pub fn verify_payload(&self) -> Result<(), VoicePackError> {
        let mut file = File::open(&self.path).map_err(|error| io_error(&self.path, error))?;
        file.seek(SeekFrom::Start(self.payload_offset))
            .map_err(|error| io_error(&self.path, error))?;
        // The aggregate digest catches reordered or omitted entries that could
        // still pass per-entry checks when accessed selectively.
        let mut limited = file.take(self.payload_size);
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 1024 * 1024];
        loop {
            let read = limited
                .read(&mut buffer)
                .map_err(|error| io_error(&self.path, error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if hasher.finalize().as_slice() != self.payload_sha256 {
            return Err(VoicePackError::PayloadHashMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(name)
    }

    #[test]
    #[ignore = "requires production voice packs not present in the repository"]
    fn opens_both_production_voice_packs() {
        let jp = VoicePack::open(resource("voices-jp.nvpack")).unwrap();
        let en = VoicePack::open(resource("voices-en.nvpack")).unwrap();
        assert_eq!(jp.language(), Language::Japanese);
        assert_eq!(en.language(), Language::English);
        assert_eq!(jp.entries().len(), 6_414);
        assert_eq!(en.entries().len(), 6_414);
        assert_eq!(jp.payload_size(), 182_806_656);
        assert_eq!(en.payload_size(), 147_356_608);
    }

    #[test]
    #[ignore = "requires production voice packs not present in the repository"]
    fn copies_and_verifies_real_entries() {
        for name in ["voices-jp.nvpack", "voices-en.nvpack"] {
            let pack = VoicePack::open(resource(name)).unwrap();
            for index in [0, 3_207, 6_413] {
                let mut output = Vec::new();
                pack.copy_entry_to(index, &mut output).unwrap();
                assert_eq!(output.len(), pack.entries()[index].size as usize);
                assert_eq!(&output[..4], b"SIR0");
            }
        }
    }
}
