use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use clap::{ArgGroup, Parser};
use nonary_voice_patcher_lib::patcher::{
    fsb,
    profile::{ExactByteEdit, ExactRepairProfile, ProfileVoice, ScriptProfile, VoiceProfile},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const EXACT_FRENCH_REPAIRS: &[(&str, usize, [u8; 2], [u8; 2])] = &[
    ("scr/a31.fsb", 40_792, [0x84, 0xca], [b'~', b'E']),
    ("scr/a32.fsb", 40_760, [0x84, 0xc9], [b'~', b'D']),
    ("scr/b12.fsb", 43_232, [0x84, 0xcd], [b'~', b'H']),
    ("scr/b21.fsb", 54_556, [0x84, 0xce], [b'~', b'I']),
    ("scr/b31.fsb", 60_364, [0x84, 0xc2], [b'~', b'3']),
];

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("compatibility_source")
        .required(true)
        .args(["compatibility_manifest", "french_fsb_root"])
))]
struct Args {
    voice_map: PathBuf,
    romfs_dir: PathBuf,
    output_json: PathBuf,
    #[arg(long, value_name = "JSON")]
    compatibility_manifest: Option<PathBuf>,
    #[arg(long, value_name = "DIR")]
    french_fsb_root: Option<PathBuf>,
    #[arg(long, value_name = "DIR", requires = "french_fsb_root")]
    alternative_fsb_root: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CompatibilityManifest {
    version: u32,
    game_code: String,
    #[serde(default)]
    alternative_scripts: Vec<AlternativeScript>,
    #[serde(default)]
    exact_repairs: Vec<ExactRepairProfile>,
}

#[derive(Debug, Deserialize)]
struct AlternativeScript {
    path: String,
    alternative_structural_sha256: Vec<String>,
}

fn column(header: &[&str], name: &str) -> usize {
    header
        .iter()
        .position(|value| *value == name)
        .unwrap_or_else(|| panic!("missing TSV column {name}"))
}

fn optional_column(header: &[&str], name: &str) -> Option<usize> {
    header.iter().position(|value| *value == name)
}

fn target_text_hashes(row: &[&str], hash_column: Option<usize>) -> Vec<String> {
    hash_column
        .and_then(|index| row.get(index))
        .into_iter()
        .flat_map(|value| value.split('|'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn script_path(root: &Path, path: &str) -> PathBuf {
    let nested = root.join(path);
    if nested.is_file() {
        nested
    } else {
        root.join(Path::new(path).file_name().expect("script basename"))
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn exact_french_repairs(stock_root: &Path, french_root: &Path) -> Vec<ExactRepairProfile> {
    EXACT_FRENCH_REPAIRS
        .iter()
        .map(|(path, offset, expected, replacement)| {
            let stock = fs::read(script_path(stock_root, path)).expect("read retail repair FSB");
            let damaged = fs::read(script_path(french_root, path)).expect("read French repair FSB");
            assert_eq!(
                damaged.get(*offset..*offset + expected.len()),
                Some(expected.as_slice()),
                "unexpected damaged bytes for {path} at 0x{offset:x}"
            );
            let mut repaired = damaged.clone();
            repaired[*offset..*offset + replacement.len()].copy_from_slice(replacement);
            fsb::parse(&repaired).expect("parse exactly repaired French FSB");
            let repaired_sha256 = sha256_hex(&repaired);
            let mut accepted_clean_sha256 = vec![sha256_hex(&stock), repaired_sha256.clone()];
            accepted_clean_sha256.sort();
            accepted_clean_sha256.dedup();
            ExactRepairProfile {
                path: (*path).to_owned(),
                damaged_sha256: sha256_hex(&damaged),
                repaired_sha256,
                accepted_clean_sha256,
                edits: vec![ExactByteEdit {
                    offset: *offset,
                    expected_hex: expected.iter().map(|byte| format!("{byte:02x}")).collect(),
                    replacement_hex: replacement
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                }],
            }
        })
        .collect()
}

fn compatibility_manifest(path: &Path) -> (BTreeMap<String, Vec<String>>, Vec<ExactRepairProfile>) {
    let manifest: CompatibilityManifest =
        serde_json::from_slice(&fs::read(path).expect("read compatibility manifest"))
            .expect("parse compatibility manifest");
    assert_eq!(manifest.version, 1, "unsupported compatibility manifest");
    assert_eq!(
        manifest.game_code, "BSKE",
        "compatibility manifest targets another game"
    );
    let mut alternatives = BTreeMap::new();
    for script in manifest.alternative_scripts {
        assert!(
            alternatives
                .insert(script.path.clone(), script.alternative_structural_sha256)
                .is_none(),
            "duplicate compatibility entry for {}",
            script.path
        );
    }
    (alternatives, manifest.exact_repairs)
}

fn main() {
    let args = Args::parse();
    let map = fs::read_to_string(&args.voice_map).expect("read voice map");
    let mut lines = map.lines();
    let header = lines.next().expect("voice map header");
    let header = header.split('\t').collect::<Vec<_>>();
    let script_column = column(&header, "ds_script");
    let ordinal_column = column(&header, "ds_settext_ordinal");
    let symbol_column = column(&header, "symbol");
    let target_text_hash_column = optional_column(&header, "target_text_sha256");

    let mut grouped: BTreeMap<String, Vec<ProfileVoice>> = BTreeMap::new();
    for line in lines {
        let row = line.split('\t').collect::<Vec<_>>();
        let script = Path::new(row[script_column])
            .file_name()
            .expect("script basename")
            .to_string_lossy();
        grouped
            .entry(format!("scr/{}", script.trim_end_matches(".txt")))
            .or_default()
            .push(ProfileVoice {
                ordinal: row[ordinal_column].parse().expect("setText ordinal"),
                symbol: row[symbol_column].to_owned(),
                target_text_sha256: target_text_hashes(&row, target_text_hash_column),
            });
    }

    let (mut manifest_alternatives, exact_repairs) = args
        .compatibility_manifest
        .as_deref()
        .map(compatibility_manifest)
        .unwrap_or_else(|| {
            (
                BTreeMap::new(),
                exact_french_repairs(
                    &args.romfs_dir,
                    args.french_fsb_root
                        .as_deref()
                        .expect("clap requires a compatibility source"),
                ),
            )
        });
    let alternative_roots = args
        .french_fsb_root
        .iter()
        .chain(&args.alternative_fsb_root)
        .collect::<Vec<_>>();

    let mut scripts = Vec::with_capacity(grouped.len());
    for (path, voices) in grouped {
        let source_path = script_path(&args.romfs_dir, &path);
        let script = fsb::parse(&fs::read(&source_path).expect("read retail FSB"))
            .expect("parse retail FSB");
        let structural_sha256 = script.structural_sha256();
        let mut alternative_structural_sha256 =
            manifest_alternatives.remove(&path).unwrap_or_default();
        for alternative_root in &alternative_roots {
            let alternative_path = script_path(alternative_root, &path);
            let alternative = fsb::parse_repairing_invalid_relocated_pointers(
                &fs::read(&alternative_path).expect("read alternative FSB"),
            )
            .expect("parse alternative FSB")
            .script;
            assert_eq!(
                alternative.set_text_count(),
                script.set_text_count(),
                "setText count differs for {}",
                path
            );
            let fingerprint = alternative.structural_sha256();
            if fingerprint != structural_sha256 {
                alternative_structural_sha256.push(fingerprint);
            }
        }
        alternative_structural_sha256.sort();
        alternative_structural_sha256.dedup();
        scripts.push(ScriptProfile {
            path,
            structural_sha256,
            alternative_structural_sha256,
            set_text_count: script.set_text_count(),
            voices,
        });
    }
    assert!(
        manifest_alternatives.is_empty(),
        "compatibility manifest contains scripts absent from the voice map: {:?}",
        manifest_alternatives.keys().collect::<Vec<_>>()
    );
    let profile = VoiceProfile {
        version: 1,
        game_code: "BSKE".to_owned(),
        voice_count: scripts.iter().map(|script| script.voices.len()).sum(),
        scripts,
        exact_repairs,
    };
    fs::write(
        &args.output_json,
        profile.to_pretty_json().expect("serialize profile"),
    )
    .expect("write profile");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_hash_column_is_optional_and_accepts_reviewed_alternatives() {
        let legacy_header = ["symbol", "ds_script"];
        assert_eq!(
            target_text_hashes(
                &["SE_V0000", "a.fsb.txt"],
                optional_column(&legacy_header, "target_text_sha256")
            ),
            Vec::<String>::new()
        );

        let header = ["symbol", "target_text_sha256"];
        assert_eq!(
            target_text_hashes(
                &["SE_V0000", " aa | bb  |  cc "],
                optional_column(&header, "target_text_sha256")
            ),
            ["aa", "bb", "cc"]
        );
        assert_eq!(
            target_text_hashes(
                &["SE_V0000", "  "],
                optional_column(&header, "target_text_sha256")
            ),
            Vec::<String>::new()
        );
    }
}
