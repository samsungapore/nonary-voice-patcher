use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    fsb::{self, FsbError, VoiceTarget},
    nds::NdsRom,
};

const PROFILE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub version: u32,
    pub game_code: String,
    pub voice_count: usize,
    pub scripts: Vec<ScriptProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_repairs: Vec<ExactRepairProfile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptProfile {
    pub path: String,
    pub structural_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternative_structural_sha256: Vec<String>,
    pub set_text_count: usize,
    pub voices: Vec<ProfileVoice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileVoice {
    pub ordinal: usize,
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactRepairProfile {
    pub path: String,
    pub damaged_sha256: String,
    pub repaired_sha256: String,
    pub accepted_clean_sha256: Vec<String>,
    pub edits: Vec<ExactByteEdit>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactByteEdit {
    pub offset: usize,
    pub expected_hex: String,
    pub replacement_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactRepairOutcome {
    pub data: Vec<u8>,
    pub repaired_fields: usize,
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("invalid voice profile JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inconsistent voice profile: {0}")]
    Invalid(String),
    #[error("the ROM does not contain {0}")]
    MissingScript(String),
    #[error("script {path} is incompatible: {reason}")]
    IncompatibleScript { path: String, reason: String },
    #[error("script {path} is invalid: {source}")]
    Fsb {
        path: String,
        #[source]
        source: FsbError,
    },
}

impl VoiceProfile {
    pub fn from_json(data: &[u8]) -> Result<Self, ProfileError> {
        let profile: Self = serde_json::from_slice(data)?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn to_pretty_json(&self) -> Result<Vec<u8>, ProfileError> {
        self.validate()?;
        Ok(serde_json::to_vec_pretty(self)?)
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.version != PROFILE_VERSION {
            return Err(ProfileError::Invalid(format!(
                "unsupported version {}",
                self.version
            )));
        }
        if self.game_code.len() != 4 || !self.game_code.is_ascii() {
            return Err(ProfileError::Invalid(format!(
                "invalid game code: {:?}",
                self.game_code
            )));
        }
        if self.voice_count == 0 || self.voice_count > 10_000 {
            return Err(ProfileError::Invalid(format!(
                "invalid voice count: {}",
                self.voice_count
            )));
        }
        if self.scripts.is_empty() {
            return Err(ProfileError::Invalid(
                "profile contains no scripts".to_owned(),
            ));
        }
        let mut paths = HashSet::new();
        let mut symbols = Vec::new();
        for script in &self.scripts {
            if !script.path.starts_with("scr/")
                || !script.path.ends_with(".fsb")
                || script.path.contains("..")
                || !paths.insert(script.path.as_str())
            {
                return Err(ProfileError::Invalid(format!(
                    "invalid or duplicate script path: {:?}",
                    script.path
                )));
            }
            let mut fingerprints = HashSet::new();
            for fingerprint in std::iter::once(&script.structural_sha256)
                .chain(&script.alternative_structural_sha256)
            {
                if fingerprint.len() != 64
                    || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(ProfileError::Invalid(format!(
                        "invalid structural hash for {}",
                        script.path
                    )));
                }
                if !fingerprints.insert(fingerprint) {
                    return Err(ProfileError::Invalid(format!(
                        "duplicate structural hash for {}: {}",
                        script.path, fingerprint
                    )));
                }
            }
            let mut ordinals = BTreeSet::new();
            for voice in &script.voices {
                if voice.ordinal >= script.set_text_count || !ordinals.insert(voice.ordinal) {
                    return Err(ProfileError::Invalid(format!(
                        "invalid or duplicate ordinal {} in {}",
                        voice.ordinal, script.path
                    )));
                }
                symbols.push(voice.symbol.as_str());
            }
        }
        for repair in &self.exact_repairs {
            if !repair.path.starts_with("scr/")
                || !repair.path.ends_with(".fsb")
                || repair.path.contains("..")
                || !paths.insert(repair.path.as_str())
            {
                return Err(ProfileError::Invalid(format!(
                    "invalid, duplicate, or already voiced repair path: {:?}",
                    repair.path
                )));
            }
            repair.validate()?;
        }
        if symbols.len() != self.voice_count {
            return Err(ProfileError::Invalid(format!(
                "profile contains {} symbols instead of {}",
                symbols.len(),
                self.voice_count
            )));
        }
        for (index, symbol) in symbols.into_iter().enumerate() {
            let expected = format!("SE_V{index:04}");
            if symbol != expected {
                return Err(ProfileError::Invalid(format!(
                    "symbol {symbol:?}; expected {expected:?}"
                )));
            }
        }
        Ok(())
    }

    pub fn validate_rom(&self, rom: &NdsRom<'_>) -> Result<(), ProfileError> {
        let game_code = String::from_utf8_lossy(&rom.header().game_code);
        if game_code != self.game_code {
            return Err(ProfileError::Invalid(format!(
                "ROM game code is {game_code:?}; the patch requires {:?}",
                self.game_code
            )));
        }
        for profile in &self.scripts {
            let data = rom
                .data_for_path(&profile.path)
                .ok_or_else(|| ProfileError::MissingScript(profile.path.clone()))?;
            profile.parse_compatible(data)?;
        }
        for repair in &self.exact_repairs {
            let data = rom
                .data_for_path(&repair.path)
                .ok_or_else(|| ProfileError::MissingScript(repair.path.clone()))?;
            repair.apply(data)?;
        }
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

impl ScriptProfile {
    pub fn accepts_structural_sha256(&self, fingerprint: &str) -> bool {
        fingerprint == self.structural_sha256
            || self
                .alternative_structural_sha256
                .iter()
                .any(|alternative| fingerprint == alternative)
    }

    pub fn parse_compatible(&self, data: &[u8]) -> Result<fsb::CompatibleParse, ProfileError> {
        let parsed = fsb::parse_repairing_invalid_relocated_pointers(data).map_err(|source| {
            ProfileError::Fsb {
                path: self.path.clone(),
                source,
            }
        })?;
        // Text must remain replaceable by translations, while control flow must
        // match a reviewed build so voice calls cannot drift onto another line.
        let structural_sha256 = parsed.script.structural_sha256();
        if !self.accepts_structural_sha256(&structural_sha256) {
            return Err(ProfileError::IncompatibleScript {
                path: self.path.clone(),
                reason: format!(
                    "unknown structure {structural_sha256} (primary fingerprint {} plus {} alternative(s))",
                    self.structural_sha256,
                    self.alternative_structural_sha256.len()
                ),
            });
        }
        if parsed.script.set_text_count() != self.set_text_count {
            return Err(ProfileError::IncompatibleScript {
                path: self.path.clone(),
                reason: format!(
                    "{} text operations instead of {}",
                    parsed.script.set_text_count(),
                    self.set_text_count
                ),
            });
        }
        Ok(parsed)
    }

    pub fn targets(&self) -> Vec<VoiceTarget> {
        self.voices
            .iter()
            .map(|voice| VoiceTarget {
                set_text_ordinal: voice.ordinal,
                symbol: voice.symbol.clone(),
            })
            .collect()
    }
}

impl ExactRepairProfile {
    fn validate(&self) -> Result<(), ProfileError> {
        if !valid_sha256(&self.damaged_sha256) || !valid_sha256(&self.repaired_sha256) {
            return Err(ProfileError::Invalid(format!(
                "invalid exact hash for {}",
                self.path
            )));
        }
        if self.accepted_clean_sha256.is_empty()
            || !self
                .accepted_clean_sha256
                .iter()
                .all(|hash| valid_sha256(hash))
        {
            return Err(ProfileError::Invalid(format!(
                "invalid clean-hash list for {}",
                self.path
            )));
        }
        let mut hashes = HashSet::new();
        if !hashes.insert(self.damaged_sha256.as_str())
            || self
                .accepted_clean_sha256
                .iter()
                .any(|hash| !hashes.insert(hash.as_str()))
            || !self
                .accepted_clean_sha256
                .iter()
                .any(|hash| hash == &self.repaired_sha256)
        {
            return Err(ProfileError::Invalid(format!(
                "inconsistent or duplicate exact hashes for {}",
                self.path
            )));
        }
        if self.edits.is_empty() {
            return Err(ProfileError::Invalid(format!(
                "no exact edits for {}",
                self.path
            )));
        }
        let mut previous_end = 0usize;
        for edit in &self.edits {
            let expected = decode_hex(&edit.expected_hex).ok_or_else(|| {
                ProfileError::Invalid(format!(
                    "invalid expected bytes at 0x{:x} in {}",
                    edit.offset, self.path
                ))
            })?;
            let replacement = decode_hex(&edit.replacement_hex).ok_or_else(|| {
                ProfileError::Invalid(format!(
                    "invalid replacement bytes at 0x{:x} in {}",
                    edit.offset, self.path
                ))
            })?;
            if expected.len() != replacement.len() || edit.offset < previous_end {
                return Err(ProfileError::Invalid(format!(
                    "overlapping or size-mismatched exact edit at 0x{:x} in {}",
                    edit.offset, self.path
                )));
            }
            previous_end = edit.offset.checked_add(expected.len()).ok_or_else(|| {
                ProfileError::Invalid(format!("offset overflow in {}", self.path))
            })?;
        }
        Ok(())
    }

    pub fn apply(&self, data: &[u8]) -> Result<ExactRepairOutcome, ProfileError> {
        let actual_sha256 = sha256_hex(data);
        // Exact hashes keep the narrowly approved French-patcher repair from
        // touching an unknown binary that merely happens to parse similarly.
        if actual_sha256 != self.damaged_sha256 {
            fsb::parse(data).map_err(|source| ProfileError::IncompatibleScript {
                path: self.path.clone(),
                reason: format!(
                    "unknown exact binary hash {actual_sha256} and invalid script: {source}"
                ),
            })?;
            return Ok(ExactRepairOutcome {
                data: data.to_vec(),
                repaired_fields: 0,
            });
        }

        let mut repaired = data.to_vec();
        for edit in &self.edits {
            let expected = decode_hex(&edit.expected_hex).expect("profile validated");
            let replacement = decode_hex(&edit.replacement_hex).expect("profile validated");
            let end = edit.offset.checked_add(expected.len()).ok_or_else(|| {
                ProfileError::IncompatibleScript {
                    path: self.path.clone(),
                    reason: format!("edit is out of bounds at 0x{:x}", edit.offset),
                }
            })?;
            let target = repaired.get_mut(edit.offset..end).ok_or_else(|| {
                ProfileError::IncompatibleScript {
                    path: self.path.clone(),
                    reason: format!("edit is out of bounds at 0x{:x}", edit.offset),
                }
            })?;
            if target != expected {
                return Err(ProfileError::IncompatibleScript {
                    path: self.path.clone(),
                    reason: format!(
                        "unexpected bytes at 0x{:x}: {:02x?} instead of {:02x?}",
                        edit.offset, target, expected
                    ),
                });
            }
            target.copy_from_slice(&replacement);
        }
        let repaired_sha256 = sha256_hex(&repaired);
        if repaired_sha256 != self.repaired_sha256 {
            return Err(ProfileError::IncompatibleScript {
                path: self.path.clone(),
                reason: format!(
                    "post-repair hash is {repaired_sha256}; expected {}",
                    self.repaired_sha256
                ),
            });
        }
        fsb::parse(&repaired).map_err(|source| ProfileError::Fsb {
            path: self.path.clone(),
            source,
        })?;
        Ok(ExactRepairOutcome {
            data: repaired,
            repaired_fields: self.edits.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    #[ignore = "requires a private retail ROM fixture"]
    fn production_profile_matches_the_stock_rom() {
        let profile = VoiceProfile::from_json(
            &fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let rom = fs::read(root().join("original/Nine Hours, Nine Persons, Nine Doors (USA).nds"))
            .unwrap();
        profile.validate_rom(&NdsRom::parse(&rom).unwrap()).unwrap();
        assert_eq!(profile.voice_count, 6_414);
        assert_eq!(profile.scripts.len(), 51);
        assert_eq!(profile.exact_repairs.len(), 5);
    }

    #[test]
    #[ignore = "requires private French-patcher scripts and generated comparison artifacts"]
    fn production_profile_matches_the_french_patcher_scripts() {
        let profile = VoiceProfile::from_json(
            &fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let french = root().join("Patcheur_auto_999_DS/tool/new_fsb");
        let mut repaired_files = 0usize;
        let mut repaired_pointers = 0usize;
        let mut alternative_fingerprints = 0usize;
        for expected in &profile.scripts {
            let source = fs::read(
                french.join(
                    Path::new(&expected.path)
                        .file_name()
                        .expect("script basename"),
                ),
            )
            .unwrap();
            let parsed = expected.parse_compatible(&source).unwrap();
            repaired_files += usize::from(parsed.repaired_invalid_pointers != 0);
            repaired_pointers += parsed.repaired_invalid_pointers;
            alternative_fingerprints +=
                usize::from(parsed.script.structural_sha256() != expected.structural_sha256);
        }
        assert_eq!(repaired_files, 11);
        assert_eq!(repaired_pointers, 12);
        assert_eq!(alternative_fingerprints, 21);

        let mut exact_repaired_fields = 0usize;
        for repair in &profile.exact_repairs {
            let source = fs::read(
                french.join(
                    Path::new(&repair.path)
                        .file_name()
                        .expect("script basename"),
                ),
            )
            .unwrap();
            let repaired = repair.apply(&source).unwrap();
            exact_repaired_fields += repaired.repaired_fields;
            assert_eq!(
                repaired.data,
                fs::read(
                    root()
                        .join("build/french-patcher-validation/safe-romfs/scr")
                        .join(Path::new(&repair.path).file_name().unwrap())
                )
                .unwrap()
            );
        }
        assert_eq!(exact_repaired_fields, 5);
        assert_eq!(repaired_pointers + exact_repaired_fields, 17);
    }

    #[test]
    #[ignore = "requires private script data extracted from a retail ROM"]
    fn rejects_an_unknown_structure_even_when_the_text_count_is_unchanged() {
        let profile = VoiceProfile::from_json(
            &fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let expected = profile
            .scripts
            .iter()
            .find(|script| script.path == "scr/a01b.fsb")
            .unwrap();
        let mut script =
            fsb::parse(&fs::read(root().join("work/romfs/scr/a01b.fsb")).unwrap()).unwrap();
        let value = script
            .functions
            .iter_mut()
            .flat_map(|function| &mut function.operations)
            .find_map(|operation| match operation {
                fsb::Operation::Number(value) => Some(value),
                _ => None,
            })
            .unwrap();
        *value = value.wrapping_add(1);
        let changed = script.to_bytes().unwrap();
        assert_eq!(
            fsb::parse(&changed).unwrap().set_text_count(),
            expected.set_text_count
        );
        assert!(matches!(
            expected.parse_compatible(&changed),
            Err(ProfileError::IncompatibleScript { .. })
        ));
    }

    #[test]
    #[ignore = "requires private French-patcher scripts and generated repair artifacts"]
    fn exact_repair_rejects_unknown_hash_and_unexpected_bytes() {
        let profile = VoiceProfile::from_json(
            &fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let repair = profile
            .exact_repairs
            .iter()
            .find(|repair| repair.path == "scr/a31.fsb")
            .unwrap();
        let damaged = fs::read(root().join("Patcheur_auto_999_DS/tool/new_fsb/a31.fsb")).unwrap();

        let mut unknown = damaged.clone();
        unknown[0x20] ^= 1;
        assert!(matches!(
            repair.apply(&unknown),
            Err(ProfileError::IncompatibleScript { reason, .. })
                if reason.contains("unknown exact binary hash") && reason.contains("invalid script")
        ));

        let mut wrong_bytes = damaged;
        wrong_bytes[repair.edits[0].offset] ^= 1;
        let mut matching_hash_but_wrong_bytes = repair.clone();
        matching_hash_but_wrong_bytes.damaged_sha256 = sha256_hex(&wrong_bytes);
        assert!(matches!(
            matching_hash_but_wrong_bytes.apply(&wrong_bytes),
            Err(ProfileError::IncompatibleScript { reason, .. })
                if reason.contains("unexpected bytes")
        ));

        let safe = fs::read(root().join("build/french-patcher-validation/safe-romfs/scr/a31.fsb"))
            .unwrap();
        let mut valid_other_translation = fsb::parse(&safe).unwrap();
        let text = valid_other_translation
            .strings
            .iter_mut()
            .find(|value| !value.is_empty())
            .unwrap();
        text.push(b'!');
        let valid_other_translation = valid_other_translation.to_bytes().unwrap();
        let outcome = repair.apply(&valid_other_translation).unwrap();
        assert_eq!(outcome.repaired_fields, 0);
        assert_eq!(outcome.data, valid_other_translation);
    }

    #[test]
    #[ignore = "requires a private ROM generated by the third-party French patcher"]
    fn production_profile_matches_the_french_patcher_rom() {
        let profile = VoiceProfile::from_json(
            &fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/voice-profile.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let rom = fs::read(root().join("build/french-patcher-validation/french-patcher-as-is.nds"))
            .unwrap();
        profile.validate_rom(&NdsRom::parse(&rom).unwrap()).unwrap();
    }
}
