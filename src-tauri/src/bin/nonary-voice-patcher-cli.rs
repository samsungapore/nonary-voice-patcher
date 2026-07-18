use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{error::ErrorKind as ClapErrorKind, Parser, Subcommand, ValueEnum};
use nonary_voice_patcher_lib::patcher::engine::{
    self, ApplyOptions, EngineError, PatchLanguage, PatchProgress, PatchResult, ResourcePaths,
    RomInfo, RomPatchState,
};
use serde::Serialize;
use serde_json::Value;

const JSON_SCHEMA_VERSION: u32 = 1;
#[cfg(any(target_os = "linux", test))]
const PACKAGE_RESOURCE_DIR: &str = "nonary-voice-patcher/resources";

#[derive(Debug, Parser)]
#[command(
    name = "nonary-voice-patcher-cli",
    version,
    about = "Command-line voice patcher for 999 (Nintendo DS)",
    long_about = None,
    propagate_version = true
)]
struct Cli {
    /// Emit a stable JSON object on stdout and JSONL progress events on stderr.
    #[arg(long, global = true)]
    json: bool,

    /// Directory containing voice-profile.json and the voice packs.
    #[arg(long, global = true, value_name = "DIR")]
    resources_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect a ROM and any reversible patch receipt it contains.
    Inspect {
        #[arg(value_name = "INPUT_ROM")]
        input: PathBuf,
    },
    /// Create a voiced ROM without modifying the source file.
    Apply {
        #[arg(value_name = "INPUT_ROM")]
        input: PathBuf,
        #[arg(value_name = "OUTPUT_ROM")]
        output: PathBuf,
        #[arg(long, value_enum)]
        language: LanguageArg,
    },
    /// Restore the exact source ROM from a reversible patch.
    Reset {
        #[arg(value_name = "PATCHED_ROM")]
        input: PathBuf,
        #[arg(value_name = "OUTPUT_ROM")]
        output: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LanguageArg {
    #[value(name = "jp")]
    Japanese,
    #[value(name = "en")]
    English,
}

impl From<LanguageArg> for PatchLanguage {
    fn from(value: LanguageArg) -> Self {
        match value {
            LanguageArg::Japanese => Self::Japanese,
            LanguageArg::English => Self::English,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum CliErrorCode {
    #[serde(rename = "usage_error")]
    Usage,
    #[serde(rename = "resource_error")]
    Resource,
    #[serde(rename = "input_error")]
    Input,
    #[serde(rename = "io_error")]
    Io,
    #[serde(rename = "operation_error")]
    Operation,
    #[serde(rename = "internal_error")]
    Internal,
}

impl CliErrorCode {
    fn exit_code(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Resource => 3,
            Self::Input => 4,
            Self::Io => 5,
            Self::Operation => 6,
            Self::Internal => 70,
        }
    }
}

#[derive(Debug)]
struct CliError {
    code: CliErrorCode,
    message: String,
}

impl CliError {
    fn new(code: CliErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn resource(message: impl Into<String>) -> Self {
        Self::new(CliErrorCode::Resource, message)
    }

    fn io(message: impl Into<String>) -> Self {
        Self::new(CliErrorCode::Io, message)
    }
}

impl From<EngineError> for CliError {
    fn from(error: EngineError) -> Self {
        let code = match error {
            EngineError::Io { .. } => CliErrorCode::Io,
            EngineError::VoicePack(_)
            | EngineError::Profile(_)
            | EngineError::VoicePackLanguage { .. }
            | EngineError::PayloadSize { .. } => CliErrorCode::Resource,
            EngineError::SameInputOutput
            | EngineError::Nds(_)
            | EngineError::Receipt(_)
            | EngineError::RomTooLarge
            | EngineError::LegacyPatch => CliErrorCode::Input,
            EngineError::Fsb(_)
            | EngineError::Registry(_)
            | EngineError::Bleep(_)
            | EngineError::Verification(_) => CliErrorCode::Operation,
        };
        Self::new(code, error.to_string())
    }
}

enum Outcome {
    Inspect(RomInfo),
    Apply(PatchResult),
    Reset(PatchResult),
}

impl Outcome {
    fn command_name(&self) -> &'static str {
        match self {
            Self::Inspect(_) => "inspect",
            Self::Apply(_) => "apply",
            Self::Reset(_) => "reset",
        }
    }

    fn json_value(&self) -> Result<Value, CliError> {
        let value = match self {
            Self::Inspect(value) => serde_json::to_value(value),
            Self::Apply(value) | Self::Reset(value) => serde_json::to_value(value),
        };
        value.map_err(|error| {
            CliError::new(
                CliErrorCode::Internal,
                format!("failed to serialize the result: {error}"),
            )
        })
    }

    fn print_human(&self) {
        match self {
            Self::Inspect(info) => {
                println!("ROM: {}", info.path);
                println!("Title: {} ({})", info.title, info.game_code);
                println!("Size: {} bytes", info.bytes);
                println!("SHA-256: {}", info.sha256);
                println!("State: {}", rom_state_code(info.state));
                println!("Compatible: {}", if info.compatible { "yes" } else { "no" });
                println!("Detail: {}", info.detail);
            }
            Self::Apply(result) => print_patch_result("Created ROM", result),
            Self::Reset(result) => print_patch_result("Restored ROM", result),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SuccessEnvelope {
    schema_version: u32,
    ok: bool,
    command: &'static str,
    result: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope<'a> {
    schema_version: u32,
    ok: bool,
    command: &'a str,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody<'a> {
    code: CliErrorCode,
    message: &'a str,
    exit_code: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEnvelope<'a> {
    schema_version: u32,
    event: &'static str,
    command: &'a str,
    progress: &'a PatchProgress,
}

fn main() -> ExitCode {
    let arguments = env::args_os().collect::<Vec<_>>();
    let wants_json = arguments.iter().any(|argument| argument == "--json");
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return handle_clap_error(error, wants_json),
    };
    let json = cli.json;
    match execute(cli) {
        Ok(outcome) => match print_success(&outcome, json) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                print_error(outcome.command_name(), &error, json);
                ExitCode::from(error.code.exit_code())
            }
        },
        Err((command, error)) => {
            print_error(command, &error, json);
            ExitCode::from(error.code.exit_code())
        }
    }
}

fn execute(cli: Cli) -> Result<Outcome, (&'static str, CliError)> {
    let json = cli.json;
    match cli.command {
        Command::Inspect { input } => {
            let command = "inspect";
            let resources = resolve_resources(cli.resources_dir.as_deref(), ResourceNeed::Inspect)
                .map_err(|error| (command, error))?;
            emit_progress(json, command, "inspect", 0, 1, "Inspecting ROM");
            let info =
                engine::inspect_rom(input, &resources).map_err(|error| (command, error.into()))?;
            emit_progress(json, command, "inspect", 1, 1, "Inspection complete");
            Ok(Outcome::Inspect(info))
        }
        Command::Apply {
            input,
            output,
            language,
        } => {
            let command = "apply";
            let output = normalize_output_path(output);
            ensure_output_parent(&output).map_err(|error| (command, error))?;
            let language = PatchLanguage::from(language);
            let resources = resolve_resources(
                cli.resources_dir.as_deref(),
                ResourceNeed::Apply { language },
            )
            .map_err(|error| (command, error))?;
            let options = ApplyOptions { language };
            let result = engine::apply_patch(input, output, &resources, &options, |progress| {
                print_progress(json, command, &progress)
            })
            .map_err(|error| (command, error.into()))?;
            Ok(Outcome::Apply(result))
        }
        Command::Reset { input, output } => {
            let command = "reset";
            let output = normalize_output_path(output);
            ensure_output_parent(&output).map_err(|error| (command, error))?;
            let result = engine::reset_patch(input, output, |progress| {
                print_progress(json, command, &progress)
            })
            .map_err(|error| (command, error.into()))?;
            Ok(Outcome::Reset(result))
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceNeed {
    Inspect,
    Apply { language: PatchLanguage },
}

fn resolve_resources(
    explicit: Option<&Path>,
    need: ResourceNeed,
) -> Result<ResourcePaths, CliError> {
    let directory = if let Some(directory) = explicit {
        directory.to_owned()
    } else {
        default_resource_candidates()?
            .into_iter()
            .find(|candidate| candidate.join("voice-profile.json").is_file())
            .ok_or_else(|| {
                CliError::resource(
                    "resources were not found next to the executable; use --resources-dir DIR",
                )
            })?
    };
    if !directory.is_dir() {
        return Err(CliError::resource(format!(
            "the resource directory does not exist: {}",
            directory.display()
        )));
    }

    let profile = directory.join("voice-profile.json");
    require_resource(&profile, "voice profile")?;
    let japanese_voicepack = directory.join("voices-jp.nvpack");
    let english_voicepack = directory.join("voices-en.nvpack");
    match need {
        ResourceNeed::Inspect => {}
        ResourceNeed::Apply {
            language: PatchLanguage::Japanese,
        } => {
            require_resource(&japanese_voicepack, "Japanese voice pack")?;
        }
        ResourceNeed::Apply {
            language: PatchLanguage::English,
        } => {
            require_resource(&english_voicepack, "English voice pack")?;
        }
    }

    Ok(ResourcePaths {
        profile,
        japanese_voicepack,
        english_voicepack,
    })
}

fn default_resource_candidates() -> Result<Vec<PathBuf>, CliError> {
    let executable = env::current_exe()
        .map_err(|error| CliError::io(format!("failed to locate the executable: {error}")))?;
    let executable_dir = executable.parent().ok_or_else(|| {
        CliError::resource(format!(
            "the executable has no parent directory: {}",
            executable.display()
        ))
    })?;
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("NONARY_VOICE_PATCHER_RESOURCES") {
        candidates.push(PathBuf::from(value));
    }
    candidates.push(executable_dir.join("resources"));
    candidates.push(executable_dir.to_owned());
    if let Some(contents) = executable_dir.parent() {
        candidates.push(contents.join("Resources/resources"));
        candidates.push(contents.join("Resources"));
    }
    #[cfg(target_os = "linux")]
    {
        let app_dir = env::var_os("APPDIR").map(PathBuf::from);
        append_linux_resource_candidates(&mut candidates, app_dir.as_deref());
    }
    candidates.push(executable_dir.join("../share/nonary-voice-patcher/resources"));
    // The source-tree fallback keeps local development convenient without
    // exposing a builder-specific absolute path in distributable binaries.
    #[cfg(debug_assertions)]
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
    candidates.dedup();
    Ok(candidates)
}

#[cfg(any(target_os = "linux", test))]
fn append_linux_resource_candidates(candidates: &mut Vec<PathBuf>, app_dir: Option<&Path>) {
    if let Some(app_dir) = app_dir {
        candidates.push(app_dir.join("usr/lib").join(PACKAGE_RESOURCE_DIR));
    }
    candidates.push(Path::new("/usr/lib").join(PACKAGE_RESOURCE_DIR));
}

fn require_resource(path: &Path, label: &str) -> Result<(), CliError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(CliError::resource(format!(
            "missing {label}: {}",
            path.display()
        )))
    }
}

fn ensure_output_parent(output: &Path) -> Result<(), CliError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        CliError::io(format!(
            "failed to create output directory {}: {error}",
            parent.display()
        ))
    })
}

fn normalize_output_path(output: PathBuf) -> PathBuf {
    if output
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        Path::new(".").join(output)
    } else {
        output
    }
}

fn emit_progress(
    json: bool,
    command: &str,
    stage: &str,
    completed: u64,
    total: u64,
    message: &str,
) {
    print_progress(
        json,
        command,
        &PatchProgress {
            stage: stage.to_owned(),
            completed,
            total,
            message: message.to_owned(),
        },
    );
}

fn print_progress(json: bool, command: &str, progress: &PatchProgress) {
    if json {
        let envelope = ProgressEnvelope {
            schema_version: JSON_SCHEMA_VERSION,
            event: "progress",
            command,
            progress,
        };
        match serde_json::to_string(&envelope) {
            Ok(line) => eprintln!("{line}"),
            Err(error) => eprintln!("progress serialization error: {error}"),
        }
    } else if progress.total > 1 {
        eprintln!(
            "[{}] {}/{} — {}",
            progress.stage, progress.completed, progress.total, progress.message
        );
    } else {
        eprintln!("[{}] {}", progress.stage, progress.message);
    }
}

fn print_success(outcome: &Outcome, json: bool) -> Result<(), CliError> {
    if json {
        let envelope = SuccessEnvelope {
            schema_version: JSON_SCHEMA_VERSION,
            ok: true,
            command: outcome.command_name(),
            result: outcome.json_value()?,
        };
        println!(
            "{}",
            serde_json::to_string(&envelope).map_err(|error| {
                CliError::new(
                    CliErrorCode::Internal,
                    format!("failed to serialize the response: {error}"),
                )
            })?
        );
    } else {
        outcome.print_human();
    }
    Ok(())
}

fn print_error(command: &str, error: &CliError, json: bool) {
    if json {
        let envelope = ErrorEnvelope {
            schema_version: JSON_SCHEMA_VERSION,
            ok: false,
            command,
            error: ErrorBody {
                code: error.code,
                message: &error.message,
                exit_code: error.code.exit_code(),
            },
        };
        match serde_json::to_string(&envelope) {
            Ok(value) => println!("{value}"),
            Err(_) => println!(
                "{{\"schemaVersion\":1,\"ok\":false,\"command\":\"cli\",\"error\":{{\"code\":\"internal_error\",\"message\":\"JSON serialization failed\",\"exitCode\":70}}}}"
            ),
        }
    } else {
        eprintln!("Error: {}", error.message);
    }
}

fn handle_clap_error(error: clap::Error, json: bool) -> ExitCode {
    let is_information = matches!(
        error.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    );
    if is_information {
        let _ = error.print();
        return ExitCode::SUCCESS;
    }
    if json {
        let cli_error = CliError::new(CliErrorCode::Usage, error.to_string());
        print_error("cli", &cli_error, true);
    } else {
        let _ = error.print();
    }
    ExitCode::from(CliErrorCode::Usage.exit_code())
}

fn rom_state_code(state: RomPatchState) -> &'static str {
    match state {
        RomPatchState::Clean => "clean",
        RomPatchState::Japanese => "japanese",
        RomPatchState::English => "english",
        RomPatchState::LegacyVoicePatch => "legacy_voice_patch",
        RomPatchState::Unsupported => "unsupported",
    }
}

fn print_patch_result(label: &str, result: &PatchResult) {
    println!("{label}: {}", result.output_path);
    println!("Size: {} bytes", result.bytes);
    println!("SHA-256: {}", result.sha256);
    if let Some(language) = result.language {
        println!("Language: {}", language.code());
    }
    println!("Voices: {}", result.voices);
    println!("Scripts: {}", result.scripts);
    println!(
        "Exact restoration: {}",
        if result.reset_exact { "yes" } else { "no" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_rom_commands_and_global_options_in_any_position() {
        let inspect = Cli::try_parse_from([
            "cli",
            "inspect",
            "game.nds",
            "--json",
            "--resources-dir",
            "assets",
        ])
        .unwrap();
        assert!(inspect.json);
        assert_eq!(inspect.resources_dir, Some(PathBuf::from("assets")));
        assert!(matches!(inspect.command, Command::Inspect { .. }));

        let apply =
            Cli::try_parse_from(["cli", "apply", "in.nds", "out.nds", "--language", "jp"]).unwrap();
        assert!(matches!(
            apply.command,
            Command::Apply {
                language: LanguageArg::Japanese,
                ..
            }
        ));

        let reset = Cli::try_parse_from(["cli", "reset", "patched.nds", "clean.nds"]).unwrap();
        assert!(matches!(reset.command, Command::Reset { .. }));
    }

    #[test]
    fn rejects_unknown_languages() {
        assert!(
            Cli::try_parse_from(["cli", "apply", "in.nds", "out.nds", "--language", "fr"]).is_err()
        );
    }

    #[test]
    fn explicit_resource_directory_requires_only_the_selected_language() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("voice-profile.json"), b"{}").unwrap();
        fs::write(temporary.path().join("voices-jp.nvpack"), b"placeholder").unwrap();

        assert!(resolve_resources(
            Some(temporary.path()),
            ResourceNeed::Apply {
                language: PatchLanguage::Japanese,
            }
        )
        .is_ok());
        assert!(resolve_resources(
            Some(temporary.path()),
            ResourceNeed::Apply {
                language: PatchLanguage::English,
            }
        )
        .is_err());
    }

    #[test]
    fn linux_bundle_candidates_cover_deb_and_appimage_layouts() {
        let mut candidates = Vec::new();
        append_linux_resource_candidates(
            &mut candidates,
            Some(Path::new("/tmp/Nonary Voice Patcher.AppDir")),
        );

        assert!(candidates.contains(&PathBuf::from("/usr/lib/nonary-voice-patcher/resources")));
        assert!(candidates.contains(&PathBuf::from(
            "/tmp/Nonary Voice Patcher.AppDir/usr/lib/nonary-voice-patcher/resources"
        )));
    }

    #[test]
    fn incompatible_resource_errors_exit_with_resource_code() {
        let wrong_language = CliError::from(EngineError::VoicePackLanguage {
            expected: "jp",
            actual: "en",
        });
        let wrong_payload_size = CliError::from(EngineError::PayloadSize {
            path: "sound/se_v0000.se".to_owned(),
            actual: 1,
            expected: 2,
        });

        assert_eq!(wrong_language.code, CliErrorCode::Resource);
        assert_eq!(wrong_language.code.exit_code(), 3);
        assert_eq!(wrong_payload_size.code, CliErrorCode::Resource);
        assert_eq!(wrong_payload_size.code.exit_code(), 3);
    }
}
