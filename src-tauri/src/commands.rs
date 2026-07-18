use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{
    ipc::{Channel, Response},
    path::BaseDirectory,
    AppHandle, Emitter, Manager, State,
};

use crate::dubbing::{
    build::{self, BuildScope, TestRomBuildResult},
    project::{self, DubbingProjectSnapshot, TakeMetadata, TargetProgress, TargetStatus},
    recording::{AudioInputDevice, RecorderState, RecordingStartInfo, RecordingSummary},
};
use crate::patcher::engine::{
    self, ApplyOptions, PatchLanguage, PatchResult, ResourcePaths, RomInfo,
};

#[derive(Clone, Default)]
pub struct OperationState {
    busy: Arc<AtomicBool>,
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl OperationState {
    fn acquire(&self) -> Result<BusyGuard, String> {
        // Reject overlap because two jobs could target the same output path
        // even when the frontend normally prevents that race.
        if self.busy.swap(true, Ordering::AcqRel) {
            Err("Another operation is already running.".to_owned())
        } else {
            Ok(BusyGuard(Arc::clone(&self.busy)))
        }
    }
}

#[derive(Clone)]
pub struct DubbingRecordingState {
    inner: Arc<DubbingRecordingInner>,
}

struct DubbingRecordingInner {
    recorder: RecorderState,
    active: Mutex<Option<ActiveDubbingRecording>>,
    mutations: Mutex<()>,
}

struct ActiveDubbingRecording {
    project_dir: PathBuf,
    profile_path: PathBuf,
    symbol: String,
    capture_path: PathBuf,
}

impl Default for DubbingRecordingState {
    fn default() -> Self {
        Self {
            inner: Arc::new(DubbingRecordingInner {
                recorder: RecorderState::default(),
                active: Mutex::new(None),
                mutations: Mutex::new(()),
            }),
        }
    }
}

impl Drop for DubbingRecordingInner {
    fn drop(&mut self) {
        // A capture that never became a take has no durable meaning and should not survive an
        // application shutdown or an abandoned project session.
        let _ = self.recorder.stop();
        let active = self
            .active
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(active) = active.take() {
            let _ = remove_capture_file(&active.capture_path);
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingTakeResult {
    take: TakeMetadata,
    progress: TargetProgress,
    summary: RecordingSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    app_version: &'static str,
    japanese_voices: usize,
    english_voices: usize,
    reversible_patch: bool,
}

fn resource(app: &AppHandle, relative: &str) -> Result<PathBuf, String> {
    app.path()
        .resolve(relative, BaseDirectory::Resource)
        .map_err(|error| format!("Resource {relative} was not found: {error}"))
}

fn resources(app: &AppHandle) -> Result<ResourcePaths, String> {
    Ok(ResourcePaths {
        profile: resource(app, "resources/voice-profile.json")?,
        japanese_voicepack: resource(app, "resources/voices-jp.nvpack")?,
        english_voicepack: resource(app, "resources/voices-en.nvpack")?,
    })
}

fn dubbing_profile(app: &AppHandle) -> Result<PathBuf, String> {
    resource(app, "resources/voice-profile.json")
}

fn remove_capture_file(path: &PathBuf) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not remove incomplete recording {}: {error}",
            path.display()
        )),
    }
}

fn error_with_cleanup(error: impl ToString, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => error.to_string(),
        Err(cleanup_error) => format!("{} {cleanup_error}", error.to_string()),
    }
}

fn lock_project_mutations(
    inner: &DubbingRecordingInner,
) -> Result<std::sync::MutexGuard<'_, ()>, String> {
    inner.mutations.lock().map_err(|_| {
        "Project state is unavailable after an internal synchronization failure.".to_owned()
    })
}

#[tauri::command]
pub fn get_capabilities(app: AppHandle) -> Result<Capabilities, String> {
    let resources = resources(&app)?;
    let japanese = crate::patcher::voicepack::VoicePack::open(&resources.japanese_voicepack)
        .map_err(|error| error.to_string())?;
    let english = crate::patcher::voicepack::VoicePack::open(&resources.english_voicepack)
        .map_err(|error| error.to_string())?;
    Ok(Capabilities {
        app_version: env!("CARGO_PKG_VERSION"),
        japanese_voices: japanese.entries().len(),
        english_voices: english.entries().len(),
        reversible_patch: true,
    })
}

#[tauri::command]
pub async fn inspect_rom(app: AppHandle, path: String) -> Result<RomInfo, String> {
    let resources = resources(&app)?;
    tauri::async_runtime::spawn_blocking(move || engine::inspect_rom(path, &resources))
        .await
        .map_err(|error| format!("ROM inspection was interrupted: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_voice_patch(
    app: AppHandle,
    state: State<'_, OperationState>,
    input_path: String,
    output_path: String,
    language: PatchLanguage,
) -> Result<PatchResult, String> {
    let guard = state.acquire()?;
    let resources = resources(&app)?;
    let event_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        engine::apply_patch(
            input_path,
            output_path,
            &resources,
            &ApplyOptions {
                language,
                voicepack_override: None,
            },
            |progress| {
                let _ = event_app.emit("patch-progress", progress);
            },
        )
    })
    .await
    .map_err(|error| format!("Patch operation was interrupted: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reset_voice_patch(
    app: AppHandle,
    state: State<'_, OperationState>,
    input_path: String,
    output_path: String,
) -> Result<PatchResult, String> {
    let guard = state.acquire()?;
    let event_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        engine::reset_patch(input_path, output_path, |progress| {
            let _ = event_app.emit("patch-progress", progress);
        })
    })
    .await
    .map_err(|error| format!("Reset operation was interrupted: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_dubbing_project(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    rom_path: String,
    name: String,
) -> Result<DubbingProjectSnapshot, String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = lock_project_mutations(&inner)?;
        project::create_project(project_dir, rom_path, profile_path, &name)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Project creation was interrupted: {error}"))?
}

#[tauri::command]
pub async fn open_dubbing_project(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    rom_path: String,
) -> Result<DubbingProjectSnapshot, String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = lock_project_mutations(&inner)?;
        project::open_project(project_dir, rom_path, profile_path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Project loading was interrupted: {error}"))?
}

#[tauri::command]
pub async fn update_dubbing_target(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    symbol: String,
    status: TargetStatus,
    notes: String,
) -> Result<TargetProgress, String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = lock_project_mutations(&inner)?;
        project::update_target(project_dir, profile_path, &symbol, status, notes)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Target update was interrupted: {error}"))?
}

#[tauri::command]
pub async fn list_dubbing_input_devices(
    state: State<'_, DubbingRecordingState>,
) -> Result<Vec<AudioInputDevice>, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || inner.recorder.list_input_devices())
        .await
        .map_err(|error| format!("Audio input discovery was interrupted: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn start_dubbing_recording(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    symbol: String,
    device_id: Option<String>,
) -> Result<RecordingStartInfo, String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let project_dir = PathBuf::from(project_dir);
        let mut active = inner.active.lock().map_err(|_| {
            "Recording state is unavailable after an internal synchronization failure.".to_owned()
        })?;
        if active.is_some() || inner.recorder.is_recording() {
            return Err("A recording is already in progress.".to_owned());
        }

        let _mutation = lock_project_mutations(&inner)?;
        let capture_path = project::prepare_recording_path(&project_dir, &profile_path, &symbol)
            .map_err(|error| error.to_string())?;
        match inner.recorder.start(&capture_path, device_id.as_deref()) {
            Ok(start_info) => {
                *active = Some(ActiveDubbingRecording {
                    project_dir,
                    profile_path,
                    symbol,
                    capture_path,
                });
                Ok(start_info)
            }
            Err(error) => Err(error_with_cleanup(
                error,
                remove_capture_file(&capture_path),
            )),
        }
    })
    .await
    .map_err(|error| format!("Recording start was interrupted: {error}"))?
}

#[tauri::command]
pub async fn stop_dubbing_recording(
    state: State<'_, DubbingRecordingState>,
) -> Result<DubbingTakeResult, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        // Keeping this lock through finalization prevents a new session from racing the WAV move
        // and manifest update of the current take.
        let mut active_slot = inner.active.lock().map_err(|_| {
            "Recording state is unavailable after an internal synchronization failure.".to_owned()
        })?;
        let active = active_slot
            .take()
            .ok_or_else(|| "No recording is in progress.".to_owned())?;
        let mut summary = match inner.recorder.stop() {
            Ok(summary) => summary,
            Err(error) => {
                return Err(error_with_cleanup(
                    error,
                    remove_capture_file(&active.capture_path),
                ));
            }
        };
        if summary.path != active.capture_path {
            let summary_cleanup = remove_capture_file(&summary.path);
            let active_cleanup = remove_capture_file(&active.capture_path);
            return Err(error_with_cleanup(
                error_with_cleanup(
                    "The recorder returned a capture for a different project target.",
                    summary_cleanup,
                ),
                active_cleanup,
            ));
        }

        let _mutation = lock_project_mutations(&inner).map_err(|error| {
            format!(
                "{error} The completed WAV was preserved for recovery at {}.",
                active.capture_path.display()
            )
        })?;
        let (take, progress) = match project::commit_recording(
            &active.project_dir,
            &active.profile_path,
            &active.symbol,
            &summary,
        ) {
            Ok(result) => result,
            Err(error) => {
                return Err(format!(
                    "{error} The completed WAV was preserved for recovery at {}.",
                    active.capture_path.display()
                ));
            }
        };
        summary.path = active.project_dir.join(&take.file);
        Ok(DubbingTakeResult {
            take,
            progress,
            summary,
        })
    })
    .await
    .map_err(|error| format!("Recording finalization was interrupted: {error}"))?
}

#[tauri::command]
pub async fn cancel_dubbing_recording(
    state: State<'_, DubbingRecordingState>,
) -> Result<(), String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let mut active_slot = inner.active.lock().map_err(|_| {
            "Recording state is unavailable after an internal synchronization failure.".to_owned()
        })?;
        let active = active_slot
            .take()
            .ok_or_else(|| "No recording is in progress.".to_owned())?;
        let stop_result = inner.recorder.stop();
        let summary_cleanup = stop_result
            .as_ref()
            .ok()
            .filter(|summary| summary.path != active.capture_path)
            .map(|summary| remove_capture_file(&summary.path))
            .unwrap_or(Ok(()));
        let capture_cleanup = remove_capture_file(&active.capture_path);

        if let Err(error) = stop_result {
            return Err(error_with_cleanup(
                error_with_cleanup(error, summary_cleanup),
                capture_cleanup,
            ));
        }
        summary_cleanup?;
        capture_cleanup
    })
    .await
    .map_err(|error| format!("Recording cancellation was interrupted: {error}"))?
}

#[tauri::command]
pub async fn read_dubbing_take(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    symbol: String,
    take_id: String,
    on_data: Channel<Response>,
) -> Result<(), String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = lock_project_mutations(&inner)?;
        let bytes = project::read_take(project_dir, profile_path, &symbol, &take_id)
            .map_err(|error| error.to_string())?;
        // A raw channel avoids expanding multi-megabyte WAV data into a JSON number array on
        // platforms whose direct command response bridge serializes byte vectors as JSON.
        on_data
            .send(Response::new(bytes))
            .map_err(|error| format!("Could not transfer the take audio: {error}"))
    })
    .await
    .map_err(|error| format!("Take loading was interrupted: {error}"))?
}

#[tauri::command]
pub async fn select_dubbing_take(
    app: AppHandle,
    state: State<'_, DubbingRecordingState>,
    project_dir: String,
    symbol: String,
    take_id: String,
) -> Result<TargetProgress, String> {
    let profile_path = dubbing_profile(&app)?;
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = lock_project_mutations(&inner)?;
        project::select_take(project_dir, profile_path, &symbol, &take_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Take selection was interrupted: {error}"))?
}

#[tauri::command]
pub async fn build_dubbing_test_rom(
    app: AppHandle,
    operation_state: State<'_, OperationState>,
    dubbing_state: State<'_, DubbingRecordingState>,
    project_dir: String,
    rom_path: String,
    output_path: String,
) -> Result<TestRomBuildResult, String> {
    let operation_guard = operation_state.acquire()?;
    let resources = resources(&app)?;
    let event_app = app.clone();
    let inner = Arc::clone(&dubbing_state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let _operation_guard = operation_guard;
        let active = inner.active.lock().map_err(|_| {
            "Recording state is unavailable after an internal synchronization failure.".to_owned()
        })?;
        if active.is_some() || inner.recorder.is_recording() {
            return Err("Stop the active recording before building a test ROM.".to_owned());
        }
        // The build fingerprints every active take and must observe one stable manifest from
        // preflight through packaging; short metadata edits wait until the derived ROM is done.
        let _mutation = lock_project_mutations(&inner)?;
        let mut last_stage = String::new();
        let mut last_emit: Option<Instant> = None;
        build::build_test_rom(
            project_dir,
            rom_path,
            output_path,
            &resources,
            BuildScope::Preview,
            |progress| {
                let now = Instant::now();
                let stage_changed = progress.stage != last_stage;
                let stage_boundary = progress.completed == 0
                    || (progress.total > 0 && progress.completed >= progress.total);
                let interval_elapsed = last_emit.is_none_or(|previous| {
                    now.duration_since(previous) >= Duration::from_millis(50)
                });
                // Thousands of encoded entries complete quickly; throttling protects the webview
                // event queue while preserving every stage transition and final count.
                if stage_changed || stage_boundary || interval_elapsed {
                    last_stage.clone_from(&progress.stage);
                    last_emit = Some(now);
                    let _ = event_app.emit("dubbing-build-progress", progress);
                }
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Test ROM build was interrupted: {error}"))?
}
