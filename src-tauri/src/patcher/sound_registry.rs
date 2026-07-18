//! Strict reader/rebuilder for 999's `etc/sound.dat` SIR0 registry.
//!
//! A full canonical rebuild is safer than growing one pointer table in place:
//! it preserves every original category while giving SIR0 a complete,
//! independently verifiable relocation list for the added voice symbols.

use std::collections::BTreeSet;

use thiserror::Error;

const SIR0_MAGIC: &[u8; 4] = b"SIR0";
const SIR0_HEADER_SIZE: usize = 0x10;
const MAX_RELOCATION_DELTA: usize = 0x0fff_ffff;

pub const DEFAULT_SE_CATEGORY: &str = "SE_CHUN";
pub const MAX_VOICE_SYMBOLS: usize = 10_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid sound.dat registry: {0}")]
pub struct RegistryError(String);

impl RegistryError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    pub pair_offset: usize,
    pub name: Vec<u8>,
    pub table_offset: usize,
    pub entries: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    pub root_offset: usize,
    pub relocation_offset: usize,
    pub categories: Vec<Category>,
    pub relocations: Vec<usize>,
}

/// Reject registries with incomplete relocation metadata because a file that
/// appears readable on one layout may fail when the engine relocates it.
pub fn parse(data: &[u8]) -> Result<Registry> {
    if data.len() < SIR0_HEADER_SIZE || data.get(..4) != Some(SIR0_MAGIC) {
        return Err(RegistryError::new("the file is not a SIR0 container"));
    }

    let root_offset = read_u32(data, 4)? as usize;
    let relocation_offset = read_u32(data, 8)? as usize;
    if root_offset < SIR0_HEADER_SIZE || root_offset >= relocation_offset {
        return Err(RegistryError::new(format!(
            "root is out of bounds: 0x{root_offset:x}"
        )));
    }

    let categories = parse_categories(data, root_offset, relocation_offset)?;
    let relocations = decode_relocations(data, relocation_offset)?;
    let relocation_set: BTreeSet<usize> = relocations.iter().copied().collect();
    let mut required = BTreeSet::from([4, 8]);

    for category in &categories {
        required.insert(category.pair_offset);
        required.insert(category.pair_offset + 4);
        let mut entry_offset = category.table_offset;
        while read_u32(data, entry_offset)? != 0 {
            required.insert(entry_offset);
            entry_offset = entry_offset
                .checked_add(4)
                .ok_or_else(|| RegistryError::new("entry table offset overflow"))?;
        }
    }

    let category_terminator = root_offset
        .checked_add(
            categories
                .len()
                .checked_mul(8)
                .ok_or_else(|| RegistryError::new("category table size overflow"))?,
        )
        .ok_or_else(|| RegistryError::new("category table offset overflow"))?;
    let self_pointer_offset = category_terminator
        .checked_add(4)
        .ok_or_else(|| RegistryError::new("root self-pointer offset overflow"))?;
    if read_u32(data, self_pointer_offset)? as usize == root_offset {
        required.insert(self_pointer_offset);
    }

    let missing: Vec<_> = required.difference(&relocation_set).copied().collect();
    if !missing.is_empty() {
        return Err(RegistryError::new(format!(
            "pointer fields missing from relocations: {}",
            missing
                .iter()
                .take(10)
                .map(|offset| format!("0x{offset:x}"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    for &position in &relocations {
        let pointer = read_u32(data, position)? as usize;
        if position == 8 {
            if pointer != relocation_offset {
                return Err(RegistryError::new(format!(
                    "relocation pointer 0x{pointer:x} does not equal 0x{relocation_offset:x}"
                )));
            }
        } else if pointer >= relocation_offset {
            return Err(RegistryError::new(format!(
                "pointer 0x{pointer:x} at 0x{position:x} reaches the relocation data"
            )));
        }
    }

    Ok(Registry {
        root_offset,
        relocation_offset,
        categories,
        relocations,
    })
}

/// Require one unambiguous target category so a regional registry difference
/// cannot place voice symbols in the wrong engine namespace.
pub fn append_entries<S: AsRef<str>>(
    data: &[u8],
    category_name: &str,
    symbols: &[S],
) -> Result<Vec<u8>> {
    let registry = parse(data)?;
    let matches: Vec<usize> = registry
        .categories
        .iter()
        .enumerate()
        .filter_map(|(index, category)| {
            (category.name.as_slice() == category_name.as_bytes()).then_some(index)
        })
        .collect();
    if matches.len() != 1 {
        return Err(RegistryError::new(format!(
            "expected one {category_name:?} category; found {}",
            matches.len()
        )));
    }
    if symbols.is_empty() {
        return Err(RegistryError::new("no symbols were provided"));
    }

    let symbols: Vec<&str> = symbols.iter().map(AsRef::as_ref).collect();
    validate_new_symbols(&registry.categories, &symbols)?;
    let target_index = matches[0];
    let mut categories = registry.categories;
    categories[target_index]
        .entries
        .extend(symbols.iter().map(|symbol| symbol.as_bytes().to_vec()));

    let output = rebuild_registry(&categories)?;
    let patched = parse(&output)?;
    let patched_category = patched
        .categories
        .iter()
        .find(|category| category.name.as_slice() == category_name.as_bytes())
        .ok_or_else(|| RegistryError::new("target category is missing after rebuild"))?;
    if patched_category.entries != categories[target_index].entries {
        return Err(RegistryError::new("target table differs after rebuild"));
    }
    Ok(output)
}

/// Keep voice symbols contiguous because script ordinals and streamed pack
/// entries intentionally share the same stable numeric index.
pub fn append_voice_symbols(data: &[u8], voice_count: usize) -> Result<Vec<u8>> {
    if voice_count == 0 {
        return Err(RegistryError::new("no voices were provided"));
    }
    if voice_count > MAX_VOICE_SYMBOLS {
        return Err(RegistryError::new(format!(
            "{voice_count} voices exceed the SE_V#### namespace"
        )));
    }
    let symbols = (0..voice_count)
        .map(|index| format!("SE_V{index:04}"))
        .collect::<Vec<_>>();
    append_entries(data, DEFAULT_SE_CATEGORY, &symbols)
}

fn parse_categories(
    data: &[u8],
    root_offset: usize,
    relocation_offset: usize,
) -> Result<Vec<Category>> {
    let mut categories = Vec::new();
    let mut pair_offset = root_offset;
    loop {
        let name_pointer = read_u32(data, pair_offset)? as usize;
        if name_pointer == 0 {
            return Ok(categories);
        }
        let table_offset = read_u32(data, pair_offset + 4)? as usize;
        if table_offset == 0 || table_offset >= relocation_offset {
            return Err(RegistryError::new(format!(
                "invalid table 0x{table_offset:x} at 0x{:x}",
                pair_offset + 4
            )));
        }

        let mut entries = Vec::new();
        let mut entry_offset = table_offset;
        loop {
            let string_pointer = read_u32(data, entry_offset)? as usize;
            if string_pointer == 0 {
                break;
            }
            entries.push(read_c_string(data, string_pointer)?.to_vec());
            entry_offset = entry_offset
                .checked_add(4)
                .ok_or_else(|| RegistryError::new("entry table offset overflow"))?;
            if entry_offset >= relocation_offset {
                return Err(RegistryError::new(format!(
                    "unterminated entry table at 0x{table_offset:x}"
                )));
            }
        }

        categories.push(Category {
            pair_offset,
            name: read_c_string(data, name_pointer)?.to_vec(),
            table_offset,
            entries,
        });
        pair_offset = pair_offset
            .checked_add(8)
            .ok_or_else(|| RegistryError::new("category table offset overflow"))?;
        if pair_offset >= relocation_offset {
            return Err(RegistryError::new("unterminated category table"));
        }
    }
}

fn validate_new_symbols(categories: &[Category], symbols: &[&str]) -> Result<()> {
    let existing: BTreeSet<Vec<u8>> = categories
        .iter()
        .flat_map(|category| &category.entries)
        .map(|entry| entry.iter().map(u8::to_ascii_lowercase).collect())
        .collect();
    let mut seen = BTreeSet::new();
    for &symbol in symbols {
        if symbol.is_empty()
            || !symbol.is_ascii()
            || !symbol
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(RegistryError::new(format!(
                "invalid symbol {symbol:?}; expected uppercase ASCII letters, digits, or '_'"
            )));
        }
        let folded = symbol
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .collect::<Vec<_>>();
        if existing.contains(&folded) || !seen.insert(folded) {
            return Err(RegistryError::new(format!("duplicate symbol: {symbol}")));
        }
        if symbol.len() + ".se".len() > 127 {
            return Err(RegistryError::new(format!(
                "NitroFS name is too long for {symbol}"
            )));
        }
    }
    Ok(())
}

fn rebuild_registry(categories: &[Category]) -> Result<Vec<u8>> {
    if categories.is_empty() {
        return Err(RegistryError::new("cannot serialize an empty registry"));
    }

    let mut output = vec![0; SIR0_HEADER_SIZE];
    let mut string_pointers = Vec::with_capacity(categories.len());
    for category in categories {
        let name_pointer = as_u32(output.len(), "category pointer")?;
        write_c_string(&mut output, &category.name)?;
        let mut entry_pointers = Vec::with_capacity(category.entries.len());
        for entry in &category.entries {
            entry_pointers.push(as_u32(output.len(), "symbol pointer")?);
            write_c_string(&mut output, entry)?;
        }
        string_pointers.push((name_pointer, entry_pointers));
    }
    align(&mut output, 4, 0xaa);

    let mut table_pointers = Vec::with_capacity(categories.len());
    let mut relocation_fields = vec![4usize, 8];
    for (_, entry_pointers) in &string_pointers {
        table_pointers.push(as_u32(output.len(), "symbol table")?);
        for &pointer in entry_pointers {
            relocation_fields.push(output.len());
            output.extend_from_slice(&pointer.to_le_bytes());
        }
        output.extend_from_slice(&0u32.to_le_bytes());
    }

    let root = as_u32(output.len(), "root")?;
    for ((name_pointer, _), table_pointer) in string_pointers.iter().zip(&table_pointers) {
        relocation_fields.extend([output.len(), output.len() + 4]);
        output.extend_from_slice(&name_pointer.to_le_bytes());
        output.extend_from_slice(&table_pointer.to_le_bytes());
    }
    output.extend_from_slice(&0u32.to_le_bytes());
    relocation_fields.push(output.len());
    output.extend_from_slice(&root.to_le_bytes());
    align(&mut output, 16, 0xaa);

    let relocation_offset = as_u32(output.len(), "relocations")?;
    output.extend_from_slice(&encode_relocations(&relocation_fields)?);
    align(&mut output, 16, 0xaa);

    output[..4].copy_from_slice(SIR0_MAGIC);
    write_u32(&mut output, 4, root)?;
    write_u32(&mut output, 8, relocation_offset)?;

    let parsed = parse(&output)?;
    let expected = categories
        .iter()
        .map(|category| (&category.name, &category.entries))
        .collect::<Vec<_>>();
    let actual = parsed
        .categories
        .iter()
        .map(|category| (&category.name, &category.entries))
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(RegistryError::new(
            "serialized registry does not parse back identically",
        ));
    }
    Ok(output)
}

fn decode_relocations(data: &[u8], mut offset: usize) -> Result<Vec<usize>> {
    if offset >= data.len() {
        return Err(RegistryError::new("relocation table is out of bounds"));
    }
    let mut positions = Vec::new();
    let mut absolute = 0usize;
    loop {
        let mut delta = 0usize;
        let mut groups = 0;
        loop {
            let byte = *data
                .get(offset)
                .ok_or_else(|| RegistryError::new("unterminated relocation table"))?;
            offset += 1;
            if byte == 0 && groups == 0 {
                return Ok(positions);
            }
            delta = delta
                .checked_shl(7)
                .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
                .ok_or_else(|| RegistryError::new("relocation delta is too large"))?;
            groups += 1;
            if groups > 4 {
                return Err(RegistryError::new(
                    "relocation delta uses more than four encoded groups",
                ));
            }
            if byte & 0x80 == 0 {
                break;
            }
        }
        if delta == 0 {
            return Err(RegistryError::new(
                "relocation positions are not strictly increasing",
            ));
        }
        absolute = absolute
            .checked_add(delta)
            .ok_or_else(|| RegistryError::new("relocation position is too large"))?;
        if absolute.checked_add(4).is_none_or(|end| end > data.len()) {
            return Err(RegistryError::new(format!(
                "relocation position is out of bounds: 0x{absolute:x}"
            )));
        }
        positions.push(absolute);
    }
}

fn encode_relocations(positions: &[usize]) -> Result<Vec<u8>> {
    let positions: BTreeSet<usize> = positions.iter().copied().collect();
    let mut output = Vec::new();
    let mut previous = 0usize;
    for absolute in positions {
        if absolute <= previous {
            return Err(RegistryError::new(
                "relocation positions are zero or not increasing",
            ));
        }
        let mut delta = absolute - previous;
        if delta > MAX_RELOCATION_DELTA {
            return Err(RegistryError::new(format!(
                "relocation delta is too large: 0x{delta:x}"
            )));
        }
        previous = absolute;

        let mut groups = vec![(delta & 0x7f) as u8];
        delta >>= 7;
        while delta != 0 {
            groups.push((delta & 0x7f) as u8);
            delta >>= 7;
        }
        for (index, group) in groups.iter().rev().enumerate() {
            output.push(*group | (u8::from(index + 1 < groups.len()) * 0x80));
        }
    }
    output.push(0);
    Ok(output)
}

fn read_c_string(data: &[u8], offset: usize) -> Result<&[u8]> {
    let tail = data
        .get(offset..)
        .ok_or_else(|| RegistryError::new(format!("string is out of bounds at 0x{offset:x}")))?;
    let length = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| RegistryError::new(format!("unterminated string at 0x{offset:x}")))?;
    Ok(&tail[..length])
}

fn write_c_string(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    if value.contains(&0) {
        return Err(RegistryError::new("string contains a NUL byte"));
    }
    output.extend_from_slice(value);
    output.push(0);
    Ok(())
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(
            offset..offset.checked_add(4).ok_or_else(|| {
                RegistryError::new(format!("read offset overflow at 0x{offset:x}"))
            })?,
        )
        .ok_or_else(|| RegistryError::new(format!("u32 is out of bounds at 0x{offset:x}")))?
        .try_into()
        .expect("four-byte slice was validated");
    Ok(u32::from_le_bytes(bytes))
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| RegistryError::new("u32 write offset overflow"))?;
    data.get_mut(offset..end)
        .ok_or_else(|| RegistryError::new(format!("u32 write is out of bounds at 0x{offset:x}")))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn as_u32(value: usize, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| RegistryError::new(format!("{field} exceeds 32 bits")))
}

fn align(output: &mut Vec<u8>, alignment: usize, fill: u8) {
    let remainder = output.len() % alignment;
    if remainder != 0 {
        output.resize(output.len() + alignment - remainder, fill);
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
    #[ignore = "requires private extracted sound data and generated registry artifacts"]
    fn extended_dialogue_registry_matches_python_byte_for_byte() {
        let root = project_root();
        let source = fs::read(root.join("work/romfs/etc/sound.dat")).unwrap();
        let expected = fs::read(root.join("build/sound_extended_dialogue_only.dat")).unwrap();
        let names = fs::read_to_string(root.join("build/voice_symbols_extended_dialogue_only.txt"))
            .unwrap();
        let symbols = names.lines().collect::<Vec<_>>();
        assert_eq!(symbols.len(), 6_414);
        assert!(symbols
            .iter()
            .enumerate()
            .all(|(index, symbol)| *symbol == format!("SE_V{index:04}")));

        let generated = append_voice_symbols(&source, symbols.len()).unwrap();
        assert_eq!(generated, expected);

        let parsed = parse(&generated).unwrap();
        let chun = parsed
            .categories
            .iter()
            .find(|category| category.name == DEFAULT_SE_CATEGORY.as_bytes())
            .unwrap();
        assert_eq!(chun.entries.len(), 12 + symbols.len());
    }

    #[test]
    #[ignore = "requires private sound data extracted from a retail ROM"]
    fn rejects_duplicate_or_noncanonical_symbols() {
        let source = fs::read(project_root().join("work/romfs/etc/sound.dat")).unwrap();
        assert!(append_entries(&source, DEFAULT_SE_CATEGORY, &["SE_SYS_MESS_0"]).is_err());
        assert!(append_entries(&source, DEFAULT_SE_CATEGORY, &["se_v0000"]).is_err());
        assert!(append_entries(&source, DEFAULT_SE_CATEGORY, &["SE_V0000", "SE_V0000"]).is_err());
    }
}
