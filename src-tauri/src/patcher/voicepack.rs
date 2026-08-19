use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"NVPACK01";
const VERSION_V1: u32 = 1;
const VERSION_V2: u32 = 2;
const HEADER_SIZE_V1: usize = 0x70;
const HEADER_SIZE_V2: usize = 0x90;
const CATALOG_INDEX_DOMAIN: &[u8] = b"NVPACK02-INDEX\0";
const ENTRY_SIZE: usize = 40;
const MAX_VOICES: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Japanese,
    English,
    French,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::Japanese => "jp",
            Self::English => "en",
            Self::French => "fr",
        }
    }

    fn parse(raw: &[u8]) -> Result<Self, VoicePackError> {
        match raw {
            [b'j', b'p', 0, 0] => Ok(Self::Japanese),
            [b'e', b'n', 0, 0] => Ok(Self::English),
            [b'f', b'r', 0, 0] => Ok(Self::French),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrittenVoicePack {
    pub entries: usize,
    pub payload_size: u64,
    pub sha256: [u8; 32],
    pub catalog_sha256: Option<[u8; 32]>,
}

#[derive(Debug)]
pub struct VoicePack {
    path: PathBuf,
    language: Language,
    entries: Vec<VoiceEntry>,
    payload_offset: u64,
    payload_size: u64,
    payload_sha256: [u8; 32],
    catalog_sha256: Option<[u8; 32]>,
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
    /// Build a pack from an already ordered sequence of voice resources.
    ///
    /// The pack index is deliberately derived from source files before any
    /// destination is replaced. Reading every source again while writing also
    /// detects concurrent changes instead of authenticating stale metadata.
    pub fn write_atomic_from_files(
        output: impl AsRef<Path>,
        language: Language,
        sources: &[PathBuf],
    ) -> Result<WrittenVoicePack, VoicePackError> {
        Self::write_atomic_from_files_impl(output.as_ref(), language, sources, None)
    }

    /// Write a v2 pack whose ordered entries are bound to a reviewed target
    /// catalogue. French builds require this identity at patch time so a pack
    /// built for a reordered profile cannot silently voice the wrong lines.
    pub fn write_atomic_catalog_from_files(
        output: impl AsRef<Path>,
        language: Language,
        sources: &[PathBuf],
        catalog_sha256: [u8; 32],
    ) -> Result<WrittenVoicePack, VoicePackError> {
        Self::write_atomic_from_files_impl(output.as_ref(), language, sources, Some(catalog_sha256))
    }

    fn write_atomic_from_files_impl(
        output: &Path,
        language: Language,
        sources: &[PathBuf],
        catalog_sha256: Option<[u8; 32]>,
    ) -> Result<WrittenVoicePack, VoicePackError> {
        if sources.is_empty() || sources.len() > MAX_VOICES {
            return Err(VoicePackError::InvalidLayout(format!(
                "invalid voice count: {}",
                sources.len()
            )));
        }
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        let output_absolute = if output.exists() {
            fs::canonicalize(output).map_err(|error| io_error(output, error))?
        } else {
            fs::canonicalize(parent)
                .map_err(|error| io_error(parent, error))?
                .join(output.file_name().ok_or_else(|| {
                    VoicePackError::InvalidLayout("output path has no filename".to_owned())
                })?)
        };
        for source in sources {
            let source_absolute =
                fs::canonicalize(source).map_err(|error| io_error(source, error))?;
            if source_absolute == output_absolute {
                return Err(VoicePackError::InvalidLayout(format!(
                    "voice pack output collides with source {}",
                    source.display()
                )));
            }
        }

        let mut index = Vec::with_capacity(sources.len() * ENTRY_SIZE);
        let mut payload_hasher = Sha256::new();
        let mut payload_size = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        for source in sources {
            let mut file = File::open(source).map_err(|error| io_error(source, error))?;
            let size = file
                .metadata()
                .map_err(|error| io_error(source, error))?
                .len();
            if size == 0 || size > u64::from(u32::MAX) {
                return Err(VoicePackError::InvalidLayout(format!(
                    "voice source {} has invalid size {size}",
                    source.display()
                )));
            }
            let mut entry_hasher = Sha256::new();
            let mut read_total = 0_u64;
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|error| io_error(source, error))?;
                if read == 0 {
                    break;
                }
                entry_hasher.update(&buffer[..read]);
                payload_hasher.update(&buffer[..read]);
                read_total += read as u64;
            }
            if read_total != size {
                return Err(VoicePackError::InvalidLayout(format!(
                    "voice source {} changed while hashing",
                    source.display()
                )));
            }
            payload_size = payload_size
                .checked_add(size)
                .ok_or_else(|| VoicePackError::InvalidLayout("payload size overflow".to_owned()))?;
            index.extend_from_slice(&(size as u32).to_le_bytes());
            index.extend_from_slice(&0_u32.to_le_bytes());
            index.extend_from_slice(&entry_hasher.finalize());
        }

        let (version, header_size) = if catalog_sha256.is_some() {
            (VERSION_V2, HEADER_SIZE_V2)
        } else {
            (VERSION_V1, HEADER_SIZE_V1)
        };
        let payload_offset = (header_size + index.len() + 0x0f) & !0x0f;
        let mut header = vec![0_u8; header_size];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&version.to_le_bytes());
        header[12..16].copy_from_slice(&(sources.len() as u32).to_le_bytes());
        header[16..20].copy_from_slice(match language {
            Language::Japanese => b"jp\0\0",
            Language::English => b"en\0\0",
            Language::French => b"fr\0\0",
        });
        header[24..32].copy_from_slice(&(header_size as u64).to_le_bytes());
        header[32..40].copy_from_slice(&(payload_offset as u64).to_le_bytes());
        header[40..48].copy_from_slice(&payload_size.to_le_bytes());
        let index_sha256 = if let Some(catalog) = catalog_sha256 {
            let mut hasher = Sha256::new();
            hasher.update(CATALOG_INDEX_DOMAIN);
            hasher.update(catalog);
            hasher.update(&index);
            header[112..144].copy_from_slice(&catalog);
            hasher.finalize()
        } else {
            Sha256::digest(&index)
        };
        header[48..80].copy_from_slice(&index_sha256);
        header[80..112].copy_from_slice(&payload_hasher.finalize());

        let mut temporary =
            NamedTempFile::new_in(parent).map_err(|error| io_error(parent, error))?;
        temporary
            .write_all(&header)
            .and_then(|_| temporary.write_all(&index))
            .and_then(|_| {
                temporary.write_all(&vec![0_u8; payload_offset - header_size - index.len()])
            })
            .map_err(|error| io_error(temporary.path(), error))?;

        for (entry_index, source) in sources.iter().enumerate() {
            let expected_size = u32_at(&index, entry_index * ENTRY_SIZE)? as u64;
            let expected_hash = digest_at(&index, entry_index * ENTRY_SIZE + 8)?;
            let mut file = File::open(source).map_err(|error| io_error(source, error))?;
            let mut hasher = Sha256::new();
            let mut written = 0_u64;
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|error| io_error(source, error))?;
                if read == 0 {
                    break;
                }
                temporary
                    .write_all(&buffer[..read])
                    .map_err(|error| io_error(temporary.path(), error))?;
                hasher.update(&buffer[..read]);
                written += read as u64;
            }
            if written != expected_size || hasher.finalize().as_slice() != expected_hash {
                return Err(VoicePackError::InvalidLayout(format!(
                    "voice source {} changed while writing",
                    source.display()
                )));
            }
        }
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|error| io_error(temporary.path(), error))?;
        let verified = VoicePack::open(temporary.path())?;
        verified.verify_payload()?;

        let mut complete_hasher = Sha256::new();
        let mut complete =
            File::open(temporary.path()).map_err(|error| io_error(temporary.path(), error))?;
        loop {
            let read = complete
                .read(&mut buffer)
                .map_err(|error| io_error(temporary.path(), error))?;
            if read == 0 {
                break;
            }
            complete_hasher.update(&buffer[..read]);
        }
        let sha256 = complete_hasher.finalize().into();
        temporary
            .persist(output)
            .map_err(|error| io_error(output, error.error))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io_error(parent, error))?;
        Ok(WrittenVoicePack {
            entries: sources.len(),
            payload_size,
            sha256,
            catalog_sha256,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, VoicePackError> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|error| io_error(path, error))?;
        let file_len = file
            .metadata()
            .map_err(|error| io_error(path, error))?
            .len();
        let mut header = vec![0_u8; HEADER_SIZE_V1];
        file.read_exact(&mut header)
            .map_err(|error| io_error(path, error))?;
        if &header[..8] != MAGIC {
            return Err(VoicePackError::InvalidMagic);
        }
        let version = u32_at(&header, 8)?;
        let (header_size, catalog_sha256) = match version {
            VERSION_V1 => (HEADER_SIZE_V1, None),
            VERSION_V2 => {
                header.resize(HEADER_SIZE_V2, 0);
                file.read_exact(&mut header[HEADER_SIZE_V1..])
                    .map_err(|error| io_error(path, error))?;
                (HEADER_SIZE_V2, Some(digest_at(&header, 112)?))
            }
            _ => return Err(VoicePackError::InvalidVersion(version)),
        };
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
        let expected_payload_min = header_size as u64 + index_len as u64;
        if index_offset != header_size as u64
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
        let actual_index_sha256 = if let Some(catalog) = catalog_sha256 {
            let mut hasher = Sha256::new();
            hasher.update(CATALOG_INDEX_DOMAIN);
            hasher.update(catalog);
            hasher.update(&index);
            hasher.finalize()
        } else {
            Sha256::digest(&index)
        };
        if actual_index_sha256.as_slice() != index_sha256 {
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
            catalog_sha256,
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

    pub fn catalog_sha256(&self) -> Option<[u8; 32]> {
        self.catalog_sha256
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
        // A heap buffer preserves throughput without consuming the 1 MiB stack
        // reserved for a stock Windows MSVC console entry point.
        let mut buffer = vec![0_u8; 1024 * 1024];
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
        // Keep the verification path under the same Windows stack limit as the
        // per-entry copy path instead of relying on a larger linker reserve.
        let mut buffer = vec![0_u8; 1024 * 1024];
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

    fn synthetic_pack(language: [u8; 4]) -> tempfile::NamedTempFile {
        let payload = b"SIR0synthetic-test-payload";
        let mut index = Vec::with_capacity(ENTRY_SIZE);
        index.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        index.extend_from_slice(&0_u32.to_le_bytes());
        index.extend_from_slice(&Sha256::digest(payload));
        let payload_offset = (HEADER_SIZE_V1 + index.len() + 0x0f) & !0x0f;
        let mut header = [0_u8; HEADER_SIZE_V1];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION_V1.to_le_bytes());
        header[12..16].copy_from_slice(&1_u32.to_le_bytes());
        header[16..20].copy_from_slice(&language);
        header[24..32].copy_from_slice(&(HEADER_SIZE_V1 as u64).to_le_bytes());
        header[32..40].copy_from_slice(&(payload_offset as u64).to_le_bytes());
        header[40..48].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        header[48..80].copy_from_slice(&Sha256::digest(&index));
        header[80..112].copy_from_slice(&Sha256::digest(payload));

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&index).unwrap();
        file.write_all(&vec![0_u8; payload_offset - HEADER_SIZE_V1 - index.len()])
            .unwrap();
        file.write_all(payload).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn parses_all_supported_language_codes() {
        assert_eq!(Language::parse(b"jp\0\0").unwrap(), Language::Japanese);
        assert_eq!(Language::parse(b"en\0\0").unwrap(), Language::English);
        assert_eq!(Language::parse(b"fr\0\0").unwrap(), Language::French);
        assert_eq!(Language::French.code(), "fr");
        assert!(matches!(
            Language::parse(b"de\0\0"),
            Err(VoicePackError::InvalidLanguage(_))
        ));
    }

    #[test]
    fn opens_and_verifies_a_french_pack() {
        let file = synthetic_pack(*b"fr\0\0");
        let pack = VoicePack::open(file.path()).unwrap();
        assert_eq!(pack.language(), Language::French);
        assert_eq!(pack.catalog_sha256(), None);
        assert_eq!(pack.entries().len(), 1);
        pack.verify_payload().unwrap();
        let mut copied = Vec::new();
        pack.copy_entry_to(0, &mut copied).unwrap();
        assert_eq!(copied, b"SIR0synthetic-test-payload");
    }

    #[test]
    fn atomically_writes_an_authenticated_french_pack() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("se_v0000.se");
        let second = directory.path().join("se_v0001.se");
        fs::write(&first, b"SIR0first").unwrap();
        fs::write(&second, b"SIR0second-resource").unwrap();
        let output = directory.path().join("voices-fr.nvpack");

        let catalog = [0x5a; 32];
        let written = VoicePack::write_atomic_catalog_from_files(
            &output,
            Language::French,
            &[first, second],
            catalog,
        )
        .unwrap();
        assert_eq!(written.entries, 2);
        assert_eq!(written.payload_size, 28);
        assert_eq!(written.catalog_sha256, Some(catalog));

        let pack = VoicePack::open(&output).unwrap();
        assert_eq!(pack.language(), Language::French);
        assert_eq!(pack.catalog_sha256(), Some(catalog));
        pack.verify_payload().unwrap();
        let mut first_payload = Vec::new();
        let mut second_payload = Vec::new();
        pack.copy_entry_to(0, &mut first_payload).unwrap();
        pack.copy_entry_to(1, &mut second_payload).unwrap();
        assert_eq!(first_payload, b"SIR0first");
        assert_eq!(second_payload, b"SIR0second-resource");

        let mut tampered = fs::read(&output).unwrap();
        tampered[112] ^= 1;
        fs::write(&output, tampered).unwrap();
        assert!(matches!(
            VoicePack::open(&output),
            Err(VoicePackError::IndexHashMismatch)
        ));
    }

    #[test]
    fn writer_refuses_to_replace_a_voice_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("se_v0000.se");
        fs::write(&source, b"SIR0source").unwrap();
        assert!(matches!(
            VoicePack::write_atomic_catalog_from_files(
                &source,
                Language::French,
                std::slice::from_ref(&source),
                [7; 32],
            ),
            Err(VoicePackError::InvalidLayout(_))
        ));
        assert_eq!(fs::read(source).unwrap(), b"SIR0source");
    }

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
        assert_eq!(jp.entries().len(), 6_475);
        assert_eq!(en.entries().len(), 6_475);
        assert_eq!(jp.payload_size(), 185_035_296);
        assert_eq!(en.payload_size(), 149_225_824);
    }

    #[test]
    #[ignore = "requires production voice packs not present in the repository"]
    fn copies_and_verifies_real_entries() {
        for name in ["voices-jp.nvpack", "voices-en.nvpack"] {
            let pack = VoicePack::open(resource(name)).unwrap();
            for index in [0, 3_237, 6_474] {
                let mut output = Vec::new();
                pack.copy_entry_to(index, &mut output).unwrap();
                assert_eq!(output.len(), pack.entries()[index].size as usize);
                assert_eq!(&output[..4], b"SIR0");
            }
        }
    }
}
