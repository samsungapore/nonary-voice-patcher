use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::{self, Cursor, Read},
    path::{Component, Path, PathBuf},
};

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder as TempBuilder;
use thiserror::Error;

use super::project::{self, ProjectError, TakeMetadata, TargetProgress, TargetStatus};
use crate::patcher::{
    engine::{self, ApplyOptions, EngineError, PatchLanguage, PatchResult, ResourcePaths},
    nds::{self, NdsRom},
    profile::{ProfileError, VoiceProfile},
    receipt::{self, ReceiptError},
    voicepack::{Language, VoicePack, VoicePackError},
};

const SAMPLE_RATE: u32 = 16_384;
const TEMPLATE_PATH: &str = "sound/se_a01b_wake.se";
const TEMPLATE_INTERNAL_ID: u16 = 0x6604;
const PREVIEW_SILENCE_MS: u64 = 80;
const MAX_DURATION_MS: u64 = 45_000;
const MAX_WAV_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ROM_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SAMPLE_RATE: u32 = 384_000;
const FIRST_ID_HIGH: u16 = 0xC8;
const LAST_ID_HIGH: u16 = 0x64;
const FIRST_ID_LOW: u16 = 0x01;
const LAST_ID_LOW: u16 = 0x65;

const SIR0_HEADER_SIZE: usize = 0x10;
const SWDL_HEADER_SIZE: usize = 0x50;
const CHUNK_HEADER_SIZE: usize = 0x10;
const WAVI_ENTRY_SIZE: usize = 0x40;
const ADPCM_PREAMBLE_SIZE: usize = 4;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildScope {
    #[default]
    Preview,
    Production,
}

impl BuildScope {
    fn filename(self) -> &'static str {
        match self {
            Self::Preview => "voices-fr-preview.nvpack",
            Self::Production => "voices-fr-production.nvpack",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingBuildProgress {
    pub stage: String,
    pub completed: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingBuildReport {
    pub scope: BuildScope,
    pub pack_path: String,
    pub pack_sha256: String,
    pub catalog_sha256: String,
    pub build_input_sha256: String,
    pub source_rom_sha256: String,
    pub profile_sha256: String,
    pub voices: usize,
    pub recorded: usize,
    pub approved: usize,
    pub silent_preview_entries: usize,
    pub payload_bytes: u64,
    pub duration_ms: u64,
    pub clipped_samples: u64,
    pub first_internal_id: String,
    pub last_internal_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRomBuildResult {
    pub voice_pack: DubbingBuildReport,
    pub rom: PatchResult,
}

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("invalid Nintendo DS ROM: {0}")]
    Nds(#[from] nds::NdsError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error(transparent)]
    VoicePack(#[from] VoicePackError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("invalid WAV master {path}: {message}")]
    Wav { path: PathBuf, message: String },
    #[error("invalid voice resource: {0}")]
    Voice(String),
    #[error("production requires all {total} targets to have an approved active take; {missing} are missing or unapproved (first: {first})")]
    ProductionIncomplete {
        total: usize,
        missing: usize,
        first: String,
    },
    #[error("invalid dubbing build: {0}")]
    Invalid(String),
}

fn io_error(path: &Path, source: io::Error) -> BuildError {
    BuildError::Io {
        path: path.to_owned(),
        source,
    }
}

fn emit(
    progress: &mut impl FnMut(DubbingBuildProgress),
    stage: &str,
    completed: u64,
    total: u64,
    message: impl Into<String>,
) {
    progress(DubbingBuildProgress {
        stage: stage.to_owned(),
        completed,
        total,
        message: message.into(),
    });
}

fn read_limited_regular_file(path: &Path, limit: u64) -> Result<Vec<u8>, BuildError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BuildError::Invalid(format!(
            "{} must be a regular file, not a symlink",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(BuildError::Invalid(format!(
            "{} exceeds the {limit} byte safety limit",
            path.display()
        )));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| BuildError::Invalid(format!("{} is too large", path.display())))?;
    let mut data = Vec::with_capacity(capacity);
    File::open(path)
        .map_err(|error| io_error(path, error))?
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| io_error(path, error))?;
    if data.len() as u64 > limit {
        return Err(BuildError::Invalid(format!(
            "{} grew beyond the {limit} byte safety limit while being read",
            path.display()
        )));
    }
    Ok(data)
}

fn read_clean_or_restored_base(path: &Path) -> Result<Vec<u8>, BuildError> {
    match receipt::locate(path) {
        Ok(located) => {
            if located.receipt.original_len > MAX_ROM_BYTES {
                return Err(BuildError::Invalid(
                    "source ROM exceeds the Nintendo DS 512 MiB limit".to_owned(),
                ));
            }
            Ok(receipt::restore_base(path)?.0)
        }
        Err(ReceiptError::NotPatched) => read_limited_regular_file(path, MAX_ROM_BYTES),
        Err(error) => Err(error.into()),
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn active_take(target: &TargetProgress) -> Option<&TakeMetadata> {
    let active = target.active_take.as_deref()?;
    target.takes.iter().find(|take| take.id == active)
}

fn safe_take_path(project_dir: &Path, relative: &str) -> Result<PathBuf, BuildError> {
    let relative_path = Path::new(relative);
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || !relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(BuildError::Invalid(format!(
            "take path is not a safe project-relative path: {relative:?}"
        )));
    }
    let root = project_dir
        .canonicalize()
        .map_err(|error| io_error(project_dir, error))?;
    let candidate = project_dir.join(relative_path);
    let resolved = candidate
        .canonicalize()
        .map_err(|error| io_error(&candidate, error))?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(BuildError::Invalid(format!(
            "take path escapes the project: {relative:?}"
        )));
    }
    Ok(resolved)
}

fn absolute_destination(path: &Path) -> Result<PathBuf, BuildError> {
    if path.exists() {
        return path.canonicalize().map_err(|error| io_error(path, error));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|error| io_error(parent, error))?;
    let filename = path
        .file_name()
        .ok_or_else(|| BuildError::Invalid("output path has no filename".to_owned()))?;
    Ok(parent.join(filename))
}

fn validate_output_destination(
    project_dir: &Path,
    rom_path: &Path,
    profile_path: &Path,
    output: &Path,
) -> Result<(), BuildError> {
    let output = absolute_destination(output)?;
    let project = project_dir
        .canonicalize()
        .map_err(|error| io_error(project_dir, error))?;
    let recordings = project.join("recordings");
    if output.starts_with(&recordings) {
        return Err(BuildError::Invalid(
            "build output cannot be placed inside project recordings".to_owned(),
        ));
    }
    let mut protected = vec![
        rom_path
            .canonicalize()
            .map_err(|error| io_error(rom_path, error))?,
        profile_path
            .canonicalize()
            .map_err(|error| io_error(profile_path, error))?,
        project.join("project.nvdub.json"),
        project.join(project::PROJECT_LOCK_FILE),
    ];
    if recordings.exists() {
        protected.push(
            recordings
                .canonicalize()
                .map_err(|error| io_error(&recordings, error))?,
        );
    }
    if protected.contains(&output) {
        return Err(BuildError::Invalid(
            "build output collides with a project input".to_owned(),
        ));
    }
    Ok(())
}

fn safe_builds_directory(project_dir: &Path) -> Result<PathBuf, BuildError> {
    let project = project_dir
        .canonicalize()
        .map_err(|error| io_error(project_dir, error))?;
    let builds = project_dir.join("builds");
    match fs::symlink_metadata(&builds) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(BuildError::Invalid(
                "project builds must be a real directory, not a symlink".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&builds).map_err(|error| io_error(&builds, error))?;
        }
        Err(error) => return Err(io_error(&builds, error)),
    }
    let resolved = builds
        .canonicalize()
        .map_err(|error| io_error(&builds, error))?;
    if resolved.parent() != Some(project.as_path()) {
        return Err(BuildError::Invalid(
            "project builds resolves outside the project root".to_owned(),
        ));
    }
    Ok(resolved)
}

struct AudioConverter {
    resamplers: HashMap<u32, SincFixedIn<f64>>,
}

impl AudioConverter {
    fn new() -> Self {
        Self {
            resamplers: HashMap::new(),
        }
    }

    fn resample(&mut self, input: &[f64], input_rate: u32) -> Result<Vec<f64>, BuildError> {
        if input_rate == SAMPLE_RATE {
            return Ok(input.to_vec());
        }
        let parameters = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let resampler = match self.resamplers.entry(input_rate) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
                SincFixedIn::<f64>::new(
                    SAMPLE_RATE as f64 / input_rate as f64,
                    1.0,
                    parameters,
                    1024,
                    1,
                )
                .map_err(|error| {
                    BuildError::Invalid(format!("cannot create resampler: {error}"))
                })?,
            ),
        };
        resampler.reset();
        let output_length =
            ((input.len() as f64 * SAMPLE_RATE as f64 / input_rate as f64).round()) as usize;
        let delay = resampler.output_delay();
        let mut remaining = input;
        let mut output = Vec::with_capacity(output_length + delay + 1024);

        while remaining.len() >= resampler.input_frames_next() {
            let input_frames = resampler.input_frames_next();
            let channels = [&remaining[..input_frames]];
            let converted = resampler.process(&channels, None).map_err(|error| {
                BuildError::Invalid(format!("audio resampling failed: {error}"))
            })?;
            output.extend_from_slice(&converted[0]);
            remaining = &remaining[input_frames..];
        }
        if !remaining.is_empty() {
            let channels = [remaining];
            let converted = resampler
                .process_partial(Some(&channels), None)
                .map_err(|error| {
                    BuildError::Invalid(format!("audio resampling failed: {error}"))
                })?;
            output.extend_from_slice(&converted[0]);
        }
        while output.len() < output_length + delay {
            let converted = resampler
                .process_partial::<&[f64]>(None, None)
                .map_err(|error| {
                    BuildError::Invalid(format!("audio resampling failed: {error}"))
                })?;
            if converted[0].is_empty() {
                return Err(BuildError::Invalid(
                    "audio resampler stopped before producing the requested duration".to_owned(),
                ));
            }
            output.extend_from_slice(&converted[0]);
        }
        Ok(output[delay..delay + output_length].to_vec())
    }

    fn decode_master(
        &mut self,
        path: &Path,
        take: &TakeMetadata,
        target: &TargetProgress,
    ) -> Result<(Vec<i16>, u64), BuildError> {
        let source = read_limited_regular_file(path, MAX_WAV_BYTES)?;
        if source.is_empty() {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: "file is empty".to_owned(),
            });
        }
        let actual_sha256 = Sha256::digest(&source)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual_sha256 != take.sha256 {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: "WAV changed after build preflight".to_owned(),
            });
        }
        let mut reader =
            hound::WavReader::new(Cursor::new(source)).map_err(|error| BuildError::Wav {
                path: path.to_owned(),
                message: error.to_string(),
            })?;
        let spec = reader.spec();
        if spec.channels != 1
            || spec.bits_per_sample != 16
            || spec.sample_format != hound::SampleFormat::Int
            || spec.sample_rate == 0
        {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: "expected mono 16-bit PCM".to_owned(),
            });
        }
        if spec.sample_rate != take.sample_rate {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: "sample rate differs from take metadata".to_owned(),
            });
        }
        let samples = reader
            .samples::<i16>()
            .map(|sample| {
                sample
                    .map(|value| f64::from(value) / 32768.0)
                    .map_err(|error| BuildError::Wav {
                        path: path.to_owned(),
                        message: error.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if samples.len() as u64 != take.frames || samples.is_empty() {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: format!(
                    "contains {} frames instead of the recorded {}",
                    samples.len(),
                    take.frames
                ),
            });
        }
        let converted = self.resample(&samples, spec.sample_rate)?;
        let trim_start = usize::try_from(
            target
                .trim_start_ms
                .saturating_mul(u64::from(SAMPLE_RATE))
                .saturating_add(500)
                / 1_000,
        )
        .map_err(|_| BuildError::Invalid("trim offset exceeds this platform".to_owned()))?;
        let trim_end = usize::try_from(
            target
                .trim_end_ms
                .saturating_mul(u64::from(SAMPLE_RATE))
                .saturating_add(500)
                / 1_000,
        )
        .map_err(|_| BuildError::Invalid("trim offset exceeds this platform".to_owned()))?;
        if trim_start
            .checked_add(trim_end)
            .is_none_or(|trimmed| trimmed >= converted.len())
        {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: "trim range removes the complete recording".to_owned(),
            });
        }
        if !target.gain_db.is_finite() || !(-60.0..=24.0).contains(&target.gain_db) {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: format!("gain {} dB is outside -60..24 dB", target.gain_db),
            });
        }
        let end = converted.len() - trim_end;
        let gain = 10_f64.powf(f64::from(target.gain_db) / 20.0);
        let gained = converted[trim_start..end]
            .iter()
            .map(|sample| sample * gain)
            .collect::<Vec<_>>();
        let clipped_samples = gained
            .iter()
            .filter(|sample| sample.abs() > 0.999_969)
            .count() as u64;
        let result = gained
            .iter()
            .map(|sample| sample.clamp(-0.999_969, 0.999_969) * f64::from(i16::MAX))
            .map(|sample| sample.round_ties_even() as i16)
            .collect::<Vec<_>>();
        let duration_ms = result.len() as u64 * 1_000 / u64::from(SAMPLE_RATE);
        if result.is_empty() || duration_ms > MAX_DURATION_MS {
            return Err(BuildError::Wav {
                path: path.to_owned(),
                message: format!("duration after trimming is {duration_ms} ms"),
            });
        }
        Ok((result, clipped_samples))
    }
}

fn read_dse_internal_id(data: &[u8], source: &str) -> Result<u16, BuildError> {
    if data.len() < SIR0_HEADER_SIZE || data.get(..4) != Some(b"SIR0") {
        return Err(BuildError::Voice(format!(
            "{source} is not a SIR0 resource"
        )));
    }
    let subheader = usize::try_from(u32_at(data, 4)?)
        .map_err(|_| BuildError::Voice(format!("{source} subheader is too large")))?;
    let relocations = usize::try_from(u32_at(data, 8)?)
        .map_err(|_| BuildError::Voice(format!("{source} relocation offset is too large")))?;
    if !(SIR0_HEADER_SIZE <= subheader
        && subheader
            .checked_add(8)
            .is_some_and(|end| end <= relocations)
        && relocations < data.len())
    {
        return Err(BuildError::Voice(format!(
            "{source} has invalid SIR0 header pointers"
        )));
    }
    let swdl = u32_at_usize(data, subheader)?;
    let sedl = u32_at_usize(data, subheader + 4)?;
    if !(SIR0_HEADER_SIZE <= swdl && swdl < sedl && sedl < subheader)
        || data.get(swdl..swdl + 4) != Some(b"swdl")
        || data.get(sedl..sedl + 4) != Some(b"sedl")
    {
        return Err(BuildError::Voice(format!(
            "{source} has invalid SWDL/SEDL pointers"
        )));
    }
    let swdl_size = u32_at_usize(data, swdl + 8)?;
    let sedl_size = u32_at_usize(data, sedl + 8)?;
    if swdl_size < SWDL_HEADER_SIZE
        || swdl.checked_add(swdl_size).is_none_or(|end| end > sedl)
        || sedl_size < 0x30
        || sedl
            .checked_add(sedl_size)
            .is_none_or(|end| end > subheader)
        || decode_sir0_relocations(data, relocations)? != [4, 8, subheader, subheader + 4]
    {
        return Err(BuildError::Voice(format!(
            "{source} has an invalid DSE layout"
        )));
    }
    let swdl_id = u16_at(data, swdl + 0x0e)?;
    let sedl_id = u16_at(data, sedl + 0x0e)?;
    if swdl_id != sedl_id {
        return Err(BuildError::Voice(format!(
            "{source} has mismatched SWDL/SEDL IDs"
        )));
    }
    Ok(swdl_id)
}

fn collect_retail_internal_ids(rom: &NdsRom<'_>) -> Result<BTreeSet<u16>, BuildError> {
    let mut identifiers = BTreeSet::new();
    let mut count = 0_usize;
    for file in rom.files() {
        let Some(filename) = file.path.strip_prefix("sound/") else {
            continue;
        };
        if filename.contains('/') || !filename.to_ascii_lowercase().ends_with(".se") {
            continue;
        }
        let data = rom
            .data_for_id(file.id)
            .ok_or_else(|| BuildError::Voice(format!("{} has no FAT payload", file.path)))?;
        identifiers.insert(read_dse_internal_id(data, &file.path)?);
        count += 1;
    }
    if count == 0 {
        return Err(BuildError::Voice(
            "ROM contains no top-level sound/*.se resources".to_owned(),
        ));
    }
    Ok(identifiers)
}

fn allocate_internal_ids(retail_ids: &BTreeSet<u16>, count: usize) -> Result<Vec<u16>, BuildError> {
    let mut available = Vec::new();
    for high in (LAST_ID_HIGH..=FIRST_ID_HIGH).rev() {
        let greatest_retail_low = retail_ids
            .iter()
            .copied()
            .filter(|identifier| {
                identifier >> 8 == high
                    && (FIRST_ID_LOW..=LAST_ID_LOW).contains(&(identifier & 0xff))
            })
            .map(|identifier| identifier & 0xff)
            .max();
        let first_low = greatest_retail_low.map_or(FIRST_ID_LOW, |low| low + 1);
        for low in first_low..=LAST_ID_LOW {
            let identifier = (high << 8) | low;
            if !retail_ids.contains(&identifier) {
                available.push(identifier);
            }
        }
    }
    if count > available.len() {
        return Err(BuildError::Invalid(format!(
            "need {count} generated DSE IDs but only {} collision-free pairs remain",
            available.len()
        )));
    }
    available.truncate(count);
    Ok(available)
}

fn checked_range(data: &[u8], offset: usize, size: usize) -> Result<&[u8], BuildError> {
    data.get(offset..offset.saturating_add(size))
        .ok_or_else(|| BuildError::Voice(format!("read outside resource at 0x{offset:x}")))
}

fn checked_range_mut(data: &mut [u8], offset: usize, size: usize) -> Result<&mut [u8], BuildError> {
    data.get_mut(offset..offset.saturating_add(size))
        .ok_or_else(|| BuildError::Voice(format!("write outside resource at 0x{offset:x}")))
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, BuildError> {
    Ok(u16::from_le_bytes(
        checked_range(data, offset, 2)?.try_into().unwrap(),
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, BuildError> {
    Ok(u32::from_le_bytes(
        checked_range(data, offset, 4)?.try_into().unwrap(),
    ))
}

fn u32_at_usize(data: &[u8], offset: usize) -> Result<usize, BuildError> {
    usize::try_from(u32_at(data, offset)?)
        .map_err(|_| BuildError::Voice(format!("offset at 0x{offset:x} exceeds this platform")))
}

fn put_u16(data: &mut [u8], offset: usize, value: u16) -> Result<(), BuildError> {
    checked_range_mut(data, offset, 2)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_u16_be(data: &mut [u8], offset: usize, value: u16) -> Result<(), BuildError> {
    checked_range_mut(data, offset, 2)?.copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn put_u32(data: &mut [u8], offset: usize, value: usize) -> Result<(), BuildError> {
    let value = u32::try_from(value)
        .map_err(|_| BuildError::Voice("voice resource exceeds 4 GiB".to_owned()))?;
    checked_range_mut(data, offset, 4)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn align(value: usize, alignment: usize) -> Result<usize, BuildError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(BuildError::Voice("invalid alignment".to_owned()));
    }
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or_else(|| BuildError::Voice("voice layout overflow".to_owned()))
}

fn find_once(data: &[u8], needle: &[u8], start: usize, end: usize) -> Result<usize, BuildError> {
    let haystack = data
        .get(start..end)
        .ok_or_else(|| BuildError::Voice("invalid search range".to_owned()))?;
    let mut matches = haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(offset, candidate)| (candidate == needle).then_some(start + offset));
    let first = matches.next().ok_or_else(|| {
        BuildError::Voice(format!(
            "missing {:?} between 0x{start:x} and 0x{end:x}",
            String::from_utf8_lossy(needle)
        ))
    })?;
    if matches.next().is_some() {
        return Err(BuildError::Voice(format!(
            "ambiguous {:?} between 0x{start:x} and 0x{end:x}",
            String::from_utf8_lossy(needle)
        )));
    }
    Ok(first)
}

fn read_fixed_ascii(data: &[u8], offset: usize, size: usize) -> Result<String, BuildError> {
    let field = checked_range(data, offset, size)?;
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    if !field[..end].is_ascii() {
        return Err(BuildError::Voice(format!(
            "non-ASCII internal name at 0x{offset:x}"
        )));
    }
    Ok(String::from_utf8(field[..end].to_vec()).unwrap())
}

fn write_fixed_ascii(
    data: &mut [u8],
    offset: usize,
    size: usize,
    value: &str,
) -> Result<(), BuildError> {
    if !value.is_ascii() || value.len() >= size {
        return Err(BuildError::Voice(format!(
            "{value:?} does not fit a {size}-byte NUL-terminated field"
        )));
    }
    let field = checked_range_mut(data, offset, size)?;
    field.fill(0);
    field[..value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

fn mcrl_hash(name: &str) -> Result<u8, BuildError> {
    if !name.is_ascii() {
        return Err(BuildError::Voice(
            "MCRL symbols must use 7-bit ASCII".to_owned(),
        ));
    }
    Ok(name.bytes().fold(0_u8, |sum, byte| sum.wrapping_add(byte)))
}

fn validate_symbol(symbol: &str) -> Result<&str, BuildError> {
    if symbol.is_empty()
        || symbol.len() > 12
        || !symbol
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(BuildError::Voice(format!(
            "invalid generated voice symbol {symbol:?}"
        )));
    }
    Ok(symbol)
}

fn encode_sir0_relocations(pointer_fields: &[usize]) -> Result<Vec<u8>, BuildError> {
    let mut result = Vec::new();
    let mut previous = 0_usize;
    let mut sorted = pointer_fields.to_vec();
    sorted.sort_unstable();
    for pointer in sorted {
        let mut delta = pointer.checked_sub(previous).ok_or_else(|| {
            BuildError::Voice("SIR0 relocation pointers are not increasing".to_owned())
        })?;
        if delta == 0 {
            return Err(BuildError::Voice(
                "SIR0 relocation pointers are duplicated".to_owned(),
            ));
        }
        let mut groups = vec![(delta & 0x7f) as u8];
        delta >>= 7;
        while delta != 0 {
            groups.push((delta & 0x7f) as u8);
            delta >>= 7;
        }
        groups.reverse();
        let last = groups.len() - 1;
        for (index, group) in groups.into_iter().enumerate() {
            result.push(group | if index < last { 0x80 } else { 0 });
        }
        previous = pointer;
    }
    result.push(0);
    Ok(result)
}

fn decode_sir0_relocations(data: &[u8], mut offset: usize) -> Result<Vec<usize>, BuildError> {
    let mut result = Vec::new();
    let mut location = 0_usize;
    let mut delta = 0_usize;
    while let Some(value) = data.get(offset).copied() {
        offset += 1;
        if value == 0 {
            if delta != 0 {
                return Err(BuildError::Voice(
                    "truncated SIR0 relocation varint".to_owned(),
                ));
            }
            return Ok(result);
        }
        let group = usize::from(value & 0x7f);
        delta = delta
            .checked_shl(7)
            .and_then(|shifted| shifted.checked_add(group))
            .ok_or_else(|| BuildError::Voice("SIR0 relocation overflow".to_owned()))?;
        if value & 0x80 == 0 {
            location = location
                .checked_add(delta)
                .ok_or_else(|| BuildError::Voice("SIR0 relocation overflow".to_owned()))?;
            result.push(location);
            delta = 0;
        }
    }
    Err(BuildError::Voice(
        "unterminated SIR0 relocation table".to_owned(),
    ))
}

#[derive(Clone, Copy)]
struct TemplateLayout {
    swdl: usize,
    pcmd: usize,
    eod: usize,
    sedl: usize,
    subheader: usize,
}

fn template_layout(template: &[u8]) -> Result<TemplateLayout, BuildError> {
    if template.len() < 0x100 || template.get(..4) != Some(b"SIR0") {
        return Err(BuildError::Voice("template is not a SIR0 file".to_owned()));
    }
    let subheader = u32_at_usize(template, 4)?;
    let swdl = u32_at_usize(template, subheader)?;
    let sedl = u32_at_usize(template, subheader + 4)?;
    if template.get(swdl..swdl + 4) != Some(b"swdl")
        || template.get(sedl..sedl + 4) != Some(b"sedl")
    {
        return Err(BuildError::Voice(
            "template subheader does not point to SWDL/SEDL".to_owned(),
        ));
    }
    let pcmd = find_once(template, b"pcmd", swdl + SWDL_HEADER_SIZE, sedl)?;
    let pcmd_size = u32_at_usize(template, pcmd + 12)?;
    let eod = align(
        pcmd.checked_add(CHUNK_HEADER_SIZE)
            .and_then(|value| value.checked_add(pcmd_size))
            .ok_or_else(|| BuildError::Voice("template PCMD overflow".to_owned()))?,
        16,
    )?;
    if template.get(eod..eod + 4) != Some(b"eod ") {
        return Err(BuildError::Voice(
            "template SWDL has no EOD after PCMD".to_owned(),
        ));
    }
    let sedl_size = u32_at_usize(template, sedl + 8)?;
    if sedl.checked_add(sedl_size) != Some(subheader) {
        return Err(BuildError::Voice(
            "template SEDL does not end at the SIR0 subheader".to_owned(),
        ));
    }
    Ok(TemplateLayout {
        swdl,
        pcmd,
        eod,
        sedl,
        subheader,
    })
}

fn patch_names(
    data: &mut [u8],
    swdl: usize,
    sedl: usize,
    subheader: usize,
    name: &str,
) -> Result<(), BuildError> {
    let old_swdl_name = read_fixed_ascii(data, swdl + 0x20, 16)?;
    let old_base = old_swdl_name
        .strip_suffix(".SW")
        .or_else(|| old_swdl_name.strip_suffix(".SWD"))
        .ok_or_else(|| {
            BuildError::Voice(format!("unexpected template SWDL name {old_swdl_name:?}"))
        })?;
    if old_base.len() > 12 {
        return Err(BuildError::Voice(
            "template MCRL symbol slot is too small".to_owned(),
        ));
    }
    let old_base = old_base.to_owned();
    write_fixed_ascii(data, swdl + 0x20, 16, &format!("{name}.SW"))?;
    write_fixed_ascii(data, sedl + 0x20, 16, &format!("{name}.SE"))?;

    let mcrl = find_once(data, b"mcrl", sedl, subheader)?;
    let macro_name = find_once(data, old_base.as_bytes(), mcrl, subheader)?;
    if macro_name < 4 || u16_at(data, macro_name - 4)? != 1 {
        return Err(BuildError::Voice(
            "template MCRL name record is malformed".to_owned(),
        ));
    }
    let record_size = usize::from(u16_at(data, macro_name - 2)?);
    let slot_size = record_size
        .checked_sub(4)
        .ok_or_else(|| BuildError::Voice("template MCRL record size is invalid".to_owned()))?;
    if record_size < 6
        || macro_name
            .checked_add(slot_size)
            .is_none_or(|end| end > subheader)
        || name.len() + 1 > slot_size
    {
        return Err(BuildError::Voice(
            "generated name does not fit the MCRL record".to_owned(),
        ));
    }
    let bucket_base = mcrl + CHUNK_HEADER_SIZE;
    if bucket_base + 0x200 > subheader {
        return Err(BuildError::Voice(
            "template MCRL bucket table is truncated".to_owned(),
        ));
    }
    let record = macro_name - 4;
    let old_bucket = bucket_base + usize::from(mcrl_hash(&old_base)?) * 2;
    let old_value = u16_at(data, old_bucket)?;
    if bucket_base + usize::from(old_value) != record
        || u16_at(data, record)? != 1
        || usize::from(u16_at(data, record + 2)?) != record_size
        || record
            .checked_add(record_size + 2)
            .is_none_or(|end| end > subheader)
        || u16_at(data, record + record_size)? != 0xffff
    {
        return Err(BuildError::Voice(
            "template MCRL old-name bucket is malformed".to_owned(),
        ));
    }
    let new_bucket = bucket_base + usize::from(mcrl_hash(name)?) * 2;
    if new_bucket != old_bucket {
        let new_value = u16_at(data, new_bucket)?;
        let chain = bucket_base + usize::from(new_value);
        if chain + 2 > subheader || u16_at(data, chain)? != 0xffff {
            return Err(BuildError::Voice(
                "generated MCRL hash bucket is not empty".to_owned(),
            ));
        }
        put_u16(data, old_bucket, new_value)?;
        put_u16(data, new_bucket, old_value)?;
    }
    let slot = checked_range_mut(data, macro_name, slot_size)?;
    slot.fill(0);
    slot[..name.len()].copy_from_slice(name.as_bytes());
    Ok(())
}

fn patch_bank_id(
    data: &mut [u8],
    swdl: usize,
    sedl: usize,
    subheader: usize,
    bank_id: u16,
) -> Result<(), BuildError> {
    put_u16(data, swdl + 0x0e, bank_id)?;
    put_u16(data, sedl + 0x0e, bank_id)?;
    let track = find_once(data, b"trk ", sedl, subheader)?;
    let track_end = track
        .checked_add(CHUNK_HEADER_SIZE)
        .and_then(|value| value.checked_add(u32_at_usize(data, track + 12).ok()?))
        .ok_or_else(|| BuildError::Voice("track chunk overflow".to_owned()))?;
    let opcode = find_once(data, b"\xa8", track + CHUNK_HEADER_SIZE, track_end)?;
    if opcode + 3 > track_end {
        return Err(BuildError::Voice(
            "truncated A8 bank-select opcode".to_owned(),
        ));
    }
    put_u16_be(data, opcode + 1, bank_id)?;
    let bnkl = find_once(data, b"bnkl", sedl, subheader)?;
    if bnkl + 0x2a > subheader {
        return Err(BuildError::Voice("truncated BNKL chunk".to_owned()));
    }
    put_u16(data, bnkl + 0x28, bank_id)?;
    Ok(())
}

const IMA_INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

fn encode_nibble(target: i32, predictor: i32, index: i32) -> (u8, i32, i32) {
    let step = IMA_STEP_TABLE[index as usize];
    let difference = target - predictor;
    let mut code = if difference < 0 { 8_u8 } else { 0_u8 };
    let mut magnitude = difference.abs();
    if magnitude >= step {
        code |= 4;
        magnitude -= step;
    }
    if magnitude >= step >> 1 {
        code |= 2;
        magnitude -= step >> 1;
    }
    if magnitude >= step >> 2 {
        code |= 1;
    }
    let mut delta = step >> 3;
    if code & 1 != 0 {
        delta += step >> 2;
    }
    if code & 2 != 0 {
        delta += step >> 1;
    }
    if code & 4 != 0 {
        delta += step;
    }
    let predictor = if code & 8 != 0 {
        predictor - delta
    } else {
        predictor + delta
    }
    .clamp(-32767, 32767);
    let index = (index + IMA_INDEX_TABLE[usize::from(code)]).clamp(0, 88);
    (code, predictor, index)
}

fn encode_ima_adpcm(samples: &[i16]) -> Result<Vec<u8>, BuildError> {
    if samples.is_empty() {
        return Err(BuildError::Voice("cannot encode empty PCM".to_owned()));
    }
    let mut padded = samples
        .iter()
        .map(|sample| i32::from(*sample).clamp(-32767, 32767))
        .collect::<Vec<_>>();
    let last = *padded.last().unwrap();
    padded.extend(std::iter::repeat_n(last, (8 - padded.len() % 8) % 8));
    let mut predictor = padded[0];
    let mut index = 0_i32;
    let mut nibbles = Vec::with_capacity(padded.len());
    for target in padded {
        let (code, next_predictor, next_index) = encode_nibble(target, predictor, index);
        nibbles.push(code);
        predictor = next_predictor;
        index = next_index;
    }
    let mut result = Vec::with_capacity(ADPCM_PREAMBLE_SIZE + nibbles.len() / 2);
    result.extend_from_slice(&(samples[0].max(-32767)).to_le_bytes());
    result.extend_from_slice(&0_u16.to_le_bytes());
    result.extend(nibbles.chunks_exact(2).map(|pair| pair[0] | (pair[1] << 4)));
    if result.len() % 4 != 0 {
        return Err(BuildError::Voice(
            "ADPCM encoder produced an incomplete 32-bit block".to_owned(),
        ));
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SeInfo {
    file_size: usize,
    pcmd_size: usize,
    sample_count: usize,
    bank_id: u16,
    name: String,
}

fn inspect_se(
    data: &[u8],
    expected_name: Option<&str>,
    expected_bank_id: Option<u16>,
) -> Result<SeInfo, BuildError> {
    if data.len() < 0x100 || !data.len().is_multiple_of(16) || data.get(..4) != Some(b"SIR0") {
        return Err(BuildError::Voice(
            "output is not an aligned SIR0 file".to_owned(),
        ));
    }
    let subheader = u32_at_usize(data, 4)?;
    let relocations = u32_at_usize(data, 8)?;
    if subheader + 8 > data.len() || relocations >= data.len() {
        return Err(BuildError::Voice(
            "SIR0 header pointer is out of range".to_owned(),
        ));
    }
    let swdl = u32_at_usize(data, subheader)?;
    let sedl = u32_at_usize(data, subheader + 4)?;
    if data.get(swdl..swdl + 4) != Some(b"swdl")
        || data.get(sedl..sedl + 4) != Some(b"sedl")
        || decode_sir0_relocations(data, relocations)? != [4, 8, subheader, subheader + 4]
    {
        return Err(BuildError::Voice(
            "SIR0 subheader or relocation table is invalid".to_owned(),
        ));
    }
    let sedl_size = u32_at_usize(data, sedl + 8)?;
    if sedl.checked_add(sedl_size) != Some(subheader) {
        return Err(BuildError::Voice(
            "SEDL length does not end at the SIR0 subheader".to_owned(),
        ));
    }
    let pcmd = find_once(data, b"pcmd", swdl + SWDL_HEADER_SIZE, sedl)?;
    let pcmd_size = u32_at_usize(data, pcmd + 12)?;
    if pcmd_size != u32_at_usize(data, swdl + 0x40)? || pcmd_size < 8 || pcmd_size % 4 != 0 {
        return Err(BuildError::Voice(
            "SWDL and PCMD sample lengths are invalid".to_owned(),
        ));
    }
    let eod = align(pcmd + CHUNK_HEADER_SIZE + pcmd_size, 16)?;
    let swdl_size = u32_at_usize(data, swdl + 8)?;
    if data.get(eod..eod + 4) != Some(b"eod ")
        || swdl.checked_add(swdl_size) != Some(eod + CHUNK_HEADER_SIZE)
        || sedl != align(eod + CHUNK_HEADER_SIZE, 64)?
    {
        return Err(BuildError::Voice(
            "generated SWDL layout is inconsistent".to_owned(),
        ));
    }
    let wavi = find_once(data, b"wavi", swdl + SWDL_HEADER_SIZE, pcmd)?;
    let wavi_entry = wavi + 0x20;
    if u16_at(data, wavi_entry + 0x12)? != 0x0200
        || u32_at(data, wavi_entry + 0x20)? != SAMPLE_RATE
        || u32_at(data, wavi_entry + 0x24)? != 0
        || u32_at(data, wavi_entry + 0x28)? != 1
        || u32_at_usize(data, wavi_entry + 0x2c)? != (pcmd_size - ADPCM_PREAMBLE_SIZE) / 4
    {
        return Err(BuildError::Voice(
            "generated WAVI fields are inconsistent".to_owned(),
        ));
    }
    let swdl_name = read_fixed_ascii(data, swdl + 0x20, 16)?;
    let name = swdl_name
        .strip_suffix(".SW")
        .ok_or_else(|| BuildError::Voice("generated SWDL name lacks .SW".to_owned()))?
        .to_owned();
    if read_fixed_ascii(data, sedl + 0x20, 16)? != format!("{name}.SE")
        || expected_name.is_some_and(|expected| name != expected)
    {
        return Err(BuildError::Voice(
            "SWDL and SEDL internal names disagree".to_owned(),
        ));
    }
    let mcrl = find_once(data, b"mcrl", sedl, subheader)?;
    let mut macro_matches = 0_usize;
    for record in ((mcrl + CHUNK_HEADER_SIZE)..subheader.saturating_sub(5)).step_by(2) {
        if u16_at(data, record)? != 1 {
            continue;
        }
        let record_size = usize::from(u16_at(data, record + 2)?);
        if record_size < 6
            || record_size % 2 != 0
            || record
                .checked_add(record_size)
                .is_none_or(|end| end > subheader)
        {
            continue;
        }
        let symbol = checked_range(data, record + 4, record_size - 4)?;
        let symbol_end = symbol
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(symbol.len());
        if &symbol[..symbol_end] == name.as_bytes() {
            macro_matches += 1;
        }
    }
    if macro_matches != 1 {
        return Err(BuildError::Voice(format!(
            "expected one matching MCRL symbol, found {macro_matches}"
        )));
    }
    let bucket_base = mcrl + CHUNK_HEADER_SIZE;
    let bucket = bucket_base + usize::from(mcrl_hash(&name)?) * 2;
    let record = bucket_base + usize::from(u16_at(data, bucket)?);
    let record_size = usize::from(u16_at(data, record + 2)?);
    if record + 4 > subheader
        || u16_at(data, record)? != 1
        || record_size < 6
        || record_size % 2 != 0
        || record
            .checked_add(record_size)
            .is_none_or(|end| end > subheader)
    {
        return Err(BuildError::Voice(
            "MCRL hash bucket does not resolve a symbol".to_owned(),
        ));
    }
    let hashed = checked_range(data, record + 4, record_size - 4)?;
    let hashed_end = hashed
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(hashed.len());
    if &hashed[..hashed_end] != name.as_bytes() {
        return Err(BuildError::Voice(
            "MCRL hash bucket resolves a different symbol".to_owned(),
        ));
    }
    let bank_id = u16_at(data, swdl + 0x0e)?;
    if u16_at(data, sedl + 0x0e)? != bank_id {
        return Err(BuildError::Voice(
            "SWDL and SEDL bank IDs disagree".to_owned(),
        ));
    }
    let track = find_once(data, b"trk ", sedl, subheader)?;
    let track_end = track + CHUNK_HEADER_SIZE + u32_at_usize(data, track + 12)?;
    let mut bank_select = [0_u8; 3];
    bank_select[0] = 0xa8;
    bank_select[1..].copy_from_slice(&bank_id.to_be_bytes());
    find_once(data, &bank_select, track + CHUNK_HEADER_SIZE, track_end)?;
    let bnkl = find_once(data, b"bnkl", sedl, subheader)?;
    if u16_at(data, bnkl + 0x28)? != bank_id
        || expected_bank_id.is_some_and(|expected| bank_id != expected)
    {
        return Err(BuildError::Voice(
            "generated DSE bank IDs disagree".to_owned(),
        ));
    }
    let pcmd_data = pcmd + CHUNK_HEADER_SIZE;
    if pcmd_data + pcmd_size > data.len() || u16_at(data, pcmd_data + 2)? > 88 {
        return Err(BuildError::Voice(
            "generated PCMD preamble is invalid".to_owned(),
        ));
    }
    Ok(SeInfo {
        file_size: data.len(),
        pcmd_size,
        sample_count: (pcmd_size - ADPCM_PREAMBLE_SIZE) * 2,
        bank_id,
        name,
    })
}

fn build_se(
    template: &[u8],
    pcm: &[i16],
    bank_id: u16,
    symbol: &str,
) -> Result<Vec<u8>, BuildError> {
    validate_symbol(symbol)?;
    let adpcm = encode_ima_adpcm(pcm)?;
    let layout = template_layout(template)?;
    let pcmd_data = layout.pcmd + CHUNK_HEADER_SIZE;
    let old_pcmd_size = u32_at_usize(template, layout.pcmd + 12)?;
    if pcmd_data
        .checked_add(old_pcmd_size)
        .is_none_or(|end| end > layout.eod)
    {
        return Err(BuildError::Voice(
            "template PCMD extends beyond its EOD".to_owned(),
        ));
    }
    let mut output = template[..pcmd_data].to_vec();
    output.extend_from_slice(&adpcm);
    let new_eod = align(output.len(), 16)?;
    output.resize(new_eod, 0xaa);
    output.extend_from_slice(checked_range(template, layout.eod, CHUNK_HEADER_SIZE)?);
    let new_sedl = align(output.len(), 64)?;
    output.resize(new_sedl, 0xaa);
    output.extend_from_slice(checked_range(
        template,
        layout.sedl,
        layout.subheader - layout.sedl,
    )?);
    let sedl_size = u32_at_usize(&output, new_sedl + 8)?;
    let new_subheader = new_sedl
        .checked_add(sedl_size)
        .ok_or_else(|| BuildError::Voice("generated SEDL overflow".to_owned()))?;
    if output.len() != new_subheader {
        return Err(BuildError::Voice(
            "copied SEDL does not end at its declared length".to_owned(),
        ));
    }
    output.extend_from_slice(
        &u32::try_from(layout.swdl)
            .map_err(|_| BuildError::Voice("SWDL offset exceeds 32 bits".to_owned()))?
            .to_le_bytes(),
    );
    output.extend_from_slice(
        &u32::try_from(new_sedl)
            .map_err(|_| BuildError::Voice("SEDL offset exceeds 32 bits".to_owned()))?
            .to_le_bytes(),
    );
    let new_relocations = align(output.len(), 16)?;
    output.resize(new_relocations, 0xaa);
    output.extend_from_slice(&encode_sir0_relocations(&[
        4,
        8,
        new_subheader,
        new_subheader + 4,
    ])?);
    output.resize(align(output.len(), 16)?, 0xaa);

    put_u32(&mut output, 4, new_subheader)?;
    put_u32(&mut output, 8, new_relocations)?;
    put_u32(
        &mut output,
        layout.swdl + 8,
        new_eod + CHUNK_HEADER_SIZE - layout.swdl,
    )?;
    put_u32(&mut output, layout.swdl + 0x40, adpcm.len())?;
    put_u32(&mut output, layout.pcmd + 12, adpcm.len())?;
    let wavi = find_once(
        &output,
        b"wavi",
        layout.swdl + SWDL_HEADER_SIZE,
        layout.pcmd,
    )?;
    let wavi_entry = wavi + 0x20;
    if wavi_entry + WAVI_ENTRY_SIZE > layout.pcmd {
        return Err(BuildError::Voice(
            "template WAVI entry is truncated".to_owned(),
        ));
    }
    put_u32(&mut output, wavi_entry + 0x20, SAMPLE_RATE as usize)?;
    put_u32(&mut output, wavi_entry + 0x24, 0)?;
    put_u32(&mut output, wavi_entry + 0x28, 1)?;
    put_u32(
        &mut output,
        wavi_entry + 0x2c,
        (adpcm.len() - ADPCM_PREAMBLE_SIZE) / 4,
    )?;
    patch_names(&mut output, layout.swdl, new_sedl, new_subheader, symbol)?;
    patch_bank_id(&mut output, layout.swdl, new_sedl, new_subheader, bank_id)?;
    inspect_se(&output, Some(symbol), Some(bank_id))?;
    Ok(output)
}

fn verify_take_file(path: &Path, take: &TakeMetadata) -> Result<(), BuildError> {
    if take.sample_rate == 0
        || take.sample_rate > MAX_SAMPLE_RATE
        || take.frames == 0
        || take.frames > u64::from(take.sample_rate) * (MAX_DURATION_MS / 1_000)
    {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: "take rate, frame count, or duration exceeds recording limits".to_owned(),
        });
    }
    let expected_duration_ms = ((u128::from(take.frames) * 1_000
        + u128::from(take.sample_rate / 2))
        / u128::from(take.sample_rate)) as u64;
    if take.duration_ms != expected_duration_ms || take.duration_ms > MAX_DURATION_MS {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: "take duration metadata is inconsistent with its frames".to_owned(),
        });
    }
    let source = read_limited_regular_file(path, MAX_WAV_BYTES)?;
    if source.is_empty() {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: "file is empty".to_owned(),
        });
    }
    let actual_sha256 = Sha256::digest(&source)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_sha256 != take.sha256 {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: format!(
                "SHA-256 {actual_sha256} does not match the recorded {}",
                take.sha256
            ),
        });
    }
    let mut reader =
        hound::WavReader::new(Cursor::new(source)).map_err(|error| BuildError::Wav {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
        || spec.sample_rate != take.sample_rate
    {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: "format differs from the recorded mono PCM16 take".to_owned(),
        });
    }
    let frames = reader.samples::<i16>().try_fold(0_u64, |count, sample| {
        sample.map(|_| count + 1).map_err(|error| BuildError::Wav {
            path: path.to_owned(),
            message: error.to_string(),
        })
    })?;
    if frames != take.frames {
        return Err(BuildError::Wav {
            path: path.to_owned(),
            message: format!("contains {frames} frames instead of {}", take.frames),
        });
    }
    Ok(())
}

fn build_input_sha256(
    scope: BuildScope,
    profile: &VoiceProfile,
    targets: &[(&str, &TargetProgress)],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"NVDUB-BUILD-INPUT-V1\0");
    hasher.update(match scope {
        BuildScope::Preview => b"preview".as_slice(),
        BuildScope::Production => b"production".as_slice(),
    });
    hasher.update(profile.catalog_sha256());
    for (symbol, target) in targets {
        hasher.update((symbol.len() as u64).to_le_bytes());
        hasher.update(symbol.as_bytes());
        hasher.update([match target.status {
            TargetStatus::Missing => 0,
            TargetStatus::Recorded => 1,
            TargetStatus::NeedsReview => 2,
            TargetStatus::Approved => 3,
            TargetStatus::Skipped => 4,
        }]);
        hasher.update(target.gain_db.to_bits().to_le_bytes());
        hasher.update(target.trim_start_ms.to_le_bytes());
        hasher.update(target.trim_end_ms.to_le_bytes());
        if let Some(take) = active_take(target) {
            hasher.update([1]);
            hasher.update(take.sha256.as_bytes());
        } else {
            hasher.update([0]);
        }
        if let Some(approval) = &target.approval_sha256 {
            hasher.update([1]);
            hasher.update(approval.as_bytes());
        } else {
            hasher.update([0]);
        }
    }
    hasher.finalize().into()
}

/// Build an authenticated French pack without modifying the project masters
/// or the selected ROM. Every temporary `.se` derivative disappears after the
/// atomically verified pack replaces its destination.
pub fn build_voice_pack(
    project_dir: impl AsRef<Path>,
    rom_path: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    output_pack: impl AsRef<Path>,
    scope: BuildScope,
    progress: impl FnMut(DubbingBuildProgress),
) -> Result<DubbingBuildReport, BuildError> {
    let project_dir = project_dir.as_ref();
    let rom_path = rom_path.as_ref();
    let profile_path = profile_path.as_ref();
    let output_pack = output_pack.as_ref();
    // A finished pack must identify one manifest state. Keeping the same
    // cross-process guard through atomic publication prevents a later edit
    // from making a successful build stale before the caller can observe it.
    let project_lock = project::lock_project_mutations(project_dir)?;
    build_voice_pack_locked(
        project_dir,
        rom_path,
        profile_path,
        output_pack,
        &project_lock,
        scope,
        progress,
    )
}

fn build_voice_pack_locked(
    project_dir: &Path,
    rom_path: &Path,
    profile_path: &Path,
    output_pack: &Path,
    _project_lock: &project::ProjectMutationLock,
    scope: BuildScope,
    mut progress: impl FnMut(DubbingBuildProgress),
) -> Result<DubbingBuildReport, BuildError> {
    let output_parent = output_pack.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).map_err(|error| io_error(output_parent, error))?;
    emit(
        &mut progress,
        "validate",
        0,
        1,
        "Validating the project, profile, and restored ROM",
    );
    let snapshot = project::open_project(project_dir, rom_path, profile_path)?;
    validate_output_destination(project_dir, rom_path, profile_path, output_pack)?;
    let profile_data = read_limited_regular_file(profile_path, MAX_PROFILE_BYTES)?;
    let profile = VoiceProfile::from_json(&profile_data)?;
    let base = read_clean_or_restored_base(rom_path)?;
    if sha256_hex(&base) != snapshot.manifest.source_rom_sha256 {
        return Err(BuildError::Invalid(
            "restored ROM hash changed after project validation".to_owned(),
        ));
    }
    let rom = NdsRom::parse(&base)?;
    profile.validate_rom(&rom)?;
    let catalog_sha256 = profile.catalog_sha256();
    if hex_digest(&catalog_sha256) != snapshot.manifest.profile_catalog_sha256 {
        return Err(BuildError::Invalid(
            "project catalogue digest does not match the selected profile".to_owned(),
        ));
    }

    let ordered = profile
        .scripts
        .iter()
        .flat_map(|script| script.voices.iter())
        .map(|voice| {
            snapshot
                .manifest
                .targets
                .get(&voice.symbol)
                .map(|target| (voice.symbol.as_str(), target))
                .ok_or_else(|| BuildError::Invalid(format!("project is missing {}", voice.symbol)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ordered.len() != profile.voice_count {
        return Err(BuildError::Invalid(format!(
            "profile yielded {} targets instead of {}",
            ordered.len(),
            profile.voice_count
        )));
    }
    if scope == BuildScope::Production {
        let incomplete = ordered
            .iter()
            .filter(|(_, target)| {
                target.status != TargetStatus::Approved || active_take(target).is_none()
            })
            .map(|(symbol, _)| *symbol)
            .collect::<Vec<_>>();
        if let Some(first) = incomplete.first() {
            return Err(BuildError::ProductionIncomplete {
                total: ordered.len(),
                missing: incomplete.len(),
                first: (*first).to_owned(),
            });
        }
    }

    let recorded = ordered
        .iter()
        .filter(|(_, target)| active_take(target).is_some())
        .count();
    let approved = ordered
        .iter()
        .filter(|(_, target)| {
            target.status == TargetStatus::Approved && active_take(target).is_some()
        })
        .count();
    emit(
        &mut progress,
        "preflight",
        0,
        recorded as u64,
        "Authenticating active WAV masters",
    );
    let mut verified_paths = HashMap::new();
    let mut verified = 0_u64;
    for (symbol, target) in &ordered {
        let Some(take) = active_take(target) else {
            continue;
        };
        let path = safe_take_path(project_dir, &take.file)?;
        verify_take_file(&path, take)?;
        verified_paths.insert((*symbol).to_owned(), path);
        verified += 1;
        emit(
            &mut progress,
            "preflight",
            verified,
            recorded as u64,
            format!("Authenticated {symbol}"),
        );
    }

    let template = rom
        .data_for_path(TEMPLATE_PATH)
        .ok_or_else(|| BuildError::Voice(format!("ROM does not contain {TEMPLATE_PATH}")))?;
    inspect_se(template, None, Some(TEMPLATE_INTERNAL_ID))?;
    let internal_ids = allocate_internal_ids(&collect_retail_internal_ids(&rom)?, ordered.len())?;
    let parent = output_pack.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let derivatives = TempBuilder::new()
        .prefix(".nonary-fr-voices-")
        .tempdir_in(parent)
        .map_err(|error| io_error(parent, error))?;
    let silence = vec![
        0_i16;
        usize::try_from((PREVIEW_SILENCE_MS * u64::from(SAMPLE_RATE) + 500) / 1_000)
            .unwrap()
    ];
    let mut converter = AudioConverter::new();
    let mut sources = Vec::with_capacity(ordered.len());
    let mut total_samples = 0_u64;
    let mut clipped_samples = 0_u64;
    emit(
        &mut progress,
        "encode",
        0,
        ordered.len() as u64,
        "Encoding Nintendo DS voice resources",
    );
    for (index, ((symbol, target), internal_id)) in
        ordered.iter().zip(internal_ids.iter()).enumerate()
    {
        let (pcm, target_clipped) = if let Some(take) = active_take(target) {
            let path = verified_paths.get(*symbol).ok_or_else(|| {
                BuildError::Invalid(format!("preflight did not authenticate {symbol}"))
            })?;
            converter.decode_master(path, take, target)?
        } else {
            if scope == BuildScope::Production {
                return Err(BuildError::Invalid(
                    "production preflight allowed a missing take".to_owned(),
                ));
            }
            (silence.clone(), 0)
        };
        if scope == BuildScope::Production && target_clipped != 0 {
            return Err(BuildError::Invalid(format!(
                "{symbol} clips {target_clipped} sample(s) after approved gain; reduce its gain and approve it again"
            )));
        }
        clipped_samples = clipped_samples.saturating_add(target_clipped);
        total_samples = total_samples
            .checked_add(pcm.len() as u64)
            .ok_or_else(|| BuildError::Invalid("combined duration overflow".to_owned()))?;
        let generated = build_se(template, &pcm, *internal_id, symbol)?;
        let output = derivatives.path().join(format!("se_v{index:04}.se"));
        fs::write(&output, generated).map_err(|error| io_error(&output, error))?;
        sources.push(output);
        emit(
            &mut progress,
            "encode",
            (index + 1) as u64,
            ordered.len() as u64,
            format!("Encoded {symbol}"),
        );
    }
    emit(
        &mut progress,
        "package",
        0,
        1,
        "Authenticating the French voice pack",
    );
    let written = VoicePack::write_atomic_catalog_from_files(
        output_pack,
        Language::French,
        &sources,
        catalog_sha256,
    )?;
    let reopened = VoicePack::open(output_pack)?;
    if reopened.catalog_sha256() != Some(catalog_sha256)
        || reopened.entries().len() != ordered.len()
    {
        return Err(BuildError::Invalid(
            "final French pack failed catalogue verification".to_owned(),
        ));
    }
    reopened.verify_payload()?;
    emit(
        &mut progress,
        "done",
        1,
        1,
        "French voice pack created and verified",
    );
    let build_input = build_input_sha256(scope, &profile, &ordered);
    Ok(DubbingBuildReport {
        scope,
        pack_path: output_pack.to_string_lossy().into_owned(),
        pack_sha256: hex_digest(&written.sha256),
        catalog_sha256: hex_digest(&catalog_sha256),
        build_input_sha256: hex_digest(&build_input),
        source_rom_sha256: snapshot.manifest.source_rom_sha256.clone(),
        profile_sha256: snapshot.manifest.profile_sha256.clone(),
        voices: written.entries,
        recorded,
        approved,
        silent_preview_entries: if scope == BuildScope::Preview {
            ordered.len() - recorded
        } else {
            0
        },
        payload_bytes: written.payload_size,
        duration_ms: total_samples.saturating_mul(1_000) / u64::from(SAMPLE_RATE),
        clipped_samples,
        first_internal_id: format!("{:04x}", internal_ids[0]),
        last_internal_id: format!("{:04x}", internal_ids[internal_ids.len() - 1]),
    })
}

/// Build the derived pack and immediately apply it through the same reversible
/// engine used by normal patches. The source may itself be patched because both
/// validation and patching restore its exact base before proceeding.
pub fn build_test_rom(
    project_dir: impl AsRef<Path>,
    rom_path: impl AsRef<Path>,
    output_rom: impl AsRef<Path>,
    resources: &ResourcePaths,
    scope: BuildScope,
    mut progress: impl FnMut(DubbingBuildProgress),
) -> Result<TestRomBuildResult, BuildError> {
    let project_dir = project_dir.as_ref();
    let rom_path = rom_path.as_ref();
    let output_rom = output_rom.as_ref();
    // The pack is an intermediate publication for this operation. Holding one
    // guard until the ROM is atomically published keeps both outputs bound to
    // the manifest snapshot validated below.
    let project_lock = project::lock_project_mutations(project_dir)?;
    let output_parent = output_rom.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).map_err(|error| io_error(output_parent, error))?;
    validate_output_destination(project_dir, rom_path, &resources.profile, output_rom)?;
    let builds = safe_builds_directory(project_dir)?;
    let voice_pack_path = builds.join(scope.filename());
    if absolute_destination(output_rom)? == absolute_destination(&voice_pack_path)? {
        return Err(BuildError::Invalid(
            "test ROM output collides with the derived French voice pack".to_owned(),
        ));
    }
    let voice_pack = build_voice_pack_locked(
        project_dir,
        rom_path,
        &resources.profile,
        &voice_pack_path,
        &project_lock,
        scope,
        &mut progress,
    )?;
    let rom = engine::apply_patch(
        rom_path,
        output_rom,
        resources,
        &ApplyOptions {
            language: PatchLanguage::French,
            voicepack_override: Some(voice_pack_path),
        },
        |event| {
            progress(DubbingBuildProgress {
                stage: format!("rom_{}", event.stage),
                completed: event.completed,
                total: event.total,
                message: event.message,
            });
        },
    )?;
    Ok(TestRomBuildResult { voice_pack, rom })
}

#[cfg(test)]
mod tests {
    use std::{f64::consts::TAU, process::Command};

    use super::*;

    #[test]
    fn ima_encoder_matches_the_python_reference_fixture() {
        assert_eq!(
            encode_ima_adpcm(&[0, 1000, -1000, 32767, -32768, 1234, -2222, 99]).unwrap(),
            [0x00, 0x00, 0x00, 0x00, 0x70, 0x7f, 0x7f, 0x2f]
        );
        assert_eq!(
            encode_ima_adpcm(&[100, 200, 300, 400, 500, 600, 700, 800, 900]).unwrap(),
            [0x64, 0x00, 0x00, 0x00, 0x70, 0x77, 0x67, 0x11, 0x01, 0x08, 0x80, 0x08,]
        );
    }

    #[test]
    fn sir0_relocations_match_the_python_reference_fixture() {
        let encoded = encode_sir0_relocations(&[4, 8, 0x1234, 0x1238]).unwrap();
        assert_eq!(encoded, [0x04, 0x04, 0xa4, 0x2c, 0x04, 0x00]);
        assert_eq!(
            decode_sir0_relocations(&encoded, 0).unwrap(),
            [4, 8, 0x1234, 0x1238]
        );
    }

    #[test]
    fn internal_id_allocation_leaves_retail_holes_unused() {
        let retail = BTreeSet::from([0xc801, 0xc807, 0xc80d, 0xc90a]);
        let generated = allocate_internal_ids(&retail, 4).unwrap();
        assert_eq!(generated, [0xc80e, 0xc80f, 0xc810, 0xc811]);
    }

    #[test]
    fn high_quality_resampler_preserves_duration_and_signal() {
        let input = (0..4_800)
            .map(|index| (TAU * 440.0 * index as f64 / 48_000.0).sin() * 0.5)
            .collect::<Vec<_>>();
        let output = AudioConverter::new().resample(&input, 48_000).unwrap();
        assert_eq!(output.len(), 1_638);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().copied().fold(0.0_f64, f64::max) > 0.45);
    }

    #[test]
    fn master_decoder_verifies_and_applies_trim() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("take.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..4_800 {
            writer
                .write_sample(((TAU * 440.0 * index as f64 / 48_000.0).sin() * 8_000.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
        let take = TakeMetadata {
            id: "take-test".to_owned(),
            file: "recordings/SE_V0000/take-test.wav".to_owned(),
            created_at_ms: 1,
            duration_ms: 100,
            sample_rate: 48_000,
            frames: 4_800,
            peak_dbfs: -12.0,
            clipped_samples: 0,
            sha256: sha256_hex(&fs::read(&path).unwrap()),
        };
        let target = TargetProgress {
            target_id: "scr/a.fsb#1".to_owned(),
            status: TargetStatus::Recorded,
            active_take: Some(take.id.clone()),
            takes: vec![take.clone()],
            notes: String::new(),
            gain_db: 0.0,
            trim_start_ms: 10,
            trim_end_ms: 10,
            approval_sha256: None,
        };
        verify_take_file(&path, &take).unwrap();
        let (decoded, clipped) = AudioConverter::new()
            .decode_master(&path, &take, &target)
            .unwrap();
        assert!((1_309..=1_312).contains(&decoded.len()));
        assert_eq!(clipped, 0);
    }

    #[test]
    fn build_outputs_cannot_replace_inputs_or_recordings() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let recordings = project.join("recordings");
        fs::create_dir_all(&recordings).unwrap();
        fs::write(project.join("project.nvdub.json"), b"{}").unwrap();
        let rom = directory.path().join("source.nds");
        let profile = directory.path().join("profile.json");
        fs::write(&rom, b"rom").unwrap();
        fs::write(&profile, b"profile").unwrap();
        assert!(validate_output_destination(&project, &rom, &profile, &rom).is_err());
        assert!(validate_output_destination(
            &project,
            &rom,
            &profile,
            &project.join(project::PROJECT_LOCK_FILE)
        )
        .is_err());
        assert!(validate_output_destination(
            &project,
            &rom,
            &profile,
            &recordings.join("destructive.nvpack")
        )
        .is_err());
        fs::create_dir_all(project.join("builds")).unwrap();
        assert!(validate_output_destination(
            &project,
            &rom,
            &profile,
            &project.join("builds/voices-fr.nvpack")
        )
        .is_ok());
    }

    #[test]
    fn native_build_directory_is_a_real_project_child() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        fs::create_dir(&project).unwrap();
        let builds = safe_builds_directory(&project).unwrap();
        assert_eq!(builds, project.canonicalize().unwrap().join("builds"));
        assert!(builds.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn native_build_directory_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        let outside = directory.path().join("outside");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, project.join("builds")).unwrap();
        let error = safe_builds_directory(&project).unwrap_err();
        assert!(error.to_string().contains("not a symlink"));
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[test]
    #[ignore = "requires the private compatible ROM and Python reference implementation"]
    fn generated_se_is_byte_identical_to_the_python_reference() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let rom_path = repository.join("../build/french-patcher-validation/runner/999-fr-safe.nds");
        let base = read_clean_or_restored_base(&rom_path).unwrap();
        let rom = NdsRom::parse(&base).unwrap();
        let template = rom.data_for_path(TEMPLATE_PATH).unwrap();
        let pcm = [0, 1000, -1000, 32767, -32768, 1234, -2222, 99];
        let native = build_se(template, &pcm, 0xc80e, "SE_V0000").unwrap();

        let directory = tempfile::tempdir().unwrap();
        let template_path = directory.path().join("template.se");
        let python_path = directory.path().join("python.se");
        fs::write(&template_path, template).unwrap();
        let status = Command::new("python3")
            .arg("-c")
            .arg("from pathlib import Path; from voice_audio import build_se; t=Path(__import__('sys').argv[1]).read_bytes(); Path(__import__('sys').argv[2]).write_bytes(build_se(t,[0,1000,-1000,32767,-32768,1234,-2222,99],16384,0xc80e,'SE_V0000'))")
            .arg(&template_path)
            .arg(&python_path)
            .env("PYTHONPATH", repository.join("scripts"))
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(native, fs::read(python_path).unwrap());
    }

    #[test]
    #[ignore = "requires the private compatible French ROM"]
    fn private_preview_builds_and_patches_all_catalogue_entries() {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let rom_path = repository.join("../build/french-patcher-validation/runner/999-fr-safe.nds");
        let profile_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json");
        let directory = tempfile::tempdir().unwrap();
        let project_dir = directory.path().join("project");
        project::create_project(
            &project_dir,
            &rom_path,
            &profile_path,
            "Native preview integration",
        )
        .unwrap();
        let production_pack = directory.path().join("production.nvpack");
        let error = build_voice_pack(
            &project_dir,
            &rom_path,
            &profile_path,
            &production_pack,
            BuildScope::Production,
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(
            error,
            BuildError::ProductionIncomplete {
                total: 6_475,
                missing: 6_475,
                ..
            }
        ));
        assert!(!production_pack.exists());
        let output_rom = directory.path().join("preview.nds");
        let resources = ResourcePaths {
            profile: profile_path,
            japanese_voicepack: directory.path().join("unused-jp.nvpack"),
            english_voicepack: directory.path().join("unused-en.nvpack"),
        };
        let result = build_test_rom(
            &project_dir,
            &rom_path,
            &output_rom,
            &resources,
            BuildScope::Preview,
            |_| {},
        )
        .unwrap();
        assert_eq!(result.voice_pack.voices, 6_475);
        assert_eq!(result.voice_pack.silent_preview_entries, 6_475);
        assert_eq!(result.rom.voices, 6_475);
        let (restored, located) = receipt::restore_base(&output_rom).unwrap();
        assert_eq!(located.receipt.language, receipt::PatchedLanguage::French);
        assert_eq!(restored, read_clean_or_restored_base(&rom_path).unwrap());
    }
}
