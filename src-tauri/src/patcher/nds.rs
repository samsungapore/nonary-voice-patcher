//! Minimal, append-only Nintendo DS NitroFS reader and planner.
//!
//! Append-only output makes restoration auditable: original payloads remain in
//! the ROM prefix and only new FAT entries select replacements. The planner
//! owns offsets rather than payload bytes so large voice packs can be streamed
//! without holding a second ROM-sized buffer in memory.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;

const HEADER_CORE_LEN: usize = 0x200;
const HEADER_CRC_END: usize = 0x15e;
const MAX_NDS_ROM_SIZE: u64 = 512 * 1024 * 1024;
const ROOT_DIRECTORY_ID: u16 = 0xf000;
const DEFAULT_APPEND_ALIGNMENT: u32 = 4;

pub type Result<T> = std::result::Result<T, NdsError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NdsError {
    message: String,
}

impl NdsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for NdsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NdsError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FatEntry {
    pub start: u32,
    pub end: u32,
}

impl FatEntry {
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NdsHeader {
    pub game_title: [u8; 12],
    pub game_code: [u8; 4],
    pub maker_code: [u8; 2],
    pub device_capacity: u8,
    pub fnt_offset: u32,
    pub fnt_size: u32,
    pub fat_offset: u32,
    pub fat_size: u32,
    pub arm9_overlay_offset: u32,
    pub arm9_overlay_size: u32,
    pub arm7_overlay_offset: u32,
    pub arm7_overlay_size: u32,
    pub rom_size: u32,
    pub header_size: u32,
    pub stored_header_crc16: u16,
    pub computed_header_crc16: u16,
}

impl NdsHeader {
    pub fn header_crc_valid(&self) -> bool {
        self.stored_header_crc16 == self.computed_header_crc16
    }

    pub fn declared_capacity_bytes(&self) -> u64 {
        1_u64 << (u32::from(self.device_capacity) + 17)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRecord {
    pub id: u32,
    pub path: String,
    pub fat: FatEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayProcessor {
    Arm9,
    Arm7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayReference {
    pub processor: OverlayProcessor,
    pub overlay_id: u32,
    pub file_id: u32,
}

#[derive(Clone, Debug)]
struct Directory {
    id: u16,
    first_file_id: u16,
    parent_id: u16,
    files: Vec<Vec<u8>>,
    folders: Vec<(Vec<u8>, u16)>,
}

/// Borrowing the image keeps structural inspection from duplicating a ROM-sized
/// buffer before the streaming writer has even planned its output.
#[derive(Debug)]
pub struct NdsRom<'image> {
    image: &'image [u8],
    header: NdsHeader,
    fat: Vec<FatEntry>,
    directories: Vec<Directory>,
    files: Vec<FileRecord>,
    overlays: Vec<OverlayReference>,
    path_to_id: BTreeMap<String, u32>,
    directory_paths: BTreeMap<String, u16>,
}

impl<'image> NdsRom<'image> {
    pub fn parse(image: &'image [u8]) -> Result<Self> {
        if image.len() < HEADER_CORE_LEN {
            return Err(NdsError::new(format!(
                "NDS image is only {} bytes; expected at least 0x{HEADER_CORE_LEN:x}",
                image.len()
            )));
        }

        let header = parse_header(image)?;
        if header.header_size < HEADER_CORE_LEN as u32 || header.header_size as usize > image.len()
        {
            return Err(NdsError::new(format!(
                "declared NDS header size 0x{:x} is outside 0x{HEADER_CORE_LEN:x}..=0x{:x}",
                header.header_size,
                image.len()
            )));
        }
        if header.rom_size < header.header_size || header.rom_size as usize > image.len() {
            return Err(NdsError::new(format!(
                "declared used ROM size 0x{:x} is outside header..image range 0x{:x}..=0x{:x}",
                header.rom_size,
                header.header_size,
                image.len()
            )));
        }
        let fnt = checked_slice(
            image,
            header.fnt_offset as usize,
            header.fnt_size as usize,
            "FNT",
        )?;
        let fat_data = checked_slice(
            image,
            header.fat_offset as usize,
            header.fat_size as usize,
            "FAT",
        )?;
        if fat_data.is_empty() || fat_data.len() % 8 != 0 {
            return Err(NdsError::new(format!(
                "FAT size 0x{:x} is not a non-zero multiple of 8",
                fat_data.len()
            )));
        }

        let mut fat = Vec::with_capacity(fat_data.len() / 8);
        for (id, entry) in fat_data.chunks_exact(8).enumerate() {
            let start = read_u32(entry, 0, "FAT start")?;
            let end = read_u32(entry, 4, "FAT end")?;
            if start > end || end as usize > image.len() {
                return Err(NdsError::new(format!(
                    "FAT entry {id} has invalid range 0x{start:x}..0x{end:x} for a 0x{:x}-byte ROM",
                    image.len()
                )));
            }
            fat.push(FatEntry { start, end });
        }

        let directories = parse_fnt(fnt, fat.len())?;
        let (mut files, path_to_id, directory_paths) = index_paths(&directories, &fat)?;
        files.sort_by_key(|record| record.id);
        let mut overlays = parse_overlay_table(
            image,
            header.arm9_overlay_offset,
            header.arm9_overlay_size,
            OverlayProcessor::Arm9,
            fat.len(),
        )?;
        overlays.extend(parse_overlay_table(
            image,
            header.arm7_overlay_offset,
            header.arm7_overlay_size,
            OverlayProcessor::Arm7,
            fat.len(),
        )?);

        Ok(Self {
            image,
            header,
            fat,
            directories,
            files,
            overlays,
            path_to_id,
            directory_paths,
        })
    }

    pub fn image(&self) -> &'image [u8] {
        self.image
    }

    pub fn header(&self) -> &NdsHeader {
        &self.header
    }

    pub fn fat(&self) -> &[FatEntry] {
        &self.fat
    }

    pub fn files(&self) -> &[FileRecord] {
        &self.files
    }

    pub fn overlays(&self) -> &[OverlayReference] {
        &self.overlays
    }

    pub fn path_index(&self) -> &BTreeMap<String, u32> {
        &self.path_to_id
    }

    pub fn id_for_path(&self, path: &str) -> Option<u32> {
        self.path_to_id.get(normalize_path(path)).copied()
    }

    pub fn fat_for_id(&self, id: u32) -> Option<FatEntry> {
        self.fat.get(id as usize).copied()
    }

    pub fn data_for_id(&self, id: u32) -> Option<&'image [u8]> {
        let entry = self.fat_for_id(id)?;
        self.image.get(entry.range())
    }

    pub fn data_for_path(&self, path: &str) -> Option<&'image [u8]> {
        self.data_for_id(self.id_for_path(path)?)
    }

    pub fn sound_insertion_id(&self) -> Result<u32> {
        let sound_id = self
            .directory_paths
            .get("sound")
            .copied()
            .ok_or_else(|| NdsError::new("NitroFS has no root sound directory"))?;
        let sound = directory_by_id(&self.directories, sound_id)?;
        Ok(u32::from(sound.first_file_id) + sound.files.len() as u32)
    }

    /// Keep added voices inside the `sound` directory's contiguous file-ID
    /// range because NitroFS assigns IDs by FNT traversal order.
    ///
    /// `replacements` must name files already present in the source ROM.
    /// Their original bytes remain in the copied prefix; only their new FAT
    /// entries point at the appended payloads. `voices` must be the contiguous
    /// sequence `sound/se_v0000.se`, `sound/se_v0001.se`, ... in stream order.
    pub fn plan_sound_voice_append(
        &self,
        voices: &[StreamedFile],
        replacements: &[StreamedFile],
    ) -> Result<AppendPlan> {
        self.plan_sound_voice_append_aligned(voices, replacements, DEFAULT_APPEND_ALIGNMENT)
    }

    pub fn plan_sound_voice_append_aligned(
        &self,
        voices: &[StreamedFile],
        replacements: &[StreamedFile],
        alignment: u32,
    ) -> Result<AppendPlan> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(NdsError::new(format!(
                "append alignment {alignment} is not a non-zero power of two"
            )));
        }
        if voices.is_empty() {
            return Err(NdsError::new("at least one voice file is required"));
        }
        if voices.len() > 10_000 {
            return Err(NdsError::new(
                "voice sequence exceeds the four-digit se_v#### namespace",
            ));
        }

        let mut added_paths = BTreeSet::new();
        for (index, voice) in voices.iter().enumerate() {
            let expected = format!("sound/se_v{index:04}.se");
            let actual = normalize_path(&voice.path);
            if actual != expected {
                return Err(NdsError::new(format!(
                    "voice {index} is {actual:?}; expected {expected:?}"
                )));
            }
            if voice.size == 0 {
                return Err(NdsError::new(format!(
                    "new voice {actual} has an empty payload"
                )));
            }
            if self.path_to_id.contains_key(actual) || !added_paths.insert(actual.to_owned()) {
                return Err(NdsError::new(format!(
                    "voice path already exists or is duplicated: {actual}"
                )));
            }
        }

        let mut replacement_ids = Vec::with_capacity(replacements.len());
        let mut replaced_paths = BTreeSet::new();
        for replacement in replacements {
            let path = normalize_path(&replacement.path);
            if !replaced_paths.insert(path.to_owned()) {
                return Err(NdsError::new(format!(
                    "replacement path is duplicated: {path}"
                )));
            }
            let old_id = self.path_to_id.get(path).copied().ok_or_else(|| {
                NdsError::new(format!("replacement file is not in the source ROM: {path}"))
            })?;
            replacement_ids.push(old_id);
        }

        let sound_id = self
            .directory_paths
            .get("sound")
            .copied()
            .ok_or_else(|| NdsError::new("NitroFS has no root sound directory"))?;
        let sound = directory_by_id(&self.directories, sound_id)?;
        let insertion_id = usize::from(sound.first_file_id)
            .checked_add(sound.files.len())
            .ok_or_else(|| NdsError::new("sound insertion ID overflow"))?;
        if insertion_id > self.fat.len() {
            return Err(NdsError::new(format!(
                "sound insertion ID {insertion_id} exceeds FAT count {}",
                self.fat.len()
            )));
        }
        if let Some(reference) = self
            .overlays
            .iter()
            .find(|reference| reference.file_id as usize >= insertion_id)
        {
            // Overlay tables store raw FAT IDs outside NitroFS, so shifting one
            // would require a broader executable-format rewrite than this patch promises.
            return Err(NdsError::new(format!(
                "{:?} overlay {} references FAT ID {} at or after sound insertion ID {insertion_id}; append-only overlay-table remapping is not supported",
                reference.processor, reference.overlay_id, reference.file_id
            )));
        }
        let final_file_count = self
            .fat
            .len()
            .checked_add(voices.len())
            .ok_or_else(|| NdsError::new("final FAT count overflow"))?;
        if final_file_count > usize::from(u16::MAX) + 1 {
            return Err(NdsError::new(format!(
                "final file count {final_file_count} exceeds the 16-bit NitroFS ID space"
            )));
        }

        let added_count = voices.len();
        // Existing IDs after `sound` must shift together or the unchanged FNT
        // paths would resolve to the wrong files after voice insertion.
        let old_to_new_file_ids: Vec<u32> = (0..self.fat.len())
            .map(|old_id| {
                if old_id >= insertion_id {
                    (old_id + added_count) as u32
                } else {
                    old_id as u32
                }
            })
            .collect();

        let mut directories = self.directories.clone();
        for directory in &mut directories {
            let first = usize::from(directory.first_file_id);
            let end = first + directory.files.len();
            if directory.id != sound_id && first < insertion_id && insertion_id < end {
                return Err(NdsError::new(format!(
                    "directory 0x{:04x} file range {first}..{end} straddles sound insertion ID {insertion_id}",
                    directory.id
                )));
            }
            if directory.id != sound_id && first >= insertion_id {
                let shifted = first + added_count;
                directory.first_file_id = u16::try_from(shifted).map_err(|_| {
                    NdsError::new(format!(
                        "shifted first file ID {shifted} does not fit in the FNT"
                    ))
                })?;
            }
        }
        let planned_sound = directory_by_id_mut(&mut directories, sound_id)?;
        for voice in voices {
            let basename = normalize_path(&voice.path)
                .strip_prefix("sound/")
                .expect("validated voice path prefix");
            planned_sound.files.push(latin1_encode(basename)?);
        }
        let fnt_bytes = serialize_fnt(&directories)?;

        let mut final_fat = vec![FatEntry { start: 0, end: 0 }; final_file_count];
        for (old_id, entry) in self.fat.iter().copied().enumerate() {
            final_fat[old_to_new_file_ids[old_id] as usize] = entry;
        }

        let mut payloads = Vec::with_capacity(replacements.len() + voices.len());
        let mut cursor = self.image.len() as u64;
        for (source_index, (replacement, old_id)) in replacements
            .iter()
            .zip(replacement_ids.iter().copied())
            .enumerate()
        {
            cursor = align_up(cursor, alignment)?;
            let end = checked_payload_end(cursor, replacement.size)?;
            let file_id = old_to_new_file_ids[old_id as usize];
            let entry = FatEntry {
                start: u32::try_from(cursor)
                    .map_err(|_| NdsError::new("replacement start offset exceeds u32"))?,
                end: u32::try_from(end)
                    .map_err(|_| NdsError::new("replacement end offset exceeds u32"))?,
            };
            final_fat[file_id as usize] = entry;
            payloads.push(PayloadPlacement {
                role: PayloadRole::Replacement,
                source_index,
                path: normalize_path(&replacement.path).to_owned(),
                file_id,
                offset: entry.start,
                size: replacement.size,
            });
            cursor = end;
        }
        for (source_index, voice) in voices.iter().enumerate() {
            cursor = align_up(cursor, alignment)?;
            let end = checked_payload_end(cursor, voice.size)?;
            let file_id = (insertion_id + source_index) as u32;
            let entry = FatEntry {
                start: u32::try_from(cursor)
                    .map_err(|_| NdsError::new("voice start offset exceeds u32"))?,
                end: u32::try_from(end)
                    .map_err(|_| NdsError::new("voice end offset exceeds u32"))?,
            };
            final_fat[file_id as usize] = entry;
            payloads.push(PayloadPlacement {
                role: PayloadRole::Voice,
                source_index,
                path: normalize_path(&voice.path).to_owned(),
                file_id,
                offset: entry.start,
                size: voice.size,
            });
            cursor = end;
        }

        let fnt_offset = align_up(cursor, 4)?;
        let after_fnt = fnt_offset
            .checked_add(fnt_bytes.len() as u64)
            .ok_or_else(|| NdsError::new("FNT append offset overflow"))?;
        let fat_offset = align_up(after_fnt, 4)?;
        let fat_bytes = serialize_fat(&final_fat);
        let final_len = fat_offset
            .checked_add(fat_bytes.len() as u64)
            .ok_or_else(|| NdsError::new("FAT append offset overflow"))?;
        if final_len > MAX_NDS_ROM_SIZE {
            return Err(NdsError::new(format!(
                "planned ROM size {final_len} exceeds the Nintendo DS 512 MiB limit"
            )));
        }

        let device_capacity = device_capacity_for_len(final_len)?;
        let declared_capacity_bytes = 1_u64 << (u32::from(device_capacity) + 17);
        let mut patched_header = self.image[..HEADER_CORE_LEN].to_vec();
        patched_header[0x14] = device_capacity;
        write_u32(&mut patched_header, 0x40, fnt_offset as u32)?;
        write_u32(&mut patched_header, 0x44, fnt_bytes.len() as u32)?;
        write_u32(&mut patched_header, 0x48, fat_offset as u32)?;
        write_u32(&mut patched_header, 0x4c, fat_bytes.len() as u32)?;
        write_u32(&mut patched_header, 0x80, final_len as u32)?;
        let header_crc16 = crc16_nintendo(&patched_header[..HEADER_CRC_END]);
        write_u16(&mut patched_header, 0x15e, header_crc16)?;

        let mut final_path_to_id = BTreeMap::new();
        for (path, old_id) in &self.path_to_id {
            final_path_to_id.insert(path.clone(), old_to_new_file_ids[*old_id as usize]);
        }
        for (index, voice) in voices.iter().enumerate() {
            final_path_to_id.insert(
                normalize_path(&voice.path).to_owned(),
                (insertion_id + index) as u32,
            );
        }

        Ok(AppendPlan {
            original_len: self.image.len() as u64,
            insertion_file_id: insertion_id as u32,
            old_to_new_file_ids,
            final_path_to_id,
            payload_alignment: alignment,
            payloads,
            fnt: PlannedTable {
                offset: fnt_offset as u32,
                bytes: fnt_bytes,
            },
            fat: PlannedTable {
                offset: fat_offset as u32,
                bytes: fat_bytes,
            },
            final_fat,
            patched_header,
            header_crc16,
            device_capacity,
            declared_capacity_bytes,
            final_len,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamedFile {
    pub path: String,
    pub size: u32,
}

impl StreamedFile {
    pub fn new(path: impl Into<String>, size: u32) -> Self {
        Self {
            path: path.into(),
            size,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadRole {
    Replacement,
    Voice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadPlacement {
    /// Kept as an input index so the writer can stream from the selected pack
    /// without transferring ownership of its payload into the layout plan.
    pub source_index: usize,
    pub role: PayloadRole,
    pub path: String,
    pub file_id: u32,
    pub offset: u32,
    pub size: u32,
}

impl PayloadPlacement {
    pub fn end(&self) -> u32 {
        self.offset + self.size
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedTable {
    pub offset: u32,
    pub bytes: Vec<u8>,
}

impl PlannedTable {
    pub fn end(&self) -> u32 {
        self.offset + self.bytes.len() as u32
    }
}

/// Separating the immutable plan from the writer lets validation inspect every
/// address and file-ID mapping before any output file is committed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppendPlan {
    pub original_len: u64,
    pub insertion_file_id: u32,
    pub old_to_new_file_ids: Vec<u32>,
    pub final_path_to_id: BTreeMap<String, u32>,
    pub payload_alignment: u32,
    pub payloads: Vec<PayloadPlacement>,
    pub fnt: PlannedTable,
    pub fat: PlannedTable,
    pub final_fat: Vec<FatEntry>,
    pub patched_header: Vec<u8>,
    pub header_crc16: u16,
    pub device_capacity: u8,
    pub declared_capacity_bytes: u64,
    pub final_len: u64,
}

impl AppendPlan {
    pub fn new_id_for_old(&self, old_id: u32) -> Option<u32> {
        self.old_to_new_file_ids.get(old_id as usize).copied()
    }

    pub fn id_for_path(&self, path: &str) -> Option<u32> {
        self.final_path_to_id.get(normalize_path(path)).copied()
    }
}

/// The retail header checksum is recomputed because emulators and hardware may
/// reject an otherwise valid append-only image when header metadata changes.
pub fn crc16_nintendo(data: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in data {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn parse_header(image: &[u8]) -> Result<NdsHeader> {
    let mut game_title = [0; 12];
    game_title.copy_from_slice(&image[0..12]);
    let mut game_code = [0; 4];
    game_code.copy_from_slice(&image[0x0c..0x10]);
    let mut maker_code = [0; 2];
    maker_code.copy_from_slice(&image[0x10..0x12]);
    let stored_header_crc16 = read_u16(image, 0x15e, "header CRC16")?;
    Ok(NdsHeader {
        game_title,
        game_code,
        maker_code,
        device_capacity: image[0x14],
        fnt_offset: read_u32(image, 0x40, "FNT offset")?,
        fnt_size: read_u32(image, 0x44, "FNT size")?,
        fat_offset: read_u32(image, 0x48, "FAT offset")?,
        fat_size: read_u32(image, 0x4c, "FAT size")?,
        arm9_overlay_offset: read_u32(image, 0x50, "ARM9 overlay offset")?,
        arm9_overlay_size: read_u32(image, 0x54, "ARM9 overlay size")?,
        arm7_overlay_offset: read_u32(image, 0x58, "ARM7 overlay offset")?,
        arm7_overlay_size: read_u32(image, 0x5c, "ARM7 overlay size")?,
        rom_size: read_u32(image, 0x80, "ROM size")?,
        header_size: read_u32(image, 0x84, "header size")?,
        stored_header_crc16,
        computed_header_crc16: crc16_nintendo(&image[..HEADER_CRC_END]),
    })
}

fn parse_overlay_table(
    image: &[u8],
    offset: u32,
    size: u32,
    processor: OverlayProcessor,
    file_count: usize,
) -> Result<Vec<OverlayReference>> {
    if size == 0 {
        return Ok(Vec::new());
    }
    if !size.is_multiple_of(0x20) {
        return Err(NdsError::new(format!(
            "{processor:?} overlay table size 0x{size:x} is not a multiple of 0x20"
        )));
    }
    let table = checked_slice(
        image,
        offset as usize,
        size as usize,
        match processor {
            OverlayProcessor::Arm9 => "ARM9 overlay table",
            OverlayProcessor::Arm7 => "ARM7 overlay table",
        },
    )?;
    let mut references = Vec::with_capacity(table.len() / 0x20);
    for entry in table.chunks_exact(0x20) {
        let overlay_id = read_u32(entry, 0, "overlay ID")?;
        let file_id = read_u32(entry, 0x18, "overlay FAT file ID")?;
        if file_id as usize >= file_count {
            return Err(NdsError::new(format!(
                "{processor:?} overlay {overlay_id} references FAT ID {file_id}, but FAT count is {file_count}"
            )));
        }
        references.push(OverlayReference {
            processor,
            overlay_id,
            file_id,
        });
    }
    Ok(references)
}

fn parse_fnt(fnt: &[u8], file_count: usize) -> Result<Vec<Directory>> {
    if fnt.len() < 8 {
        return Err(NdsError::new("FNT is shorter than its root directory row"));
    }
    let directory_count = usize::from(read_u16(fnt, 6, "root directory count")?);
    if directory_count == 0 || directory_count > 0x1000 {
        return Err(NdsError::new(format!(
            "invalid FNT directory count {directory_count}"
        )));
    }
    let table_size = directory_count
        .checked_mul(8)
        .ok_or_else(|| NdsError::new("FNT directory table size overflow"))?;
    if table_size > fnt.len() {
        return Err(NdsError::new(format!(
            "FNT directory table needs 0x{table_size:x} bytes but file has 0x{:x}",
            fnt.len()
        )));
    }

    let mut directories = Vec::with_capacity(directory_count);
    for index in 0..directory_count {
        let row = index * 8;
        let entries_offset = read_u32(fnt, row, "FNT entries offset")? as usize;
        let first_file_id = read_u16(fnt, row + 4, "FNT first file ID")?;
        let parent_id = read_u16(fnt, row + 6, "FNT parent ID")?;
        if entries_offset < table_size || entries_offset >= fnt.len() {
            return Err(NdsError::new(format!(
                "directory 0x{:04x} entries offset 0x{entries_offset:x} is outside FNT payload",
                ROOT_DIRECTORY_ID + index as u16
            )));
        }

        let mut cursor = entries_offset;
        let mut files = Vec::new();
        let mut folders = Vec::new();
        loop {
            let control = *fnt.get(cursor).ok_or_else(|| {
                NdsError::new(format!(
                    "unterminated FNT directory 0x{:04x}",
                    ROOT_DIRECTORY_ID + index as u16
                ))
            })?;
            cursor += 1;
            if control == 0 {
                break;
            }
            let name_len = usize::from(control & 0x7f);
            if name_len == 0 {
                return Err(NdsError::new("FNT entry has an empty name"));
            }
            let name = checked_slice(fnt, cursor, name_len, "FNT name")?.to_vec();
            validate_name(&name)?;
            cursor += name_len;
            if control & 0x80 == 0 {
                files.push(name);
            } else {
                let child_id = read_u16(fnt, cursor, "FNT child directory ID")?;
                cursor += 2;
                let child_index = child_id.wrapping_sub(ROOT_DIRECTORY_ID) as usize;
                if child_id < ROOT_DIRECTORY_ID || child_index >= directory_count {
                    return Err(NdsError::new(format!(
                        "FNT child directory ID 0x{child_id:04x} is outside the directory table"
                    )));
                }
                folders.push((name, child_id));
            }
        }
        if usize::from(first_file_id) + files.len() > file_count {
            return Err(NdsError::new(format!(
                "directory 0x{:04x} file IDs {}..{} exceed FAT count {file_count}",
                ROOT_DIRECTORY_ID + index as u16,
                first_file_id,
                usize::from(first_file_id) + files.len()
            )));
        }
        directories.push(Directory {
            id: ROOT_DIRECTORY_ID + index as u16,
            first_file_id,
            parent_id,
            files,
            folders,
        });
    }
    if usize::from(directories[0].parent_id) != directory_count {
        return Err(NdsError::new(format!(
            "root FNT row declares {} directories, parsed {directory_count}",
            directories[0].parent_id
        )));
    }
    Ok(directories)
}

type IndexedPaths = (
    Vec<FileRecord>,
    BTreeMap<String, u32>,
    BTreeMap<String, u16>,
);

#[derive(Default)]
struct PathIndexState {
    files: Vec<FileRecord>,
    path_to_id: BTreeMap<String, u32>,
    directory_paths: BTreeMap<String, u16>,
    visited_directories: BTreeSet<u16>,
    visited_file_ids: BTreeSet<u32>,
}

fn visit_indexed_paths(
    directories: &[Directory],
    fat: &[FatEntry],
    id: u16,
    path: &str,
    state: &mut PathIndexState,
) -> Result<()> {
    if !state.visited_directories.insert(id) {
        return Err(NdsError::new(format!(
            "FNT directory 0x{id:04x} is referenced more than once or recursively"
        )));
    }
    if state.directory_paths.insert(path.to_owned(), id).is_some() {
        return Err(NdsError::new(format!(
            "duplicate NitroFS directory path {path:?}"
        )));
    }
    let directory = directory_by_id(directories, id)?;
    for (index, name) in directory.files.iter().enumerate() {
        let file_id = u32::from(directory.first_file_id) + index as u32;
        if !state.visited_file_ids.insert(file_id) {
            return Err(NdsError::new(format!(
                "NitroFS file ID {file_id} is assigned by multiple directories"
            )));
        }
        let name = latin1_decode(name);
        let file_path = join_path(path, &name);
        if state
            .path_to_id
            .insert(file_path.clone(), file_id)
            .is_some()
        {
            return Err(NdsError::new(format!(
                "duplicate NitroFS file path {file_path:?}"
            )));
        }
        let fat_entry = fat
            .get(file_id as usize)
            .copied()
            .ok_or_else(|| NdsError::new(format!("FNT file ID {file_id} has no FAT entry")))?;
        state.files.push(FileRecord {
            id: file_id,
            path: file_path,
            fat: fat_entry,
        });
    }
    for (name, child_id) in &directory.folders {
        let child = directory_by_id(directories, *child_id)?;
        if child.parent_id != id {
            return Err(NdsError::new(format!(
                "FNT child 0x{child_id:04x} declares parent 0x{:04x}, expected 0x{id:04x}",
                child.parent_id
            )));
        }
        let child_name = latin1_decode(name);
        let child_path = join_path(path, &child_name);
        visit_indexed_paths(directories, fat, *child_id, &child_path, state)?;
    }
    Ok(())
}

fn index_paths(directories: &[Directory], fat: &[FatEntry]) -> Result<IndexedPaths> {
    let mut state = PathIndexState::default();
    visit_indexed_paths(directories, fat, ROOT_DIRECTORY_ID, "", &mut state)?;
    if state.visited_directories.len() != directories.len() {
        return Err(NdsError::new(format!(
            "only {} of {} FNT directories are reachable from root",
            state.visited_directories.len(),
            directories.len()
        )));
    }
    Ok((state.files, state.path_to_id, state.directory_paths))
}

fn serialize_fnt(directories: &[Directory]) -> Result<Vec<u8>> {
    if directories.is_empty() || directories.len() > 0x1000 {
        return Err(NdsError::new("cannot serialize an empty or oversized FNT"));
    }
    let mut fnt = vec![0_u8; directories.len() * 8];
    for (index, directory) in directories.iter().enumerate() {
        let expected_id = ROOT_DIRECTORY_ID + index as u16;
        if directory.id != expected_id {
            return Err(NdsError::new(format!(
                "non-contiguous FNT directory ID 0x{:04x}; expected 0x{expected_id:04x}",
                directory.id
            )));
        }
        let entries_offset =
            u32::try_from(fnt.len()).map_err(|_| NdsError::new("FNT exceeds u32 offset space"))?;
        let row = index * 8;
        write_u32(&mut fnt, row, entries_offset)?;
        write_u16(&mut fnt, row + 4, directory.first_file_id)?;
        let parent = if index == 0 {
            directories.len() as u16
        } else {
            directory.parent_id
        };
        write_u16(&mut fnt, row + 6, parent)?;

        for name in &directory.files {
            validate_name(name)?;
            fnt.push(name.len() as u8);
            fnt.extend_from_slice(name);
        }
        for (name, child_id) in &directory.folders {
            validate_name(name)?;
            if directory_by_id(directories, *child_id)?.parent_id != directory.id {
                return Err(NdsError::new(format!(
                    "cannot serialize child 0x{child_id:04x} under the wrong parent"
                )));
            }
            fnt.push((name.len() as u8) | 0x80);
            fnt.extend_from_slice(name);
            fnt.extend_from_slice(&child_id.to_le_bytes());
        }
        fnt.push(0);
    }
    Ok(fnt)
}

fn serialize_fat(entries: &[FatEntry]) -> Vec<u8> {
    let mut data = Vec::with_capacity(entries.len() * 8);
    for entry in entries {
        data.extend_from_slice(&entry.start.to_le_bytes());
        data.extend_from_slice(&entry.end.to_le_bytes());
    }
    data
}

fn directory_by_id(directories: &[Directory], id: u16) -> Result<&Directory> {
    let index = id
        .checked_sub(ROOT_DIRECTORY_ID)
        .ok_or_else(|| NdsError::new(format!("invalid directory ID 0x{id:04x}")))?
        as usize;
    directories
        .get(index)
        .ok_or_else(|| NdsError::new(format!("unknown directory ID 0x{id:04x}")))
}

fn directory_by_id_mut(directories: &mut [Directory], id: u16) -> Result<&mut Directory> {
    let index = id
        .checked_sub(ROOT_DIRECTORY_ID)
        .ok_or_else(|| NdsError::new(format!("invalid directory ID 0x{id:04x}")))?
        as usize;
    directories
        .get_mut(index)
        .ok_or_else(|| NdsError::new(format!("unknown directory ID 0x{id:04x}")))
}

fn validate_name(name: &[u8]) -> Result<()> {
    if name.is_empty() || name.len() > 127 {
        return Err(NdsError::new(format!(
            "NitroFS name length {} is outside 1..=127",
            name.len()
        )));
    }
    if name.contains(&b'/') || name.contains(&0) {
        return Err(NdsError::new("NitroFS names cannot contain '/' or NUL"));
    }
    Ok(())
}

fn latin1_decode(data: &[u8]) -> String {
    data.iter().map(|byte| char::from(*byte)).collect()
}

fn latin1_encode(value: &str) -> Result<Vec<u8>> {
    value
        .chars()
        .map(|character| {
            u8::try_from(character as u32).map_err(|_| {
                NdsError::new(format!(
                    "NitroFS name {value:?} contains a non-Latin-1 character {character:?}"
                ))
            })
        })
        .collect()
}

fn normalize_path(path: &str) -> &str {
    path.trim_matches('/')
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn checked_payload_end(start: u64, size: u32) -> Result<u64> {
    let end = start
        .checked_add(u64::from(size))
        .ok_or_else(|| NdsError::new("payload end offset overflow"))?;
    if end > MAX_NDS_ROM_SIZE {
        return Err(NdsError::new(format!(
            "payload ending at {end} exceeds the Nintendo DS 512 MiB limit"
        )));
    }
    Ok(end)
}

fn align_up(value: u64, alignment: u32) -> Result<u64> {
    let mask = u64::from(alignment - 1);
    value
        .checked_add(mask)
        .map(|sum| sum & !mask)
        .ok_or_else(|| NdsError::new("aligned offset overflow"))
}

fn device_capacity_for_len(length: u64) -> Result<u8> {
    if length > MAX_NDS_ROM_SIZE {
        return Err(NdsError::new(format!(
            "ROM size {length} exceeds the Nintendo DS 512 MiB limit"
        )));
    }
    let mut exponent = 17_u32;
    while (1_u64 << exponent) < length {
        exponent += 1;
    }
    Ok((exponent - 17) as u8)
}

fn checked_slice<'a>(data: &'a [u8], offset: usize, size: usize, name: &str) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| NdsError::new(format!("{name} range overflow")))?;
    data.get(offset..end).ok_or_else(|| {
        NdsError::new(format!(
            "{name} range 0x{offset:x}..0x{end:x} exceeds buffer size 0x{:x}",
            data.len()
        ))
    })
}

fn read_u16(data: &[u8], offset: usize, name: &str) -> Result<u16> {
    let bytes: [u8; 2] = checked_slice(data, offset, 2, name)?
        .try_into()
        .expect("checked two-byte slice");
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(data: &[u8], offset: usize, name: &str) -> Result<u32> {
    let bytes: [u8; 4] = checked_slice(data, offset, 4, name)?
        .try_into()
        .expect("checked four-byte slice");
    Ok(u32::from_le_bytes(bytes))
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) -> Result<()> {
    let target = data
        .get_mut(offset..offset + 2)
        .ok_or_else(|| NdsError::new(format!("u16 write at 0x{offset:x} exceeds buffer")))?;
    target.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let target = data
        .get_mut(offset..offset + 4)
        .ok_or_else(|| NdsError::new(format!("u32 write at 0x{offset:x} exceeds buffer")))?;
    target.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn synthetic_rom() -> Vec<u8> {
        let directories = vec![
            Directory {
                id: 0xf000,
                first_file_id: 0,
                parent_id: 3,
                files: vec![b"root.bin".to_vec()],
                folders: vec![(b"sound".to_vec(), 0xf001), (b"etc".to_vec(), 0xf002)],
            },
            Directory {
                id: 0xf001,
                first_file_id: 1,
                parent_id: 0xf000,
                files: vec![b"se_sys.se".to_vec()],
                folders: vec![],
            },
            Directory {
                id: 0xf002,
                first_file_id: 2,
                parent_id: 0xf000,
                files: vec![b"sound.dat".to_vec()],
                folders: vec![],
            },
        ];
        let fnt = serialize_fnt(&directories).unwrap();
        let fnt_offset = 0x200_u32;
        let fat_offset = align_up(u64::from(fnt_offset) + fnt.len() as u64, 4).unwrap() as u32;
        let data_offset = align_up(u64::from(fat_offset) + 3 * 8, 4).unwrap() as u32;
        let payloads: [&[u8]; 3] = [b"ROOT", b"SYSTEM", b"REGISTRY"];
        let mut cursor = data_offset;
        let mut fat = Vec::new();
        for payload in payloads {
            fat.push(FatEntry {
                start: cursor,
                end: cursor + payload.len() as u32,
            });
            cursor += payload.len() as u32;
        }
        let fat_bytes = serialize_fat(&fat);
        let mut image = vec![0xff; cursor as usize];
        image[0..12].copy_from_slice(b"SYNTHETICNDS");
        image[0x0c..0x10].copy_from_slice(b"TEST");
        image[0x10..0x12].copy_from_slice(b"ZZ");
        image[0x14] = device_capacity_for_len(image.len() as u64).unwrap();
        write_u32(&mut image, 0x40, fnt_offset).unwrap();
        write_u32(&mut image, 0x44, fnt.len() as u32).unwrap();
        write_u32(&mut image, 0x48, fat_offset).unwrap();
        write_u32(&mut image, 0x4c, fat_bytes.len() as u32).unwrap();
        write_u32(&mut image, 0x50, 0).unwrap();
        write_u32(&mut image, 0x54, 0).unwrap();
        write_u32(&mut image, 0x58, 0).unwrap();
        write_u32(&mut image, 0x5c, 0).unwrap();
        let image_len = image.len() as u32;
        write_u32(&mut image, 0x80, image_len).unwrap();
        write_u32(&mut image, 0x84, 0x200).unwrap();
        image[fnt_offset as usize..fnt_offset as usize + fnt.len()].copy_from_slice(&fnt);
        image[fat_offset as usize..fat_offset as usize + fat_bytes.len()]
            .copy_from_slice(&fat_bytes);
        for (entry, payload) in fat.iter().zip(payloads) {
            image[entry.range()].copy_from_slice(payload);
        }
        let crc = crc16_nintendo(&image[..HEADER_CRC_END]);
        write_u16(&mut image, 0x15e, crc).unwrap();
        image
    }

    fn materialize_for_test(
        source: &[u8],
        plan: &AppendPlan,
        replacements: &[Vec<u8>],
        voices: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut output = vec![0xff; plan.final_len as usize];
        output[..source.len()].copy_from_slice(source);
        for placement in &plan.payloads {
            let payload = match placement.role {
                PayloadRole::Replacement => &replacements[placement.source_index],
                PayloadRole::Voice => &voices[placement.source_index],
            };
            assert_eq!(payload.len(), placement.size as usize);
            output[placement.offset as usize..placement.end() as usize].copy_from_slice(payload);
        }
        output[plan.fnt.offset as usize..plan.fnt.end() as usize].copy_from_slice(&plan.fnt.bytes);
        output[plan.fat.offset as usize..plan.fat.end() as usize].copy_from_slice(&plan.fat.bytes);
        output[..HEADER_CORE_LEN].copy_from_slice(&plan.patched_header);
        output
    }

    #[test]
    fn parses_and_indexes_synthetic_nitrofs() {
        let image = synthetic_rom();
        let rom = NdsRom::parse(&image).unwrap();
        assert!(rom.header().header_crc_valid());
        assert_eq!(rom.fat().len(), 3);
        assert_eq!(rom.id_for_path("/sound/se_sys.se/"), Some(1));
        assert_eq!(rom.data_for_path("root.bin"), Some(b"ROOT".as_slice()));
        assert_eq!(
            rom.data_for_path("sound/se_sys.se"),
            Some(b"SYSTEM".as_slice())
        );
        assert_eq!(
            rom.data_for_path("etc/sound.dat"),
            Some(b"REGISTRY".as_slice())
        );
        assert_eq!(rom.sound_insertion_id().unwrap(), 2);
    }

    #[test]
    fn append_plan_shifts_ids_and_keeps_original_prefix_payloads() {
        let image = synthetic_rom();
        let rom = NdsRom::parse(&image).unwrap();
        let replacements = vec![StreamedFile::new("etc/sound.dat", 4)];
        let voices = vec![
            StreamedFile::new("sound/se_v0000.se", 3),
            StreamedFile::new("sound/se_v0001.se", 5),
        ];
        let plan = rom.plan_sound_voice_append(&voices, &replacements).unwrap();

        assert_eq!(plan.insertion_file_id, 2);
        assert_eq!(plan.old_to_new_file_ids, vec![0, 1, 4]);
        assert_eq!(plan.id_for_path("sound/se_v0000.se"), Some(2));
        assert_eq!(plan.id_for_path("sound/se_v0001.se"), Some(3));
        assert_eq!(plan.id_for_path("etc/sound.dat"), Some(4));
        assert!(plan
            .payloads
            .iter()
            .all(|item| u64::from(item.offset) >= plan.original_len));
        assert_eq!(plan.final_fat[0], rom.fat()[0]);
        assert_eq!(plan.final_fat[1], rom.fat()[1]);
        assert_eq!(plan.fnt.offset as u64 % 4, 0);
        assert_eq!(plan.fat.offset as u64 % 4, 0);
        assert_eq!(
            read_u16(&plan.patched_header, 0x15e, "planned CRC").unwrap(),
            crc16_nintendo(&plan.patched_header[..HEADER_CRC_END])
        );

        let output = materialize_for_test(
            &image,
            &plan,
            &[b"NEWS".to_vec()],
            &[b"ONE".to_vec(), b"TWO!!".to_vec()],
        );
        assert_eq!(
            &output[HEADER_CORE_LEN..image.len()],
            &image[HEADER_CORE_LEN..]
        );
        let patched = NdsRom::parse(&output).unwrap();
        assert!(patched.header().header_crc_valid());
        assert_eq!(patched.fat().len(), 5);
        assert_eq!(patched.data_for_path("root.bin"), Some(b"ROOT".as_slice()));
        assert_eq!(
            patched.data_for_path("sound/se_sys.se"),
            Some(b"SYSTEM".as_slice())
        );
        assert_eq!(
            patched.data_for_path("sound/se_v0000.se"),
            Some(b"ONE".as_slice())
        );
        assert_eq!(
            patched.data_for_path("sound/se_v0001.se"),
            Some(b"TWO!!".as_slice())
        );
        assert_eq!(
            patched.data_for_path("etc/sound.dat"),
            Some(b"NEWS".as_slice())
        );
    }

    #[test]
    fn rejects_non_contiguous_voice_names() {
        let image = synthetic_rom();
        let rom = NdsRom::parse(&image).unwrap();
        let error = rom
            .plan_sound_voice_append(&[StreamedFile::new("sound/se_v0001.se", 8)], &[])
            .unwrap_err();
        assert!(error.to_string().contains("expected \"sound/se_v0000.se\""));
    }

    #[test]
    fn rejects_id_shift_that_would_stale_an_overlay_reference() {
        let mut image = synthetic_rom();
        let overlay_offset = image.len();
        image.resize(overlay_offset + 0x20, 0);
        write_u32(&mut image, overlay_offset + 0x18, 2).unwrap();
        write_u32(&mut image, 0x50, overlay_offset as u32).unwrap();
        write_u32(&mut image, 0x54, 0x20).unwrap();
        let image_len = image.len() as u32;
        write_u32(&mut image, 0x80, image_len).unwrap();
        image[0x14] = device_capacity_for_len(image.len() as u64).unwrap();
        let crc = crc16_nintendo(&image[..HEADER_CRC_END]);
        write_u16(&mut image, 0x15e, crc).unwrap();

        let rom = NdsRom::parse(&image).unwrap();
        assert_eq!(
            rom.overlays(),
            &[OverlayReference {
                processor: OverlayProcessor::Arm9,
                overlay_id: 0,
                file_id: 2,
            }]
        );
        let error = rom
            .plan_sound_voice_append(&[StreamedFile::new("sound/se_v0000.se", 8)], &[])
            .unwrap_err();
        assert!(error.to_string().contains("overlay 0 references FAT ID 2"));
    }

    #[test]
    #[ignore = "requires a private retail ROM fixture and generated voice bank"]
    fn reads_retail_999_rom_and_plans_without_materializing_large_output() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../original/Nine Hours, Nine Persons, Nine Doors (USA).nds");
        let image = fs::read(&path).unwrap_or_else(|error| {
            panic!("failed to read retail test ROM {}: {error}", path.display())
        });
        let rom = NdsRom::parse(&image).unwrap();
        assert_eq!(&rom.header().game_title, b"999HRPERDOOR");
        assert_eq!(&rom.header().game_code, b"BSKE");
        assert_eq!(&rom.header().maker_code, b"XS");
        assert!(rom.header().header_crc_valid());
        assert_eq!(rom.fat().len(), 6_420);
        assert_eq!(
            rom.overlays(),
            &[OverlayReference {
                processor: OverlayProcessor::Arm9,
                overlay_id: 0,
                file_id: 0,
            }]
        );
        assert_eq!(rom.id_for_path("etc/sound.dat"), Some(5_398));
        assert_eq!(rom.id_for_path("sound/se_sys.se"), Some(6_415));
        assert_eq!(rom.sound_insertion_id().unwrap(), 6_416);
        assert_eq!(&rom.data_for_path("sound/se_sys.se").unwrap()[..4], b"SIR0");

        // Large declared lengths exercise 512-MiB capacity math, but the plan
        // stores no payload bytes and therefore never builds the 300-MiB ROM.
        let plan = rom
            .plan_sound_voice_append(
                &[
                    StreamedFile::new("sound/se_v0000.se", 100 * 1024 * 1024),
                    StreamedFile::new("sound/se_v0001.se", 80 * 1024 * 1024),
                ],
                &[StreamedFile::new("etc/sound.dat", 104 * 1024)],
            )
            .unwrap();
        assert_eq!(plan.insertion_file_id, 6_416);
        assert_eq!(plan.new_id_for_old(6_415), Some(6_415));
        assert_eq!(plan.new_id_for_old(6_416), Some(6_418));
        assert_eq!(plan.id_for_path("sound/se_v0000.se"), Some(6_416));
        assert_eq!(plan.id_for_path("sound/se_v0001.se"), Some(6_417));
        assert_eq!(plan.device_capacity, 12);
        assert_eq!(plan.declared_capacity_bytes, 512 * 1024 * 1024);
        assert!(plan.final_len < plan.declared_capacity_bytes);
        assert_eq!(
            read_u32(&plan.patched_header, 0x40, "planned FNT offset").unwrap(),
            plan.fnt.offset
        );
        assert_eq!(
            read_u32(&plan.patched_header, 0x48, "planned FAT offset").unwrap(),
            plan.fat.offset
        );
        assert_eq!(
            read_u32(&plan.patched_header, 0x80, "planned ROM size").unwrap(),
            plan.final_len as u32
        );

        let voices_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../build/voices_extended_dialogue_only");
        let mut voice_files: Vec<_> = fs::read_dir(&voices_directory)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to list existing voice bank {}: {error}",
                    voices_directory.display()
                )
            })
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".se"))
            .collect();
        voice_files.sort_by_key(|entry| entry.file_name());
        let voice_specs: Vec<_> = voice_files
            .iter()
            .map(|entry| {
                StreamedFile::new(
                    format!("sound/{}", entry.file_name().to_string_lossy()),
                    u32::try_from(entry.metadata().unwrap().len()).unwrap(),
                )
            })
            .collect();
        assert_eq!(voice_specs.len(), 6_475);
        assert_eq!(
            voice_specs
                .iter()
                .map(|voice| u64::from(voice.size))
                .sum::<u64>(),
            185_035_296
        );
        let complete_plan = rom.plan_sound_voice_append(&voice_specs, &[]).unwrap();
        assert_eq!(complete_plan.final_fat.len(), 12_895);
        assert_eq!(complete_plan.payloads.len(), 6_475);
        assert_eq!(complete_plan.id_for_path("sound/se_v6474.se"), Some(12_890));
        assert_eq!(complete_plan.new_id_for_old(6_419), Some(12_894));
        assert_eq!(complete_plan.device_capacity, 12);
        assert!(complete_plan.final_len < complete_plan.declared_capacity_bytes);
    }
}
