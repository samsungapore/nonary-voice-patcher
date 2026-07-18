//! Parser and minimal patcher for 999's `sound/se_sys.se` bank.
//!
//! Resolving all ten labels through the complete routing chain avoids relying
//! on offsets that vary between compatible ROMs. Limiting the mutation to
//! their verified volume operands preserves every unrelated sound-bank byte.

use std::collections::BTreeSet;

use thiserror::Error;

const SIR0_HEADER_SIZE: usize = 0x10;
const CHUNK_HEADER_SIZE: usize = 0x10;
const MESS_LABELS: [&str; 10] = [
    "SE_SYS_MESS_0",
    "SE_SYS_MESS_1",
    "SE_SYS_MESS_2",
    "SE_SYS_MESS_3",
    "SE_SYS_MESS_4",
    "SE_SYS_MESS_5",
    "SE_SYS_MESS_6",
    "SE_SYS_MESS_7",
    "SE_SYS_MESS_8",
    "SE_SYS_MESS_9",
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid se_sys.se bank: {0}")]
pub struct BleepPatchError(String);

impl BleepPatchError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub type Result<T> = std::result::Result<T, BleepPatchError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleepState {
    Audible,
    Silenced,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleepRoute {
    pub label: &'static str,
    pub sequence_id: usize,
    pub sequence_offset: usize,
    pub sequence_end: usize,
    pub track_offset: usize,
    pub track_payload_size: usize,
    pub volume_opcode_offset: usize,
    pub volume_value_offset: usize,
    pub volume: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedBank {
    pub file_size: usize,
    pub swdl_offset: usize,
    pub sedl_offset: usize,
    pub sequence_chunk_offset: usize,
    pub macro_chunk_offset: usize,
    pub sequence_count: usize,
    pub routes: Vec<BleepRoute>,
}

impl ParsedBank {
    pub fn state(&self) -> BleepState {
        let volumes: BTreeSet<u8> = self.routes.iter().map(|route| route.volume).collect();
        if volumes == BTreeSet::from([0x7f]) {
            BleepState::Audible
        } else if volumes == BTreeSet::from([0x00]) {
            BleepState::Silenced
        } else {
            BleepState::Mixed
        }
    }
}

/// Require all ten independent routes so a partial match cannot mute an
/// unrelated sequence in a regional or translated bank.
pub fn parse(data: &[u8]) -> Result<ParsedBank> {
    if data.len() < 0x100 || !data.len().is_multiple_of(0x10) || data.get(..4) != Some(b"SIR0") {
        return Err(BleepPatchError::new("input is not an aligned SIR0 file"));
    }

    let subheader = read_u32(data, 4)? as usize;
    let relocation_offset = read_u32(data, 8)? as usize;
    if subheader.checked_add(8).is_none_or(|end| end > data.len()) {
        return Err(BleepPatchError::new("SIR0 subheader is out of bounds"));
    }
    let relocations = decode_sir0_relocations(data, relocation_offset)?;
    let expected_relocations = vec![4, 8, subheader, subheader + 4];
    if relocations != expected_relocations {
        return Err(BleepPatchError::new(format!(
            "unexpected SIR0 relocations {relocations:?}; expected {expected_relocations:?}"
        )));
    }

    let swdl = read_u32(data, subheader)? as usize;
    let sedl = read_u32(data, subheader + 4)? as usize;
    if data.get(swdl..swdl.saturating_add(4)) != Some(b"swdl")
        || data.get(sedl..sedl.saturating_add(4)) != Some(b"sedl")
    {
        return Err(BleepPatchError::new(
            "subheader does not point to SWDL followed by SEDL",
        ));
    }
    if !(SIR0_HEADER_SIZE <= swdl && swdl < sedl && sedl < subheader) {
        return Err(BleepPatchError::new("invalid SWDL/SEDL/subheader order"));
    }

    let sedl_end = sedl
        .checked_add(read_u32(data, sedl + 8)? as usize)
        .ok_or_else(|| BleepPatchError::new("SEDL size is too large"))?;
    let eod_offset = sedl_end
        .checked_sub(CHUNK_HEADER_SIZE)
        .ok_or_else(|| BleepPatchError::new("invalid declared SEDL size"))?;
    if sedl_end > subheader || eod_offset < sedl {
        return Err(BleepPatchError::new("invalid declared SEDL size"));
    }
    if data.get(eod_offset..eod_offset + 4) != Some(b"eod ") {
        return Err(BleepPatchError::new("SEDL does not end with an EOD chunk"));
    }

    let sequence_chunk = find_unique(data, b"seq ", sedl + 0x30, sedl_end, "chunk SEQ")?;
    let macro_chunk = find_unique(data, b"mcrl", sequence_chunk + 4, sedl_end, "chunk MCRL")?;
    let sequence_end = chunk_end(data, sequence_chunk, macro_chunk, b"seq ")?;
    let macro_end = chunk_end(data, macro_chunk, sedl_end, b"mcrl")?;
    if sequence_end > macro_chunk {
        return Err(BleepPatchError::new("SEQ and MCRL chunks overlap"));
    }

    let sequence_count = usize::from(read_u16(data, sedl + 0x30)?);
    if sequence_count <= 9 {
        return Err(BleepPatchError::new(format!(
            "invalid SEDL sequence count: {sequence_count}"
        )));
    }
    let sequence_payload = sequence_chunk + CHUNK_HEADER_SIZE;
    if sequence_payload
        .checked_add(
            sequence_count
                .checked_mul(2)
                .ok_or_else(|| BleepPatchError::new("SEQ pointer table is too large"))?,
        )
        .is_none_or(|end| end > sequence_end)
    {
        return Err(BleepPatchError::new("truncated SEQ pointer table"));
    }
    let pointers = (0..sequence_count)
        .map(|index| read_u16(data, sequence_payload + 2 * index).map(usize::from))
        .collect::<Result<Vec<_>>>()?;
    let minimum_pointer = 2 * sequence_count;
    let sequence_payload_size = sequence_end - sequence_payload;
    for (sequence_id, &pointer) in pointers.iter().enumerate() {
        if pointer != 0 && !(minimum_pointer <= pointer && pointer < sequence_payload_size) {
            return Err(BleepPatchError::new(format!(
                "pointer 0x{pointer:x} is outside SEQ for sequence {sequence_id}"
            )));
        }
    }

    let mut routes = Vec::with_capacity(MESS_LABELS.len());
    let mut used_sequence_ids = BTreeSet::new();
    let mut used_volume_offsets = BTreeSet::new();
    let macro_payload = macro_chunk + CHUNK_HEADER_SIZE;
    for label in MESS_LABELS {
        let sequence_id = parse_label_record(data, macro_payload, macro_end, label)?;
        if sequence_id >= sequence_count {
            return Err(BleepPatchError::new(format!(
                "{label} points to out-of-bounds sequence {sequence_id}"
            )));
        }
        if !used_sequence_ids.insert(sequence_id) {
            return Err(BleepPatchError::new(format!(
                "multiple MESS labels point to sequence {sequence_id}"
            )));
        }

        let pointer = pointers[sequence_id];
        if pointer == 0 {
            return Err(BleepPatchError::new(format!(
                "{label} points to empty sequence {sequence_id}"
            )));
        }
        let sequence_offset = sequence_payload + pointer;
        let sequence_boundary =
            next_sequence_boundary(sequence_payload, sequence_end, &pointers, pointer)?;
        if sequence_boundary <= sequence_offset {
            return Err(BleepPatchError::new(format!(
                "invalid bounds for {label} / sequence {sequence_id}"
            )));
        }

        let track = find_unique(
            data,
            b"trk ",
            sequence_offset,
            sequence_boundary,
            &format!("TRK de {label}"),
        )?;
        let track_end = chunk_end(data, track, sequence_boundary, b"trk ")?;
        let track_payload = track + CHUNK_HEADER_SIZE;
        let payload = &data[track_payload..track_end];
        let first_volume_opcode = payload
            .iter()
            .position(|byte| *byte == 0xe0)
            .ok_or_else(|| BleepPatchError::new(format!("{label} has no E0 event")))?;
        if first_volume_opcode + 1 >= payload.len() {
            return Err(BleepPatchError::new(format!(
                "{label} has an incomplete E0 event"
            )));
        }
        let volume_opcode_offset = track_payload + first_volume_opcode;
        let volume_value_offset = volume_opcode_offset + 1;
        let volume = data[volume_value_offset];
        if volume != 0x00 && volume != 0x7f {
            return Err(BleepPatchError::new(format!(
                "first E0 operand for {label} is 0x{volume:02x}; expected 00 or 7f"
            )));
        }
        if !used_volume_offsets.insert(volume_value_offset) {
            return Err(BleepPatchError::new(format!(
                "duplicate volume operand at 0x{volume_value_offset:x}"
            )));
        }

        routes.push(BleepRoute {
            label,
            sequence_id,
            sequence_offset,
            sequence_end: sequence_boundary,
            track_offset: track,
            track_payload_size: track_end - track_payload,
            volume_opcode_offset,
            volume_value_offset,
            volume,
        });
    }

    if routes.len() != 10 || used_volume_offsets.len() != 10 {
        return Err(BleepPatchError::new(
            "could not resolve all ten MESS volume operands",
        ));
    }
    Ok(ParsedBank {
        file_size: data.len(),
        swdl_offset: swdl,
        sedl_offset: sedl,
        sequence_chunk_offset: sequence_chunk,
        macro_chunk_offset: macro_chunk,
        sequence_count,
        routes,
    })
}

/// Preserve file size and demand an all-audible source so applying the patch
/// twice or to an unknown partial modification fails instead of compounding it.
pub fn silence_text_bleeps(data: &[u8]) -> Result<Vec<u8>> {
    let parsed_source = parse(data)?;
    if parsed_source.state() != BleepState::Audible {
        return Err(BleepPatchError::new(format!(
            "source state is {:?}; expected fully audible",
            parsed_source.state()
        )));
    }

    let mut output = data.to_vec();
    for route in &parsed_source.routes {
        if output.get(route.volume_opcode_offset..=route.volume_value_offset) != Some(b"\xe0\x7f") {
            return Err(BleepPatchError::new(format!(
                "unexpected bytes at 0x{:x}",
                route.volume_opcode_offset
            )));
        }
        output[route.volume_value_offset] = 0;
    }

    let changed_offsets = data
        .iter()
        .zip(&output)
        .enumerate()
        .filter_map(|(offset, (before, after))| (before != after).then_some(offset))
        .collect::<Vec<_>>();
    let expected_offsets = parsed_source
        .routes
        .iter()
        .map(|route| route.volume_value_offset)
        .collect::<Vec<_>>();
    if changed_offsets != expected_offsets {
        return Err(BleepPatchError::new(format!(
            "changed offsets {changed_offsets:?}; expected {expected_offsets:?}"
        )));
    }
    if output.len() != data.len() {
        return Err(BleepPatchError::new("silencing changed the file size"));
    }
    let parsed_output = parse(&output)?;
    if parsed_output.state() != BleepState::Silenced {
        return Err(BleepPatchError::new(
            "output does not parse back as fully silenced",
        ));
    }
    Ok(output)
}

/// Keep reset/apply detection strict: a mixed bank is not a known state and
/// should never be accepted as if the patch had completed successfully.
pub fn verify_silenced(data: &[u8]) -> Result<ParsedBank> {
    let parsed = parse(data)?;
    if parsed.state() != BleepState::Silenced {
        return Err(BleepPatchError::new(format!(
            "bank state is {:?}; expected fully silenced",
            parsed.state()
        )));
    }
    Ok(parsed)
}

fn parse_label_record(
    data: &[u8],
    mcrl_payload: usize,
    mcrl_end: usize,
    label: &'static str,
) -> Result<usize> {
    let encoded = label.as_bytes();
    let name_offset = find_unique(
        data,
        encoded,
        mcrl_payload,
        mcrl_end,
        &format!("label MCRL {label}"),
    )?;
    let record_offset = name_offset
        .checked_sub(4)
        .ok_or_else(|| BleepPatchError::new(format!("invalid MCRL record for {label}")))?;
    if record_offset < mcrl_payload || record_offset & 1 != 0 {
        return Err(BleepPatchError::new(format!(
            "misaligned MCRL record for {label}"
        )));
    }
    let record_size = usize::from(read_u16(data, record_offset + 2)?);
    let mut expected_size = 4 + encoded.len() + 1;
    expected_size += expected_size & 1;
    let record_end = record_offset
        .checked_add(record_size)
        .ok_or_else(|| BleepPatchError::new("MCRL record size is too large"))?;
    if record_size != expected_size || record_end > mcrl_end {
        return Err(BleepPatchError::new(format!(
            "invalid MCRL size for {label}: {record_size}; expected {expected_size}"
        )));
    }
    if data.get(name_offset + encoded.len()) != Some(&0) {
        return Err(BleepPatchError::new(format!(
            "MCRL label {label} is not NUL-terminated"
        )));
    }
    let record_name = data
        .get(name_offset..record_end)
        .ok_or_else(|| BleepPatchError::new("MCRL name is out of bounds"))?
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default();
    if record_name != encoded {
        return Err(BleepPatchError::new(format!(
            "MCRL record for {label} contains trailing name bytes"
        )));
    }
    Ok(usize::from(read_u16(data, record_offset)?))
}

fn next_sequence_boundary(
    sequence_payload: usize,
    sequence_chunk_end: usize,
    pointers: &[usize],
    current: usize,
) -> Result<usize> {
    let relative_end = pointers
        .iter()
        .copied()
        .filter(|pointer| *pointer > current)
        .min()
        .unwrap_or(sequence_chunk_end - sequence_payload);
    let absolute_end = sequence_payload
        .checked_add(relative_end)
        .ok_or_else(|| BleepPatchError::new("sequence boundary is too large"))?;
    if absolute_end > sequence_chunk_end {
        return Err(BleepPatchError::new(
            "sequence pointer exceeds the SEQ chunk",
        ));
    }
    Ok(absolute_end)
}

fn find_unique(
    data: &[u8],
    needle: &[u8],
    start: usize,
    end: usize,
    description: &str,
) -> Result<usize> {
    if start > end || end > data.len() || needle.is_empty() {
        return Err(BleepPatchError::new(format!(
            "invalid search range for {description}"
        )));
    }
    let matches = data[start..end]
        .windows(needle.len())
        .enumerate()
        .filter_map(|(offset, value)| (value == needle).then_some(start + offset))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [offset] => Ok(*offset),
        [] => Err(BleepPatchError::new(format!(
            "{description} is missing between 0x{start:x} and 0x{end:x}"
        ))),
        _ => Err(BleepPatchError::new(format!(
            "{description} is ambiguous between 0x{start:x} and 0x{end:x}"
        ))),
    }
}

fn chunk_end(data: &[u8], offset: usize, boundary: usize, name: &[u8; 4]) -> Result<usize> {
    if data.get(offset..offset.saturating_add(4)) != Some(name) {
        return Err(BleepPatchError::new(format!(
            "expected chunk {:?} at 0x{offset:x}",
            String::from_utf8_lossy(name)
        )));
    }
    let end = offset
        .checked_add(CHUNK_HEADER_SIZE)
        .and_then(|value| value.checked_add(read_u32(data, offset + 0x0c).ok()? as usize))
        .ok_or_else(|| BleepPatchError::new("chunk size is too large"))?;
    if end > boundary {
        return Err(BleepPatchError::new(format!(
            "chunk {:?} at 0x{offset:x} exceeds 0x{boundary:x}",
            String::from_utf8_lossy(name)
        )));
    }
    Ok(end)
}

fn decode_sir0_relocations(data: &[u8], offset: usize) -> Result<Vec<usize>> {
    if offset >= data.len() {
        return Err(BleepPatchError::new(
            "SIR0 relocation pointer is out of bounds",
        ));
    }
    let mut locations = Vec::new();
    let mut location = 0usize;
    let mut delta = 0usize;
    for &value in &data[offset..] {
        if value == 0 {
            if delta != 0 {
                return Err(BleepPatchError::new("truncated SIR0 relocation varint"));
            }
            return Ok(locations);
        }
        delta = delta
            .checked_shl(7)
            .and_then(|item| item.checked_add(usize::from(value & 0x7f)))
            .ok_or_else(|| BleepPatchError::new("relocation delta is too large"))?;
        if value & 0x80 == 0 {
            location = location
                .checked_add(delta)
                .ok_or_else(|| BleepPatchError::new("relocation position is too large"))?;
            if location >= data.len() {
                return Err(BleepPatchError::new(
                    "SIR0 relocation field is out of bounds",
                ));
            }
            locations.push(location);
            delta = 0;
        }
    }
    Err(BleepPatchError::new("unterminated SIR0 relocation table"))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| BleepPatchError::new("u16 read offset overflow"))?;
    let bytes: [u8; 2] = data
        .get(offset..end)
        .ok_or_else(|| BleepPatchError::new(format!("u16 is out of bounds at 0x{offset:x}")))?
        .try_into()
        .expect("two-byte slice was validated");
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| BleepPatchError::new("u32 read offset overflow"))?;
    let bytes: [u8; 4] = data
        .get(offset..end)
        .ok_or_else(|| BleepPatchError::new(format!("u32 is out of bounds at 0x{offset:x}")))?
        .try_into()
        .expect("four-byte slice was validated");
    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    #[ignore = "requires private extracted sound data and a generated comparison artifact"]
    fn silenced_bank_matches_python_byte_for_byte() {
        let root = project_root();
        let source = fs::read(root.join("work/romfs/sound/se_sys.se")).unwrap();
        let expected = fs::read(root.join("build/se_sys_silenced.se")).unwrap();

        let parsed = parse(&source).unwrap();
        assert_eq!(parsed.state(), BleepState::Audible);
        assert_eq!(parsed.routes.len(), 10);
        assert!(parsed.routes.iter().enumerate().all(|(index, route)| {
            route.label == format!("SE_SYS_MESS_{index}") && route.volume == 0x7f
        }));

        let generated = silence_text_bleeps(&source).unwrap();
        assert_eq!(generated, expected);
        let verified = verify_silenced(&generated).unwrap();
        assert_eq!(verified.state(), BleepState::Silenced);

        let changed = source
            .iter()
            .zip(&generated)
            .enumerate()
            .filter_map(|(offset, (before, after))| (before != after).then_some(offset))
            .collect::<Vec<_>>();
        assert_eq!(
            changed,
            parsed
                .routes
                .iter()
                .map(|route| route.volume_value_offset)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "requires a generated sound-bank artifact derived from private game data"]
    fn refuses_to_patch_an_already_silenced_bank() {
        let expected = fs::read(project_root().join("build/se_sys_silenced.se")).unwrap();
        assert!(silence_text_bleeps(&expected).is_err());
    }
}
