export type TargetStatus =
  | "missing"
  | "recorded"
  | "needs_review"
  | "approved"
  | "skipped";

export interface TakeMetadata {
  id: string;
  file: string;
  createdAtMs: number;
  durationMs: number;
  sampleRate: number;
  frames: number;
  peakDbfs: number;
  clippedSamples: number;
  sha256: string;
}

export interface TargetProgress {
  targetId: string;
  status: TargetStatus;
  activeTake?: string;
  takes: TakeMetadata[];
  notes: string;
  gainDb: number;
  trimStartMs: number;
  trimEndMs: number;
  approvalSha256?: string;
}

export interface DubbingProjectManifest {
  version: number;
  name: string;
  profileSha256: string;
  profileCatalogSha256: string;
  sourceRomSha256: string;
  sourceRomFilename: string;
  gameCode: string;
  createdAtMs: number;
  updatedAtMs: number;
  targets: Record<string, TargetProgress>;
}

export interface DubbingContextLine {
  ordinal: number;
  speaker: string;
  speakerRaw: string;
  text: string;
  isCurrent: boolean;
  isRecordable: boolean;
}

export interface DubbingCue {
  index: number;
  targetId: string;
  symbol: string;
  scriptPath: string;
  functionName: string;
  ordinal: number;
  speaker: string;
  speakerRaw: string;
  text: string;
  context: DubbingContextLine[];
  progress: TargetProgress;
}

export interface DubbingProjectSummary {
  total: number;
  recorded: number;
  needsReview: number;
  approved: number;
  skipped: number;
}

export interface DubbingProjectSnapshot {
  projectDir: string;
  manifestPath: string;
  romPath: string;
  manifest: DubbingProjectManifest;
  summary: DubbingProjectSummary;
  cues: DubbingCue[];
}

export interface AudioInputDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

export interface RecordingSummary {
  path: string;
  durationMs: number;
  sampleRate: number;
  frames: number;
  peakDbfs: number;
  clippedSamples: number;
  overflowed: boolean;
}

export interface RecordingStartInfo {
  path: string;
  device: AudioInputDevice;
  sampleRate: number;
}

export interface DubbingTakeResult {
  take: TakeMetadata;
  progress: TargetProgress;
  summary: RecordingSummary;
}

export interface DubbingBuildProgress {
  stage: string;
  completed: number;
  total: number;
  message: string;
}

export interface DubbingBuildReport {
  scope: "preview" | "production";
  packPath: string;
  packSha256: string;
  catalogSha256: string;
  buildInputSha256: string;
  sourceRomSha256: string;
  profileSha256: string;
  voices: number;
  recorded: number;
  approved: number;
  silentPreviewEntries: number;
  payloadBytes: number;
  durationMs: number;
  clippedSamples: number;
  firstInternalId: string;
  lastInternalId: string;
}

export interface TestRomBuildResult {
  voicePack: DubbingBuildReport;
  rom: {
    outputPath: string;
    bytes: number;
    sha256: string;
    language: "french";
    voices: number;
    scripts: number;
    resetExact: boolean;
  };
}

export type DubbingStatusFilter = "all" | TargetStatus;
