use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

use super::{
    fsb,
    nds::{self, NdsRom, PayloadRole, StreamedFile},
    profile::{ProfileError, VoiceProfile},
    receipt::{self, PatchedLanguage, Receipt, ReceiptError, SIGNATURE_SIZE},
    se_sys, sound_registry,
    voicepack::{Language, VoicePack, VoicePackError},
};

const MAX_ROM_SIZE: u64 = 512 * 1024 * 1024;
const HEADER_CRC_END: usize = 0x15e;
const AUX_SIGNATURE_POINTER: u64 = 0x1000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchLanguage {
    Japanese,
    English,
    French,
}

impl PatchLanguage {
    pub fn code(self) -> &'static str {
        match self {
            Self::Japanese => "jp",
            Self::English => "en",
            Self::French => "fr",
        }
    }

    fn voicepack_language(self) -> Language {
        match self {
            Self::Japanese => Language::Japanese,
            Self::English => Language::English,
            Self::French => Language::French,
        }
    }

    fn receipt_language(self) -> PatchedLanguage {
        match self {
            Self::Japanese => PatchedLanguage::Japanese,
            Self::English => PatchedLanguage::English,
            Self::French => PatchedLanguage::French,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourcePaths {
    pub profile: PathBuf,
    pub japanese_voicepack: PathBuf,
    pub english_voicepack: PathBuf,
}

impl ResourcePaths {
    pub fn voicepack(&self, language: PatchLanguage) -> Option<&Path> {
        match language {
            PatchLanguage::Japanese => Some(&self.japanese_voicepack),
            PatchLanguage::English => Some(&self.english_voicepack),
            PatchLanguage::French => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub language: PatchLanguage,
    pub voicepack_override: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchProgress {
    pub stage: String,
    pub completed: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RomPatchState {
    Clean,
    Japanese,
    English,
    French,
    LegacyVoicePatch,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RomInfo {
    pub path: String,
    pub title: String,
    pub game_code: String,
    pub bytes: u64,
    pub sha256: String,
    pub state: RomPatchState,
    pub compatible: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchResult {
    pub output_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub language: Option<PatchLanguage>,
    pub voices: usize,
    pub scripts: usize,
    pub reset_exact: bool,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid Nintendo DS ROM: {0}")]
    Nds(#[from] nds::NdsError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    VoicePack(#[from] VoicePackError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error("invalid FSB script: {0}")]
    Fsb(#[from] fsb::FsbError),
    #[error(transparent)]
    Registry(#[from] sound_registry::RegistryError),
    #[error(transparent)]
    Bleep(#[from] se_sys::BleepPatchError),
    #[error("the output file must be different from the source ROM")]
    SameInputOutput,
    #[error("the ROM exceeds the Nintendo DS 512 MiB limit")]
    RomTooLarge,
    #[error("the ROM already contains voice files without a restoration receipt; use a clean ROM")]
    LegacyPatch,
    #[error("voice pack language {actual} does not match the requested language {expected}")]
    VoicePackLanguage {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("invalid French voice pack catalogue: {0}")]
    VoicePackCatalog(String),
    #[error("French patches require an explicit voice pack path")]
    FrenchVoicePackRequired,
    #[error("the output file must not replace the {resource} resource")]
    ProtectedOutput { resource: &'static str },
    #[error("payload {path} has an unexpected size ({actual}, expected {expected})")]
    PayloadSize {
        path: String,
        actual: u64,
        expected: u64,
    },
    #[error("the output could not be verified: {0}")]
    Verification(String),
}

fn io_error(path: &Path, source: io::Error) -> EngineError {
    EngineError::Io {
        path: path.to_owned(),
        source,
    }
}

fn emit(
    progress: &mut impl FnMut(PatchProgress),
    stage: &str,
    completed: u64,
    total: u64,
    message: impl Into<String>,
) {
    progress(PatchProgress {
        stage: stage.to_owned(),
        completed,
        total,
        message: message.into(),
    });
}

fn read_file(path: &Path) -> Result<Vec<u8>, EngineError> {
    fs::read(path).map_err(|error| io_error(path, error))
}

fn read_clean_or_restored_base(path: &Path) -> Result<Vec<u8>, EngineError> {
    let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.len() > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    match receipt::locate(path) {
        Ok(located) => {
            if located.receipt.original_len > MAX_ROM_SIZE {
                return Err(EngineError::RomTooLarge);
            }
            Ok(receipt::restore_base(path)?.0)
        }
        Err(ReceiptError::NotPatched) => read_file(path),
        Err(error) => Err(error.into()),
    }
}

fn load_profile(path: &Path) -> Result<VoiceProfile, EngineError> {
    Ok(VoiceProfile::from_json(&read_file(path)?)?)
}

fn sha256_bytes(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> Result<String, EngineError> {
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut hasher = Sha256::new();
    // MSVC gives console entry points a 1 MiB stack, so the streaming buffer
    // must live on the heap for the CLI to remain portable to stock Windows.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn display_title(raw: &[u8; 12]) -> String {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

pub fn inspect_rom(
    path: impl AsRef<Path>,
    resources: &ResourcePaths,
) -> Result<RomInfo, EngineError> {
    let path = path.as_ref();
    let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.len() > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    let located = match receipt::locate(path) {
        Ok(value) => Some(value),
        Err(ReceiptError::NotPatched) => None,
        Err(error) => return Err(error.into()),
    };
    if located
        .as_ref()
        .is_some_and(|value| value.receipt.original_len > MAX_ROM_SIZE)
    {
        return Err(EngineError::RomTooLarge);
    }
    let digest = sha256_file(path)?;
    let base = read_clean_or_restored_base(path)?;
    let parsed = NdsRom::parse(&base);
    let (title, game_code, has_legacy_voice, profile_result) = match parsed {
        Ok(ref rom) => {
            let profile = load_profile(&resources.profile)?;
            (
                display_title(&rom.header().game_title),
                String::from_utf8_lossy(&rom.header().game_code).into_owned(),
                rom.id_for_path("sound/se_v0000.se").is_some(),
                profile.validate_rom(rom),
            )
        }
        Err(error) => {
            return Ok(RomInfo {
                path: path.to_string_lossy().into_owned(),
                title: "Unrecognized file".to_owned(),
                game_code: "----".to_owned(),
                bytes: metadata.len(),
                sha256: digest,
                state: RomPatchState::Unsupported,
                compatible: false,
                detail: error.to_string(),
            })
        }
    };

    let (state, compatible, detail) = if let Some(located) = located {
        let state = match located.receipt.language {
            PatchedLanguage::Japanese => RomPatchState::Japanese,
            PatchedLanguage::English => RomPatchState::English,
            PatchedLanguage::French => RomPatchState::French,
        };
        let compatible = profile_result.is_ok();
        (
            state,
            compatible,
            profile_result.err().map_or_else(
                || "Verified reversible patch".to_owned(),
                |error| error.to_string(),
            ),
        )
    } else if has_legacy_voice {
        (
            RomPatchState::LegacyVoicePatch,
            false,
            "Legacy voice patch detected without a restoration receipt".to_owned(),
        )
    } else {
        match profile_result {
            Ok(()) => (
                RomPatchState::Clean,
                true,
                "Compatible ROM with no voice patch".to_owned(),
            ),
            Err(error) => (RomPatchState::Unsupported, false, error.to_string()),
        }
    };
    Ok(RomInfo {
        path: path.to_string_lossy().into_owned(),
        title,
        game_code,
        bytes: metadata.len(),
        sha256: digest,
        state,
        compatible,
        detail,
    })
}

#[derive(Debug)]
struct Replacement {
    path: String,
    data: Vec<u8>,
}

fn build_replacements(
    rom: &NdsRom<'_>,
    profile: &VoiceProfile,
) -> Result<Vec<Replacement>, EngineError> {
    let mut replacements =
        Vec::with_capacity(profile.scripts.len() + profile.exact_repairs.len() + 2);
    for script_profile in &profile.scripts {
        let original = rom
            .data_for_path(&script_profile.path)
            .ok_or_else(|| ProfileError::MissingScript(script_profile.path.clone()))?;
        let mut script = script_profile.parse_compatible(original)?.script;
        // Text bytes are treated as immutable because translated ROMs must keep
        // their existing script content while receiving only voice opcodes.
        let original_texts = script
            .set_text_values()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        script.inject_voices(&script_profile.targets())?;
        let data = script.to_bytes()?;
        let reparsed = fsb::parse(&data)?;
        if reparsed.set_text_count() != script_profile.set_text_count {
            return Err(EngineError::Verification(format!(
                "script {} did not preserve its {} text entries",
                script_profile.path, script_profile.set_text_count
            )));
        }
        let reparsed_texts = reparsed
            .set_text_values()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        if reparsed_texts != original_texts {
            return Err(EngineError::Verification(format!(
                "script {} did not preserve its text bytes exactly",
                script_profile.path
            )));
        }
        replacements.push(Replacement {
            path: script_profile.path.clone(),
            data,
        });
    }
    for repair_profile in &profile.exact_repairs {
        let original = rom
            .data_for_path(&repair_profile.path)
            .ok_or_else(|| ProfileError::MissingScript(repair_profile.path.clone()))?;
        let repaired = repair_profile.apply(original)?;
        if repaired.repaired_fields != 0 {
            replacements.push(Replacement {
                path: repair_profile.path.clone(),
                data: repaired.data,
            });
        }
    }
    let sound_dat = rom
        .data_for_path("etc/sound.dat")
        .ok_or_else(|| EngineError::Verification("etc/sound.dat is missing".to_owned()))?;
    replacements.push(Replacement {
        path: "etc/sound.dat".to_owned(),
        data: sound_registry::append_voice_symbols(sound_dat, profile.voice_count)?,
    });
    let se_sys_data = rom
        .data_for_path("sound/se_sys.se")
        .ok_or_else(|| EngineError::Verification("sound/se_sys.se is missing".to_owned()))?;
    replacements.push(Replacement {
        path: "sound/se_sys.se".to_owned(),
        data: se_sys::silence_text_bleeps(se_sys_data)?,
    });
    Ok(replacements)
}

fn canonical_output_path(output: &Path) -> Result<PathBuf, EngineError> {
    let absolute = if output.exists() {
        fs::canonicalize(output).map_err(|error| io_error(output, error))?
    } else {
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let parent = fs::canonicalize(parent).map_err(|error| io_error(parent, error))?;
        parent.join(
            output
                .file_name()
                .ok_or_else(|| io_error(output, io::Error::other("missing output filename")))?,
        )
    };
    Ok(absolute)
}

fn ensure_distinct_paths(input: &Path, output: &Path) -> Result<(), EngineError> {
    let input = fs::canonicalize(input).map_err(|error| io_error(input, error))?;
    let output_absolute = canonical_output_path(output)?;
    if input == output_absolute {
        Err(EngineError::SameInputOutput)
    } else {
        Ok(())
    }
}

fn ensure_output_does_not_replace_resource(
    output: &Path,
    resource: &Path,
    label: &'static str,
) -> Result<(), EngineError> {
    let resource = fs::canonicalize(resource).map_err(|error| io_error(resource, error))?;
    if resource == canonical_output_path(output)? {
        Err(EngineError::ProtectedOutput { resource: label })
    } else {
        Ok(())
    }
}

fn pad_to(file: &mut File, path: &Path, cursor: &mut u64, target: u64) -> Result<(), EngineError> {
    if target < *cursor {
        return Err(EngineError::Verification(format!(
            "decreasing offset 0x{target:x} after 0x{:x}",
            *cursor
        )));
    }
    let padding = [0xff_u8; 64 * 1024];
    while *cursor < target {
        let size = usize::try_from((target - *cursor).min(padding.len() as u64)).unwrap();
        file.write_all(&padding[..size])
            .map_err(|error| io_error(path, error))?;
        *cursor += size as u64;
    }
    Ok(())
}

fn write_checked(
    file: &mut File,
    path: &Path,
    cursor: &mut u64,
    data: &[u8],
) -> Result<(), EngineError> {
    file.write_all(data)
        .map_err(|error| io_error(path, error))?;
    *cursor += data.len() as u64;
    Ok(())
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) -> Result<(), EngineError> {
    let target = data
        .get_mut(offset..offset + 4)
        .ok_or_else(|| EngineError::Verification(format!("header write at 0x{offset:x}")))?;
    target.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) -> Result<(), EngineError> {
    let target = data
        .get_mut(offset..offset + 2)
        .ok_or_else(|| EngineError::Verification(format!("header write at 0x{offset:x}")))?;
    target.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn device_capacity_for_len(len: u64) -> Result<u8, EngineError> {
    if len > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    let mut exponent = 17_u32;
    while (1_u64 << exponent) < len {
        exponent += 1;
    }
    Ok((exponent - 17) as u8)
}

fn original_signature(base: &[u8]) -> [u8; SIGNATURE_SIZE as usize] {
    let mut signature = [0xff_u8; SIGNATURE_SIZE as usize];
    if let Some(raw) = base.get(0x80..0x84) {
        let offset = u32::from_le_bytes(raw.try_into().unwrap()) as usize;
        if let Some(found) = base.get(offset..offset + SIGNATURE_SIZE as usize) {
            signature.copy_from_slice(found);
        }
    }
    signature
}

fn voicepack_path<'a>(
    resources: &'a ResourcePaths,
    options: &'a ApplyOptions,
) -> Result<&'a Path, EngineError> {
    options
        .voicepack_override
        .as_deref()
        .or_else(|| resources.voicepack(options.language))
        .ok_or(EngineError::FrenchVoicePackRequired)
}

fn ensure_voicepack_language(expected: PatchLanguage, actual: Language) -> Result<(), EngineError> {
    if actual == expected.voicepack_language() {
        Ok(())
    } else {
        Err(EngineError::VoicePackLanguage {
            expected: expected.code(),
            actual: actual.code(),
        })
    }
}

fn ensure_voicepack_catalog(
    language: PatchLanguage,
    expected: [u8; 32],
    actual: Option<[u8; 32]>,
) -> Result<(), EngineError> {
    if language != PatchLanguage::French {
        return Ok(());
    }
    match actual {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(EngineError::VoicePackCatalog(
            "the French voice pack targets a different voice catalogue".to_owned(),
        )),
        None => Err(EngineError::VoicePackCatalog(
            "the French voice pack is not bound to a voice catalogue".to_owned(),
        )),
    }
}

pub fn apply_patch(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    resources: &ResourcePaths,
    options: &ApplyOptions,
    mut progress: impl FnMut(PatchProgress),
) -> Result<PatchResult, EngineError> {
    let input = input.as_ref();
    let output = output.as_ref();
    ensure_distinct_paths(input, output)?;
    let voicepack_path = voicepack_path(resources, options)?;
    ensure_output_does_not_replace_resource(output, &resources.profile, "voice profile")?;
    ensure_output_does_not_replace_resource(output, voicepack_path, "selected voice pack")?;
    emit(
        &mut progress,
        "read",
        0,
        1,
        "Reading and validating the ROM",
    );
    let base = read_clean_or_restored_base(input)?;
    if base.len() as u64 > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    let rom = NdsRom::parse(&base)?;
    if !rom.header().header_crc_valid() {
        return Err(EngineError::Verification(
            "the source header CRC16 is invalid".to_owned(),
        ));
    }
    if rom.id_for_path("sound/se_v0000.se").is_some() {
        return Err(EngineError::LegacyPatch);
    }
    let profile = load_profile(&resources.profile)?;
    profile.validate_rom(&rom)?;
    let voicepack = VoicePack::open(voicepack_path)?;
    ensure_voicepack_language(options.language, voicepack.language())?;
    if voicepack.entries().len() != profile.voice_count {
        return Err(EngineError::Verification(format!(
            "{} voices in the pack, {} in the profile",
            voicepack.entries().len(),
            profile.voice_count
        )));
    }
    ensure_voicepack_catalog(
        options.language,
        profile.catalog_sha256(),
        voicepack.catalog_sha256(),
    )?;
    emit(
        &mut progress,
        "scripts",
        0,
        profile.scripts.len() as u64,
        "Injecting voice calls",
    );
    let replacements = build_replacements(&rom, &profile)?;
    emit(
        &mut progress,
        "scripts",
        profile.scripts.len() as u64,
        profile.scripts.len() as u64,
        "Scripts and system banks are ready",
    );

    let replacement_specs = replacements
        .iter()
        .map(|replacement| {
            Ok(StreamedFile::new(
                &replacement.path,
                u32::try_from(replacement.data.len()).map_err(|_| EngineError::RomTooLarge)?,
            ))
        })
        .collect::<Result<Vec<_>, EngineError>>()?;
    let voice_specs = voicepack
        .entries()
        .iter()
        .enumerate()
        .map(|(index, entry)| StreamedFile::new(format!("sound/se_v{index:04}.se"), entry.size))
        .collect::<Vec<_>>();
    let plan = rom.plan_sound_voice_append(&voice_specs, &replacement_specs)?;
    let receipt = Receipt::new(&base, options.language.receipt_language())?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    // The destination is replaced only after every structural and restoration
    // check succeeds, so an interrupted patch cannot leave a plausible bad ROM.
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| io_error(parent, error))?;
    let temporary_path = temporary.path().to_owned();
    let file = temporary.as_file_mut();
    file.write_all(&base)
        .map_err(|error| io_error(&temporary_path, error))?;
    let mut cursor = base.len() as u64;

    let total_payload = replacements
        .iter()
        .map(|replacement| replacement.data.len() as u64)
        .sum::<u64>()
        + voicepack.payload_size();
    let mut completed_payload = 0_u64;
    emit(
        &mut progress,
        "write",
        0,
        total_payload,
        "Writing the patched ROM",
    );
    for placement in &plan.payloads {
        pad_to(
            file,
            &temporary_path,
            &mut cursor,
            u64::from(placement.offset),
        )?;
        let before = cursor;
        match placement.role {
            PayloadRole::Replacement => write_checked(
                file,
                &temporary_path,
                &mut cursor,
                &replacements[placement.source_index].data,
            )?,
            PayloadRole::Voice => {
                let written = voicepack.copy_entry_to(placement.source_index, file)?;
                cursor += written;
            }
        }
        let actual = cursor - before;
        if actual != u64::from(placement.size) {
            return Err(EngineError::PayloadSize {
                path: placement.path.clone(),
                actual,
                expected: u64::from(placement.size),
            });
        }
        completed_payload += actual;
        if placement.source_index % 128 == 0 || completed_payload == total_payload {
            emit(
                &mut progress,
                "write",
                completed_payload,
                total_payload,
                format!("Writing {}", placement.path),
            );
        }
    }
    pad_to(
        file,
        &temporary_path,
        &mut cursor,
        u64::from(plan.fnt.offset),
    )?;
    write_checked(file, &temporary_path, &mut cursor, &plan.fnt.bytes)?;
    pad_to(
        file,
        &temporary_path,
        &mut cursor,
        u64::from(plan.fat.offset),
    )?;
    write_checked(file, &temporary_path, &mut cursor, &plan.fat.bytes)?;
    if cursor != plan.final_len {
        return Err(EngineError::Verification(format!(
            "NitroFS ended at 0x{cursor:x}, planned 0x{:x}",
            plan.final_len
        )));
    }

    let mut patched_header = plan.patched_header.clone();
    // The receipt is appended instead of replacing game data so reset can
    // reconstruct even translated inputs byte-for-byte.
    let receipt_offset = Receipt::padded_receipt_offset(cursor);
    pad_to(file, &temporary_path, &mut cursor, receipt_offset)?;
    write_checked(file, &temporary_path, &mut cursor, &receipt.to_bytes())?;
    write_checked(file, &temporary_path, &mut cursor, &receipt.marker())?;
    let signature_offset = cursor;
    if signature_offset & 0x0f != 0 {
        return Err(EngineError::Verification(
            "RSA signature is not aligned".to_owned(),
        ));
    }
    // Keeping the original signature bytes avoids inventing authentication
    // material while retaining the header layout expected by DS tooling.
    write_checked(
        file,
        &temporary_path,
        &mut cursor,
        &original_signature(&base),
    )?;
    if cursor > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    let signature_offset_u32 =
        u32::try_from(signature_offset).map_err(|_| EngineError::RomTooLarge)?;
    patched_header[0x14] = device_capacity_for_len(cursor)?;
    write_u32(&mut patched_header, 0x80, signature_offset_u32)?;
    let crc = nds::crc16_nintendo(&patched_header[..HEADER_CRC_END]);
    write_u16(&mut patched_header, 0x15e, crc)?;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&patched_header))
        .map_err(|error| io_error(&temporary_path, error))?;
    file.seek(SeekFrom::Start(AUX_SIGNATURE_POINTER))
        .and_then(|_| file.write_all(&signature_offset_u32.to_le_bytes()))
        .map_err(|error| io_error(&temporary_path, error))?;
    file.sync_all()
        .map_err(|error| io_error(&temporary_path, error))?;

    emit(
        &mut progress,
        "verify",
        0,
        1,
        "Verifying the receipt and exact restoration",
    );
    let (restored, located) = receipt::restore_base(&temporary_path)?;
    if restored != base {
        return Err(EngineError::Verification(
            "the final restoration differs from the source ROM".to_owned(),
        ));
    }
    if located.signature_offset != signature_offset {
        return Err(EngineError::Verification(format!(
            "signature found at 0x{:x}, expected 0x{signature_offset:x}",
            located.signature_offset
        )));
    }
    if located.file_len != cursor {
        return Err(EngineError::Verification(format!(
            "file size is 0x{:x}, expected 0x{cursor:x}",
            located.file_len
        )));
    }
    let metadata_image = patched_header_and_metadata(&temporary_path, &patched_header, &plan)?;
    NdsRom::parse(&metadata_image).map_err(|error| {
        EngineError::Verification(format!("invalid final NitroFS metadata: {error}"))
    })?;
    temporary
        .persist(output)
        .map_err(|error| io_error(output, error.error))?;
    let digest = sha256_file(output)?;
    emit(&mut progress, "done", 1, 1, "ROM created and verified");
    Ok(PatchResult {
        output_path: output.to_string_lossy().into_owned(),
        bytes: cursor,
        sha256: digest,
        language: Some(options.language),
        voices: profile.voice_count,
        scripts: profile.scripts.len(),
        reset_exact: false,
    })
}

/// Stop at the highest address needed by structural validation because the
/// receipt and trailing alignment data are verified separately and need not be
/// duplicated in this already ROM-sized buffer.
fn patched_header_and_metadata(
    path: &Path,
    patched_header: &[u8],
    plan: &nds::AppendPlan,
) -> Result<Vec<u8>, EngineError> {
    let signature_offset = u32::from_le_bytes(patched_header[0x80..0x84].try_into().unwrap());
    let required = plan
        .final_fat
        .iter()
        .map(|entry| entry.end)
        .chain([
            plan.fnt.end(),
            plan.fat.end(),
            signature_offset.saturating_add(SIGNATURE_SIZE as u32),
        ])
        .max()
        .unwrap_or(plan.fat.end()) as usize;
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut image = vec![0_u8; required];
    file.read_exact(&mut image)
        .map_err(|error| io_error(path, error))?;
    image[..patched_header.len()].copy_from_slice(patched_header);
    Ok(image)
}

pub fn reset_patch(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mut progress: impl FnMut(PatchProgress),
) -> Result<PatchResult, EngineError> {
    let input = input.as_ref();
    let output = output.as_ref();
    ensure_distinct_paths(input, output)?;
    let metadata = fs::metadata(input).map_err(|error| io_error(input, error))?;
    if metadata.len() > MAX_ROM_SIZE {
        return Err(EngineError::RomTooLarge);
    }
    emit(
        &mut progress,
        "read",
        0,
        1,
        "Reading the restoration receipt",
    );
    let (base, located) = receipt::restore_base(input)?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    // Reset uses the same atomic-output rule as patching because a partial
    // restoration would be indistinguishable from a damaged clean ROM.
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| io_error(parent, error))?;
    temporary
        .write_all(&base)
        .map_err(|error| io_error(temporary.path(), error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| io_error(temporary.path(), error))?;
    if Sha256::digest(&base).as_slice() != located.receipt.original_sha256 {
        return Err(ReceiptError::OriginalHashMismatch.into());
    }
    temporary
        .persist(output)
        .map_err(|error| io_error(output, error.error))?;
    emit(&mut progress, "done", 1, 1, "ROM restored byte-for-byte");
    Ok(PatchResult {
        output_path: output.to_string_lossy().into_owned(),
        bytes: base.len() as u64,
        sha256: sha256_bytes(&base),
        language: None,
        voices: 0,
        scripts: 0,
        reset_exact: true,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn resources() -> ResourcePaths {
        let resource_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
        ResourcePaths {
            profile: resource_dir.join("voice-profile.json"),
            japanese_voicepack: resource_dir.join("voices-jp.nvpack"),
            english_voicepack: resource_dir.join("voices-en.nvpack"),
        }
    }

    #[test]
    fn french_requires_an_explicit_pack_and_validates_its_language() {
        let resources = resources();
        let missing = ApplyOptions {
            language: PatchLanguage::French,
            voicepack_override: None,
        };
        assert!(matches!(
            voicepack_path(&resources, &missing),
            Err(EngineError::FrenchVoicePackRequired)
        ));

        let custom_pack = PathBuf::from("studio/exports/voices-fr.nvpack");
        let explicit = ApplyOptions {
            language: PatchLanguage::French,
            voicepack_override: Some(custom_pack.clone()),
        };
        assert_eq!(voicepack_path(&resources, &explicit).unwrap(), custom_pack);
        assert!(ensure_voicepack_language(PatchLanguage::French, Language::French).is_ok());
        assert!(matches!(
            ensure_voicepack_language(PatchLanguage::French, Language::English),
            Err(EngineError::VoicePackLanguage {
                expected: "fr",
                actual: "en"
            })
        ));
        let catalog = [0x42; 32];
        assert!(ensure_voicepack_catalog(PatchLanguage::French, catalog, Some(catalog)).is_ok());
        assert!(ensure_voicepack_catalog(PatchLanguage::Japanese, catalog, None).is_ok());
        assert!(matches!(
            ensure_voicepack_catalog(PatchLanguage::French, catalog, None),
            Err(EngineError::VoicePackCatalog(_))
        ));
        assert!(matches!(
            ensure_voicepack_catalog(PatchLanguage::French, catalog, Some([0x24; 32])),
            Err(EngineError::VoicePackCatalog(_))
        ));
    }

    #[test]
    fn patch_output_cannot_replace_a_required_resource() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("voice-profile.json");
        let voicepack = directory.path().join("voices-fr.nvpack");
        fs::write(&output, b"profile").unwrap();
        fs::write(&voicepack, b"pack").unwrap();

        assert!(matches!(
            ensure_output_does_not_replace_resource(&output, &output, "voice profile"),
            Err(EngineError::ProtectedOutput {
                resource: "voice profile"
            })
        ));
        assert!(matches!(
            ensure_output_does_not_replace_resource(&voicepack, &voicepack, "selected voice pack"),
            Err(EngineError::ProtectedOutput {
                resource: "selected voice pack"
            })
        ));
    }

    #[test]
    fn rom_operations_reject_oversized_physical_files_before_reading_them() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("oversized.nds");
        let output = directory.path().join("restored.nds");
        File::create(&input)
            .unwrap()
            .set_len(MAX_ROM_SIZE + 1)
            .unwrap();

        assert!(matches!(
            inspect_rom(&input, &resources()),
            Err(EngineError::RomTooLarge)
        ));
        assert!(matches!(
            reset_patch(&input, &output, |_| {}),
            Err(EngineError::RomTooLarge)
        ));
    }

    #[test]
    #[ignore = "requires a private retail ROM fixture"]
    fn inspects_stock_rom_as_clean_and_compatible() {
        let input = root().join("original/Nine Hours, Nine Persons, Nine Doors (USA).nds");
        let info = inspect_rom(input, &resources()).unwrap();
        assert_eq!(info.state, RomPatchState::Clean);
        assert!(info.compatible);
        assert_eq!(info.game_code, "BSKE");
    }

    fn assert_french_texts_and_exact_repairs_preserved(
        base_data: &[u8],
        patched_data: &[u8],
        profile: &VoiceProfile,
    ) {
        let base = NdsRom::parse(base_data).unwrap();
        let patched = NdsRom::parse(patched_data).unwrap();
        let mut checked_texts = 0usize;
        for expected in &profile.scripts {
            let before = expected
                .parse_compatible(base.data_for_path(&expected.path).unwrap())
                .unwrap()
                .script;
            let after = fsb::parse(patched.data_for_path(&expected.path).unwrap()).unwrap();
            let before_texts = before
                .set_text_values()
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>();
            let after_texts = after
                .set_text_values()
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>();
            checked_texts += before_texts.len();
            assert_eq!(
                after_texts, before_texts,
                "text mismatch in {}",
                expected.path
            );
        }
        assert_eq!(checked_texts, 14_610);

        let mut repaired_fields = 0usize;
        for repair in &profile.exact_repairs {
            let expected = repair
                .apply(base.data_for_path(&repair.path).unwrap())
                .unwrap();
            repaired_fields += expected.repaired_fields;
            assert_eq!(
                patched.data_for_path(&repair.path).unwrap(),
                expected.data,
                "exact repair mismatch in {}",
                repair.path
            );
        }
        assert_eq!(repaired_fields, 5);
    }

    #[test]
    #[ignore = "requires a private ROM generated by the French patcher"]
    fn french_patcher_raw_repairs_all_seventeen_pointer_fields() {
        let input = root().join("build/french-patcher-validation/french-patcher-as-is.nds");
        let data = fs::read(input).unwrap();
        let rom = NdsRom::parse(&data).unwrap();
        let profile = load_profile(&resources().profile).unwrap();
        profile.validate_rom(&rom).unwrap();

        let repaired_voice_fields = profile
            .scripts
            .iter()
            .map(|expected| {
                expected
                    .parse_compatible(rom.data_for_path(&expected.path).unwrap())
                    .unwrap()
                    .repaired_invalid_pointers
            })
            .sum::<usize>();
        assert_eq!(repaired_voice_fields, 12);

        let replacements = build_replacements(&rom, &profile).unwrap();
        let mut repaired_exact_fields = 0usize;
        for repair in &profile.exact_repairs {
            let replacement = replacements
                .iter()
                .find(|replacement| replacement.path == repair.path)
                .unwrap();
            let expected = repair
                .apply(rom.data_for_path(&repair.path).unwrap())
                .unwrap();
            repaired_exact_fields += expected.repaired_fields;
            assert_eq!(replacement.data, expected.data);
        }
        assert_eq!(repaired_exact_fields, 5);
        assert_eq!(repaired_voice_fields + repaired_exact_fields, 17);
        assert_eq!(replacements.len(), 51 + 5 + 2);
    }

    #[test]
    #[ignore = "requires a private French-patched ROM and proprietary production voice packs"]
    fn french_patcher_raw_japanese_and_english_preserve_text_and_reset_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let input = root().join("build/french-patcher-validation/french-patcher-as-is.nds");
        let base_data = fs::read(&input).unwrap();
        let profile = load_profile(&resources().profile).unwrap();
        let info = inspect_rom(&input, &resources()).unwrap();
        assert_eq!(info.state, RomPatchState::Clean);
        assert!(info.compatible);

        for language in [PatchLanguage::Japanese, PatchLanguage::English] {
            let patched = directory.path().join(format!("{}.nds", language.code()));
            let reset = directory
                .path()
                .join(format!("{}-reset.nds", language.code()));
            let result = apply_patch(
                &input,
                &patched,
                &resources(),
                &ApplyOptions {
                    language,
                    voicepack_override: None,
                },
                |_| {},
            )
            .unwrap();
            assert_eq!(result.voices, 6_475);
            let patched_data = fs::read(&patched).unwrap();
            assert_french_texts_and_exact_repairs_preserved(&base_data, &patched_data, &profile);
            let patched_info = inspect_rom(&patched, &resources()).unwrap();
            assert_eq!(
                patched_info.state,
                match language {
                    PatchLanguage::Japanese => RomPatchState::Japanese,
                    PatchLanguage::English => RomPatchState::English,
                    PatchLanguage::French => RomPatchState::French,
                }
            );
            assert!(patched_info.compatible);
            let reset_result = reset_patch(&patched, &reset, |_| {}).unwrap();
            assert!(reset_result.reset_exact);
            assert_eq!(fs::read(reset).unwrap(), base_data);
        }
    }

    #[test]
    #[ignore = "requires a private retail ROM fixture and production voice pack"]
    fn production_japanese_apply_and_exact_reset() {
        let directory = tempfile::tempdir().unwrap();
        let input = root().join("original/Nine Hours, Nine Persons, Nine Doors (USA).nds");
        let patched = directory.path().join("jp.nds");
        let reset = directory.path().join("reset.nds");
        let result = apply_patch(
            &input,
            &patched,
            &resources(),
            &ApplyOptions {
                language: PatchLanguage::Japanese,
                voicepack_override: None,
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(result.voices, 6_475);
        assert_eq!(
            inspect_rom(&patched, &resources()).unwrap().state,
            RomPatchState::Japanese
        );
        let reset_result = reset_patch(&patched, &reset, |_| {}).unwrap();
        assert!(reset_result.reset_exact);
        assert_eq!(fs::read(reset).unwrap(), fs::read(input).unwrap());
    }
}
