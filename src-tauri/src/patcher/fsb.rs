use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use thiserror::Error;

const SIMPLE_COMMANDS: &[u8] = &[
    0x01, 0x03, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
    0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25,
    0x27, 0x29, 0x2a, 0x2d, 0x30,
];
const STRING_COMMANDS: &[u8] = &[0x28, 0x2f, 0x31, 0x33, 0x34];
const SHORT_COMMANDS: &[u8] = &[0x2e, 0x32];
const JUMP_COMMANDS: &[u8] = &[0x35, 0x36, 0x37];

#[derive(Debug, Error)]
pub enum FsbError {
    #[error("truncated FSB at offset 0x{0:x}")]
    Truncated(usize),
    #[error("the file is not a SIR0 script")]
    InvalidMagic,
    #[error("invalid FSB offset for {field}: 0x{offset:x}")]
    InvalidOffset { field: &'static str, offset: usize },
    #[error("unterminated FSB string at offset 0x{0:x}")]
    UnterminatedString(usize),
    #[error("invalid FSB string index: {0}")]
    InvalidStringIndex(i16),
    #[error("unknown FSB opcode 0x{command:02x} at offset 0x{offset:x}")]
    UnknownCommand { command: u8, offset: usize },
    #[error("unknown FSB 0x0d sub-opcode 0x{subcommand:02x} at offset 0x{offset:x}")]
    UnknownSubcommand { subcommand: u8, offset: usize },
    #[error("FSB jump targets a non-instruction offset: 0x{0:x}")]
    InvalidJumpTarget(usize),
    #[error("recalculated FSB jump is outside the i16 range")]
    JumpOutOfRange,
    #[error("the script has no setText ordinal {0}")]
    MissingSetText(usize),
    #[error("two voices target the same setText ordinal {0}")]
    DuplicateSetText(usize),
    #[error("invalid voice symbol: {0}")]
    InvalidVoiceSymbol(String),
    #[error("too many FSB strings for an i16 index")]
    TooManyStrings,
    #[error("the rebuilt script cannot be represented in 32 bits")]
    FileTooLarge,
    #[error("the SIR0 relocation list is not strictly increasing")]
    InvalidRelocations,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceTarget {
    pub set_text_ordinal: usize,
    pub symbol: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub name: Vec<u8>,
    pub functions: Vec<Function>,
    pub strings: Vec<Vec<u8>>,
    pub exported_labels: Vec<Vec<u8>>,
    pub global_variables: Vec<Vec<u8>>,
}

/// Compatibility result kept separate so callers cannot silently ignore that
/// a known third-party patcher corruption had to be repaired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatibleParse {
    pub script: Script,
    pub repaired_invalid_pointers: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: Vec<u8>,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Simple(u8),
    Byte {
        command: u8,
        value: u8,
    },
    String {
        command: u8,
        value: Vec<u8>,
    },
    Short {
        command: u8,
        value: i16,
    },
    Jump {
        command: u8,
        target: usize,
    },
    Number(u32),
    F1(Vec<u8>),
    F4 {
        first: Vec<u8>,
        second: Option<Vec<u8>>,
    },
    Terminator(u8),
}

impl Operation {
    pub fn command(&self) -> u8 {
        match self {
            Self::Simple(command)
            | Self::Byte { command, .. }
            | Self::String { command, .. }
            | Self::Short { command, .. }
            | Self::Jump { command, .. }
            | Self::Terminator(command) => *command,
            Self::Number(_) => 0xf0,
            Self::F1(_) => 0xf1,
            Self::F4 { .. } => 0xf4,
        }
    }

    fn encoded_len(&self) -> usize {
        match self {
            Self::Simple(_) | Self::Terminator(_) => 1,
            Self::Byte { .. } => 2,
            Self::String { .. } | Self::Short { .. } | Self::Jump { .. } => 3,
            Self::Number(value) => 2 + leb128_len(*value),
            Self::F1(_) => 4,
            Self::F4 { .. } => 6,
        }
    }
}

#[derive(Debug)]
struct RawOperation {
    start: usize,
    end: usize,
    operation: Operation,
    jump_target: Option<usize>,
}

fn checked_range(data: &[u8], offset: usize, size: usize) -> Result<&[u8], FsbError> {
    data.get(offset..offset.checked_add(size).ok_or(FsbError::FileTooLarge)?)
        .ok_or(FsbError::Truncated(offset))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsbError> {
    let bytes: [u8; 2] = checked_range(data, offset, 2)?.try_into().unwrap();
    Ok(u16::from_le_bytes(bytes))
}

fn read_i16(data: &[u8], offset: usize) -> Result<i16, FsbError> {
    Ok(read_u16(data, offset)? as i16)
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsbError> {
    let bytes: [u8; 4] = checked_range(data, offset, 4)?.try_into().unwrap();
    Ok(u32::from_le_bytes(bytes))
}

fn read_c_string(data: &[u8], offset: usize) -> Result<Vec<u8>, FsbError> {
    if offset >= data.len() {
        return Err(FsbError::InvalidOffset {
            field: "string",
            offset,
        });
    }
    let length = data[offset..]
        .iter()
        .position(|value| *value == 0)
        .ok_or(FsbError::UnterminatedString(offset))?;
    Ok(data[offset..offset + length].to_vec())
}

fn read_offset_table(data: &[u8], offset: usize) -> Result<Vec<usize>, FsbError> {
    if offset >= data.len() {
        return Err(FsbError::InvalidOffset {
            field: "pointer table",
            offset,
        });
    }
    let mut cursor = offset;
    let mut result = Vec::new();
    loop {
        let value = read_u32(data, cursor)? as usize;
        cursor += 4;
        if value == 0 {
            return Ok(result);
        }
        if value >= data.len() {
            return Err(FsbError::InvalidOffset {
                field: "string pointer",
                offset: value,
            });
        }
        result.push(value);
    }
}

fn resolve_string(strings: &[Vec<u8>], index: i16) -> Result<Vec<u8>, FsbError> {
    if index < 0 {
        return Err(FsbError::InvalidStringIndex(index));
    }
    strings
        .get(index as usize)
        .cloned()
        .ok_or(FsbError::InvalidStringIndex(index))
}

fn read_leb128(data: &[u8], cursor: &mut usize) -> Result<u32, FsbError> {
    let mut value = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        let byte = *data.get(*cursor).ok_or(FsbError::Truncated(*cursor))?;
        *cursor += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(FsbError::Truncated(*cursor))
}

fn parse_function(data: &[u8], strings: &[Vec<u8>]) -> Result<Vec<Operation>, FsbError> {
    let mut raw = Vec::new();
    let mut cursor = 0usize;
    while cursor < data.len() {
        let start = cursor;
        let command = data[cursor];
        cursor += 1;
        let mut jump_target = None;
        let operation = if command == 0x26 || command == 0x45 {
            Operation::Terminator(command)
        } else if SIMPLE_COMMANDS.contains(&command) {
            Operation::Simple(command)
        } else if command == 0x2b || command == 0x2c {
            let value = *data.get(cursor).ok_or(FsbError::Truncated(cursor))?;
            cursor += 1;
            Operation::Byte { command, value }
        } else if STRING_COMMANDS.contains(&command) {
            let value = resolve_string(strings, read_i16(data, cursor)?)?;
            cursor += 2;
            Operation::String { command, value }
        } else if SHORT_COMMANDS.contains(&command) {
            let value = read_i16(data, cursor)?;
            cursor += 2;
            Operation::Short { command, value }
        } else if JUMP_COMMANDS.contains(&command) {
            let relative = read_i16(data, cursor)?;
            cursor += 2;
            let target = (cursor as isize)
                .checked_add(isize::from(relative))
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(FsbError::InvalidJumpTarget(cursor))?;
            jump_target = Some(target);
            Operation::Jump { command, target: 0 }
        } else if command == 0x0d {
            let subcommand = *data.get(cursor).ok_or(FsbError::Truncated(cursor))?;
            cursor += 1;
            match subcommand {
                0xf0 => Operation::Number(read_leb128(data, &mut cursor)?),
                0xf1 => {
                    let value = resolve_string(strings, read_i16(data, cursor)?)?;
                    cursor += 2;
                    Operation::F1(value)
                }
                0xf4 => {
                    let first = resolve_string(strings, read_i16(data, cursor)?)?;
                    let second_index = read_i16(data, cursor + 2)?;
                    cursor += 4;
                    let second = if second_index == 0 {
                        None
                    } else {
                        Some(resolve_string(strings, second_index)?)
                    };
                    Operation::F4 { first, second }
                }
                _ => {
                    return Err(FsbError::UnknownSubcommand {
                        subcommand,
                        offset: start,
                    })
                }
            }
        } else {
            return Err(FsbError::UnknownCommand {
                command,
                offset: start,
            });
        };

        let terminal = matches!(operation, Operation::Terminator(_));
        raw.push(RawOperation {
            start,
            end: cursor,
            operation,
            jump_target,
        });
        if terminal {
            break;
        }
    }

    let offsets: HashMap<usize, usize> = raw
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation.start, index))
        .collect();
    let mut operations = Vec::with_capacity(raw.len());
    for item in raw {
        let operation = match (item.operation, item.jump_target) {
            (Operation::Jump { command, .. }, Some(target)) => Operation::Jump {
                command,
                target: *offsets
                    .get(&target)
                    .ok_or(FsbError::InvalidJumpTarget(target))?,
            },
            (operation, None) => operation,
            _ => return Err(FsbError::InvalidJumpTarget(item.end)),
        };
        operations.push(operation);
    }
    Ok(operations)
}

pub fn parse(data: &[u8]) -> Result<Script, FsbError> {
    if data.get(..4) != Some(b"SIR0") {
        return Err(FsbError::InvalidMagic);
    }
    let index_offset = read_u32(data, 4)? as usize;
    if index_offset + 24 > data.len() {
        return Err(FsbError::InvalidOffset {
            field: "index",
            offset: index_offset,
        });
    }
    let name_offset = read_u32(data, index_offset)? as usize;
    let function_table_offset = read_u32(data, index_offset + 4)? as usize;
    let declared_string_count = read_u32(data, index_offset + 8)? as usize;
    let string_table_offset = read_u32(data, index_offset + 12)? as usize;
    let exported_table_offset = read_u32(data, index_offset + 16)? as usize;
    let globals_table_offset = read_u32(data, index_offset + 20)? as usize;

    let string_offsets = read_offset_table(data, string_table_offset)?;
    if string_offsets.len() != declared_string_count {
        return Err(FsbError::InvalidOffset {
            field: "string count",
            offset: string_table_offset,
        });
    }
    let exported_offsets = read_offset_table(data, exported_table_offset)?;
    let global_offsets = read_offset_table(data, globals_table_offset)?;
    let strings = string_offsets
        .iter()
        .map(|offset| read_c_string(data, *offset))
        .collect::<Result<Vec<_>, _>>()?;
    let exported_labels = exported_offsets
        .iter()
        .map(|offset| read_c_string(data, *offset))
        .collect::<Result<Vec<_>, _>>()?;
    let global_variables = global_offsets
        .iter()
        .map(|offset| read_c_string(data, *offset))
        .collect::<Result<Vec<_>, _>>()?;

    let mut entries = Vec::new();
    let mut cursor = function_table_offset;
    loop {
        let code = read_u32(data, cursor)? as usize;
        let name = read_u32(data, cursor + 4)? as usize;
        cursor += 8;
        if code == 0 && name == 0 {
            break;
        }
        if code >= data.len() || name >= data.len() {
            return Err(FsbError::InvalidOffset {
                field: "function",
                offset: code.max(name),
            });
        }
        entries.push((code, name));
    }

    let last_code_end = string_offsets
        .iter()
        .chain(exported_offsets.iter())
        .chain(global_offsets.iter())
        .copied()
        .min()
        .ok_or(FsbError::InvalidOffset {
            field: "first string",
            offset: string_table_offset,
        })?;
    let mut functions = Vec::with_capacity(entries.len());
    for (index, (code_offset, function_name_offset)) in entries.iter().copied().enumerate() {
        let code_end = entries
            .get(index + 1)
            .map(|entry| entry.0)
            .unwrap_or(last_code_end);
        if code_end <= code_offset || code_end > data.len() {
            return Err(FsbError::InvalidOffset {
                field: "function code",
                offset: code_end,
            });
        }
        functions.push(Function {
            name: read_c_string(data, function_name_offset)?,
            operations: parse_function(&data[code_offset..code_end], &strings)?,
        });
    }

    Ok(Script {
        name: read_c_string(data, name_offset)?,
        functions,
        strings,
        exported_labels,
        global_variables,
    })
}

fn decode_relocation_positions(data: &[u8]) -> Result<Vec<usize>, FsbError> {
    let mut cursor = read_u32(data, 8)? as usize;
    if cursor >= data.len() {
        return Err(FsbError::InvalidRelocations);
    }
    let mut absolute = 0usize;
    let mut positions = Vec::new();
    loop {
        let mut delta = 0usize;
        let mut groups = 0usize;
        loop {
            let byte = *data.get(cursor).ok_or(FsbError::InvalidRelocations)?;
            cursor += 1;
            if byte == 0 && groups == 0 {
                return Ok(positions);
            }
            delta = delta
                .checked_mul(0x80)
                .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
                .ok_or(FsbError::InvalidRelocations)?;
            groups += 1;
            if groups > 4 {
                return Err(FsbError::InvalidRelocations);
            }
            if byte & 0x80 == 0 {
                break;
            }
        }
        if delta == 0 {
            return Err(FsbError::InvalidRelocations);
        }
        absolute = absolute
            .checked_add(delta)
            .ok_or(FsbError::InvalidRelocations)?;
        if absolute.checked_add(4).is_none_or(|end| end > data.len()) {
            return Err(FsbError::InvalidRelocations);
        }
        positions.push(absolute);
    }
}

fn original_accent_marker(pair: &[u8]) -> Option<[u8; 2]> {
    let marker = match pair {
        [0x84, 0xbf] => b'0',
        [0x84, 0xc0] => b'1',
        [0x84, 0xc1] => b'2',
        [0x84, 0xc2] => b'3',
        [0x84, 0xc3] => b'4',
        [0x84, 0xc4] => b'5',
        [0x84, 0xc5] => b'6',
        [0x84, 0xc6] => b'7',
        [0x84, 0xc7] => b'8',
        [0x84, 0xc8] => b'9',
        [0x84, 0xc9] => b'D',
        [0x84, 0xca] => b'E',
        [0x84, 0xcb] => b'F',
        [0x84, 0xcc] => b'G',
        [0x84, 0xcd] => b'H',
        [0x84, 0xce] => b'I',
        _ => return None,
    };
    Some([b'~', marker])
}

/// Compatibility recovery is deliberately limited to the French patcher's
/// known whole-binary accent substitution bug. Restricting candidates to SIR0
/// relocation fields and requiring a structural hash prevents this workaround
/// from becoming a general-purpose repair heuristic.
pub fn parse_repairing_invalid_relocated_pointers(
    data: &[u8],
) -> Result<CompatibleParse, FsbError> {
    let initial_error = match parse(data) {
        Ok(script) => {
            return Ok(CompatibleParse {
                script,
                repaired_invalid_pointers: 0,
            })
        }
        Err(
            error @ FsbError::InvalidOffset {
                field: "string pointer",
                ..
            },
        ) => error,
        Err(error) => return Err(error),
    };

    let relocation_offset = read_u32(data, 8)? as usize;
    let positions = decode_relocation_positions(data)?;
    let mut repaired = data.to_vec();
    let mut repaired_invalid_pointers = 0usize;
    for position in positions {
        let current = read_u32(&repaired, position)? as usize;
        if current < data.len() {
            continue;
        }
        let Some(original) = original_accent_marker(&repaired[position..position + 2]) else {
            continue;
        };
        let previous = [repaired[position], repaired[position + 1]];
        repaired[position..position + 2].copy_from_slice(&original);
        let candidate = read_u32(&repaired, position)? as usize;
        if !(0x10..relocation_offset).contains(&candidate) {
            repaired[position..position + 2].copy_from_slice(&previous);
            continue;
        }
        repaired_invalid_pointers += 1;
    }
    if repaired_invalid_pointers == 0 {
        return Err(initial_error);
    }
    Ok(CompatibleParse {
        script: parse(&repaired)?,
        repaired_invalid_pointers,
    })
}

fn is_voice_symbol(symbol: &str) -> bool {
    symbol.len() == 8
        && symbol.starts_with("SE_V")
        && symbol.as_bytes()[4..].iter().all(u8::is_ascii_digit)
}

fn play_sequence(symbol: &str) -> Vec<Operation> {
    let voice = format!(":{symbol}").into_bytes();
    vec![
        Operation::F4 {
            first: b"?Sound".to_vec(),
            second: Some(b"PlaySE".to_vec()),
        },
        Operation::Simple(0x23),
        Operation::F4 {
            first: voice,
            second: None,
        },
        Operation::Number(260_096),
        Operation::Number(0),
        Operation::Simple(0x24),
        Operation::Simple(0x27),
    ]
}

fn wait_sequence(symbol: &str) -> Vec<Operation> {
    let voice = format!(":{symbol}").into_bytes();
    vec![
        Operation::F4 {
            first: b"?Sound".to_vec(),
            second: Some(b"WaitSE".to_vec()),
        },
        Operation::Simple(0x23),
        Operation::F4 {
            first: voice,
            second: None,
        },
        Operation::Simple(0x24),
        Operation::Simple(0x27),
    ]
}

impl Script {
    pub fn set_text_count(&self) -> usize {
        self.functions
            .iter()
            .flat_map(|function| &function.operations)
            .filter(|operation| operation.command() == 0x2f)
            .count()
    }

    pub fn set_text_values(&self) -> Vec<&[u8]> {
        self.functions
            .iter()
            .flat_map(|function| &function.operations)
            .filter_map(|operation| match operation {
                Operation::String {
                    command: 0x2f,
                    value,
                } => Some(value.as_slice()),
                _ => None,
            })
            .collect()
    }

    pub fn structural_sha256(&self) -> String {
        // String contents are intentionally excluded so translated dialogue is
        // accepted, while opcode shape, numeric operands, and jump topology
        // still prove that reviewed voice ordinals refer to the same events.
        let mut hasher = Sha256::new();
        hasher.update((self.functions.len() as u32).to_le_bytes());
        for function in &self.functions {
            hasher.update((function.operations.len() as u32).to_le_bytes());
            for operation in &function.operations {
                hasher.update([operation.command()]);
                match operation {
                    Operation::Simple(_) | Operation::Terminator(_) => hasher.update([0]),
                    Operation::Byte { value, .. } => hasher.update([1, *value]),
                    Operation::String { .. } | Operation::F1(_) => hasher.update([2]),
                    Operation::Short { value, .. } => {
                        hasher.update([3]);
                        hasher.update(value.to_le_bytes());
                    }
                    Operation::Jump { target, .. } => {
                        hasher.update([4]);
                        hasher.update((*target as u32).to_le_bytes());
                    }
                    Operation::Number(value) => {
                        hasher.update([5]);
                        hasher.update(value.to_le_bytes());
                    }
                    Operation::F4 { second, .. } => hasher.update([6, u8::from(second.is_some())]),
                }
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn inject_voices(&mut self, targets: &[VoiceTarget]) -> Result<(), FsbError> {
        let mut by_ordinal = HashMap::new();
        for target in targets {
            if !is_voice_symbol(&target.symbol) {
                return Err(FsbError::InvalidVoiceSymbol(target.symbol.clone()));
            }
            if by_ordinal
                .insert(target.set_text_ordinal, target.symbol.as_str())
                .is_some()
            {
                return Err(FsbError::DuplicateSetText(target.set_text_ordinal));
            }
        }

        let mut found = HashSet::new();
        let mut global_ordinal = 0usize;
        for function in &mut self.functions {
            let old = std::mem::take(&mut function.operations);
            let mut target_for_index = HashMap::new();
            for (index, operation) in old.iter().enumerate() {
                if operation.command() == 0x2f {
                    if let Some(symbol) = by_ordinal.get(&global_ordinal) {
                        target_for_index.insert(index, *symbol);
                        found.insert(global_ordinal);
                    }
                    global_ordinal += 1;
                }
            }

            let mut old_to_new = vec![0usize; old.len()];
            let extra = target_for_index.len() * 12;
            let mut updated = Vec::with_capacity(old.len() + extra);
            for (old_index, operation) in old.iter().cloned().enumerate() {
                if let Some(symbol) = target_for_index.get(&old_index) {
                    old_to_new[old_index] = updated.len();
                    updated.extend(play_sequence(symbol));
                    updated.push(operation);
                    updated.extend(wait_sequence(symbol));
                } else {
                    old_to_new[old_index] = updated.len();
                    updated.push(operation);
                }
            }
            for operation in &mut updated {
                if let Operation::Jump { target, .. } = operation {
                    // Targets are operation indices, so insertion must preserve
                    // control-flow identity instead of retaining stale positions.
                    *target = *old_to_new
                        .get(*target)
                        .ok_or(FsbError::InvalidJumpTarget(*target))?;
                }
            }
            function.operations = updated;
        }

        for ordinal in by_ordinal.keys() {
            if !found.contains(ordinal) {
                return Err(FsbError::MissingSetText(*ordinal));
            }
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, FsbError> {
        let strings = collect_strings(self);
        if strings.len() > i16::MAX as usize {
            return Err(FsbError::TooManyStrings);
        }
        let lookup: HashMap<&[u8], i16> = strings
            .iter()
            .enumerate()
            .map(|(index, value)| (value.as_slice(), index as i16))
            .collect();

        let mut output = vec![0u8; 0x10];
        let mut function_offsets = Vec::with_capacity(self.functions.len());
        for function in &self.functions {
            function_offsets.push(as_u32(output.len())?);
            write_function(&mut output, &function.operations, &lookup)?;
        }
        align(&mut output, 4, 0xaa);

        let string_table = write_strings(&mut output, &strings)?;
        let exported_table = write_strings(&mut output, &self.exported_labels)?;
        let globals_table = write_strings(&mut output, &self.global_variables)?;

        let mut function_name_offsets = Vec::with_capacity(self.functions.len());
        for function in &self.functions {
            function_name_offsets.push(as_u32(output.len())?);
            write_c_string(&mut output, &function.name);
        }
        align(&mut output, 4, 0xaa);
        let function_table = as_u32(output.len())?;
        for (code, name) in function_offsets.iter().zip(function_name_offsets.iter()) {
            output.extend_from_slice(&code.to_le_bytes());
            output.extend_from_slice(&name.to_le_bytes());
        }
        output.extend_from_slice(&[0; 8]);

        let script_name = as_u32(output.len())?;
        write_c_string(&mut output, &self.name);
        align(&mut output, 4, 0xaa);
        let index_offset = as_u32(output.len())?;
        output.extend_from_slice(&script_name.to_le_bytes());
        output.extend_from_slice(&function_table.to_le_bytes());
        output.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        output.extend_from_slice(&string_table.to_le_bytes());
        output.extend_from_slice(&exported_table.to_le_bytes());
        output.extend_from_slice(&globals_table.to_le_bytes());
        align(&mut output, 0x10, 0xaa);

        let relocation_offset = as_u32(output.len())?;
        write_relocations(
            &mut output,
            &strings,
            &self.exported_labels,
            &self.global_variables,
            self.functions.len(),
            string_table as usize,
            exported_table as usize,
            globals_table as usize,
            function_table as usize,
            index_offset as usize,
        )?;
        align(&mut output, 0x10, 0xaa);

        output[..4].copy_from_slice(b"SIR0");
        output[4..8].copy_from_slice(&index_offset.to_le_bytes());
        output[8..12].copy_from_slice(&relocation_offset.to_le_bytes());
        Ok(output)
    }
}

fn collect_strings(script: &Script) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    result.push(Vec::new());
    seen.insert(Vec::new());

    let mut add = |value: &[u8]| {
        if seen.insert(value.to_vec()) {
            result.push(value.to_vec());
        }
    };
    for value in &script.strings {
        add(value);
    }
    for operation in script
        .functions
        .iter()
        .flat_map(|function| &function.operations)
    {
        match operation {
            Operation::String { value, .. } | Operation::F1(value) => add(value),
            Operation::F4 { first, second } => {
                add(first);
                if let Some(second) = second {
                    add(second);
                }
            }
            _ => {}
        }
    }
    result
}

fn write_function(
    output: &mut Vec<u8>,
    operations: &[Operation],
    strings: &HashMap<&[u8], i16>,
) -> Result<(), FsbError> {
    let mut offsets = Vec::with_capacity(operations.len());
    let mut cursor = 0usize;
    for operation in operations {
        offsets.push(cursor);
        cursor = cursor
            .checked_add(operation.encoded_len())
            .ok_or(FsbError::FileTooLarge)?;
    }
    for (index, operation) in operations.iter().enumerate() {
        match operation {
            Operation::Simple(command) | Operation::Terminator(command) => output.push(*command),
            Operation::Byte { command, value } => output.extend_from_slice(&[*command, *value]),
            Operation::String { command, value } => {
                output.push(*command);
                output.extend_from_slice(&string_index(strings, value)?.to_le_bytes());
            }
            Operation::Short { command, value } => {
                output.push(*command);
                output.extend_from_slice(&value.to_le_bytes());
            }
            Operation::Jump { command, target } => {
                let target_offset = *offsets
                    .get(*target)
                    .ok_or(FsbError::InvalidJumpTarget(*target))?;
                let end = offsets[index] + operation.encoded_len();
                let relative =
                    isize::try_from(target_offset).unwrap() - isize::try_from(end).unwrap();
                let relative = i16::try_from(relative).map_err(|_| FsbError::JumpOutOfRange)?;
                output.push(*command);
                output.extend_from_slice(&relative.to_le_bytes());
            }
            Operation::Number(value) => {
                output.extend_from_slice(&[0x0d, 0xf0]);
                write_leb128(output, *value);
            }
            Operation::F1(value) => {
                output.extend_from_slice(&[0x0d, 0xf1]);
                output.extend_from_slice(&string_index(strings, value)?.to_le_bytes());
            }
            Operation::F4 { first, second } => {
                output.extend_from_slice(&[0x0d, 0xf4]);
                output.extend_from_slice(&string_index(strings, first)?.to_le_bytes());
                let second = second
                    .as_ref()
                    .map(|value| string_index(strings, value))
                    .transpose()?
                    .unwrap_or(0);
                output.extend_from_slice(&second.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn string_index(strings: &HashMap<&[u8], i16>, value: &[u8]) -> Result<i16, FsbError> {
    strings.get(value).copied().ok_or(FsbError::TooManyStrings)
}

fn write_strings(output: &mut Vec<u8>, strings: &[Vec<u8>]) -> Result<u32, FsbError> {
    let mut offsets = Vec::with_capacity(strings.len());
    for value in strings {
        offsets.push(as_u32(output.len())?);
        write_c_string(output, value);
    }
    align(output, 4, 0xaa);
    let table_offset = as_u32(output.len())?;
    for offset in offsets {
        output.extend_from_slice(&offset.to_le_bytes());
    }
    output.extend_from_slice(&0u32.to_le_bytes());
    Ok(table_offset)
}

#[allow(clippy::too_many_arguments)]
fn write_relocations(
    output: &mut Vec<u8>,
    strings: &[Vec<u8>],
    exported: &[Vec<u8>],
    globals: &[Vec<u8>],
    function_count: usize,
    string_table: usize,
    exported_table: usize,
    globals_table: usize,
    function_table: usize,
    index_offset: usize,
) -> Result<(), FsbError> {
    output.extend_from_slice(&[4, 4]);
    let mut last = 8usize;
    for (table, count) in [
        (string_table, strings.len()),
        (exported_table, exported.len()),
        (globals_table, globals.len()),
    ] {
        if count > 0 {
            write_relocation_delta(
                output,
                table
                    .checked_sub(last)
                    .ok_or(FsbError::InvalidRelocations)?,
            )?;
            output.extend(std::iter::repeat_n(4u8, count.saturating_sub(1)));
            last = table + count * 4 - 4;
        }
    }
    if function_count > 0 {
        write_relocation_delta(
            output,
            function_table
                .checked_sub(last)
                .ok_or(FsbError::InvalidRelocations)?,
        )?;
        output.extend(std::iter::repeat_n(4u8, function_count * 2 - 1));
        last = function_table + function_count * 8 - 4;
    }
    write_relocation_delta(
        output,
        index_offset
            .checked_sub(last)
            .ok_or(FsbError::InvalidRelocations)?,
    )?;
    output.extend_from_slice(&[4, 8, 4, 4, 0]);
    Ok(())
}

fn write_relocation_delta(output: &mut Vec<u8>, mut value: usize) -> Result<(), FsbError> {
    if value == 0 || value > 0x0fff_ffff {
        return Err(FsbError::InvalidRelocations);
    }
    let mut groups = Vec::new();
    while value > 0 {
        groups.push((value & 0x7f) as u8);
        value >>= 7;
    }
    for index in (0..groups.len()).rev() {
        output.push(groups[index] | if index > 0 { 0x80 } else { 0 });
    }
    Ok(())
}

fn write_c_string(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(value);
    output.push(0);
}

fn align(output: &mut Vec<u8>, alignment: usize, fill: u8) {
    let remainder = output.len() % alignment;
    if remainder != 0 {
        output.resize(output.len() + alignment - remainder, fill);
    }
}

fn as_u32(value: usize) -> Result<u32, FsbError> {
    u32::try_from(value).map_err(|_| FsbError::FileTooLarge)
}

fn leb128_len(mut value: u32) -> usize {
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

fn write_leb128(output: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    #[ignore = "requires private script data extracted from a retail ROM"]
    fn retail_script_round_trips_semantically() {
        let path = project_root().join("work/romfs/scr/a01b.fsb");
        let source = fs::read(path).unwrap();
        let parsed = parse(&source).unwrap();
        let rebuilt = parsed.to_bytes().unwrap();
        assert_eq!(parse(&rebuilt).unwrap(), parsed);
    }

    #[test]
    #[ignore = "requires private extracted scripts and generated voice-injection artifacts"]
    fn injection_matches_compiler_operations_for_real_script() {
        let root = project_root();
        let map =
            fs::read_to_string(root.join("build/voice_map_extended_dialogue_only.tsv")).unwrap();
        let mut lines = map.lines();
        let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let script_column = header
            .iter()
            .position(|value| *value == "ds_script")
            .unwrap();
        let ordinal_column = header
            .iter()
            .position(|value| *value == "ds_settext_ordinal")
            .unwrap();
        let symbol_column = header.iter().position(|value| *value == "symbol").unwrap();
        let targets = lines
            .map(|line| line.split('\t').collect::<Vec<_>>())
            .filter(|row| row[script_column].ends_with("a01b.fsb.txt"))
            .map(|row| VoiceTarget {
                set_text_ordinal: row[ordinal_column].parse().unwrap(),
                symbol: row[symbol_column].to_string(),
            })
            .collect::<Vec<_>>();

        let original = fs::read(root.join("work/romfs/scr/a01b.fsb")).unwrap();
        let expected =
            fs::read(root.join("build/scripts_extended_dialogue_only/a01b.fsb")).unwrap();
        let mut script = parse(&original).unwrap();
        script.inject_voices(&targets).unwrap();
        let generated = parse(&script.to_bytes().unwrap()).unwrap();
        let expected = parse(&expected).unwrap();
        assert_eq!(generated.functions.len(), expected.functions.len());
        for target in &targets {
            let needle = format!(":{}", target.symbol).into_bytes();
            let local_sequence = |script: &Script| {
                script
                    .functions
                    .iter()
                    .find_map(|function| {
                        let voice = function.operations.iter().position(
                            |operation| matches!(operation, Operation::F4 { first, .. } if *first == needle),
                        )?;
                        voice.checked_sub(2).and_then(|start| {
                            function.operations.get(start..start + 13).map(<[_]>::to_vec)
                        })
                    })
                    .unwrap_or_else(|| panic!("missing compiled sequence for {}", target.symbol))
            };
            assert_eq!(
                local_sequence(&generated),
                local_sequence(&expected),
                "injected bytecode differs for {}",
                target.symbol
            );
        }
        assert_eq!(generated.name, expected.name);
        assert_eq!(generated.exported_labels, expected.exported_labels);
        assert_eq!(generated.global_variables, expected.global_variables);
    }

    #[test]
    #[ignore = "requires private script data extracted from a retail ROM"]
    fn structural_hash_ignores_text_bytes() {
        let path = project_root().join("work/romfs/scr/a01b.fsb");
        let mut script = parse(&fs::read(path).unwrap()).unwrap();
        let before = script.structural_sha256();
        let first = script
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.operations)
            .find_map(|operation| match operation {
                Operation::String {
                    command: 0x2f,
                    value,
                } => Some(value),
                _ => None,
            })
            .unwrap();
        *first = b"Texte francais beaucoup plus long".to_vec();
        assert_eq!(script.structural_sha256(), before);
        let rebuilt = parse(&script.to_bytes().unwrap()).unwrap();
        assert_eq!(rebuilt.structural_sha256(), before);
    }

    #[test]
    #[ignore = "requires private scripts produced by the third-party French patcher"]
    fn repairs_only_an_invalid_relocated_french_accent_pointer() {
        let source =
            fs::read(project_root().join("Patcheur_auto_999_DS/tool/new_fsb/a11b.fsb")).unwrap();
        assert!(matches!(
            parse(&source),
            Err(FsbError::InvalidOffset {
                field: "string pointer",
                ..
            })
        ));

        let parsed = parse_repairing_invalid_relocated_pointers(&source).unwrap();
        assert_eq!(parsed.repaired_invalid_pointers, 1);
        assert_eq!(parsed.script.set_text_count(), 182);
        assert_eq!(
            parsed.script.structural_sha256(),
            "5d871cb2d8fe06bf48ce90a1cf086011986ff2b7a31158fa94045a5224ee0553"
        );
    }

    #[test]
    #[ignore = "requires private scripts produced by the third-party French patcher"]
    fn leaves_a_valid_pointer_that_looks_like_an_accent_replacement_untouched() {
        let source =
            fs::read(project_root().join("Patcheur_auto_999_DS/tool/new_fsb/a11.fsb")).unwrap();
        let normally_parsed = parse(&source).unwrap();
        let compatible = parse_repairing_invalid_relocated_pointers(&source).unwrap();
        assert_eq!(compatible.repaired_invalid_pointers, 0);
        assert_eq!(compatible.script, normally_parsed);
    }
}
