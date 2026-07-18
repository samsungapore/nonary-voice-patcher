use std::{collections::BTreeMap, env, fs, path::PathBuf, process::ExitCode};

use nonary_voice_patcher_lib::patcher::{
    fsb::{self, Operation},
    nds::NdsRom,
    profile::VoiceProfile,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreservationAudit {
    base_file_count: usize,
    patched_file_count: usize,
    added_file_count: usize,
    added_voice_file_count: usize,
    unchanged_regular_files: usize,
    unchanged_bg_files: usize,
    unchanged_etc_files: usize,
    changed_system_files: Vec<String>,
    checked_script_count: usize,
    checked_set_text_count: usize,
    exact_repair_script_count: usize,
    applied_exact_repair_count: usize,
    regular_file_mismatches: Vec<String>,
    script_text_mismatches: Vec<String>,
    missing_paths: Vec<String>,
    ok: bool,
}

fn set_text_values(script: &fsb::Script) -> Vec<Vec<u8>> {
    script
        .functions
        .iter()
        .flat_map(|function| &function.operations)
        .filter_map(|operation| match operation {
            Operation::String {
                command: 0x2f,
                value,
            } => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn main() -> ExitCode {
    let args = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.len() != 4 {
        eprintln!(
            "usage: audit_voice_patch_preservation BASE_ROM PATCHED_ROM PROFILE_JSON OUTPUT_JSON"
        );
        return ExitCode::from(2);
    }

    let base_data = match fs::read(&args[0]) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("unable to read {}: {error}", args[0].display());
            return ExitCode::FAILURE;
        }
    };
    let patched_data = match fs::read(&args[1]) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("unable to read {}: {error}", args[1].display());
            return ExitCode::FAILURE;
        }
    };
    let base = match NdsRom::parse(&base_data) {
        Ok(rom) => rom,
        Err(error) => {
            eprintln!("unable to parse base ROM: {error}");
            return ExitCode::FAILURE;
        }
    };
    let patched = match NdsRom::parse(&patched_data) {
        Ok(rom) => rom,
        Err(error) => {
            eprintln!("unable to parse patched ROM: {error}");
            return ExitCode::FAILURE;
        }
    };
    let profile = match fs::read(&args[2])
        .map_err(|error| error.to_string())
        .and_then(|data| VoiceProfile::from_json(&data).map_err(|error| error.to_string()))
    {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("unable to load profile: {error}");
            return ExitCode::FAILURE;
        }
    };
    let scripted = profile
        .scripts
        .iter()
        .map(|script| (script.path.as_str(), script))
        .collect::<BTreeMap<_, _>>();
    let exact_repairs = profile
        .exact_repairs
        .iter()
        .map(|repair| (repair.path.as_str(), repair))
        .collect::<BTreeMap<_, _>>();

    let mut regular_file_mismatches = Vec::new();
    let mut script_text_mismatches = Vec::new();
    let mut missing_paths = Vec::new();
    let mut changed_system_files = Vec::new();
    let mut unchanged_regular_files = 0;
    let mut unchanged_bg_files = 0;
    let mut unchanged_etc_files = 0;
    let mut checked_set_text_count = 0;
    let mut applied_exact_repair_count = 0;
    for record in base.files() {
        let Some(before) = base.data_for_path(&record.path) else {
            missing_paths.push(record.path.clone());
            continue;
        };
        let Some(after) = patched.data_for_path(&record.path) else {
            missing_paths.push(record.path.clone());
            continue;
        };
        if let Some(expected) = scripted.get(record.path.as_str()) {
            match (expected.parse_compatible(before), fsb::parse(after)) {
                (Ok(before), Ok(after)) => {
                    let before = set_text_values(&before.script);
                    let after = set_text_values(&after);
                    checked_set_text_count += before.len();
                    if before != after {
                        script_text_mismatches.push(record.path.clone());
                    }
                }
                (before, after) => script_text_mismatches.push(format!(
                    "{}: base={:?}, patched={:?}",
                    record.path,
                    before.err(),
                    after.err()
                )),
            }
        } else if let Some(repair) = exact_repairs.get(record.path.as_str()) {
            match repair.apply(before) {
                Ok(expected) => {
                    applied_exact_repair_count += expected.repaired_fields;
                    if after != expected.data {
                        regular_file_mismatches.push(format!(
                            "{} differs from its exact repaired form",
                            record.path
                        ));
                    }
                }
                Err(error) => regular_file_mismatches
                    .push(format!("{}: exact repair failed: {error}", record.path)),
            }
        } else if record.path == "etc/sound.dat" || record.path == "sound/se_sys.se" {
            if before != after {
                changed_system_files.push(record.path.clone());
            } else {
                regular_file_mismatches.push(format!(
                    "{} should have been patched but is unchanged",
                    record.path
                ));
            }
        } else if before == after {
            unchanged_regular_files += 1;
            if record.path.starts_with("bg/") {
                unchanged_bg_files += 1;
            }
            if record.path.starts_with("etc/") {
                unchanged_etc_files += 1;
            }
        } else {
            regular_file_mismatches.push(record.path.clone());
        }
    }

    let added = patched
        .path_index()
        .keys()
        .filter(|path| !base.path_index().contains_key(*path))
        .collect::<Vec<_>>();
    let added_voice_file_count = added
        .iter()
        .filter(|path| {
            path.starts_with("sound/se_v")
                && path.ends_with(".se")
                && path.len() == "sound/se_v0000.se".len()
        })
        .count();
    let ok = regular_file_mismatches.is_empty()
        && script_text_mismatches.is_empty()
        && missing_paths.is_empty()
        && changed_system_files.len() == 2
        && added_voice_file_count == profile.voice_count;
    let result = PreservationAudit {
        base_file_count: base.files().len(),
        patched_file_count: patched.files().len(),
        added_file_count: added.len(),
        added_voice_file_count,
        unchanged_regular_files,
        unchanged_bg_files,
        unchanged_etc_files,
        changed_system_files,
        checked_script_count: profile.scripts.len(),
        checked_set_text_count,
        exact_repair_script_count: profile.exact_repairs.len(),
        applied_exact_repair_count,
        regular_file_mismatches,
        script_text_mismatches,
        missing_paths,
        ok,
    };

    let output = match serde_json::to_vec_pretty(&result) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("unable to serialize audit: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&args[3], output) {
        eprintln!("unable to write {}: {error}", args[3].display());
        return ExitCode::FAILURE;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
