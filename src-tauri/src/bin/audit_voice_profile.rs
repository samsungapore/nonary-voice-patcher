use std::{env, fs, path::PathBuf, process::ExitCode};

use nonary_voice_patcher_lib::patcher::profile::VoiceProfile;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScriptAudit {
    path: String,
    voice_targets: usize,
    expected_structural_sha256: String,
    actual_structural_sha256: Option<String>,
    expected_set_text_count: usize,
    actual_set_text_count: Option<usize>,
    compatible: bool,
    error: Option<String>,
}

fn main() -> ExitCode {
    let args = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.len() != 3 {
        eprintln!("usage: audit_voice_profile PROFILE_JSON ROMFS_DATA_DIR OUTPUT_JSON");
        return ExitCode::from(2);
    }

    let profile = match fs::read(&args[0])
        .map_err(|error| error.to_string())
        .and_then(|data| VoiceProfile::from_json(&data).map_err(|error| error.to_string()))
    {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("unable to load profile: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut result = Vec::with_capacity(profile.scripts.len());
    for expected in profile.scripts {
        let source = args[1].join(&expected.path);
        let row = match fs::read(&source) {
            Ok(data) => match expected.parse_compatible(&data) {
                Ok(parsed) => {
                    let script = parsed.script;
                    let actual_hash = script.structural_sha256();
                    let actual_count = script.set_text_count();
                    let compatible = expected.accepts_structural_sha256(&actual_hash)
                        && actual_count == expected.set_text_count;
                    ScriptAudit {
                        path: expected.path,
                        voice_targets: expected.voices.len(),
                        expected_structural_sha256: expected.structural_sha256,
                        actual_structural_sha256: Some(actual_hash),
                        expected_set_text_count: expected.set_text_count,
                        actual_set_text_count: Some(actual_count),
                        compatible,
                        error: None,
                    }
                }
                Err(error) => ScriptAudit {
                    path: expected.path,
                    voice_targets: expected.voices.len(),
                    expected_structural_sha256: expected.structural_sha256,
                    actual_structural_sha256: None,
                    expected_set_text_count: expected.set_text_count,
                    actual_set_text_count: None,
                    compatible: false,
                    error: Some(error.to_string()),
                },
            },
            Err(error) => ScriptAudit {
                path: expected.path,
                voice_targets: expected.voices.len(),
                expected_structural_sha256: expected.structural_sha256,
                actual_structural_sha256: None,
                expected_set_text_count: expected.set_text_count,
                actual_set_text_count: None,
                compatible: false,
                error: Some(error.to_string()),
            },
        };
        result.push(row);
    }

    let output = match serde_json::to_vec_pretty(&result) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("unable to serialize audit: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&args[2], output) {
        eprintln!("unable to write {}: {error}", args[2].display());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
