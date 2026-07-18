use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

const DATA_MAGIC: &[u8; 8] = b"NVPBASE1";
const MARKER_MAGIC: &[u8; 8] = b"NVPRCP01";
const VERSION: u32 = 1;
const ORIGINAL_HEADER_SIZE: usize = 0x200;
const AUX_OFFSET: usize = 0x1000;
const RECEIPT_SIZE: usize = 580;
const MARKER_SIZE: u64 = 16;
pub const SIGNATURE_SIZE: u64 = 0x88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchedLanguage {
    Japanese,
    English,
    French,
}

impl PatchedLanguage {
    fn bytes(self) -> [u8; 4] {
        match self {
            Self::Japanese => *b"jp\0\0",
            Self::English => *b"en\0\0",
            Self::French => *b"fr\0\0",
        }
    }

    fn parse(data: &[u8]) -> Result<Self, ReceiptError> {
        match data {
            b"jp\0\0" => Ok(Self::Japanese),
            b"en\0\0" => Ok(Self::English),
            b"fr\0\0" => Ok(Self::French),
            _ => Err(ReceiptError::Invalid("unknown language".to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub original_len: u64,
    pub original_sha256: [u8; 32],
    pub original_header: [u8; ORIGINAL_HEADER_SIZE],
    pub original_aux_1000: [u8; 4],
    pub game_code: [u8; 4],
    pub language: PatchedLanguage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedReceipt {
    pub receipt: Receipt,
    pub receipt_offset: u64,
    pub signature_offset: u64,
    pub file_len: u64,
}

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("ROM is too short")]
    TooShort,
    #[error("no Nonary Voice Patcher patch detected")]
    NotPatched,
    #[error("invalid restoration receipt: {0}")]
    Invalid(String),
    #[error("the original ROM stored by the patch no longer matches its SHA-256")]
    OriginalHashMismatch,
}

fn io_error(path: &Path, source: io::Error) -> ReceiptError {
    ReceiptError::Io {
        path: path.to_owned(),
        source,
    }
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ReceiptError> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or_else(|| ReceiptError::Invalid("truncated u32 field".to_owned()))?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, ReceiptError> {
    Ok(u64::from_le_bytes(
        data.get(offset..offset + 8)
            .ok_or_else(|| ReceiptError::Invalid("truncated u64 field".to_owned()))?
            .try_into()
            .unwrap(),
    ))
}

impl Receipt {
    pub fn new(base: &[u8], language: PatchedLanguage) -> Result<Self, ReceiptError> {
        if base.len() < AUX_OFFSET + 4 {
            return Err(ReceiptError::TooShort);
        }
        let original_len = u64::try_from(base.len())
            .map_err(|_| ReceiptError::Invalid("ROM is too large".to_owned()))?;
        let original_sha256 = Sha256::digest(base).into();
        let original_header = base[..ORIGINAL_HEADER_SIZE].try_into().unwrap();
        let original_aux_1000 = base[AUX_OFFSET..AUX_OFFSET + 4].try_into().unwrap();
        let game_code = base[0x0c..0x10].try_into().unwrap();
        Ok(Self {
            original_len,
            original_sha256,
            original_header,
            original_aux_1000,
            game_code,
            language,
        })
    }

    pub fn to_bytes(&self) -> [u8; RECEIPT_SIZE] {
        let mut output = [0_u8; RECEIPT_SIZE];
        output[..8].copy_from_slice(DATA_MAGIC);
        output[8..12].copy_from_slice(&VERSION.to_le_bytes());
        // Reserved bytes stay zero so future format changes fail closed instead
        // of being misread as a valid version-one receipt.
        output[12..16].copy_from_slice(&0_u32.to_le_bytes());
        output[16..24].copy_from_slice(&self.original_len.to_le_bytes());
        output[24..56].copy_from_slice(&self.original_sha256);
        output[56..568].copy_from_slice(&self.original_header);
        output[568..572].copy_from_slice(&self.original_aux_1000);
        output[572..576].copy_from_slice(&self.language.bytes());
        output[576..580].copy_from_slice(&self.game_code);
        output
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, ReceiptError> {
        if data.len() != RECEIPT_SIZE || data.get(..8) != Some(DATA_MAGIC) {
            return Err(ReceiptError::Invalid("unknown receipt format".to_owned()));
        }
        let version = read_u32(data, 8)?;
        if version != VERSION {
            return Err(ReceiptError::Invalid(format!(
                "unsupported version {version}"
            )));
        }
        let reserved = read_u32(data, 12)?;
        if reserved != 0 {
            return Err(ReceiptError::Invalid(format!(
                "reserved field is nonzero: 0x{reserved:x}"
            )));
        }
        let original_len = read_u64(data, 16)?;
        if original_len < (AUX_OFFSET + 4) as u64 || original_len > 512 * 1024 * 1024 {
            return Err(ReceiptError::Invalid(format!(
                "invalid original size: {original_len}"
            )));
        }
        let original_sha256 = data[24..56].try_into().unwrap();
        let original_header: [u8; ORIGINAL_HEADER_SIZE] = data[56..568].try_into().unwrap();
        let original_aux_1000: [u8; 4] = data[568..572].try_into().unwrap();
        let language = PatchedLanguage::parse(&data[572..576])?;
        let game_code: [u8; 4] = data[576..580].try_into().unwrap();
        if original_header[0x0c..0x10] != game_code {
            return Err(ReceiptError::Invalid(
                "receipt game code is inconsistent".to_owned(),
            ));
        }
        Ok(Self {
            original_len,
            original_sha256,
            original_header,
            original_aux_1000,
            game_code,
            language,
        })
    }

    pub fn marker(&self) -> [u8; MARKER_SIZE as usize] {
        let mut marker = [0_u8; MARKER_SIZE as usize];
        marker[..8].copy_from_slice(MARKER_MAGIC);
        marker[8..].copy_from_slice(&(RECEIPT_SIZE as u64).to_le_bytes());
        marker
    }

    pub fn padded_receipt_offset(cursor: u64) -> u64 {
        let remainder = (cursor + RECEIPT_SIZE as u64 + MARKER_SIZE) & 0x0f;
        if remainder == 0 {
            cursor
        } else {
            cursor + (16 - remainder)
        }
    }
}

pub fn locate(path: impl AsRef<Path>) -> Result<LocatedReceipt, ReceiptError> {
    let path = path.as_ref();
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let file_len = file
        .metadata()
        .map_err(|error| io_error(path, error))?
        .len();
    if file_len < ORIGINAL_HEADER_SIZE as u64 {
        return Err(ReceiptError::TooShort);
    }
    let mut header = [0_u8; ORIGINAL_HEADER_SIZE];
    file.read_exact(&mut header)
        .map_err(|error| io_error(path, error))?;
    let signature_offset = u64::from(u32::from_le_bytes(header[0x80..0x84].try_into().unwrap()));
    if signature_offset < MARKER_SIZE || signature_offset + SIGNATURE_SIZE > file_len {
        return Err(ReceiptError::NotPatched);
    }
    let marker_offset = signature_offset - MARKER_SIZE;
    file.seek(SeekFrom::Start(marker_offset))
        .map_err(|error| io_error(path, error))?;
    let mut marker = [0_u8; MARKER_SIZE as usize];
    file.read_exact(&mut marker)
        .map_err(|error| io_error(path, error))?;
    if marker.get(..8) != Some(MARKER_MAGIC) {
        return Err(ReceiptError::NotPatched);
    }
    let receipt_len = u64::from_le_bytes(marker[8..16].try_into().unwrap());
    if receipt_len != RECEIPT_SIZE as u64 || marker_offset < receipt_len {
        return Err(ReceiptError::Invalid(format!(
            "invalid receipt size: {receipt_len}"
        )));
    }
    let receipt_offset = marker_offset - receipt_len;
    file.seek(SeekFrom::Start(receipt_offset))
        .map_err(|error| io_error(path, error))?;
    let mut data = [0_u8; RECEIPT_SIZE];
    file.read_exact(&mut data)
        .map_err(|error| io_error(path, error))?;
    let receipt = Receipt::from_bytes(&data)?;
    if receipt.original_len > receipt_offset {
        return Err(ReceiptError::Invalid(
            "receipt overlaps the original ROM".to_owned(),
        ));
    }
    Ok(LocatedReceipt {
        receipt,
        receipt_offset,
        signature_offset,
        file_len,
    })
}

pub fn restore_base(path: impl AsRef<Path>) -> Result<(Vec<u8>, LocatedReceipt), ReceiptError> {
    let path = path.as_ref();
    let located = locate(path)?;
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut base = vec![0_u8; located.receipt.original_len as usize];
    file.read_exact(&mut base)
        .map_err(|error| io_error(path, error))?;
    // Header and auxiliary pointer preimages are stored explicitly because
    // those are the only original-range bytes the append-only patch rewrites.
    base[..ORIGINAL_HEADER_SIZE].copy_from_slice(&located.receipt.original_header);
    base[AUX_OFFSET..AUX_OFFSET + 4].copy_from_slice(&located.receipt.original_aux_1000);
    if Sha256::digest(&base).as_slice() != located.receipt.original_sha256 {
        return Err(ReceiptError::OriginalHashMismatch);
    }
    Ok((base, located))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn receipt_round_trips_all_supported_languages() {
        let mut base = vec![0xff; 0x3000];
        base[0x0c..0x10].copy_from_slice(b"BSKE");
        for language in [
            PatchedLanguage::Japanese,
            PatchedLanguage::English,
            PatchedLanguage::French,
        ] {
            let receipt = Receipt::new(&base, language).unwrap();
            assert_eq!(Receipt::from_bytes(&receipt.to_bytes()).unwrap(), receipt);
        }

        let mut unknown = Receipt::new(&base, PatchedLanguage::French)
            .unwrap()
            .to_bytes();
        unknown[572..576].copy_from_slice(b"de\0\0");
        assert!(matches!(
            Receipt::from_bytes(&unknown),
            Err(ReceiptError::Invalid(message)) if message == "unknown language"
        ));
    }

    #[test]
    fn footer_is_located_and_base_is_restored_exactly() {
        let mut base = vec![0xff; 0x3000];
        base[..12].copy_from_slice(b"999HRPERDOOR");
        base[0x0c..0x10].copy_from_slice(b"BSKE");
        base[0x80..0x84].copy_from_slice(&0x2000_u32.to_le_bytes());
        base[0x1000..0x1004].copy_from_slice(&0x1234_u32.to_le_bytes());
        let receipt = Receipt::new(&base, PatchedLanguage::French).unwrap();
        let mut patched = base.clone();
        patched[0x14] = 12;
        patched[0x1000..0x1004].copy_from_slice(&0xdeadbeef_u32.to_le_bytes());
        let receipt_offset = Receipt::padded_receipt_offset(patched.len() as u64);
        patched.resize(receipt_offset as usize, 0xff);
        patched.extend_from_slice(&receipt.to_bytes());
        patched.extend_from_slice(&receipt.marker());
        let signature_offset = patched.len() as u32;
        patched.extend_from_slice(&[0xff; SIGNATURE_SIZE as usize]);
        patched[0x80..0x84].copy_from_slice(&signature_offset.to_le_bytes());

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&patched).unwrap();
        file.flush().unwrap();
        let located = locate(file.path()).unwrap();
        assert_eq!(located.signature_offset, u64::from(signature_offset));
        assert_eq!(located.receipt.language, PatchedLanguage::French);
        let (restored, _) = restore_base(file.path()).unwrap();
        assert_eq!(restored, base);
    }
}
