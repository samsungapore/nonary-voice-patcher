use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig,
};
use hound::{WavSpec, WavWriter};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_RECORDING_SECONDS: u64 = 45;
const MAX_SAMPLE_RATE: u32 = 384_000;
const CAPTURE_CHUNK_FRAMES: usize = 2_048;
const CAPTURE_BUFFER_COUNT: usize = 32;
const SILENCE_DBFS: f32 = -120.0;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStartInfo {
    pub path: PathBuf,
    pub device: AudioInputDevice,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSummary {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub frames: u64,
    pub peak_dbfs: f32,
    pub clipped_samples: u64,
    pub overflowed: bool,
}

#[derive(Debug, Error)]
pub enum RecordingError {
    #[error("No audio input device is available.")]
    NoInputDevice,
    #[error("Audio input device `{0}` is no longer available.")]
    DeviceNotFound(String),
    #[error("Could not enumerate audio input devices: {0}")]
    DeviceEnumeration(String),
    #[error("Could not inspect audio input device `{device}`: {message}")]
    DeviceConfiguration { device: String, message: String },
    #[error("Audio sample format `{0}` is not supported for recording.")]
    UnsupportedSampleFormat(String),
    #[error("Audio input sample rate {0} Hz exceeds the supported 384000 Hz limit.")]
    UnsupportedSampleRate(u32),
    #[error("A recording is already in progress.")]
    AlreadyRecording,
    #[error("No recording is in progress.")]
    NotRecording,
    #[error("Recorder state is unavailable after an internal synchronization failure.")]
    StatePoisoned,
    #[error("Could not create recording `{path}`: {message}")]
    CreateOutput { path: PathBuf, message: String },
    #[error("Could not initialize audio input: {0}")]
    BuildStream(String),
    #[error("Could not start audio input: {0}")]
    StartStream(String),
    #[error("Could not start the recording writer: {0}")]
    StartWriter(String),
    #[error("The recording writer failed: {0}")]
    WriterFailed(String),
    #[error("The recording writer stopped unexpectedly.")]
    WriterPanicked,
    #[error("The recorder thread stopped unexpectedly.")]
    RecorderPanicked,
    #[error("{reason}")]
    InvalidRecording {
        reason: String,
        summary: Box<RecordingSummary>,
    },
}

impl RecordingError {
    pub fn summary(&self) -> Option<&RecordingSummary> {
        match self {
            Self::InvalidRecording { summary, .. } => Some(summary),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct RecorderState {
    session: Mutex<Option<ControllerSession>>,
}

impl RecorderState {
    pub fn list_input_devices(&self) -> Result<Vec<AudioInputDevice>, RecordingError> {
        list_input_devices()
    }

    pub fn start(
        &self,
        output_temp_path: impl AsRef<Path>,
        device_identifier: Option<&str>,
    ) -> Result<RecordingStartInfo, RecordingError> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| RecordingError::StatePoisoned)?;
        if session.is_some() {
            return Err(RecordingError::AlreadyRecording);
        }
        let output_path = output_temp_path.as_ref().to_path_buf();
        let requested_device = device_identifier.map(str::to_owned);
        let (control_tx, control_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        // CoreAudio streams are deliberately confined to their owner thread because CPAL cannot
        // safely move its macOS property listener across threads.
        let thread_handle = thread::Builder::new()
            .name("dubbing-recorder".to_owned())
            .spawn(move || {
                run_recording_session(output_path, requested_device, ready_tx, control_rx)
            })
            .map_err(|error| RecordingError::BuildStream(error.to_string()))?;

        match ready_rx.recv() {
            Ok(ReadyMessage::Started(start_info)) => {
                *session = Some(ControllerSession {
                    control_tx: Some(control_tx),
                    thread_handle: Some(thread_handle),
                    finished: false,
                });
                Ok(start_info)
            }
            Ok(ReadyMessage::Failed) | Err(_) => match thread_handle.join() {
                Ok(Err(error)) => Err(error),
                Ok(Ok(_)) => Err(RecordingError::BuildStream(
                    "the recorder stopped before audio input was ready".to_owned(),
                )),
                Err(_) => Err(RecordingError::RecorderPanicked),
            },
        }
    }

    pub fn stop(&self) -> Result<RecordingSummary, RecordingError> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| RecordingError::StatePoisoned)?;
        let mut active = session.take().ok_or(RecordingError::NotRecording)?;
        active.finish()
    }

    pub fn is_recording(&self) -> bool {
        self.session
            .lock()
            .map(|session| session.is_some())
            .unwrap_or(false)
    }
}

impl Drop for RecorderState {
    fn drop(&mut self) {
        let session = self
            .session
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut active) = session.take() {
            let _ = active.finish();
        }
    }
}

pub fn list_input_devices() -> Result<Vec<AudioInputDevice>, RecordingError> {
    Ok(enumerate_input_devices()?
        .into_iter()
        .map(|device| device.info)
        .collect())
}

enum RecorderControl {
    Stop,
}

enum ReadyMessage {
    Started(RecordingStartInfo),
    Failed,
}

fn run_recording_session(
    output_path: PathBuf,
    requested_device: Option<String>,
    ready: SyncSender<ReadyMessage>,
    control: Receiver<RecorderControl>,
) -> Result<RecordingSummary, RecordingError> {
    let (start_info, mut session) =
        match prepare_recording_session(output_path, requested_device.as_deref()) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = ready.send(ReadyMessage::Failed);
                return Err(error);
            }
        };
    if ready.send(ReadyMessage::Started(start_info)).is_err() {
        return session.finish();
    }
    let _ = control.recv();
    session.finish()
}

fn prepare_recording_session(
    output_path: PathBuf,
    requested_device: Option<&str>,
) -> Result<(RecordingStartInfo, NativeSession), RecordingError> {
    let mut devices = enumerate_input_devices()?;
    let selected = match requested_device {
        Some(identifier) => devices
            .drain(..)
            .find(|candidate| candidate.info.id == identifier)
            .ok_or_else(|| RecordingError::DeviceNotFound(identifier.to_owned()))?,
        None => devices
            .drain(..)
            .find(|candidate| candidate.info.is_default)
            .ok_or(RecordingError::NoInputDevice)?,
    };

    let sample_rate = selected.config.sample_rate().0;
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
        return Err(RecordingError::UnsupportedSampleRate(sample_rate));
    }
    let source_channels = selected.config.channels() as usize;
    let stream_config: StreamConfig = selected.config.clone().into();
    let status = Arc::new(CaptureStatus::default());
    let (writer_tx, writer_rx) = mpsc::sync_channel(CAPTURE_BUFFER_COUNT);
    let (pool_tx, pool_rx) = mpsc::sync_channel(CAPTURE_BUFFER_COUNT);
    for _ in 0..CAPTURE_BUFFER_COUNT {
        pool_tx
            .send(Vec::with_capacity(CAPTURE_CHUNK_FRAMES))
            .map_err(|error| RecordingError::StartWriter(error.to_string()))?;
    }

    let capture = CaptureQueue {
        writer_tx: writer_tx.clone(),
        pool_tx: pool_tx.clone(),
        pool_rx,
        status: Arc::clone(&status),
        source_channels,
        captured_frames: 0,
        max_frames: u64::from(sample_rate) * MAX_RECORDING_SECONDS,
        stranded_buffer: None,
    };
    let stream = build_capture_stream(
        &selected.device,
        &stream_config,
        selected.config.sample_format(),
        capture,
        Arc::clone(&status),
    )?;

    let wav_writer = match create_wav_writer(&output_path, sample_rate) {
        Ok(writer) => writer,
        Err(error) => {
            drop(stream);
            return Err(error);
        }
    };
    let writer_handle = match thread::Builder::new()
        .name("dubbing-wav-writer".to_owned())
        .spawn(move || run_writer(wav_writer, writer_rx, pool_tx))
    {
        Ok(handle) => handle,
        Err(error) => {
            drop(stream);
            let _ = fs::remove_file(&output_path);
            return Err(RecordingError::StartWriter(error.to_string()));
        }
    };

    if let Err(error) = stream.play() {
        drop(stream);
        let _ = writer_tx.send(WriterMessage::Finish);
        let _ = writer_handle.join();
        let _ = fs::remove_file(&output_path);
        return Err(RecordingError::StartStream(error.to_string()));
    }

    let start_info = RecordingStartInfo {
        path: output_path.clone(),
        device: selected.info,
        sample_rate,
    };
    Ok((
        start_info,
        NativeSession {
            path: output_path,
            sample_rate,
            stream: Some(stream),
            writer_tx: Some(writer_tx),
            writer_handle: Some(writer_handle),
            status,
            finished: false,
        },
    ))
}

struct EnumeratedInputDevice {
    device: Device,
    config: SupportedStreamConfig,
    info: AudioInputDevice,
}

fn enumerate_input_devices() -> Result<Vec<EnumeratedInputDevice>, RecordingError> {
    let host = cpal::default_host();
    let host_name = format!("{:?}", host.id());
    let default_signature = host.default_input_device().and_then(|device| {
        let name = device.name().ok()?;
        let config = device.default_input_config().ok()?;
        Some(device_signature(&name, &config))
    });
    let devices = host
        .input_devices()
        .map_err(|error| RecordingError::DeviceEnumeration(error.to_string()))?;
    let mut candidates = Vec::new();
    for (enumeration_index, device) in devices.enumerate() {
        let name = device
            .name()
            .map_err(|error| RecordingError::DeviceEnumeration(error.to_string()))?;
        let config =
            device
                .default_input_config()
                .map_err(|error| RecordingError::DeviceConfiguration {
                    device: name.clone(),
                    message: error.to_string(),
                })?;
        let signature = device_signature(&name, &config);
        candidates.push((signature, enumeration_index, name, device, config));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut default_assigned = false;
    let mut result = Vec::with_capacity(candidates.len());
    for (signature, _, name, device, config) in candidates {
        let occurrence = occurrences.entry(name.clone()).or_default();
        let identifier = stable_device_identifier(&host_name, &name, *occurrence);
        *occurrence += 1;
        let is_default = !default_assigned && default_signature.as_ref() == Some(&signature);
        default_assigned |= is_default;
        result.push(EnumeratedInputDevice {
            device,
            info: AudioInputDevice {
                id: identifier,
                name,
                is_default,
            },
            config,
        });
    }
    Ok(result)
}

fn device_signature(name: &str, config: &SupportedStreamConfig) -> String {
    format!(
        "{}\0{}\0{}\0{:?}",
        name,
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    )
}

fn stable_device_identifier(host: &str, name: &str, occurrence: usize) -> String {
    let mut digest = Sha256::new();
    digest.update(host.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(occurrence.to_le_bytes());
    let hash = digest.finalize();
    format!("cpal-{}", hex_prefix(&hash, 16))
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    let mut output = String::with_capacity(length * 2);
    for byte in bytes.iter().take(length) {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[derive(Default)]
struct CaptureStatus {
    queue_overflowed: AtomicBool,
    limit_exceeded: AtomicBool,
    stream_failed: AtomicBool,
    stream_error: Mutex<Option<String>>,
}

impl CaptureStatus {
    fn should_discard_input(&self) -> bool {
        self.queue_overflowed.load(Ordering::Acquire)
            || self.limit_exceeded.load(Ordering::Acquire)
            || self.stream_failed.load(Ordering::Acquire)
    }

    fn overflowed(&self) -> bool {
        self.should_discard_input()
    }
}

struct CaptureQueue {
    writer_tx: SyncSender<WriterMessage>,
    pool_tx: SyncSender<Vec<f32>>,
    pool_rx: Receiver<Vec<f32>>,
    status: Arc<CaptureStatus>,
    source_channels: usize,
    captured_frames: u64,
    max_frames: u64,
    stranded_buffer: Option<Vec<f32>>,
}

impl CaptureQueue {
    fn process<T: Copy>(&mut self, input: &[T], convert: fn(T) -> f32) {
        if self.status.should_discard_input() || input.is_empty() {
            return;
        }
        if self.source_channels == 0 || !input.len().is_multiple_of(self.source_channels) {
            self.status.queue_overflowed.store(true, Ordering::Release);
            return;
        }

        let mut buffer = match self.pool_rx.try_recv() {
            Ok(buffer) => buffer,
            Err(_) => {
                self.status.queue_overflowed.store(true, Ordering::Release);
                return;
            }
        };
        for frame in input.chunks_exact(self.source_channels) {
            if self.captured_frames >= self.max_frames {
                self.status.limit_exceeded.store(true, Ordering::Release);
                break;
            }
            buffer.push(downmix_frame(frame, convert));
            self.captured_frames += 1;
            if buffer.len() == CAPTURE_CHUNK_FRAMES {
                if !self.submit(buffer) {
                    return;
                }
                buffer = match self.pool_rx.try_recv() {
                    Ok(buffer) => buffer,
                    Err(_) => {
                        self.status.queue_overflowed.store(true, Ordering::Release);
                        return;
                    }
                };
            }
        }
        if buffer.is_empty() {
            self.recycle(buffer);
        } else {
            let _ = self.submit(buffer);
        }
    }

    fn submit(&mut self, buffer: Vec<f32>) -> bool {
        match self.writer_tx.try_send(WriterMessage::Samples(buffer)) {
            Ok(()) => true,
            Err(TrySendError::Full(WriterMessage::Samples(buffer)))
            | Err(TrySendError::Disconnected(WriterMessage::Samples(buffer))) => {
                // Retaining this allocation avoids invoking the allocator from the real-time thread.
                self.stranded_buffer = Some(buffer);
                self.status.queue_overflowed.store(true, Ordering::Release);
                false
            }
            Err(TrySendError::Full(WriterMessage::Finish))
            | Err(TrySendError::Disconnected(WriterMessage::Finish)) => unreachable!(),
        }
    }

    fn recycle(&mut self, buffer: Vec<f32>) {
        match self.pool_tx.try_send(buffer) {
            Ok(()) => {}
            Err(TrySendError::Full(buffer)) | Err(TrySendError::Disconnected(buffer)) => {
                self.stranded_buffer = Some(buffer);
                self.status.queue_overflowed.store(true, Ordering::Release);
            }
        }
    }
}

fn downmix_frame<T: Copy>(frame: &[T], convert: fn(T) -> f32) -> f32 {
    let sum = frame
        .iter()
        .copied()
        .map(convert)
        .filter(|sample| sample.is_finite())
        .sum::<f32>();
    sum / frame.len() as f32
}

fn sample_f32(sample: f32) -> f32 {
    sample
}

fn sample_i16(sample: i16) -> f32 {
    f32::from(sample) / 32_768.0
}

fn sample_u16(sample: u16) -> f32 {
    (f32::from(sample) - 32_768.0) / 32_768.0
}

fn build_capture_stream(
    device: &Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    capture: CaptureQueue,
    status: Arc<CaptureStatus>,
) -> Result<Stream, RecordingError> {
    match sample_format {
        SampleFormat::F32 => build_typed_stream(device, config, capture, status, sample_f32),
        SampleFormat::I16 => build_typed_stream(device, config, capture, status, sample_i16),
        SampleFormat::U16 => build_typed_stream(device, config, capture, status, sample_u16),
        format => Err(RecordingError::UnsupportedSampleFormat(format!(
            "{format:?}"
        ))),
    }
}

fn build_typed_stream<T: SizedSample + Copy + 'static>(
    device: &Device,
    config: &StreamConfig,
    mut capture: CaptureQueue,
    status: Arc<CaptureStatus>,
    convert: fn(T) -> f32,
) -> Result<Stream, RecordingError> {
    let error_status = Arc::clone(&status);
    device
        .build_input_stream(
            config,
            move |input: &[T], _| capture.process(input, convert),
            move |error| {
                error_status.stream_failed.store(true, Ordering::Release);
                if let Ok(mut detail) = error_status.stream_error.try_lock() {
                    *detail = Some(error.to_string());
                }
            },
            None,
        )
        .map_err(|error| RecordingError::BuildStream(error.to_string()))
}

enum WriterMessage {
    Samples(Vec<f32>),
    Finish,
}

#[derive(Default)]
struct WriterStats {
    frames: u64,
    peak: f32,
    clipped_samples: u64,
}

impl WriterStats {
    fn observe(&mut self, sample: f32) {
        let amplitude = sample.abs();
        self.peak = self.peak.max(amplitude);
        if amplitude >= 1.0 {
            self.clipped_samples += 1;
        }
        self.frames += 1;
    }

    fn peak_dbfs(&self) -> f32 {
        if self.peak > 0.0 {
            20.0 * self.peak.log10()
        } else {
            SILENCE_DBFS
        }
    }
}

type PcmWriter = WavWriter<BufWriter<File>>;

fn create_wav_writer(path: &Path, sample_rate: u32) -> Result<PcmWriter, RecordingError> {
    // Refusing replacement keeps a failed take from destroying an earlier recording.
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| RecordingError::CreateOutput {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    WavWriter::new(
        BufWriter::new(file),
        WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .map_err(|error| RecordingError::CreateOutput {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn run_writer(
    mut writer: PcmWriter,
    receiver: Receiver<WriterMessage>,
    pool: SyncSender<Vec<f32>>,
) -> Result<WriterStats, String> {
    let mut stats = WriterStats::default();
    let mut write_error = None;
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Samples(mut samples) => {
                if write_error.is_none() {
                    for sample in samples.iter().copied() {
                        match writer.write_sample(pcm_i16(sample)) {
                            Ok(()) => stats.observe(sample),
                            Err(error) => {
                                write_error = Some(error.to_string());
                                break;
                            }
                        }
                    }
                }
                samples.clear();
                let _ = pool.try_send(samples);
            }
            WriterMessage::Finish => break,
        }
    }
    let finalize_result = writer.finalize().map_err(|error| error.to_string());
    if let Some(error) = write_error {
        Err(error)
    } else {
        finalize_result.map(|()| stats)
    }
}

fn pcm_i16(sample: f32) -> i16 {
    let sample = if sample.is_finite() { sample } else { 0.0 };
    if sample <= -1.0 {
        i16::MIN
    } else if sample >= 1.0 {
        i16::MAX
    } else {
        (sample * f32::from(i16::MAX)).round() as i16
    }
}

struct ControllerSession {
    control_tx: Option<SyncSender<RecorderControl>>,
    thread_handle: Option<JoinHandle<Result<RecordingSummary, RecordingError>>>,
    finished: bool,
}

impl ControllerSession {
    fn finish(&mut self) -> Result<RecordingSummary, RecordingError> {
        if self.finished {
            return Err(RecordingError::NotRecording);
        }
        self.finished = true;
        if let Some(sender) = self.control_tx.take() {
            let _ = sender.send(RecorderControl::Stop);
        }
        match self
            .thread_handle
            .take()
            .expect("recorder thread handle is present")
            .join()
        {
            Ok(result) => result,
            Err(_) => Err(RecordingError::RecorderPanicked),
        }
    }
}

impl Drop for ControllerSession {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}

struct NativeSession {
    path: PathBuf,
    sample_rate: u32,
    stream: Option<Stream>,
    writer_tx: Option<SyncSender<WriterMessage>>,
    writer_handle: Option<JoinHandle<Result<WriterStats, String>>>,
    status: Arc<CaptureStatus>,
    finished: bool,
}

impl NativeSession {
    fn finish(&mut self) -> Result<RecordingSummary, RecordingError> {
        if self.finished {
            return Err(RecordingError::NotRecording);
        }
        self.finished = true;
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        if let Some(sender) = self.writer_tx.take() {
            let _ = sender.send(WriterMessage::Finish);
        }
        let stats = match self
            .writer_handle
            .take()
            .expect("writer handle is present")
            .join()
        {
            Ok(Ok(stats)) => stats,
            Ok(Err(error)) => {
                let _ = fs::remove_file(&self.path);
                return Err(RecordingError::WriterFailed(error));
            }
            Err(_) => {
                let _ = fs::remove_file(&self.path);
                return Err(RecordingError::WriterPanicked);
            }
        };

        let overflowed = self.status.overflowed();
        let summary = RecordingSummary {
            path: self.path.clone(),
            duration_ms: duration_ms(stats.frames, self.sample_rate),
            sample_rate: self.sample_rate,
            frames: stats.frames,
            peak_dbfs: stats.peak_dbfs(),
            clipped_samples: stats.clipped_samples,
            overflowed,
        };
        let invalid_reason = if self.status.limit_exceeded.load(Ordering::Acquire) {
            Some(format!(
                "Recording exceeded the {MAX_RECORDING_SECONDS}-second limit and was discarded."
            ))
        } else if self.status.queue_overflowed.load(Ordering::Acquire) {
            Some(
                "Audio input outran WAV writing; the incomplete recording was discarded."
                    .to_owned(),
            )
        } else if self.status.stream_failed.load(Ordering::Acquire) {
            let detail = self
                .status
                .stream_error
                .lock()
                .ok()
                .and_then(|error| error.clone())
                .unwrap_or_else(|| "the input device stopped unexpectedly".to_owned());
            Some(format!(
                "Audio input failed ({detail}); the incomplete recording was discarded."
            ))
        } else if stats.frames == 0 {
            Some("No audio was captured; the empty recording was discarded.".to_owned())
        } else {
            None
        };

        if let Some(reason) = invalid_reason {
            let _ = fs::remove_file(&self.path);
            Err(RecordingError::InvalidRecording {
                reason,
                summary: Box::new(summary),
            })
        } else {
            Ok(summary)
        }
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}

fn duration_ms(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    let numerator = u128::from(frames) * 1_000 + u128::from(sample_rate / 2);
    (numerator / u128::from(sample_rate)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_samples_are_normalized_before_downmixing() {
        assert_eq!(sample_i16(i16::MIN), -1.0);
        assert!((sample_i16(i16::MAX) - 0.999_969_5).abs() < 0.000_001);
        assert_eq!(sample_u16(u16::MIN), -1.0);
        assert!((sample_u16(u16::MAX) - 0.999_969_5).abs() < 0.000_001);
        assert!(
            (downmix_frame(&[i16::MAX, i16::MIN], sample_i16) + 0.000_015_25).abs() < 0.000_001
        );
    }

    #[test]
    fn metering_reports_full_scale_clipping_and_silence() {
        let mut stats = WriterStats::default();
        assert_eq!(stats.peak_dbfs(), SILENCE_DBFS);
        for sample in [-1.0, 0.5, 1.25] {
            stats.observe(sample);
        }
        assert_eq!(stats.frames, 3);
        assert_eq!(stats.clipped_samples, 2);
        assert!((stats.peak_dbfs() - 1.938_200_2).abs() < 0.000_01);
    }

    #[test]
    fn writer_produces_a_mono_pcm_master_at_the_hardware_rate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("take.wav");
        let writer = create_wav_writer(&path, 48_000).unwrap();
        let (message_tx, message_rx) = mpsc::sync_channel(2);
        let (pool_tx, _pool_rx) = mpsc::sync_channel(2);
        let handle = thread::spawn(move || run_writer(writer, message_rx, pool_tx));
        message_tx
            .send(WriterMessage::Samples(vec![-1.0, 0.0, 0.5, 1.2]))
            .unwrap();
        message_tx.send(WriterMessage::Finish).unwrap();
        let stats = handle.join().unwrap().unwrap();

        assert_eq!(stats.frames, 4);
        assert_eq!(stats.clipped_samples, 2);
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![i16::MIN, 0, 16_384, i16::MAX]
        );
    }

    #[test]
    fn duration_uses_frame_count_instead_of_callback_timing() {
        assert_eq!(duration_ms(48_000, 48_000), 1_000);
        assert_eq!(duration_ms(1, 48_000), 0);
        assert_eq!(duration_ms(24, 48_000), 1);
    }

    #[test]
    fn recorder_state_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecorderState>();
    }
}
