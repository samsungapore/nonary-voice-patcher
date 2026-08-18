use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

use super::recording::RecordingSummary;
use crate::patcher::{
    fsb,
    nds::{self, NdsRom},
    profile::{ProfileError, VoiceProfile},
    receipt::{self, ReceiptError},
};

const PROJECT_VERSION: u32 = 1;
const PROJECT_FILE: &str = "project.nvdub.json";
pub(crate) const PROJECT_LOCK_FILE: &str = ".project.nvdub.lock";
const CONTEXT_RADIUS: usize = 2;
const MAX_ROM_SIZE: u64 = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WAV_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WAV_CHUNK_OVERHEAD: u64 = 1024 * 1024;
const MAX_SAMPLE_RATE: u32 = 384_000;
const MAX_RECORDING_MS: u64 = 45_000;
const MAX_NOTES_CHARS: usize = 4_000;
const MIN_GAIN_DB: f32 = -60.0;
const MAX_GAIN_DB: f32 = 24.0;
const APPROVAL_DOMAIN: &[u8] = b"NVDUB_APPROVAL_V1\0";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    #[default]
    Missing,
    Recorded,
    NeedsReview,
    Approved,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeMetadata {
    pub id: String,
    pub file: String,
    pub created_at_ms: u64,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub frames: u64,
    pub peak_dbfs: f32,
    pub clipped_samples: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetProgress {
    pub target_id: String,
    #[serde(default)]
    pub status: TargetStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_take: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<TakeMetadata>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub trim_start_ms: u64,
    #[serde(default)]
    pub trim_end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingProjectManifest {
    pub version: u32,
    pub name: String,
    pub profile_sha256: String,
    pub profile_catalog_sha256: String,
    pub source_rom_sha256: String,
    pub source_rom_filename: String,
    pub game_code: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub targets: BTreeMap<String, TargetProgress>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingContextLine {
    pub ordinal: usize,
    pub speaker: String,
    pub speaker_raw: String,
    pub text: String,
    pub is_current: bool,
    pub is_recordable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingCue {
    pub index: usize,
    pub target_id: String,
    pub symbol: String,
    pub script_path: String,
    pub function_name: String,
    pub ordinal: usize,
    pub speaker: String,
    pub speaker_raw: String,
    pub text: String,
    pub context: Vec<DubbingContextLine>,
    pub progress: TargetProgress,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingProjectSummary {
    pub total: usize,
    pub recorded: usize,
    pub needs_review: usize,
    pub approved: usize,
    pub skipped: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingProjectSnapshot {
    pub project_dir: String,
    pub manifest_path: String,
    pub rom_path: String,
    pub manifest: DubbingProjectManifest,
    pub summary: DubbingProjectSummary,
    pub cues: Vec<DubbingCue>,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid project JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid Nintendo DS ROM: {0}")]
    Nds(#[from] nds::NdsError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error("invalid FSB script: {0}")]
    Fsb(#[from] fsb::FsbError),
    #[error("invalid dubbing project: {0}")]
    Invalid(String),
}

fn io_error(path: &Path, source: io::Error) -> ProjectError {
    ProjectError::Io {
        path: path.to_owned(),
        source,
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), ProjectError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), ProjectError> {
    Ok(())
}

fn remove_link_after_failed_commit(path: &Path, parent: &Path) {
    let _ = fs::remove_file(path);
    let _ = sync_directory(parent);
}

pub(crate) struct ProjectMutationLock {
    file: File,
}

impl Drop for ProjectMutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn lock_project_mutations(
    project_dir: &Path,
) -> Result<ProjectMutationLock, ProjectError> {
    ensure_project_root(project_dir)?;
    let lock_path = project_dir.join(PROJECT_LOCK_FILE);
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ProjectError::Invalid(format!(
                "{} must be a regular file, not a symlink",
                lock_path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(&lock_path, error)),
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| io_error(&lock_path, error))?;
    if fs::symlink_metadata(&lock_path)
        .map_err(|error| io_error(&lock_path, error))?
        .file_type()
        .is_symlink()
    {
        return Err(ProjectError::Invalid(format!(
            "{} must not be a symlink",
            lock_path.display()
        )));
    }
    file.lock_exclusive()
        .map_err(|error| io_error(&lock_path, error))?;
    Ok(ProjectMutationLock { file })
}

fn now_ms() -> Result<u64, ProjectError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ProjectError::Invalid(format!("system clock is invalid: {error}")))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| ProjectError::Invalid("system time exceeds the project format".to_owned()))
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn read_limited_regular_file(path: &Path, limit: u64) -> Result<Vec<u8>, ProjectError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProjectError::Invalid(format!(
            "{} must be a regular file, not a symlink",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(ProjectError::Invalid(format!(
            "{} exceeds the {} byte safety limit",
            path.display(),
            limit
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ProjectError::Invalid(format!("{} is too large for this system", path.display()))
    })?;
    let mut data = Vec::with_capacity(capacity);
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| io_error(path, error))?;
    if data.len() as u64 > limit {
        return Err(ProjectError::Invalid(format!(
            "{} grew beyond the {} byte safety limit while being read",
            path.display(),
            limit
        )));
    }
    Ok(data)
}

fn ensure_directory_no_symlink(path: &Path) -> Result<PathBuf, ProjectError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ProjectError::Invalid(format!(
            "{} must be a real directory, not a symlink",
            path.display()
        )));
    }
    path.canonicalize().map_err(|error| io_error(path, error))
}

fn ensure_project_root(project_dir: &Path) -> Result<PathBuf, ProjectError> {
    ensure_directory_no_symlink(project_dir)
}

fn ensure_child_directory(
    project_dir: &Path,
    components: &[&str],
    create_missing: bool,
) -> Result<PathBuf, ProjectError> {
    let root = ensure_project_root(project_dir)?;
    let mut directory = project_dir.to_owned();
    for component in components {
        if !valid_path_component(component) {
            return Err(ProjectError::Invalid(format!(
                "invalid project directory component {component:?}"
            )));
        }
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ProjectError::Invalid(format!(
                        "{} must be a real directory, not a symlink",
                        directory.display()
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && create_missing => {
                fs::create_dir(&directory).map_err(|error| io_error(&directory, error))?;
                ensure_directory_no_symlink(&directory)?;
            }
            Err(error) => return Err(io_error(&directory, error)),
        }
        let canonical = directory
            .canonicalize()
            .map_err(|error| io_error(&directory, error))?;
        if !canonical.starts_with(&root) {
            return Err(ProjectError::Invalid(format!(
                "{} escapes the project directory",
                directory.display()
            )));
        }
    }
    Ok(directory)
}

fn read_rom_base(path: &Path) -> Result<Vec<u8>, ProjectError> {
    match receipt::locate(path) {
        Ok(located) => {
            if located.receipt.original_len > MAX_ROM_SIZE {
                return Err(ProjectError::Invalid(
                    "the source ROM exceeds the Nintendo DS 512 MiB limit".to_owned(),
                ));
            }
            Ok(receipt::restore_base(path)?.0)
        }
        Err(ReceiptError::NotPatched) => read_limited_regular_file(path, MAX_ROM_SIZE),
        Err(error) => Err(error.into()),
    }
}

fn project_paths(project_dir: &Path) -> (PathBuf, PathBuf) {
    (
        project_dir.join(PROJECT_FILE),
        project_dir.join("recordings"),
    )
}

fn validate_name(name: &str) -> Result<String, ProjectError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 120 || trimmed.chars().any(char::is_control)
    {
        return Err(ProjectError::Invalid(
            "the project name must contain 1 to 120 printable characters".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn target_id(script_path: &str, ordinal: usize) -> String {
    format!("{script_path}#{ordinal}")
}

fn validate_relative_file(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_path_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn expected_take_file(symbol: &str, take_id: &str) -> String {
    format!("recordings/{symbol}/{take_id}.wav")
}

fn validate_notes(notes: &str) -> Result<(), ProjectError> {
    if notes.chars().count() > MAX_NOTES_CHARS
        || notes
            .chars()
            .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
    {
        return Err(ProjectError::Invalid(format!(
            "target notes may contain at most {MAX_NOTES_CHARS} printable characters"
        )));
    }
    Ok(())
}

fn duration_ms(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    let numerator = u128::from(frames) * 1_000 + u128::from(sample_rate / 2);
    (numerator / u128::from(sample_rate)) as u64
}

fn approval_sha256(target: &TargetProgress, take: &TakeMetadata) -> String {
    let mut digest = Sha256::new();
    digest.update(APPROVAL_DOMAIN);
    digest.update(take.sha256.as_bytes());
    digest.update(target.gain_db.to_bits().to_le_bytes());
    digest.update(target.trim_start_ms.to_le_bytes());
    digest.update(target.trim_end_ms.to_le_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn active_take<'a>(
    target: &'a TargetProgress,
    symbol: &str,
) -> Result<&'a TakeMetadata, ProjectError> {
    let take_id = target.active_take.as_deref().ok_or_else(|| {
        ProjectError::Invalid(format!("target {symbol} does not select an active take"))
    })?;
    target
        .takes
        .iter()
        .find(|take| take.id == take_id)
        .ok_or_else(|| ProjectError::Invalid(format!("target {symbol} selects a missing take")))
}

fn status_requires_take(status: TargetStatus) -> bool {
    matches!(
        status,
        TargetStatus::Recorded | TargetStatus::NeedsReview | TargetStatus::Approved
    )
}

fn validate_target_progress(symbol: &str, target: &TargetProgress) -> Result<(), ProjectError> {
    validate_notes(&target.notes)?;
    if !target.gain_db.is_finite() || target.gain_db < MIN_GAIN_DB || target.gain_db > MAX_GAIN_DB {
        return Err(ProjectError::Invalid(format!(
            "target {symbol} gain must be finite and between {MIN_GAIN_DB} and {MAX_GAIN_DB} dB"
        )));
    }
    if target.trim_start_ms > MAX_RECORDING_MS || target.trim_end_ms > MAX_RECORDING_MS {
        return Err(ProjectError::Invalid(format!(
            "target {symbol} trim values exceed the recording limit"
        )));
    }
    if status_requires_take(target.status) && target.active_take.is_none() {
        return Err(ProjectError::Invalid(format!(
            "target {symbol} requires an active take in status {:?}",
            target.status
        )));
    }

    let mut take_ids = BTreeSet::new();
    for take in &target.takes {
        let expected_duration = duration_ms(take.frames, take.sample_rate);
        if take.id.is_empty()
            || !take.id.starts_with("take-")
            || !valid_path_component(&take.id)
            || !take_ids.insert(take.id.as_str())
            || !validate_relative_file(&take.file)
            || take.file != expected_take_file(symbol, &take.id)
            || take.duration_ms == 0
            || take.duration_ms > MAX_RECORDING_MS
            || take.duration_ms != expected_duration
            || take.sample_rate == 0
            || take.sample_rate > MAX_SAMPLE_RATE
            || take.frames == 0
            || take.frames > u64::from(take.sample_rate) * (MAX_RECORDING_MS / 1_000)
            || !take.peak_dbfs.is_finite()
            || !(-200.0..=60.0).contains(&take.peak_dbfs)
            || take.clipped_samples > take.frames
            || !valid_sha256(&take.sha256)
        {
            return Err(ProjectError::Invalid(format!(
                "target {symbol} contains invalid take metadata"
            )));
        }
    }

    let selected = match target.active_take.as_ref() {
        Some(_) => Some(active_take(target, symbol)?),
        None => None,
    };
    if let Some(take) = selected {
        let total_trim = target
            .trim_start_ms
            .checked_add(target.trim_end_ms)
            .ok_or_else(|| ProjectError::Invalid(format!("target {symbol} trim overflow")))?;
        if total_trim >= take.duration_ms {
            return Err(ProjectError::Invalid(format!(
                "target {symbol} trims away the entire active take"
            )));
        }
    }

    match target.status {
        TargetStatus::Approved => {
            let take = selected.expect("approved targets were required to have an active take");
            let expected = approval_sha256(target, take);
            if target.approval_sha256.as_deref() != Some(expected.as_str()) {
                return Err(ProjectError::Invalid(format!(
                    "target {symbol} approval does not match its active take and processing settings"
                )));
            }
        }
        _ if target.approval_sha256.is_some() => {
            return Err(ProjectError::Invalid(format!(
                "target {symbol} retains an approval signature outside approved status"
            )));
        }
        _ => {}
    }
    Ok(())
}

fn validate_manifest(
    manifest: &DubbingProjectManifest,
    profile: &VoiceProfile,
) -> Result<(), ProjectError> {
    if manifest.version != PROJECT_VERSION {
        return Err(ProjectError::Invalid(format!(
            "unsupported project version {}",
            manifest.version
        )));
    }
    validate_name(&manifest.name)?;
    if !valid_sha256(&manifest.profile_sha256)
        || !valid_sha256(&manifest.profile_catalog_sha256)
        || !valid_sha256(&manifest.source_rom_sha256)
    {
        return Err(ProjectError::Invalid(
            "project fingerprints are malformed".to_owned(),
        ));
    }

    let expected = profile
        .scripts
        .iter()
        .flat_map(|script| {
            script.voices.iter().map(|voice| {
                (
                    voice.symbol.as_str(),
                    target_id(&script.path, voice.ordinal),
                )
            })
        })
        .collect::<Vec<_>>();
    if manifest.targets.len() != expected.len() {
        return Err(ProjectError::Invalid(format!(
            "project contains {} targets instead of {}",
            manifest.targets.len(),
            expected.len()
        )));
    }
    for (symbol, expected_id) in expected {
        let progress = manifest
            .targets
            .get(symbol)
            .ok_or_else(|| ProjectError::Invalid(format!("project is missing target {symbol}")))?;
        if progress.target_id != expected_id {
            return Err(ProjectError::Invalid(format!(
                "target {symbol} points to {:?} instead of {:?}",
                progress.target_id, expected_id
            )));
        }
        validate_target_progress(symbol, progress)?;
    }
    let expected_catalog = profile.catalog_sha256_hex();
    if manifest.profile_catalog_sha256 != expected_catalog {
        return Err(ProjectError::Invalid(format!(
            "the project voice catalog is {}, but this application provides {expected_catalog}",
            manifest.profile_catalog_sha256
        )));
    }
    Ok(())
}

fn write_manifest_atomic(
    project_dir: &Path,
    manifest_path: &Path,
    manifest: &DubbingProjectManifest,
    expected_sha256: Option<[u8; 32]>,
) -> Result<(), ProjectError> {
    let json = serde_json::to_vec_pretty(manifest)?;
    if json.len() as u64 + 1 > MAX_MANIFEST_BYTES {
        return Err(ProjectError::Invalid(format!(
            "project manifest exceeds the {MAX_MANIFEST_BYTES} byte safety limit"
        )));
    }
    ensure_project_root(project_dir)?;
    match fs::symlink_metadata(manifest_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ProjectError::Invalid(format!(
                "{} must be a regular file, not a symlink",
                manifest_path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(manifest_path, error)),
    }
    let mut temporary =
        NamedTempFile::new_in(project_dir).map_err(|error| io_error(project_dir, error))?;
    temporary
        .write_all(&json)
        .and_then(|_| temporary.write_all(b"\n"))
        .and_then(|_| temporary.as_file_mut().sync_all())
        .map_err(|error| io_error(temporary.path(), error))?;
    match expected_sha256 {
        Some(expected) => {
            let current = read_limited_regular_file(manifest_path, MAX_MANIFEST_BYTES)?;
            let current_sha256: [u8; 32] = Sha256::digest(&current).into();
            if current_sha256 != expected {
                return Err(ProjectError::Invalid(
                    "the project changed in another process; reload it before saving".to_owned(),
                ));
            }
        }
        None if manifest_path.exists() => {
            return Err(ProjectError::Invalid(format!(
                "a dubbing project already exists at {}",
                manifest_path.display()
            )));
        }
        None => {}
    }
    temporary
        .persist(manifest_path)
        .map_err(|error| io_error(manifest_path, error.error))?;
    // Directory fsync makes the manifest rename durable on Unix. Windows does
    // not allow opening directories as regular files, while the file itself
    // has already been flushed before the atomic replacement.
    #[cfg(unix)]
    File::open(project_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(project_dir, error))?;
    Ok(())
}

fn load_manifest_with_profile(
    project_dir: &Path,
    profile_path: &Path,
) -> Result<(PathBuf, DubbingProjectManifest, VoiceProfile, [u8; 32]), ProjectError> {
    ensure_project_root(project_dir)?;
    let (manifest_path, _) = project_paths(project_dir);
    let profile_data = read_limited_regular_file(profile_path, MAX_MANIFEST_BYTES)?;
    let profile = VoiceProfile::from_json(&profile_data)?;
    let manifest_data = read_limited_regular_file(&manifest_path, MAX_MANIFEST_BYTES)?;
    let manifest_sha256 = Sha256::digest(&manifest_data).into();
    let manifest: DubbingProjectManifest = serde_json::from_slice(&manifest_data)?;
    validate_manifest(&manifest, &profile)?;
    Ok((manifest_path, manifest, profile, manifest_sha256))
}

fn validate_capture_path(
    project_dir: &Path,
    symbol: &str,
    path: &Path,
) -> Result<(), ProjectError> {
    let directory = ensure_child_directory(project_dir, &["recordings", symbol], false)?;
    let expected_parent = directory
        .canonicalize()
        .map_err(|error| io_error(&directory, error))?;
    let actual_parent = path
        .parent()
        .ok_or_else(|| ProjectError::Invalid("recording path has no parent".to_owned()))?
        .canonicalize()
        .map_err(|error| io_error(path, error))?;
    let valid_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with(".capture-") && name.ends_with(".wav"));
    if actual_parent != expected_parent || !valid_name {
        return Err(ProjectError::Invalid(
            "recording path is outside the selected target directory".to_owned(),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProjectError::Invalid(
            "recording capture must be a regular file, not a symlink".to_owned(),
        ));
    }
    Ok(())
}

struct WavInspection {
    sample_rate: u32,
    frames: u64,
    endpoint_samples: u64,
}

fn inspect_wav(data: &[u8]) -> Result<WavInspection, ProjectError> {
    let mut reader = hound::WavReader::new(Cursor::new(data))
        .map_err(|error| ProjectError::Invalid(format!("WAV cannot be parsed: {error}")))?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate == 0
        || spec.sample_rate > MAX_SAMPLE_RATE
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(ProjectError::Invalid(format!(
            "WAV must be mono 16-bit PCM at no more than {MAX_SAMPLE_RATE} Hz"
        )));
    }
    let mut frames = 0_u64;
    let mut endpoint_samples = 0_u64;
    for sample in reader.samples::<i16>() {
        let sample = sample.map_err(|error| {
            ProjectError::Invalid(format!("WAV sample data is corrupt: {error}"))
        })?;
        frames = frames
            .checked_add(1)
            .ok_or_else(|| ProjectError::Invalid("WAV frame count overflow".to_owned()))?;
        if sample == i16::MIN || sample == i16::MAX {
            endpoint_samples += 1;
        }
    }
    if frames == 0 || frames > u64::from(spec.sample_rate) * (MAX_RECORDING_MS / 1_000) {
        return Err(ProjectError::Invalid(
            "WAV is empty or exceeds the recording duration limit".to_owned(),
        ));
    }
    let pcm_bytes = frames
        .checked_mul(2)
        .ok_or_else(|| ProjectError::Invalid("WAV PCM size overflow".to_owned()))?;
    let minimum_size = pcm_bytes
        .checked_add(44)
        .ok_or_else(|| ProjectError::Invalid("WAV size overflow".to_owned()))?;
    let maximum_size = pcm_bytes
        .checked_add(MAX_WAV_CHUNK_OVERHEAD)
        .ok_or_else(|| ProjectError::Invalid("WAV size overflow".to_owned()))?
        .min(MAX_WAV_BYTES);
    let file_size = data.len() as u64;
    if file_size < minimum_size || file_size > maximum_size {
        return Err(ProjectError::Invalid(format!(
            "WAV size {file_size} does not match its {frames}-frame PCM payload"
        )));
    }
    Ok(WavInspection {
        sample_rate: spec.sample_rate,
        frames,
        endpoint_samples,
    })
}

fn validate_recorded_wav(summary: &RecordingSummary) -> Result<String, ProjectError> {
    if summary.overflowed
        || summary.duration_ms == 0
        || summary.duration_ms > MAX_RECORDING_MS
        || summary.sample_rate == 0
        || summary.sample_rate > MAX_SAMPLE_RATE
        || summary.frames == 0
        || summary.frames > u64::from(summary.sample_rate) * (MAX_RECORDING_MS / 1_000)
        || !summary.peak_dbfs.is_finite()
        || !(-200.0..=60.0).contains(&summary.peak_dbfs)
        || summary.clipped_samples > summary.frames
    {
        return Err(ProjectError::Invalid(
            "the captured WAV summary is invalid".to_owned(),
        ));
    }
    if summary.duration_ms != duration_ms(summary.frames, summary.sample_rate) {
        return Err(ProjectError::Invalid(
            "captured WAV duration does not match its reported rate and frame count".to_owned(),
        ));
    }
    let data = read_limited_regular_file(&summary.path, MAX_WAV_BYTES)?;
    let inspected = inspect_wav(&data)?;
    if inspected.sample_rate != summary.sample_rate
        || inspected.frames != summary.frames
        || summary.clipped_samples > inspected.endpoint_samples
    {
        return Err(ProjectError::Invalid(
            "captured WAV content does not match its reported recording metadata".to_owned(),
        ));
    }
    Ok(sha256_hex(&data))
}

fn validate_take_file(
    project_dir: &Path,
    symbol: &str,
    take: &TakeMetadata,
) -> Result<Vec<u8>, ProjectError> {
    if take.file != expected_take_file(symbol, &take.id) {
        return Err(ProjectError::Invalid(format!(
            "take {} for {symbol} has an unexpected path",
            take.id
        )));
    }
    let directory = ensure_child_directory(project_dir, &["recordings", symbol], false)?;
    let path = project_dir.join(&take.file);
    let parent = path
        .parent()
        .ok_or_else(|| ProjectError::Invalid("take path has no parent".to_owned()))?
        .canonicalize()
        .map_err(|error| io_error(&path, error))?;
    let expected_parent = directory
        .canonicalize()
        .map_err(|error| io_error(&directory, error))?;
    if parent != expected_parent {
        return Err(ProjectError::Invalid(format!(
            "take {} for {symbol} escapes its recording directory",
            take.id
        )));
    }
    let data = read_limited_regular_file(&path, MAX_WAV_BYTES)?;
    let inspected = inspect_wav(&data)?;
    if inspected.sample_rate != take.sample_rate
        || inspected.frames != take.frames
        || duration_ms(inspected.frames, inspected.sample_rate) != take.duration_ms
        || take.clipped_samples > inspected.endpoint_samples
    {
        return Err(ProjectError::Invalid(format!(
            "take {} for {symbol} no longer matches its metadata",
            take.id
        )));
    }
    let actual_hash = sha256_hex(&data);
    if actual_hash != take.sha256 {
        return Err(ProjectError::Invalid(format!(
            "take {} for {symbol} failed its SHA-256 integrity check",
            take.id
        )));
    }
    Ok(data)
}

fn validate_all_take_files(
    project_dir: &Path,
    manifest: &DubbingProjectManifest,
) -> Result<(), ProjectError> {
    ensure_child_directory(project_dir, &["recordings"], false)?;
    for (symbol, target) in &manifest.targets {
        for take in &target.takes {
            validate_take_file(project_dir, symbol, take)?;
        }
    }
    Ok(())
}

fn display_speaker(raw: &[u8]) -> (String, String) {
    let raw = fsb::decode_display_text(raw);
    let key = raw.trim().trim_start_matches('&');
    let display = match key {
        "淳平" => "Junpei",
        "一宮" => "Ace",
        "ニルス" | "ニルス２" => "Snake",
        "サンタ" => "Santa",
        "四葉" => "Clover",
        "紫" => "June",
        "セブン" => "Seven",
        "八代" => "Lotus",
        "茜" | "茜２" => "Akane",
        "ゼロ" => "Zero",
        "放送" => "Announcement",
        "？？？" | "？？？１" => "???",
        _ => key,
    };
    (display.to_owned(), key.to_owned())
}

fn summary(manifest: &DubbingProjectManifest) -> DubbingProjectSummary {
    let mut result = DubbingProjectSummary {
        total: manifest.targets.len(),
        recorded: 0,
        needs_review: 0,
        approved: 0,
        skipped: 0,
    };
    for target in manifest.targets.values() {
        if target.active_take.is_some() {
            result.recorded += 1;
        }
        match target.status {
            TargetStatus::NeedsReview => result.needs_review += 1,
            TargetStatus::Approved => result.approved += 1,
            TargetStatus::Skipped => result.skipped += 1,
            TargetStatus::Missing | TargetStatus::Recorded => {}
        }
    }
    result
}

fn build_cues(
    rom: &NdsRom<'_>,
    profile: &VoiceProfile,
    manifest: &DubbingProjectManifest,
) -> Result<Vec<DubbingCue>, ProjectError> {
    let mut cues = Vec::with_capacity(profile.voice_count);
    for script_profile in &profile.scripts {
        let data = rom
            .data_for_path(&script_profile.path)
            .ok_or_else(|| ProfileError::MissingScript(script_profile.path.clone()))?;
        let parsed = script_profile.parse_compatible(data)?.script;
        let lines = parsed.dialogue_lines();
        let line_indices = lines
            .iter()
            .enumerate()
            .map(|(index, line)| (line.ordinal, index))
            .collect::<HashMap<_, _>>();
        let recordable = script_profile
            .voices
            .iter()
            .map(|voice| voice.ordinal)
            .collect::<BTreeSet<_>>();

        for voice in &script_profile.voices {
            let line_index = *line_indices.get(&voice.ordinal).ok_or_else(|| {
                ProjectError::Invalid(format!(
                    "{} has no text ordinal {}",
                    script_profile.path, voice.ordinal
                ))
            })?;
            let line = &lines[line_index];
            let (speaker, speaker_raw) = display_speaker(&line.speaker);
            let function_lines = lines
                .iter()
                .filter(|candidate| {
                    candidate.function_name.as_slice() == line.function_name.as_slice()
                })
                .collect::<Vec<_>>();
            let function_line_index = function_lines
                .iter()
                .position(|candidate| candidate.ordinal == voice.ordinal)
                .expect("the selected line belongs to its function");
            let context_start = function_line_index.saturating_sub(CONTEXT_RADIUS);
            let context_end = (function_line_index + CONTEXT_RADIUS + 1).min(function_lines.len());
            let context = function_lines[context_start..context_end]
                .iter()
                .map(|context_line| {
                    let (context_speaker, context_speaker_raw) =
                        display_speaker(&context_line.speaker);
                    DubbingContextLine {
                        ordinal: context_line.ordinal,
                        speaker: context_speaker,
                        speaker_raw: context_speaker_raw,
                        text: fsb::decode_display_text(&context_line.text),
                        is_current: context_line.ordinal == voice.ordinal,
                        is_recordable: recordable.contains(&context_line.ordinal),
                    }
                })
                .collect();
            let progress = manifest
                .targets
                .get(&voice.symbol)
                .ok_or_else(|| {
                    ProjectError::Invalid(format!("project is missing {}", voice.symbol))
                })?
                .clone();
            cues.push(DubbingCue {
                index: cues.len(),
                target_id: progress.target_id.clone(),
                symbol: voice.symbol.clone(),
                script_path: script_profile.path.clone(),
                function_name: fsb::decode_display_text(&line.function_name),
                ordinal: voice.ordinal,
                speaker,
                speaker_raw,
                text: fsb::decode_display_text(&line.text),
                context,
                progress,
            });
        }
    }
    if cues.len() != profile.voice_count {
        return Err(ProjectError::Invalid(format!(
            "extracted {} cues instead of {}",
            cues.len(),
            profile.voice_count
        )));
    }
    Ok(cues)
}

fn load_inputs(
    rom_path: &Path,
    profile_path: &Path,
) -> Result<(Vec<u8>, Vec<u8>, VoiceProfile), ProjectError> {
    let profile_data = read_limited_regular_file(profile_path, MAX_MANIFEST_BYTES)?;
    let profile = VoiceProfile::from_json(&profile_data)?;
    let base = read_rom_base(rom_path)?;
    let rom = NdsRom::parse(&base)?;
    profile.validate_rom(&rom)?;
    Ok((base, profile_data, profile))
}

pub fn create_project(
    project_dir: impl AsRef<Path>,
    rom_path: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    name: &str,
) -> Result<DubbingProjectSnapshot, ProjectError> {
    let project_dir = project_dir.as_ref();
    let rom_path = rom_path.as_ref();
    let profile_path = profile_path.as_ref();
    fs::create_dir_all(project_dir).map_err(|error| io_error(project_dir, error))?;
    ensure_project_root(project_dir)?;
    let _project_lock = lock_project_mutations(project_dir)?;
    let (manifest_path, recordings_path) = project_paths(project_dir);
    if manifest_path.exists() {
        return Err(ProjectError::Invalid(format!(
            "a dubbing project already exists at {}",
            manifest_path.display()
        )));
    }

    let (base, profile_data, profile) = load_inputs(rom_path, profile_path)?;
    let rom = NdsRom::parse(&base)?;
    let created_at_ms = now_ms()?;
    let mut targets = BTreeMap::new();
    for script in &profile.scripts {
        for voice in &script.voices {
            targets.insert(
                voice.symbol.clone(),
                TargetProgress {
                    target_id: target_id(&script.path, voice.ordinal),
                    status: TargetStatus::Missing,
                    active_take: None,
                    takes: Vec::new(),
                    notes: String::new(),
                    gain_db: 0.0,
                    trim_start_ms: 0,
                    trim_end_ms: 0,
                    approval_sha256: None,
                },
            );
        }
    }
    let manifest = DubbingProjectManifest {
        version: PROJECT_VERSION,
        name: validate_name(name)?,
        profile_sha256: sha256_hex(&profile_data),
        profile_catalog_sha256: profile.catalog_sha256_hex(),
        source_rom_sha256: sha256_hex(&base),
        source_rom_filename: rom_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("999.nds")
            .to_owned(),
        game_code: String::from_utf8_lossy(&rom.header().game_code).into_owned(),
        created_at_ms,
        updated_at_ms: created_at_ms,
        targets,
    };
    validate_manifest(&manifest, &profile)?;
    ensure_child_directory(project_dir, &["recordings"], true)?;
    debug_assert_eq!(recordings_path, project_dir.join("recordings"));
    write_manifest_atomic(project_dir, &manifest_path, &manifest, None)?;
    let cues = build_cues(&rom, &profile, &manifest)?;
    Ok(DubbingProjectSnapshot {
        project_dir: project_dir.to_string_lossy().into_owned(),
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        rom_path: rom_path.to_string_lossy().into_owned(),
        summary: summary(&manifest),
        manifest,
        cues,
    })
}

pub fn open_project(
    project_dir: impl AsRef<Path>,
    rom_path: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
) -> Result<DubbingProjectSnapshot, ProjectError> {
    let project_dir = project_dir.as_ref();
    let rom_path = rom_path.as_ref();
    let profile_path = profile_path.as_ref();
    ensure_project_root(project_dir)?;
    let (manifest_path, _) = project_paths(project_dir);
    let manifest: DubbingProjectManifest = serde_json::from_slice(&read_limited_regular_file(
        &manifest_path,
        MAX_MANIFEST_BYTES,
    )?)?;
    let (base, _profile_data, profile) = load_inputs(rom_path, profile_path)?;
    validate_manifest(&manifest, &profile)?;
    let rom_hash = sha256_hex(&base);
    if manifest.source_rom_sha256 != rom_hash {
        return Err(ProjectError::Invalid(format!(
            "the selected ROM has SHA-256 {rom_hash}, but this project requires {}",
            manifest.source_rom_sha256
        )));
    }
    validate_all_take_files(project_dir, &manifest)?;
    let rom = NdsRom::parse(&base)?;
    let cues = build_cues(&rom, &profile, &manifest)?;
    Ok(DubbingProjectSnapshot {
        project_dir: project_dir.to_string_lossy().into_owned(),
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        rom_path: rom_path.to_string_lossy().into_owned(),
        summary: summary(&manifest),
        manifest,
        cues,
    })
}

pub fn update_target(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
    status: TargetStatus,
    notes: String,
) -> Result<TargetProgress, ProjectError> {
    let project_dir = project_dir.as_ref();
    let profile_path = profile_path.as_ref();
    let _project_lock = lock_project_mutations(project_dir)?;
    let (manifest_path, mut manifest, _profile, manifest_sha256) =
        load_manifest_with_profile(project_dir, profile_path)?;
    validate_notes(&notes)?;
    let existing = manifest
        .targets
        .get(symbol)
        .ok_or_else(|| ProjectError::Invalid(format!("unknown voice target {symbol}")))?;
    if status_requires_take(status) && existing.active_take.is_none() {
        return Err(ProjectError::Invalid(
            "a target cannot enter this status without an active take".to_owned(),
        ));
    }
    if status == TargetStatus::Approved {
        let take = active_take(existing, symbol)?;
        validate_take_file(project_dir, symbol, take)?;
    }
    let target = manifest
        .targets
        .get_mut(symbol)
        .expect("target existence was checked");
    target.status = status;
    target.notes = notes;
    target.approval_sha256 = if status == TargetStatus::Approved {
        let take = active_take(target, symbol)?;
        Some(approval_sha256(target, take))
    } else {
        None
    };
    validate_target_progress(symbol, target)?;
    let updated = target.clone();
    manifest.updated_at_ms = now_ms()?;
    write_manifest_atomic(
        project_dir,
        &manifest_path,
        &manifest,
        Some(manifest_sha256),
    )?;
    Ok(updated)
}

pub fn prepare_recording_path(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
) -> Result<PathBuf, ProjectError> {
    let project_dir = project_dir.as_ref();
    let profile_path = profile_path.as_ref();
    let (_, manifest, _, _) = load_manifest_with_profile(project_dir, profile_path)?;
    if !manifest.targets.contains_key(symbol) {
        return Err(ProjectError::Invalid(format!(
            "unknown voice target {symbol}"
        )));
    }
    let directory = ensure_child_directory(project_dir, &["recordings", symbol], true)?;
    let timestamp = now_ms()?;
    for suffix in 0..1_000_u16 {
        let path = directory.join(format!(".capture-{timestamp}-{suffix:03}.wav"));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(ProjectError::Invalid(
        "could not allocate a unique recording path".to_owned(),
    ))
}

pub fn commit_recording(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
    summary: &RecordingSummary,
) -> Result<(TakeMetadata, TargetProgress), ProjectError> {
    let project_dir = project_dir.as_ref();
    let profile_path = profile_path.as_ref();
    let _project_lock = lock_project_mutations(project_dir)?;
    let (manifest_path, mut manifest, _, manifest_sha256) =
        load_manifest_with_profile(project_dir, profile_path)?;
    if !manifest.targets.contains_key(symbol) {
        return Err(ProjectError::Invalid(format!(
            "unknown voice target {symbol}"
        )));
    }
    validate_capture_path(project_dir, symbol, &summary.path)?;
    validate_recorded_wav(summary)?;

    let timestamp = now_ms()?;
    let directory = ensure_child_directory(project_dir, &["recordings", symbol], false)?;
    let mut reserved = None;
    for suffix in 0..1_000_u16 {
        let take_id = format!("take-{timestamp}-{suffix:03}");
        let destination = directory.join(format!("{take_id}.wav"));
        match fs::hard_link(&summary.path, &destination) {
            Ok(()) => {
                reserved = Some((take_id, destination));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(&destination, error)),
        }
    }
    let (take_id, destination) = reserved
        .ok_or_else(|| ProjectError::Invalid("could not allocate a take filename".to_owned()))?;
    let mut linked_summary = summary.clone();
    linked_summary.path.clone_from(&destination);
    let take_sha256 = match validate_recorded_wav(&linked_summary) {
        Ok(hash) => hash,
        Err(error) => {
            remove_link_after_failed_commit(&destination, &directory);
            return Err(error);
        }
    };
    if let Err(error) = File::open(&destination).and_then(|file| file.sync_all()) {
        remove_link_after_failed_commit(&destination, &directory);
        return Err(io_error(&destination, error));
    }
    if let Err(error) = sync_directory(&directory) {
        remove_link_after_failed_commit(&destination, &directory);
        return Err(error);
    }
    let take = TakeMetadata {
        id: take_id.clone(),
        file: format!("recordings/{symbol}/{take_id}.wav"),
        created_at_ms: timestamp,
        duration_ms: summary.duration_ms,
        sample_rate: summary.sample_rate,
        frames: summary.frames,
        peak_dbfs: summary.peak_dbfs,
        clipped_samples: summary.clipped_samples,
        sha256: take_sha256,
    };
    let target = manifest
        .targets
        .get_mut(symbol)
        .expect("target existence was checked");
    target.takes.push(take.clone());
    target.active_take = Some(take.id.clone());
    target.status = TargetStatus::Recorded;
    target.approval_sha256 = None;
    if let Err(error) = validate_target_progress(symbol, target) {
        remove_link_after_failed_commit(&destination, &directory);
        return Err(error);
    }
    let progress = target.clone();
    manifest.updated_at_ms = timestamp;
    if let Err(error) = write_manifest_atomic(
        project_dir,
        &manifest_path,
        &manifest,
        Some(manifest_sha256),
    ) {
        remove_link_after_failed_commit(&destination, &directory);
        return Err(error);
    }
    let _ = fs::remove_file(&summary.path);
    Ok((take, progress))
}

pub fn select_take(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
    take_id: &str,
) -> Result<TargetProgress, ProjectError> {
    let project_dir = project_dir.as_ref();
    let profile_path = profile_path.as_ref();
    let _project_lock = lock_project_mutations(project_dir)?;
    let (manifest_path, mut manifest, _, manifest_sha256) =
        load_manifest_with_profile(project_dir, profile_path)?;
    let selected_take = manifest
        .targets
        .get(symbol)
        .ok_or_else(|| ProjectError::Invalid(format!("unknown voice target {symbol}")))?
        .takes
        .iter()
        .find(|take| take.id == take_id)
        .cloned()
        .ok_or_else(|| ProjectError::Invalid(format!("target {symbol} has no take {take_id}")))?;
    validate_take_file(project_dir, symbol, &selected_take)?;
    let target = manifest
        .targets
        .get_mut(symbol)
        .expect("target existence was checked");
    target.active_take = Some(take_id.to_owned());
    // Approval belongs to a specific performance, so choosing another take
    // deliberately returns the line to the review queue.
    target.status = TargetStatus::Recorded;
    target.approval_sha256 = None;
    validate_target_progress(symbol, target)?;
    let progress = target.clone();
    manifest.updated_at_ms = now_ms()?;
    write_manifest_atomic(
        project_dir,
        &manifest_path,
        &manifest,
        Some(manifest_sha256),
    )?;
    Ok(progress)
}

pub fn read_take(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
    take_id: &str,
) -> Result<Vec<u8>, ProjectError> {
    let project_dir = project_dir.as_ref();
    let profile_path = profile_path.as_ref();
    let (_, manifest, _, _) = load_manifest_with_profile(project_dir, profile_path)?;
    let target = manifest
        .targets
        .get(symbol)
        .ok_or_else(|| ProjectError::Invalid(format!("unknown voice target {symbol}")))?;
    let take = target
        .takes
        .iter()
        .find(|take| take.id == take_id)
        .ok_or_else(|| ProjectError::Invalid(format!("target {symbol} has no take {take_id}")))?;
    validate_take_file(project_dir, symbol, take)
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

    const TEST_SYMBOL: &str = "SE_V0000";

    fn synthetic_project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let project_dir = temporary.path().join("dubbing");
        fs::create_dir(&project_dir).unwrap();
        fs::create_dir(project_dir.join("recordings")).unwrap();
        let profile_path = temporary.path().join("voice-profile.json");
        let profile_data = format!(
            r#"{{
  "version": 1,
  "game_code": "BSKE",
  "voice_count": 1,
  "scripts": [{{
    "path": "scr/test.fsb",
    "structural_sha256": "{}",
    "set_text_count": 1,
    "voices": [{{"ordinal": 0, "symbol": "{TEST_SYMBOL}"}}]
  }}]
}}"#,
            "0".repeat(64)
        )
        .into_bytes();
        fs::write(&profile_path, &profile_data).unwrap();
        let profile = VoiceProfile::from_json(&profile_data).unwrap();
        let manifest = DubbingProjectManifest {
            version: PROJECT_VERSION,
            name: "Synthetic project".to_owned(),
            profile_sha256: sha256_hex(&profile_data),
            profile_catalog_sha256: profile.catalog_sha256_hex(),
            source_rom_sha256: "1".repeat(64),
            source_rom_filename: "test.nds".to_owned(),
            game_code: "BSKE".to_owned(),
            created_at_ms: 1,
            updated_at_ms: 1,
            targets: BTreeMap::from([(
                TEST_SYMBOL.to_owned(),
                TargetProgress {
                    target_id: "scr/test.fsb#0".to_owned(),
                    status: TargetStatus::Missing,
                    active_take: None,
                    takes: Vec::new(),
                    notes: String::new(),
                    gain_db: 0.0,
                    trim_start_ms: 0,
                    trim_end_ms: 0,
                    approval_sha256: None,
                },
            )]),
        };
        write_manifest_atomic(
            &project_dir,
            &project_dir.join(PROJECT_FILE),
            &manifest,
            None,
        )
        .unwrap();
        (temporary, project_dir, profile_path)
    }

    #[test]
    fn manifest_compare_and_swap_rejects_an_external_change() {
        let (_temporary, project_dir, _profile_path) = synthetic_project();
        let manifest_path = project_dir.join(PROJECT_FILE);
        let original = fs::read(&manifest_path).unwrap();
        let expected: [u8; 32] = Sha256::digest(&original).into();
        let manifest: DubbingProjectManifest = serde_json::from_slice(&original).unwrap();
        let mut changed = original;
        changed.extend_from_slice(b" \n");
        fs::write(&manifest_path, &changed).unwrap();

        let error = write_manifest_atomic(&project_dir, &manifest_path, &manifest, Some(expected))
            .unwrap_err();

        assert!(error.to_string().contains("changed in another process"));
        assert_eq!(fs::read(manifest_path).unwrap(), changed);
    }

    #[test]
    fn project_lock_waits_until_the_current_snapshot_owner_finishes() {
        let (_temporary, project_dir, _profile_path) = synthetic_project();
        let first = lock_project_mutations(&project_dir).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let contender_dir = project_dir.clone();
        let contender = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _second = lock_project_mutations(&contender_dir).unwrap();
            acquired_tx.send(()).unwrap();
        });

        ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        contender.join().unwrap();
    }

    fn write_test_wav(path: &Path, sample: i16) -> RecordingSummary {
        let sample_rate = 8_000;
        let frames = 800_u64;
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..frames {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        RecordingSummary {
            path: path.to_owned(),
            duration_ms: duration_ms(frames, sample_rate),
            sample_rate,
            frames,
            peak_dbfs: if sample == 0 { -120.0 } else { -6.0 },
            clipped_samples: if sample == i16::MIN || sample == i16::MAX {
                frames
            } else {
                0
            },
            overflowed: false,
        }
    }

    fn capture_and_commit(
        project_dir: &Path,
        profile_path: &Path,
        sample: i16,
    ) -> (TakeMetadata, TargetProgress) {
        let capture = prepare_recording_path(project_dir, profile_path, TEST_SYMBOL).unwrap();
        let summary = write_test_wav(&capture, sample);
        commit_recording(project_dir, profile_path, TEST_SYMBOL, &summary).unwrap()
    }

    #[test]
    fn relative_take_paths_reject_escape_and_absolute_paths() {
        assert!(validate_relative_file("recordings/SE_V0001/take.wav"));
        assert!(!validate_relative_file("../take.wav"));
        assert!(!validate_relative_file("recordings/../take.wav"));
        assert!(!validate_relative_file("/tmp/take.wav"));
    }

    #[test]
    fn speaker_names_are_presented_without_changing_raw_identity() {
        let (display, raw) = display_speaker(&[0x8f, 0x7e, 0x95, 0xbd]);
        assert_eq!(raw, "淳平");
        assert_eq!(display, "Junpei");
    }

    #[test]
    fn target_identity_ignores_decompiler_line_numbers() {
        assert_eq!(target_id("scr/a01b.fsb", 88), "scr/a01b.fsb#88");
    }

    #[test]
    fn committed_takes_are_hashed_selectable_and_readable() {
        let (_temporary, project_dir, profile_path) = synthetic_project();
        let (first, first_progress) = capture_and_commit(&project_dir, &profile_path, 1_000);
        assert_eq!(
            first_progress.active_take.as_deref(),
            Some(first.id.as_str())
        );
        let first_data = read_take(&project_dir, &profile_path, TEST_SYMBOL, &first.id).unwrap();
        assert_eq!(first.sha256, sha256_hex(&first_data));

        let (second, _) = capture_and_commit(&project_dir, &profile_path, 2_000);
        assert_ne!(first.sha256, second.sha256);
        let selected = select_take(&project_dir, &profile_path, TEST_SYMBOL, &first.id).unwrap();
        assert_eq!(selected.active_take.as_deref(), Some(first.id.as_str()));
        assert_eq!(selected.status, TargetStatus::Recorded);
        assert!(selected.approval_sha256.is_none());

        let approved = update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::Approved,
            "Ready".to_owned(),
        )
        .unwrap();
        assert!(approved.approval_sha256.is_some());
        let review = update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::NeedsReview,
            "Check timing".to_owned(),
        )
        .unwrap();
        assert!(review.approval_sha256.is_none());
    }

    #[test]
    fn take_tampering_is_rejected_before_read_selection_or_approval() {
        let (_temporary, project_dir, profile_path) = synthetic_project();
        let (take, _) = capture_and_commit(&project_dir, &profile_path, 1_000);
        let take_path = project_dir.join(&take.file);
        write_test_wav(&take_path, 2_000);

        let read_error = read_take(&project_dir, &profile_path, TEST_SYMBOL, &take.id).unwrap_err();
        assert!(read_error.to_string().contains("SHA-256"));
        assert!(select_take(&project_dir, &profile_path, TEST_SYMBOL, &take.id).is_err());
        assert!(update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::Approved,
            String::new(),
        )
        .is_err());
    }

    #[test]
    fn approval_is_bound_to_take_and_processing_settings() {
        let (_temporary, project_dir, profile_path) = synthetic_project();
        capture_and_commit(&project_dir, &profile_path, 1_000);
        update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::Approved,
            String::new(),
        )
        .unwrap();

        let manifest_path = project_dir.join(PROJECT_FILE);
        let mut manifest: DubbingProjectManifest = serde_json::from_slice(
            &read_limited_regular_file(&manifest_path, MAX_MANIFEST_BYTES).unwrap(),
        )
        .unwrap();
        manifest.targets.get_mut(TEST_SYMBOL).unwrap().gain_db = 1.0;
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let error = read_take(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            manifest.targets[TEST_SYMBOL]
                .active_take
                .as_deref()
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("approval does not match"));
    }

    #[test]
    fn catalog_identity_survives_non_semantic_profile_json_changes() {
        let (_temporary, project_dir, profile_path) = synthetic_project();
        let original = fs::read(&profile_path).unwrap();
        let profile = VoiceProfile::from_json(&original).unwrap();
        let mut reformatted = serde_json::to_vec(&profile).unwrap();
        reformatted.push(b'\n');
        assert_ne!(sha256_hex(&original), sha256_hex(&reformatted));
        fs::write(&profile_path, reformatted).unwrap();

        prepare_recording_path(&project_dir, &profile_path, TEST_SYMBOL).unwrap();
    }

    #[test]
    fn invalid_notes_gain_trims_and_take_free_statuses_are_rejected() {
        let (_temporary, project_dir, profile_path) = synthetic_project();
        assert!(update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::Recorded,
            String::new(),
        )
        .is_err());
        assert!(update_target(
            &project_dir,
            &profile_path,
            TEST_SYMBOL,
            TargetStatus::Missing,
            "bad\0note".to_owned(),
        )
        .is_err());

        let manifest_path = project_dir.join(PROJECT_FILE);
        let mut manifest: DubbingProjectManifest = serde_json::from_slice(
            &read_limited_regular_file(&manifest_path, MAX_MANIFEST_BYTES).unwrap(),
        )
        .unwrap();
        let target = manifest.targets.get_mut(TEST_SYMBOL).unwrap();
        target.gain_db = f32::INFINITY;
        assert!(validate_target_progress(TEST_SYMBOL, target).is_err());
        target.gain_db = 0.0;
        target.trim_start_ms = MAX_RECORDING_MS + 1;
        assert!(validate_target_progress(TEST_SYMBOL, target).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn project_recording_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let (temporary, project_dir, profile_path) = synthetic_project();
        let linked_root = temporary.path().join("linked-project");
        symlink(&project_dir, &linked_root).unwrap();
        assert!(prepare_recording_path(&linked_root, &profile_path, TEST_SYMBOL).is_err());

        let external = temporary.path().join("external-recordings");
        fs::create_dir(&external).unwrap();
        fs::remove_dir(project_dir.join("recordings")).unwrap();
        symlink(&external, project_dir.join("recordings")).unwrap();
        assert!(prepare_recording_path(&project_dir, &profile_path, TEST_SYMBOL).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_take_file_symlink_is_rejected_even_when_its_target_matches() {
        use std::os::unix::fs::symlink;

        let (temporary, project_dir, profile_path) = synthetic_project();
        let (take, _) = capture_and_commit(&project_dir, &profile_path, 1_000);
        let take_path = project_dir.join(&take.file);
        let external = temporary.path().join("external.wav");
        fs::copy(&take_path, &external).unwrap();
        fs::remove_file(&take_path).unwrap();
        symlink(&external, &take_path).unwrap();

        let error = read_take(&project_dir, &profile_path, TEST_SYMBOL, &take.id).unwrap_err();
        assert!(error.to_string().contains("regular file"));
    }

    #[test]
    #[ignore = "requires a private compatible French ROM"]
    fn french_rom_produces_the_complete_text_free_profile_catalog() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let rom = workspace.join("build/french-patcher-validation/runner/999-fr-safe.nds");
        let profile =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json");
        let project = tempfile::tempdir().unwrap();
        let snapshot = create_project(project.path(), rom, profile, "French recording test")
            .expect("the reviewed French ROM should produce a dubbing catalog");

        assert_eq!(snapshot.summary.total, 6_475);
        assert_eq!(snapshot.cues.len(), 6_475);
        assert_eq!(snapshot.cues[0].symbol, "SE_V0000");
        assert_eq!(snapshot.cues[0].speaker, "Junpei");
        assert!(snapshot
            .cues
            .iter()
            .all(|cue| !cue.text.is_empty() && cue.context.iter().any(|line| line.is_current)));
        assert!(snapshot
            .cues
            .iter()
            .all(|cue| !cue.text.contains('\u{fffd}')
                && cue
                    .context
                    .iter()
                    .all(|line| !line.text.contains('\u{fffd}'))));
        assert!(snapshot.cues.iter().any(|cue| cue.text.contains('é')));
    }
}
