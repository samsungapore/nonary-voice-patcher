use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::Serialize;
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager, State};

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
            &ApplyOptions { language },
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
