use std::{io::Cursor, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    build::{self, BuildError, SAMPLE_RATE},
    project::{self, ProjectError, TargetProgress, MAX_GAIN_DB, MIN_GAIN_DB},
};
use crate::patcher::{
    engine::ResourcePaths,
    profile::VoiceProfile,
    voicepack::{Language, VoicePack, VoicePackError},
};

const LEVEL_BLOCK_SAMPLES: usize = 328;
const ABSOLUTE_GATE_DBFS: f64 = -50.0;
const RELATIVE_GATE_DB: f64 = 20.0;
// A decoded peak of 29,204 is the largest integer sample that remains at or below -1 dBFS.
// Comparing integers keeps the safety decision stable across platforms and Rust versions.
const FINAL_DS_PEAK_LIMIT: u32 = 29_204;
const GAIN_TENTHS_MINIMUM: i32 = -600;
// Studio takes cannot exceed 45 seconds, so 2 MiB leaves ample format overhead while preventing
// an untrusted pack index from driving a multi-gigabyte allocation before entry authentication.
const MAX_REFERENCE_ENTRY_BYTES: u32 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DubbingReferenceLanguage {
    Japanese,
    English,
}

impl DubbingReferenceLanguage {
    fn pack_path(self, resources: &ResourcePaths) -> &Path {
        match self {
            Self::Japanese => &resources.japanese_voicepack,
            Self::English => &resources.english_voicepack,
        }
    }

    fn pack_language(self) -> Language {
        match self {
            Self::Japanese => Language::Japanese,
            Self::English => Language::English,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GainLimit {
    Minimum,
    Maximum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelUnavailableReason {
    TakeSilent,
    ReferenceSilent,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioLevelMetrics {
    pub sample_rate: u32,
    pub frames: u64,
    pub duration_ms: u64,
    pub peak_dbfs: Option<f32>,
    pub active_rms_dbfs: Option<f32>,
    pub total_blocks: u64,
    pub active_blocks: u64,
    pub active_frames: u64,
    pub gate_dbfs: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingLevelAnalysis {
    pub symbol: String,
    pub take_id: String,
    pub reference_language: DubbingReferenceLanguage,
    pub take: AudioLevelMetrics,
    pub processed: AudioLevelMetrics,
    pub reference: AudioLevelMetrics,
    pub current_gain_db: f32,
    pub processed_clipped_samples: u64,
    pub unconstrained_gain_db: Option<f32>,
    pub recommended_gain_db: Option<f32>,
    pub headroom_gain_db: Option<f32>,
    pub headroom_limited: bool,
    pub gain_limited: bool,
    pub gain_limit: Option<GainLimit>,
    pub unavailable_reason: Option<LevelUnavailableReason>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingLevelMatchUpdate {
    pub symbol: String,
    pub progress: TargetProgress,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubbingLevelMatchBatchResult {
    pub updated: Vec<DubbingLevelMatchUpdate>,
    pub matched: usize,
    pub headroom_limited: usize,
    pub gain_limited: usize,
    pub skipped: usize,
}

#[derive(Debug, Error)]
pub enum AudioLevelError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    VoicePack(#[from] VoicePackError),
    #[error(
        "reference voice entry {index} for {symbol} is {size} bytes; the limit is {limit} bytes"
    )]
    ReferenceEntryTooLarge {
        index: usize,
        symbol: String,
        size: u32,
        limit: u32,
    },
    #[error("invalid dubbing audio operation: {0}")]
    Invalid(String),
}

fn dbfs_from_mean_square(mean_square: f64) -> Option<f64> {
    (mean_square > 0.0).then(|| 10.0 * mean_square.log10())
}

pub fn measure_active_level(samples: &[i16]) -> AudioLevelMetrics {
    let blocks = samples
        .chunks(LEVEL_BLOCK_SAMPLES)
        .map(|block| {
            let sum_squares = block.iter().fold(0.0, |sum, sample| {
                let normalized = f64::from(*sample) / 32768.0;
                sum + normalized * normalized
            });
            let mean_square = sum_squares / block.len() as f64;
            (sum_squares, block.len(), dbfs_from_mean_square(mean_square))
        })
        .collect::<Vec<_>>();
    let loudest_block = blocks
        .iter()
        .filter_map(|(_, _, dbfs)| *dbfs)
        .fold(f64::NEG_INFINITY, f64::max);
    let gate_dbfs = if loudest_block.is_finite() {
        ABSOLUTE_GATE_DBFS.max(loudest_block - RELATIVE_GATE_DB)
    } else {
        ABSOLUTE_GATE_DBFS
    };
    let mut active_sum_squares = 0.0;
    let mut active_frames = 0_usize;
    let mut active_blocks = 0_usize;
    for (sum_squares, frames, block_dbfs) in &blocks {
        if block_dbfs.is_some_and(|level| level >= gate_dbfs) {
            active_sum_squares += sum_squares;
            active_frames += frames;
            active_blocks += 1;
        }
    }
    let peak = samples
        .iter()
        .map(|sample| i32::from(*sample).unsigned_abs())
        .max()
        .unwrap_or(0);
    let peak_dbfs = (peak > 0).then(|| 20.0 * (f64::from(peak) / 32768.0).log10());
    let active_rms_dbfs = (active_frames > 0)
        .then(|| dbfs_from_mean_square(active_sum_squares / active_frames as f64))
        .flatten();
    AudioLevelMetrics {
        sample_rate: SAMPLE_RATE,
        frames: samples.len() as u64,
        duration_ms: samples.len() as u64 * 1_000 / u64::from(SAMPLE_RATE),
        peak_dbfs: peak_dbfs.map(|value| value as f32),
        active_rms_dbfs: active_rms_dbfs.map(|value| value as f32),
        total_blocks: blocks.len() as u64,
        active_blocks: active_blocks as u64,
        active_frames: active_frames as u64,
        gate_dbfs: gate_dbfs as f32,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FinalDsPeak {
    decoded_peak: u32,
    clipped_samples: u64,
}

impl FinalDsPeak {
    fn is_safe(self) -> bool {
        self.clipped_samples == 0 && self.decoded_peak <= FINAL_DS_PEAK_LIMIT
    }
}

fn final_ds_peak(source: &[f64], gain_db: f32) -> Result<FinalDsPeak, BuildError> {
    let (pcm, clipped_samples) = build::apply_gain_to_source(source, gain_db)?;
    let decoded_peak = build::ima_roundtrip_pcm(&pcm)?
        .iter()
        .map(|sample| i32::from(*sample).unsigned_abs())
        .max()
        .unwrap_or(0);
    Ok(FinalDsPeak {
        decoded_peak,
        clipped_samples,
    })
}

fn gain_tenths_down(gain_db: f32) -> i32 {
    let scaled = f64::from(gain_db) * 10.0;
    let nearest = scaled.round();
    if (scaled - nearest).abs() <= 0.000_001 {
        nearest as i32
    } else {
        scaled.floor() as i32
    }
}

fn unclipped_gain_ceiling_db(source: &[f64]) -> Result<f32, BuildError> {
    if source.iter().any(|sample| !sample.is_finite()) {
        return Err(BuildError::Invalid(
            "audio contains a non-finite sample after resampling".to_owned(),
        ));
    }
    let source_peak = source
        .iter()
        .fold(0.0_f64, |peak, sample| peak.max(sample.abs()));
    if source_peak == 0.0 {
        return Ok(MAX_GAIN_DB);
    }
    let ceiling = 20.0 * (build::PCM_CLAMP_LIMIT / source_peak).log10();
    Ok(ceiling.clamp(f64::from(MIN_GAIN_DB), f64::from(MAX_GAIN_DB)) as f32)
}

fn codec_safe_gain(source: &[f64], candidate_gain_db: f32) -> Result<(f32, bool), BuildError> {
    let candidate_is_safe = final_ds_peak(source, candidate_gain_db)?.is_safe();
    let candidate_tenths = gain_tenths_down(candidate_gain_db);
    let unclipped_ceiling = unclipped_gain_ceiling_db(source)?;
    let start_gain = candidate_gain_db.min(unclipped_ceiling);
    let start_tenths = gain_tenths_down(start_gain).max(GAIN_TENTHS_MINIMUM);

    // IMA peak response is not monotone with input gain, so checking each persisted 0.1 dB step
    // is necessary to retain the loudest actually safe setting instead of skipping a safe island.
    for gain_tenths in (GAIN_TENTHS_MINIMUM..=start_tenths).rev() {
        let gain_db = gain_tenths as f32 / 10.0;
        if final_ds_peak(source, gain_db)?.is_safe() {
            let headroom_limited = !candidate_is_safe
                || start_gain < candidate_gain_db
                || gain_tenths < candidate_tenths;
            return Ok((gain_db, headroom_limited));
        }
    }

    Err(BuildError::Invalid(format!(
        "DS audio exceeds -1 dBFS even at the minimum {MIN_GAIN_DB} dB gain"
    )))
}

struct GainRecommendationContext<'a> {
    symbol: &'a str,
    take_id: &'a str,
    language: DubbingReferenceLanguage,
    take_source: &'a [f64],
    current_gain_db: f32,
    processed_clipped_samples: u64,
}

fn gain_recommendation(
    context: GainRecommendationContext<'_>,
    take: AudioLevelMetrics,
    processed: AudioLevelMetrics,
    reference: AudioLevelMetrics,
) -> Result<DubbingLevelAnalysis, AudioLevelError> {
    let unavailable_reason = if take.active_rms_dbfs.is_none() {
        Some(LevelUnavailableReason::TakeSilent)
    } else if reference.active_rms_dbfs.is_none() {
        Some(LevelUnavailableReason::ReferenceSilent)
    } else {
        None
    };
    let Some(take_active) = take.active_rms_dbfs else {
        return Ok(DubbingLevelAnalysis {
            symbol: context.symbol.to_owned(),
            take_id: context.take_id.to_owned(),
            reference_language: context.language,
            take,
            processed,
            reference,
            current_gain_db: context.current_gain_db,
            processed_clipped_samples: context.processed_clipped_samples,
            unconstrained_gain_db: None,
            recommended_gain_db: None,
            headroom_gain_db: None,
            headroom_limited: false,
            gain_limited: false,
            gain_limit: None,
            unavailable_reason,
        });
    };
    let Some(reference_active) = reference.active_rms_dbfs else {
        return Ok(DubbingLevelAnalysis {
            symbol: context.symbol.to_owned(),
            take_id: context.take_id.to_owned(),
            reference_language: context.language,
            take,
            processed,
            reference,
            current_gain_db: context.current_gain_db,
            processed_clipped_samples: context.processed_clipped_samples,
            unconstrained_gain_db: None,
            recommended_gain_db: None,
            headroom_gain_db: None,
            headroom_limited: false,
            gain_limited: false,
            gain_limit: None,
            unavailable_reason,
        });
    };
    let unconstrained = f64::from(reference_active) - f64::from(take_active);
    let (bounded, gain_limit) = if unconstrained < f64::from(MIN_GAIN_DB) {
        (MIN_GAIN_DB, Some(GainLimit::Minimum))
    } else if unconstrained > f64::from(MAX_GAIN_DB) {
        (MAX_GAIN_DB, Some(GainLimit::Maximum))
    } else {
        (unconstrained as f32, None)
    };
    let (recommended, headroom_limited) = codec_safe_gain(context.take_source, bounded)?;
    Ok(DubbingLevelAnalysis {
        symbol: context.symbol.to_owned(),
        take_id: context.take_id.to_owned(),
        reference_language: context.language,
        take,
        processed,
        reference,
        current_gain_db: context.current_gain_db,
        processed_clipped_samples: context.processed_clipped_samples,
        unconstrained_gain_db: Some(unconstrained as f32),
        recommended_gain_db: Some(recommended),
        headroom_gain_db: Some(recommended),
        headroom_limited,
        gain_limited: gain_limit.is_some(),
        gain_limit,
        unavailable_reason: None,
    })
}

fn voice_index(profile: &VoiceProfile, symbol: &str) -> Result<usize, AudioLevelError> {
    profile
        .scripts
        .iter()
        .flat_map(|script| &script.voices)
        .position(|voice| voice.symbol == symbol)
        .ok_or_else(|| AudioLevelError::Invalid(format!("unknown voice target {symbol}")))
}

fn open_reference_pack(
    profile: &VoiceProfile,
    resources: &ResourcePaths,
    language: DubbingReferenceLanguage,
) -> Result<VoicePack, AudioLevelError> {
    let pack = VoicePack::open(language.pack_path(resources))?;
    if pack.language() != language.pack_language() {
        return Err(AudioLevelError::Invalid(format!(
            "selected {:?} reference pack identifies itself as {}",
            language,
            pack.language().code()
        )));
    }
    if pack
        .catalog_sha256()
        .is_some_and(|catalog| catalog != profile.catalog_sha256())
    {
        return Err(AudioLevelError::Invalid(
            "reference pack does not match the project's voice catalogue".to_owned(),
        ));
    }
    if pack.entries().len() != profile.voice_count {
        return Err(AudioLevelError::Invalid(format!(
            "reference pack contains {} entries instead of {}",
            pack.entries().len(),
            profile.voice_count
        )));
    }
    Ok(pack)
}

fn reference_pcm(
    pack: &VoicePack,
    profile: &VoiceProfile,
    symbol: &str,
) -> Result<Vec<i16>, AudioLevelError> {
    let index = voice_index(profile, symbol)?;
    let entry_size = pack.entries()[index].size;
    if entry_size > MAX_REFERENCE_ENTRY_BYTES {
        return Err(AudioLevelError::ReferenceEntryTooLarge {
            index,
            symbol: symbol.to_owned(),
            size: entry_size,
            limit: MAX_REFERENCE_ENTRY_BYTES,
        });
    }
    let mut resource = Vec::with_capacity(entry_size as usize);
    pack.copy_entry_to(index, &mut resource)?;
    Ok(build::decode_named_se_pcm(&resource, symbol)?)
}

fn analyze_target(
    project_dir: &Path,
    profile: &VoiceProfile,
    pack: &VoicePack,
    language: DubbingReferenceLanguage,
    symbol: &str,
    target: &TargetProgress,
) -> Result<DubbingLevelAnalysis, AudioLevelError> {
    let take_id = target.active_take.as_deref().ok_or_else(|| {
        AudioLevelError::Invalid(format!("target {symbol} has no active take to analyse"))
    })?;
    let take_source = build::render_active_take_source(project_dir, symbol, target)?;
    let (take_pcm, _) = build::apply_gain_to_source(&take_source, 0.0)?;
    let (processed_pcm, processed_clipped_samples) =
        build::apply_gain_to_source(&take_source, target.gain_db)?;
    let processed_pcm = build::ima_roundtrip_pcm(&processed_pcm)?;
    let reference_pcm = reference_pcm(pack, profile, symbol)?;
    gain_recommendation(
        GainRecommendationContext {
            symbol,
            take_id,
            language,
            take_source: &take_source,
            current_gain_db: target.gain_db,
            processed_clipped_samples,
        },
        measure_active_level(&take_pcm),
        measure_active_level(&processed_pcm),
        measure_active_level(&reference_pcm),
    )
}

pub fn analyze_dubbing_level(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    resources: &ResourcePaths,
    symbol: &str,
    language: DubbingReferenceLanguage,
) -> Result<DubbingLevelAnalysis, AudioLevelError> {
    let project_dir = project_dir.as_ref();
    let project_lock = project::lock_project_mutations(project_dir)?;
    let state =
        project::load_locked_project_manifest(project_dir, profile_path.as_ref(), &project_lock)?;
    let target = state
        .manifest
        .targets
        .get(symbol)
        .ok_or_else(|| AudioLevelError::Invalid(format!("unknown voice target {symbol}")))?;
    let pack = open_reference_pack(&state.profile, resources, language)?;
    analyze_target(project_dir, &state.profile, &pack, language, symbol, target)
}

pub fn match_all_dubbing_levels(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    resources: &ResourcePaths,
    language: DubbingReferenceLanguage,
) -> Result<DubbingLevelMatchBatchResult, AudioLevelError> {
    let project_dir = project_dir.as_ref();
    let project_lock = project::lock_project_mutations(project_dir)?;
    let mut state =
        project::load_locked_project_manifest(project_dir, profile_path.as_ref(), &project_lock)?;
    let pack = open_reference_pack(&state.profile, resources, language)?;
    let symbols = state
        .profile
        .scripts
        .iter()
        .flat_map(|script| &script.voices)
        .map(|voice| voice.symbol.clone())
        .collect::<Vec<_>>();
    let mut result = DubbingLevelMatchBatchResult {
        updated: Vec::new(),
        matched: 0,
        headroom_limited: 0,
        gain_limited: 0,
        skipped: 0,
    };
    let mut changed = false;
    for symbol in symbols {
        let Some(target) = state.manifest.targets.get(&symbol) else {
            return Err(AudioLevelError::Invalid(format!(
                "project is missing voice target {symbol}"
            )));
        };
        if target.active_take.is_none() {
            continue;
        }
        let analysis = analyze_target(
            project_dir,
            &state.profile,
            &pack,
            language,
            &symbol,
            target,
        )?;
        let Some(recommended_gain_db) = analysis.recommended_gain_db else {
            result.skipped += 1;
            continue;
        };
        result.matched += 1;
        result.headroom_limited += usize::from(analysis.headroom_limited);
        result.gain_limited += usize::from(analysis.gain_limited);
        let target = state
            .manifest
            .targets
            .get_mut(&symbol)
            .expect("target existence was checked");
        changed |= project::apply_processing_settings(
            &symbol,
            target,
            recommended_gain_db,
            target.trim_start_ms,
            target.trim_end_ms,
        )?;
        result.updated.push(DubbingLevelMatchUpdate {
            symbol,
            progress: target.clone(),
        });
    }
    if changed {
        project::save_locked_project_manifest(project_dir, &mut state, &project_lock)?;
    }
    Ok(result)
}

pub fn processed_preview_wav(
    project_dir: impl AsRef<Path>,
    profile_path: impl AsRef<Path>,
    symbol: &str,
) -> Result<Vec<u8>, AudioLevelError> {
    let project_dir = project_dir.as_ref();
    let project_lock = project::lock_project_mutations(project_dir)?;
    let state =
        project::load_locked_project_manifest(project_dir, profile_path.as_ref(), &project_lock)?;
    let target = state
        .manifest
        .targets
        .get(symbol)
        .ok_or_else(|| AudioLevelError::Invalid(format!("unknown voice target {symbol}")))?;
    let pcm = build::render_active_take_pcm(project_dir, symbol, target, true)?;
    wav_bytes(&build::ima_roundtrip_pcm(&pcm)?)
}

pub fn reference_preview_wav(
    profile_path: impl AsRef<Path>,
    resources: &ResourcePaths,
    symbol: &str,
    language: DubbingReferenceLanguage,
) -> Result<Vec<u8>, AudioLevelError> {
    let profile_data = std::fs::read(profile_path.as_ref()).map_err(|error| {
        AudioLevelError::Invalid(format!("cannot read dubbing profile: {error}"))
    })?;
    let profile = VoiceProfile::from_json(&profile_data).map_err(ProjectError::from)?;
    let pack = open_reference_pack(&profile, resources, language)?;
    wav_bytes(&reference_pcm(&pack, &profile, symbol)?)
}

fn wav_bytes(samples: &[i16]) -> Result<Vec<u8>, AudioLevelError> {
    if samples.is_empty() {
        return Err(AudioLevelError::Invalid(
            "cannot preview empty audio".to_owned(),
        ));
    }
    let data_size = samples
        .len()
        .checked_mul(2)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| AudioLevelError::Invalid("preview WAV is too large".to_owned()))?;
    let mut output = Cursor::new(Vec::with_capacity(data_size as usize + 44));
    use std::io::Write;
    output.write_all(b"RIFF").unwrap();
    output
        .write_all(&(36_u32 + data_size).to_le_bytes())
        .unwrap();
    output.write_all(b"WAVEfmt ").unwrap();
    output.write_all(&16_u32.to_le_bytes()).unwrap();
    output.write_all(&1_u16.to_le_bytes()).unwrap();
    output.write_all(&1_u16.to_le_bytes()).unwrap();
    output.write_all(&SAMPLE_RATE.to_le_bytes()).unwrap();
    output.write_all(&(SAMPLE_RATE * 2).to_le_bytes()).unwrap();
    output.write_all(&2_u16.to_le_bytes()).unwrap();
    output.write_all(&16_u16.to_le_bytes()).unwrap();
    output.write_all(b"data").unwrap();
    output.write_all(&data_size.to_le_bytes()).unwrap();
    for sample in samples {
        output.write_all(&sample.to_le_bytes()).unwrap();
    }
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant_block(amplitude: i16, blocks: usize) -> Vec<i16> {
        vec![amplitude; LEVEL_BLOCK_SAMPLES * blocks]
    }

    fn source_from_pcm(samples: &[i16]) -> Vec<f64> {
        samples
            .iter()
            .map(|sample| f64::from(*sample) / f64::from(i16::MAX))
            .collect()
    }

    fn single_voice_profile() -> VoiceProfile {
        VoiceProfile::from_json(
            format!(
                r#"{{
  "version": 1,
  "game_code": "BSKE",
  "voice_count": 1,
  "scripts": [{{
    "path": "scr/test.fsb",
    "structural_sha256": "{}",
    "set_text_count": 1,
    "voices": [{{"ordinal": 0, "symbol": "SE_V0000"}}]
  }}]
}}"#,
                "00".repeat(32)
            )
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn active_rms_uses_the_absolute_and_relative_gate() {
        let mut pcm = constant_block(3_276, 1);
        pcm.extend(constant_block(164, 1));
        pcm.extend(constant_block(0, 1));
        let metrics = measure_active_level(&pcm);
        assert_eq!(metrics.total_blocks, 3);
        assert_eq!(metrics.active_blocks, 1);
        assert_eq!(metrics.active_frames, LEVEL_BLOCK_SAMPLES as u64);
        assert!((-20.02..=-19.98).contains(&metrics.active_rms_dbfs.unwrap()));
        assert!((-40.02..=-39.98).contains(&metrics.gate_dbfs));
    }

    #[test]
    fn final_partial_block_is_included() {
        let pcm = vec![3_276; LEVEL_BLOCK_SAMPLES + 17];
        let metrics = measure_active_level(&pcm);
        assert_eq!(metrics.total_blocks, 2);
        assert_eq!(metrics.active_blocks, 2);
        assert_eq!(metrics.active_frames, pcm.len() as u64);
    }

    #[test]
    fn silence_has_no_active_metric() {
        let metrics = measure_active_level(&vec![0; LEVEL_BLOCK_SAMPLES]);
        assert_eq!(metrics.peak_dbfs, None);
        assert_eq!(metrics.active_rms_dbfs, None);
        assert_eq!(metrics.active_blocks, 0);
        assert_eq!(metrics.gate_dbfs, ABSOLUTE_GATE_DBFS as f32);
    }

    #[test]
    fn gain_matching_reports_peak_headroom_and_gain_bounds() {
        let quiet = measure_active_level(&constant_block(328, 1));
        let loud = measure_active_level(&constant_block(16_384, 1));
        let quiet_source = source_from_pcm(&constant_block(328, 1));
        let ordinary = gain_recommendation(
            GainRecommendationContext {
                symbol: "SE_V0000",
                take_id: "take-1",
                language: DubbingReferenceLanguage::Japanese,
                take_source: &quiet_source,
                current_gain_db: 0.0,
                processed_clipped_samples: 0,
            },
            quiet.clone(),
            quiet.clone(),
            loud.clone(),
        )
        .unwrap();
        assert!((ordinary.unconstrained_gain_db.unwrap() - 33.98).abs() < 0.05);
        assert_eq!(ordinary.recommended_gain_db, Some(MAX_GAIN_DB));
        assert!(ordinary.gain_limited);
        assert_eq!(ordinary.gain_limit, Some(GainLimit::Maximum));

        let mut peaky_pcm = constant_block(3_000, 1);
        peaky_pcm[0] = 30_000;
        let peaky_source = source_from_pcm(&peaky_pcm);
        let near_peak = measure_active_level(&peaky_pcm);
        let headroom = gain_recommendation(
            GainRecommendationContext {
                symbol: "SE_V0000",
                take_id: "take-1",
                language: DubbingReferenceLanguage::English,
                take_source: &peaky_source,
                current_gain_db: 0.0,
                processed_clipped_samples: 0,
            },
            near_peak,
            quiet,
            loud,
        )
        .unwrap();
        assert!(headroom.headroom_limited);
        assert!(headroom.recommended_gain_db.unwrap() < 0.0);
        assert!(!headroom.gain_limited);
    }

    #[test]
    fn recommendation_accounts_for_ima_decode_overshoot() {
        let pcm = (0..LEVEL_BLOCK_SAMPLES)
            .map(|index| (29_204.0 * (0.01 * index as f64).sin()).round() as i16)
            .collect::<Vec<_>>();
        assert_eq!(
            pcm.iter()
                .map(|sample| i32::from(*sample).unsigned_abs())
                .max(),
            Some(FINAL_DS_PEAK_LIMIT)
        );
        assert!(
            build::ima_roundtrip_pcm(&pcm)
                .unwrap()
                .iter()
                .map(|sample| i32::from(*sample).unsigned_abs())
                .max()
                .unwrap()
                > FINAL_DS_PEAK_LIMIT
        );

        let source = source_from_pcm(&pcm);
        let take = measure_active_level(&pcm);
        let processed = measure_active_level(&build::ima_roundtrip_pcm(&pcm).unwrap());
        let analysis = gain_recommendation(
            GainRecommendationContext {
                symbol: "SE_V0000",
                take_id: "take-1",
                language: DubbingReferenceLanguage::Japanese,
                take_source: &source,
                current_gain_db: 0.0,
                processed_clipped_samples: 0,
            },
            take.clone(),
            processed,
            take,
        )
        .unwrap();
        let recommended = analysis.recommended_gain_db.unwrap();
        assert!(recommended < 0.0);
        assert!(analysis.headroom_limited);
        assert_eq!(recommended, -0.1);
        let final_peak = final_ds_peak(&source, recommended).unwrap();
        assert!(final_peak.is_safe());
    }

    #[test]
    fn downward_scan_retains_the_highest_safe_nonmonotone_ima_step() {
        let pcm = [
            -13_156_i16,
            -23_211,
            16_654,
            11_706,
            314,
            -13_840,
            3_193,
            -21_610,
            20_888,
            -4_641,
            -12_031,
            -9_870,
            19_890,
            -483,
            16_659,
            24_391,
        ];
        let source = source_from_pcm(&pcm);
        assert!(final_ds_peak(&source, 1.1).unwrap().is_safe());
        assert!(!final_ds_peak(&source, 1.4).unwrap().is_safe());
        assert!(final_ds_peak(&source, 1.7).unwrap().is_safe());
        assert!(!final_ds_peak(&source, 1.8).unwrap().is_safe());
        assert_eq!(codec_safe_gain(&source, 6.0).unwrap(), (1.7, true));
    }

    #[test]
    fn safe_recommendations_use_downward_tenths_and_never_clip_build_pcm() {
        let quiet_source = vec![0.01; LEVEL_BLOCK_SAMPLES];
        let (quantized, headroom_limited) = codec_safe_gain(&quiet_source, 1.29).unwrap();
        assert_eq!(quantized, 1.2);
        assert!(!headroom_limited);
        let (already_quantized, headroom_limited) = codec_safe_gain(&quiet_source, -1.2).unwrap();
        assert_eq!(already_quantized, -1.2);
        assert!(!headroom_limited);

        let mut transient_source = vec![0.0; LEVEL_BLOCK_SAMPLES];
        transient_source[LEVEL_BLOCK_SAMPLES / 2] = 0.1;
        let unsafe_output = final_ds_peak(&transient_source, MAX_GAIN_DB).unwrap();
        assert!(unsafe_output.clipped_samples > 0);
        let (raw_pcm, _) = build::apply_gain_to_source(&transient_source, 0.0).unwrap();
        let (current_pcm, current_clipped_samples) =
            build::apply_gain_to_source(&transient_source, MAX_GAIN_DB).unwrap();
        let take = measure_active_level(&raw_pcm);
        let analysis = gain_recommendation(
            GainRecommendationContext {
                symbol: "SE_V0000",
                take_id: "take-1",
                language: DubbingReferenceLanguage::English,
                take_source: &transient_source,
                current_gain_db: MAX_GAIN_DB,
                processed_clipped_samples: current_clipped_samples,
            },
            take.clone(),
            measure_active_level(&build::ima_roundtrip_pcm(&current_pcm).unwrap()),
            take,
        )
        .unwrap();
        assert_eq!(
            analysis.processed_clipped_samples,
            unsafe_output.clipped_samples
        );
        let (recommended, headroom_limited) =
            codec_safe_gain(&transient_source, MAX_GAIN_DB).unwrap();
        assert!(headroom_limited);
        assert_eq!(recommended * 10.0, (recommended * 10.0).round());
        let safe_output = final_ds_peak(&transient_source, recommended).unwrap();
        assert!(safe_output.is_safe());
        assert_eq!(
            build::apply_gain_to_source(&transient_source, recommended)
                .unwrap()
                .1,
            0
        );
    }

    #[test]
    fn wav_preview_is_mono_pcm16_at_the_ds_rate() {
        let bytes = wav_bytes(&[1, -2, 3]).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.duration(), 3);
    }

    #[test]
    fn reference_matching_accepts_authenticated_v1_retail_packs() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("voice.se");
        std::fs::write(&source, b"SIR0synthetic-reference").unwrap();
        let pack_path = directory.path().join("voices-jp.nvpack");
        VoicePack::write_atomic_from_files(
            &pack_path,
            Language::Japanese,
            std::slice::from_ref(&source),
        )
        .unwrap();
        let profile = single_voice_profile();
        let resources = ResourcePaths {
            profile: directory.path().join("voice-profile.json"),
            japanese_voicepack: pack_path,
            english_voicepack: directory.path().join("unused-en.nvpack"),
        };
        let pack =
            open_reference_pack(&profile, &resources, DubbingReferenceLanguage::Japanese).unwrap();
        assert_eq!(pack.catalog_sha256(), None);
        assert_eq!(pack.entries().len(), 1);
    }

    #[test]
    fn oversized_reference_entry_is_rejected_before_allocation_or_decode() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("oversized.se");
        std::fs::write(&source, vec![0_u8; MAX_REFERENCE_ENTRY_BYTES as usize + 1]).unwrap();
        let pack_path = directory.path().join("voices-en.nvpack");
        VoicePack::write_atomic_from_files(
            &pack_path,
            Language::English,
            std::slice::from_ref(&source),
        )
        .unwrap();
        let pack = VoicePack::open(&pack_path).unwrap();
        let error = reference_pcm(&pack, &single_voice_profile(), "SE_V0000").unwrap_err();
        assert!(matches!(
            error,
            AudioLevelError::ReferenceEntryTooLarge {
                index: 0,
                size,
                limit: MAX_REFERENCE_ENTRY_BYTES,
                ..
            } if size == MAX_REFERENCE_ENTRY_BYTES + 1
        ));
    }

    #[test]
    #[ignore = "requires proprietary production voice packs"]
    fn production_reference_packs_decode_selected_entries() {
        let resource_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let profile_path = resource_dir.join("voice-profile.json");
        let profile = VoiceProfile::from_json(&std::fs::read(&profile_path).unwrap()).unwrap();
        let resources = ResourcePaths {
            profile: profile_path,
            japanese_voicepack: resource_dir.join("voices-jp.nvpack"),
            english_voicepack: resource_dir.join("voices-en.nvpack"),
        };
        for language in [
            DubbingReferenceLanguage::Japanese,
            DubbingReferenceLanguage::English,
        ] {
            let pack = open_reference_pack(&profile, &resources, language).unwrap();
            for symbol in ["SE_V0000", "SE_V3237", "SE_V6474"] {
                let pcm = reference_pcm(&pack, &profile, symbol).unwrap();
                assert!(!pcm.is_empty());
                assert_eq!(measure_active_level(&pcm).sample_rate, SAMPLE_RATE);
            }

            let mut first_entry = Vec::with_capacity(pack.entries()[0].size as usize);
            pack.copy_entry_to(0, &mut first_entry).unwrap();
            let error = build::decode_named_se_pcm(&first_entry, "SE_V6474").unwrap_err();
            assert!(error
                .to_string()
                .contains("voice entry contains SE_V0000 instead of the expected SE_V6474"));
        }
    }
}
