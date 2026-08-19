import {
  memo,
  useCallback,
  useDeferredValue,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ArrowRight,
  ArrowsLeftRight,
  CheckCircle,
  Circle,
  Clock,
  FileAudio,
  FloppyDisk,
  FolderOpen,
  Headphones,
  HardDrives,
  MagnifyingGlass,
  Microphone,
  Pause,
  Play,
  SpeakerHigh,
  SpinnerGap,
  Stop,
  Warning,
  Waveform,
  X,
} from "@phosphor-icons/react";
import type {
  AudioInputDevice,
  DubbingCue,
  DubbingBuildProgress,
  DubbingLevelAnalysis,
  DubbingLevelMatchBatchResult,
  DubbingProjectSnapshot,
  DubbingProjectSummary,
  DubbingReferenceLanguage,
  DubbingStatusFilter,
  DubbingTakeResult,
  RecordingStartInfo,
  TakeMetadata,
  TargetProgress,
  TargetStatus,
  TestRomBuildResult,
} from "./types";
import "./DubbingStudio.css";

type UiLanguage = "fr" | "en";
type SetupMode = "create" | "open";
type RecordingState = "idle" | "starting" | "recording" | "stopping";

export interface DubbingStudioViewProps {
  language: UiLanguage;
  onExclusiveOperationChange?: (active: boolean) => void;
  onNavigationLockChange?: (locked: boolean) => void;
}

const COPY = {
  fr: {
    studio: "STUDIO DE DOUBLAGE",
    setupEyebrow: "PROJET FRANÇAIS · 999 NDS",
    setupTitle: "Créer les voix, réplique par réplique.",
    setupBody:
      "Le script et son contexte sont lus depuis votre ROM. Les enregistrements restent dans un dossier de projet local.",
    createProject: "Nouveau projet",
    openProject: "Ouvrir un projet",
    projectName: "Nom du projet",
    projectNamePlaceholder: "Doublage français 999",
    sourceRom: "ROM de référence",
    projectFolder: "Dossier du projet",
    chooseRom: "Choisir la ROM",
    chooseFolder: "Choisir le dossier",
    change: "Modifier",
    create: "Créer le projet",
    open: "Ouvrir le projet",
    loading: "Chargement…",
    desktopOnly: "Le studio de doublage doit être ouvert dans l’application de bureau.",
    romDialog: "Choisir la ROM 999 utilisée par le projet",
    folderCreateDialog: "Choisir le dossier du nouveau projet",
    folderOpenDialog: "Choisir le dossier du projet de doublage",
    romFilter: "ROM Nintendo DS",
    setupError: "Impossible de charger le projet.",
    setupReady: "Projet chargé.",
    backToProjects: "Changer de projet",
    search: "Rechercher une réplique, un personnage ou un identifiant",
    searchShort: "Rechercher…",
    filter: "Filtrer les répliques",
    all: "Toutes",
    missing: "À enregistrer",
    recorded: "Enregistrées",
    needs_review: "À vérifier",
    approved: "Validées",
    skipped: "Ignorées",
    inputDevice: "Microphone",
    noInput: "Aucun microphone",
    refreshDevices: "Actualiser les microphones",
    devicesError: "Les microphones disponibles n’ont pas pu être lus.",
    lines: "répliques",
    noResults: "Aucune réplique ne correspond à ce filtre.",
    context: "Contexte du script",
    narration: "Contexte",
    currentLine: "Réplique à doubler",
    previous: "Réplique précédente",
    next: "Réplique suivante",
    take: "prise",
    takes: "prises",
    noTakes: "Aucune prise enregistrée.",
    activeTake: "Prise active",
    selectTake: "Utiliser cette prise",
    playTake: "Écouter la prise",
    pauseTake: "Mettre en pause",
    readError: "La prise n’a pas pu être lue.",
    selectError: "La prise active n’a pas pu être modifiée.",
    record: "Enregistrer",
    stop: "Arrêter",
    cancel: "Annuler la prise",
    preparing: "Préparation du microphone…",
    stopping: "Enregistrement de la prise…",
    recording: "Enregistrement en cours",
    recordHint: "R pour enregistrer · Espace pour écouter",
    recordError: "L’enregistrement n’a pas pu démarrer.",
    stopError: "La prise n’a pas pu être enregistrée.",
    recordedAnnouncement: "Prise enregistrée",
    cancelledAnnouncement: "Prise annulée.",
    peak: "crête",
    clipping: "écrêtage détecté",
    overflow: "perte d’échantillons détectée",
    notes: "Notes de direction",
    notesPlaceholder: "Intention, prononciation, raccord, prise à refaire…",
    status: "État",
    save: "Enregistrer l’état, les notes et le gain",
    saved: "Informations enregistrées.",
    saveError: "Les informations n’ont pas pu être enregistrées.",
    unsaved: "Modifications non enregistrées",
    audioBalance: "Équilibrage du volume",
    audioBalanceBody:
      "Le RMS actif mesure les blocs audio les plus forts et écarte les silences plus faibles. Comparez votre prise à une voix originale, puis écoutez le son tel qu’il sera encodé dans la ROM.",
    levelNeedsTake: "Enregistrez et sélectionnez une prise pour équilibrer son volume.",
    referenceVoice: "Voix originale de référence",
    japaneseReference: "Japonais",
    englishReference: "Anglais",
    analyzeLevels: "Comparer les niveaux",
    analyzingLevels: "Mesure des niveaux…",
    analyzeError: "Les niveaux audio n’ont pas pu être comparés.",
    takeActiveLevel: "Votre voix",
    referenceActiveLevel: "Voix originale",
    processedActiveLevel: "Niveau final DS",
    currentGain: "Gain enregistré",
    suggestedGain: "Gain conseillé",
    finalPeak: "Crête finale DS",
    notMeasured: "Non mesuré",
    activeRms: "RMS des blocs actifs",
    silentTake: "Aucun bloc audio actif mesurable n’a été trouvé dans cette prise.",
    silentReference: "Aucun bloc audio actif mesurable n’a été trouvé dans la référence.",
    gain: "Gain du doublage",
    gainHelp: "Réglage appliqué à la prise active lors de la création de la ROM.",
    applySuggestedGain: "Aligner sur la référence",
    applyingGain: "Enregistrement du gain…",
    gainSaved: "Gain enregistré.",
    gainSaveError: "Le gain n’a pas pu être enregistré.",
    saveGainFirst:
      "Enregistrez d’abord le gain avec le bouton ci-dessous pour comparer les niveaux ou écouter le rendu DS.",
    approvalReset:
      "Si le gain change, une réplique validée repasse automatiquement à « À vérifier ».",
    approvalResetDone: "Le gain a changé : la réplique doit être vérifiée à nouveau.",
    headroomLimited:
      "Le gain conseillé est réduit pour garder 1 dB de marge avant l’écrêtage.",
    maximumGainLimited:
      "Le gain conseillé atteint la limite de +24 dB. Réenregistrez avec une entrée propre et plus forte au lieu d’amplifier davantage.",
    minimumGainLimited: "Le gain conseillé atteint la limite de −60 dB.",
    peakRisk:
      "Le signal est écrêté avant l’encodage DS ou sa crête finale laisse moins de 1 dB de marge.",
    compareAudio: "Comparer le rendu",
    rawTake: "Prise brute",
    finalDs: "Rendu final DS",
    originalReference: "Voix originale",
    playRawTake: "Écouter la prise brute",
    playFinalDs: "Écouter le rendu final DS",
    playReference: "Écouter la voix originale",
    pauseAudio: "Mettre en pause",
    previewError: "La prévisualisation audio n’a pas pu être lue.",
    matchAll: "Équilibrer toutes les prises",
    matchingAll: "Équilibrage des prises…",
    matchAllConfirmTitle: "Équilibrer toutes les prises actives ?",
    matchAllConfirm:
      "Le gain conseillé sera calculé et appliqué à chaque prise active avec la voix de référence sélectionnée.",
    matchAllApprovalWarning:
      "Les répliques déjà validées dont le gain change repasseront à « À vérifier ».",
    confirmMatchAll: "Équilibrer les prises",
    cancelMatchAll: "Annuler",
    matchAllError: "Les prises n’ont pas pu être équilibrées.",
    matched: "prises équilibrées",
    limitedForHeadroom: "limitées pour conserver la marge",
    limitedByGain: "limitées par la plage de gain",
    skippedDuringMatch: "ignorées faute de signal actif mesurable",
    buildTest: "Créer une ROM de test",
    buildNeedsTake: "Enregistrez au moins une prise avant de créer une ROM de test.",
    buildDialog: "Enregistrer la ROM de prévisualisation française",
    buildingTest: "Création de la ROM de test…",
    buildDone: "ROM de test créée",
    buildError: "La ROM de test n’a pas pu être créée.",
    revealBuild: "Afficher",
    summaryRecorded: "avec une prise",
    summaryApproved: "validées",
    summaryReview: "à vérifier",
    summarySkipped: "ignorées",
    source: "Source",
    shortcutHelp: "R enregistrer · Espace écouter · Flèches naviguer",
    close: "Fermer",
    statusMissing: "À enregistrer",
    statusRecorded: "Enregistrée",
    statusNeedsReview: "À vérifier",
    statusApproved: "Validée",
    statusSkipped: "Ignorée",
  },
  en: {
    studio: "DUBBING STUDIO",
    setupEyebrow: "FRENCH PROJECT · 999 NDS",
    setupTitle: "Create the voices, one line at a time.",
    setupBody:
      "The script and its context are read from your ROM. Recordings stay inside a local project folder.",
    createProject: "New project",
    openProject: "Open project",
    projectName: "Project name",
    projectNamePlaceholder: "999 French dub",
    sourceRom: "Reference ROM",
    projectFolder: "Project folder",
    chooseRom: "Choose ROM",
    chooseFolder: "Choose folder",
    change: "Change",
    create: "Create project",
    open: "Open project",
    loading: "Loading…",
    desktopOnly: "The dubbing studio must be opened in the desktop application.",
    romDialog: "Choose the 999 ROM used by this project",
    folderCreateDialog: "Choose a folder for the new project",
    folderOpenDialog: "Choose the dubbing project folder",
    romFilter: "Nintendo DS ROM",
    setupError: "The project could not be loaded.",
    setupReady: "Project loaded.",
    backToProjects: "Change project",
    search: "Search by line, character, or identifier",
    searchShort: "Search…",
    filter: "Filter lines",
    all: "All",
    missing: "To record",
    recorded: "Recorded",
    needs_review: "Needs review",
    approved: "Approved",
    skipped: "Skipped",
    inputDevice: "Microphone",
    noInput: "No microphone",
    refreshDevices: "Refresh microphones",
    devicesError: "Available microphones could not be read.",
    lines: "lines",
    noResults: "No line matches this filter.",
    context: "Script context",
    narration: "Context",
    currentLine: "Line to dub",
    previous: "Previous line",
    next: "Next line",
    take: "take",
    takes: "takes",
    noTakes: "No takes recorded.",
    activeTake: "Active take",
    selectTake: "Use this take",
    playTake: "Play take",
    pauseTake: "Pause take",
    readError: "The take could not be played.",
    selectError: "The active take could not be changed.",
    record: "Record",
    stop: "Stop",
    cancel: "Discard take",
    preparing: "Preparing microphone…",
    stopping: "Saving take…",
    recording: "Recording",
    recordHint: "R to record · Space to play",
    recordError: "Recording could not be started.",
    stopError: "The take could not be saved.",
    recordedAnnouncement: "Take recorded",
    cancelledAnnouncement: "Take discarded.",
    peak: "peak",
    clipping: "clipping detected",
    overflow: "sample loss detected",
    notes: "Direction notes",
    notesPlaceholder: "Intent, pronunciation, continuity, retake notes…",
    status: "Status",
    save: "Save status, notes, and gain",
    saved: "Line details saved.",
    saveError: "Line details could not be saved.",
    unsaved: "Unsaved changes",
    audioBalance: "Volume matching",
    audioBalanceBody:
      "Active RMS measures the louder audio blocks and excludes quieter silence. Compare your take with an original voice, then hear the audio as it will be encoded in the ROM.",
    levelNeedsTake: "Record and select a take to match its volume.",
    referenceVoice: "Original voice reference",
    japaneseReference: "Japanese",
    englishReference: "English",
    analyzeLevels: "Compare levels",
    analyzingLevels: "Measuring levels…",
    analyzeError: "The audio levels could not be compared.",
    takeActiveLevel: "Your voice",
    referenceActiveLevel: "Original voice",
    processedActiveLevel: "Final DS level",
    currentGain: "Saved gain",
    suggestedGain: "Suggested gain",
    finalPeak: "Final DS peak",
    notMeasured: "Not measured",
    activeRms: "active-block RMS",
    silentTake: "No measurable active audio block was found in this take.",
    silentReference: "No measurable active audio block was found in the reference.",
    gain: "Dubbing gain",
    gainHelp: "Applied to the active take when the ROM is built.",
    applySuggestedGain: "Match reference",
    applyingGain: "Saving gain…",
    gainSaved: "Gain saved.",
    gainSaveError: "The gain could not be saved.",
    saveGainFirst:
      "Save the gain with the button below before comparing levels or playing the DS-encoded preview.",
    approvalReset:
      "If the gain changes, an approved line automatically returns to Needs review.",
    approvalResetDone: "The gain changed: this line must be reviewed again.",
    headroomLimited:
      "The suggested gain is reduced to keep 1 dB of headroom before clipping.",
    maximumGainLimited:
      "The suggested gain reached the +24 dB limit. Record again with a stronger clean input instead of boosting further.",
    minimumGainLimited: "The suggested gain reached the −60 dB limit.",
    peakRisk:
      "The signal clips before DS encoding, or its final peak leaves less than 1 dB of headroom.",
    compareAudio: "Compare the result",
    rawTake: "Raw take",
    finalDs: "Final DS",
    originalReference: "Original reference",
    playRawTake: "Play raw take",
    playFinalDs: "Play final DS audio",
    playReference: "Play original reference",
    pauseAudio: "Pause audio",
    previewError: "The audio preview could not be played.",
    matchAll: "Match all recorded",
    matchingAll: "Matching recorded lines…",
    matchAllConfirmTitle: "Match all active takes?",
    matchAllConfirm:
      "The suggested gain will be calculated and applied to every active take using the selected reference voice.",
    matchAllApprovalWarning:
      "Approved lines whose gain changes will return to Needs review.",
    confirmMatchAll: "Match all takes",
    cancelMatchAll: "Cancel",
    matchAllError: "The recorded lines could not be matched.",
    matched: "recordings matched",
    limitedForHeadroom: "limited to preserve headroom",
    limitedByGain: "limited by the gain range",
    skippedDuringMatch: "skipped because no measurable active audio was available",
    buildTest: "Build test ROM",
    buildNeedsTake: "Record at least one take before building a test ROM.",
    buildDialog: "Save the French preview ROM",
    buildingTest: "Building test ROM…",
    buildDone: "Test ROM created",
    buildError: "The test ROM could not be built.",
    revealBuild: "Show",
    summaryRecorded: "with a take",
    summaryApproved: "approved",
    summaryReview: "needs review",
    summarySkipped: "skipped",
    source: "Source",
    shortcutHelp: "R record · Space play · Arrow keys navigate",
    close: "Close",
    statusMissing: "To record",
    statusRecorded: "Recorded",
    statusNeedsReview: "Needs review",
    statusApproved: "Approved",
    statusSkipped: "Skipped",
  },
} as const;

const STATUS_ORDER: TargetStatus[] = [
  "missing",
  "recorded",
  "needs_review",
  "approved",
  "skipped",
];

function rawError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function fileName(path: string): string {
  const segments = path.split(/[\\/]/);
  return segments[segments.length - 1] || path;
}

function previewRomPath(source: string): string {
  const separator = Math.max(source.lastIndexOf("/"), source.lastIndexOf("\\"));
  const directory = separator >= 0 ? source.slice(0, separator + 1) : "";
  const sourceName = separator >= 0 ? source.slice(separator + 1) : source;
  const stem = sourceName.replace(/\.nds$/i, "") || "999";
  return `${directory}${stem} [French Voice Preview].nds`;
}

function formatDuration(milliseconds: number, precise = false): string {
  const safe = Math.max(0, milliseconds);
  const minutes = Math.floor(safe / 60_000);
  const seconds = Math.floor((safe % 60_000) / 1_000);
  if (precise) {
    const tenths = Math.floor((safe % 1_000) / 100);
    return `${minutes.toString().padStart(2, "0")}:${seconds
      .toString()
      .padStart(2, "0")}.${tenths}`;
  }
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

const MIN_GAIN_DB = -60;
const MAX_GAIN_DB = 24;

function clampGain(value: number): number {
  return Math.min(MAX_GAIN_DB, Math.max(MIN_GAIN_DB, Math.round(value * 10) / 10));
}

function formatDb(value: number | null, signed = false): string {
  if (value === null || !Number.isFinite(value)) return "—";
  const prefix = signed && value > 0 ? "+" : "";
  return `${prefix}${value.toFixed(1)} dB`;
}

function formatDbfs(value: number | null, fractionDigits = 1): string {
  if (value === null || !Number.isFinite(value)) return "—";
  return `${value.toFixed(fractionDigits)} dBFS`;
}

function rawAudioKey(symbol: string, takeId: string): string {
  return `${symbol}:raw:${takeId}`;
}

function normalizeSearch(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase();
}

function summarize(cues: DubbingCue[]): DubbingProjectSummary {
  const result: DubbingProjectSummary = {
    total: cues.length,
    recorded: 0,
    needsReview: 0,
    approved: 0,
    skipped: 0,
  };
  for (const cue of cues) {
    if (cue.progress.activeTake) result.recorded += 1;
    if (cue.progress.status === "needs_review") result.needsReview += 1;
    if (cue.progress.status === "approved") result.approved += 1;
    if (cue.progress.status === "skipped") result.skipped += 1;
  }
  return result;
}

function normalizeProgress(progress: TargetProgress): TargetProgress {
  // Serde omits empty collections and strings so large untouched projects stay compact.
  return {
    ...progress,
    takes: progress.takes ?? [],
    notes: progress.notes ?? "",
    gainDb: progress.gainDb ?? 0,
    trimStartMs: progress.trimStartMs ?? 0,
    trimEndMs: progress.trimEndMs ?? 0,
  };
}

function normalizeSnapshot(snapshot: DubbingProjectSnapshot): DubbingProjectSnapshot {
  const cues = snapshot.cues.map((cue) => ({
    ...cue,
    progress: normalizeProgress(cue.progress),
  }));
  const targets = Object.fromEntries(
    Object.entries(snapshot.manifest.targets).map(([symbol, progress]) => [
      symbol,
      normalizeProgress(progress),
    ]),
  );
  return {
    ...snapshot,
    cues,
    summary: summarize(cues),
    manifest: { ...snapshot.manifest, targets },
  };
}

function cueMatchesFilter(cue: DubbingCue, filter: DubbingStatusFilter): boolean {
  if (filter === "all") return true;
  if (filter === "recorded") return Boolean(cue.progress.activeTake);
  if (filter === "missing") {
    return !cue.progress.activeTake && cue.progress.status !== "skipped";
  }
  return cue.progress.status === filter;
}

function visibleStatus(cue: DubbingCue): TargetStatus {
  if (cue.progress.status !== "missing") return cue.progress.status;
  return cue.progress.activeTake ? "recorded" : "missing";
}

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(
    target.closest("input, textarea, select, button, [contenteditable='true']"),
  );
}

const CUE_ROW_HEIGHT = 61;
const CUE_OVERSCAN = 7;

interface CueBrowserProps {
  cues: DubbingCue[];
  disabled: boolean;
  linesLabel: string;
  noResults: string;
  onSelect: (symbol: string) => Promise<boolean>;
  selectedSymbol: string;
  statusLabels: Record<TargetStatus, string>;
}

const CueBrowser = memo(function CueBrowser({
  cues,
  disabled,
  linesLabel,
  noResults,
  onSelect,
  selectedSymbol,
  statusLabels,
}: CueBrowserProps) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const focusSelectionRef = useRef(false);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(600);
  const selectedPosition = useMemo(
    () => cues.findIndex((cue) => cue.symbol === selectedSymbol),
    [cues, selectedSymbol],
  );
  const startIndex = Math.max(
    0,
    Math.floor(scrollTop / CUE_ROW_HEIGHT) - CUE_OVERSCAN,
  );
  const visibleCount = Math.ceil(viewportHeight / CUE_ROW_HEIGHT) + CUE_OVERSCAN * 2;
  const endIndex = Math.min(cues.length, startIndex + visibleCount);
  const tabStopIndex = selectedPosition >= 0 ? selectedPosition : 0;

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const updateHeight = () => setViewportHeight(Math.max(CUE_ROW_HEIGHT, viewport.clientHeight));
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport || selectedPosition < 0) return;
    const rowTop = selectedPosition * CUE_ROW_HEIGHT;
    const rowBottom = rowTop + CUE_ROW_HEIGHT;
    let nextScrollTop = viewport.scrollTop;
    if (rowTop < viewport.scrollTop) nextScrollTop = rowTop;
    else if (rowBottom > viewport.scrollTop + viewport.clientHeight) {
      nextScrollTop = rowBottom - viewport.clientHeight;
    }
    if (nextScrollTop !== viewport.scrollTop) {
      viewport.scrollTop = nextScrollTop;
      setScrollTop(nextScrollTop);
    }
  }, [selectedPosition]);

  useEffect(() => {
    if (
      !focusSelectionRef.current ||
      selectedPosition < startIndex ||
      selectedPosition >= endIndex
    ) {
      return;
    }
    focusSelectionRef.current = false;
    const frame = window.requestAnimationFrame(() => {
      viewportRef.current
        ?.querySelector<HTMLElement>(`[data-cue-index="${selectedPosition}"]`)
        ?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [endIndex, selectedPosition, startIndex]);

  const moveSelection = useCallback(
    (nextIndex: number) => {
      if (disabled || cues.length === 0) return;
      const bounded = Math.min(cues.length - 1, Math.max(0, nextIndex));
      if (bounded === selectedPosition) return;
      focusSelectionRef.current = true;
      void onSelect(cues[bounded].symbol).then((selected) => {
        if (!selected) focusSelectionRef.current = false;
      });
    },
    [cues, disabled, onSelect, selectedPosition],
  );

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const origin = selectedPosition >= 0 ? selectedPosition : 0;
      let nextIndex: number | null = null;
      if (event.key === "ArrowUp" || event.key === "ArrowLeft") nextIndex = origin - 1;
      else if (event.key === "ArrowDown" || event.key === "ArrowRight") nextIndex = origin + 1;
      else if (event.key === "Home") nextIndex = 0;
      else if (event.key === "End") nextIndex = cues.length - 1;
      if (nextIndex === null) return;
      event.preventDefault();
      event.stopPropagation();
      moveSelection(nextIndex);
    },
    [cues.length, moveSelection, selectedPosition],
  );

  return (
    <aside className="dub-cue-browser" aria-label={linesLabel}>
      <div className="dub-list-heading">
        <strong>{cues.length.toLocaleString()} {linesLabel}</strong>
        {selectedPosition >= 0 && (
          <span>{(selectedPosition + 1).toLocaleString()} / {cues.length.toLocaleString()}</span>
        )}
      </div>
      <div
        ref={viewportRef}
        className="dub-cue-list"
        role="listbox"
        aria-label={linesLabel}
        aria-busy={disabled}
        onKeyDown={handleKeyDown}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {cues.length === 0 ? (
          <div className="dub-empty-list">{noResults}</div>
        ) : (
          <div
            className="dub-cue-virtual-space"
            role="presentation"
            style={{ height: `${cues.length * CUE_ROW_HEIGHT}px` }}
          >
            {cues.slice(startIndex, endIndex).map((cue, visibleIndex) => {
              const index = startIndex + visibleIndex;
              const status = visibleStatus(cue);
              return (
                <button
                  key={cue.symbol}
                  type="button"
                  role="option"
                  aria-posinset={index + 1}
                  aria-setsize={cues.length}
                  aria-selected={cue.symbol === selectedSymbol}
                  className={`dub-cue-row is-${status}`}
                  data-cue-index={index}
                  disabled={disabled}
                  tabIndex={index === tabStopIndex ? 0 : -1}
                  style={{ top: `${index * CUE_ROW_HEIGHT}px` }}
                  onClick={() => void onSelect(cue.symbol)}
                >
                  <span className="dub-status-dot" aria-hidden="true" />
                  <span className="dub-row-copy">
                    <strong>{cue.speaker || "—"}</strong>
                    <span>{cue.text}</span>
                    <span className="dub-sr-only">{statusLabels[status]}</span>
                  </span>
                  <small>{cue.symbol.replace(/^SE_/i, "")}</small>
                </button>
              );
            })}
          </div>
        )}
      </div>
    </aside>
  );
});

function SetupView({
  language,
  onLoaded,
}: {
  language: UiLanguage;
  onLoaded: (snapshot: DubbingProjectSnapshot) => void;
}) {
  const copy = COPY[language];
  const [mode, setMode] = useState<SetupMode>("create");
  const [projectName, setProjectName] = useState("");
  const [romPath, setRomPath] = useState("");
  const [projectDir, setProjectDir] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const createTabRef = useRef<HTMLButtonElement>(null);
  const openTabRef = useRef<HTMLButtonElement>(null);

  const selectMode = useCallback((nextMode: SetupMode, focus = false) => {
    setMode(nextMode);
    setError("");
    if (focus) {
      window.requestAnimationFrame(() => {
        (nextMode === "create" ? createTabRef : openTabRef).current?.focus();
      });
    }
  }, []);

  const handleTabKey = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      let nextMode: SetupMode | null = null;
      if (event.key === "ArrowLeft" || event.key === "ArrowUp" || event.key === "Home") {
        nextMode = "create";
      } else if (
        event.key === "ArrowRight" ||
        event.key === "ArrowDown" ||
        event.key === "End"
      ) {
        nextMode = "open";
      }
      if (!nextMode) return;
      event.preventDefault();
      selectMode(nextMode, true);
    },
    [selectMode],
  );

  const chooseRom = useCallback(async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      title: copy.romDialog,
      filters: [{ name: copy.romFilter, extensions: ["nds"] }],
    });
    if (typeof selected === "string") setRomPath(selected);
  }, [copy.romDialog, copy.romFilter]);

  const chooseProjectDir = useCallback(async () => {
    const selected = await open({
      multiple: false,
      directory: true,
      title: mode === "create" ? copy.folderCreateDialog : copy.folderOpenDialog,
    });
    if (typeof selected === "string") setProjectDir(selected);
  }, [copy.folderCreateDialog, copy.folderOpenDialog, mode]);

  const submit = useCallback(async () => {
    if (!isTauri()) {
      setError(copy.desktopOnly);
      return;
    }
    const name = projectName.trim();
    if (!romPath || !projectDir || (mode === "create" && !name)) return;
    setBusy(true);
    setError("");
    try {
      const snapshot =
        mode === "create"
          ? await invoke<DubbingProjectSnapshot>("create_dubbing_project", {
              projectDir,
              romPath,
              name,
            })
          : await invoke<DubbingProjectSnapshot>("open_dubbing_project", {
              projectDir,
              romPath,
            });
      onLoaded(normalizeSnapshot(snapshot));
    } catch (caught) {
      setError(`${copy.setupError} ${rawError(caught)}`);
    } finally {
      setBusy(false);
    }
  }, [copy.desktopOnly, copy.setupError, mode, onLoaded, projectDir, projectName, romPath]);

  const canSubmit =
    Boolean(romPath && projectDir && (mode === "open" || projectName.trim())) && !busy;

  return (
    <section className="dub-setup" aria-labelledby="dub-setup-title">
      <div className="dub-setup-card">
        <div className="dub-setup-copy">
          <span>{copy.setupEyebrow}</span>
          <h1 id="dub-setup-title">{copy.setupTitle}</h1>
          <p>{copy.setupBody}</p>
        </div>

        <div className="dub-setup-tabs" role="tablist" aria-label={copy.studio}>
          <button
            ref={createTabRef}
            id="dub-create-tab"
            type="button"
            role="tab"
            aria-controls="dub-setup-panel"
            aria-selected={mode === "create"}
            tabIndex={mode === "create" ? 0 : -1}
            onClick={() => selectMode("create")}
            onKeyDown={handleTabKey}
          >
            <Waveform size={18} aria-hidden="true" />
            {copy.createProject}
          </button>
          <button
            ref={openTabRef}
            id="dub-open-tab"
            type="button"
            role="tab"
            aria-controls="dub-setup-panel"
            aria-selected={mode === "open"}
            tabIndex={mode === "open" ? 0 : -1}
            onClick={() => selectMode("open")}
            onKeyDown={handleTabKey}
          >
            <FolderOpen size={18} aria-hidden="true" />
            {copy.openProject}
          </button>
        </div>

        <div
          id="dub-setup-panel"
          className="dub-setup-fields"
          role="tabpanel"
          aria-labelledby={mode === "create" ? "dub-create-tab" : "dub-open-tab"}
        >
          {mode === "create" && (
            <label className="dub-field">
              <span>{copy.projectName}</span>
              <input
                value={projectName}
                maxLength={120}
                autoComplete="off"
                placeholder={copy.projectNamePlaceholder}
                onChange={(event) => setProjectName(event.currentTarget.value)}
              />
            </label>
          )}

          <div className="dub-picker-field">
            <span>{copy.sourceRom}</span>
            <button type="button" onClick={() => void chooseRom()}>
              <FileAudio size={20} aria-hidden="true" />
              <strong>{romPath ? fileName(romPath) : copy.chooseRom}</strong>
              <small>{romPath ? copy.change : ".nds"}</small>
            </button>
          </div>

          <div className="dub-picker-field">
            <span>{copy.projectFolder}</span>
            <button type="button" onClick={() => void chooseProjectDir()}>
              <HardDrives size={20} aria-hidden="true" />
              <strong>{projectDir ? fileName(projectDir) : copy.chooseFolder}</strong>
              <small>{projectDir ? copy.change : "…"}</small>
            </button>
          </div>
        </div>

        {error && (
          <div className="dub-error" role="alert">
            <Warning size={18} aria-hidden="true" />
            <span>{error}</span>
          </div>
        )}

        <button
          className="dub-primary dub-setup-submit"
          type="button"
          disabled={!canSubmit}
          onClick={() => void submit()}
        >
          {busy ? (
            <SpinnerGap className="dub-spin" size={19} aria-hidden="true" />
          ) : mode === "create" ? (
            <Waveform size={19} aria-hidden="true" />
          ) : (
            <FolderOpen size={19} aria-hidden="true" />
          )}
          {busy ? copy.loading : mode === "create" ? copy.create : copy.open}
        </button>
      </div>
    </section>
  );
}

export function DubbingStudioView({
  language,
  onExclusiveOperationChange,
  onNavigationLockChange,
}: DubbingStudioViewProps) {
  const copy = COPY[language];
  const [snapshot, setSnapshot] = useState<DubbingProjectSnapshot | null>(null);
  const [selectedSymbol, setSelectedSymbol] = useState("");
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search);
  const [filter, setFilter] = useState<DubbingStatusFilter>("all");
  const [devices, setDevices] = useState<AudioInputDevice[]>([]);
  const [deviceId, setDeviceId] = useState("");
  const [devicesBusy, setDevicesBusy] = useState(false);
  const [recordingState, setRecordingState] = useState<RecordingState>("idle");
  const [recordingStartedAt, setRecordingStartedAt] = useState(0);
  const [recordingElapsed, setRecordingElapsed] = useState(0);
  const [playingAudioKey, setPlayingAudioKey] = useState("");
  const [audioBusyKey, setAudioBusyKey] = useState("");
  const [notesDraft, setNotesDraft] = useState("");
  const [statusDraft, setStatusDraft] = useState<TargetStatus>("missing");
  const [gainDraft, setGainDraft] = useState(0);
  const [savingTarget, setSavingTarget] = useState(false);
  const [referenceLanguage, setReferenceLanguage] =
    useState<DubbingReferenceLanguage>("japanese");
  const [levelAnalysis, setLevelAnalysis] = useState<DubbingLevelAnalysis | null>(null);
  const [analysisBusy, setAnalysisBusy] = useState(false);
  const [processingBusy, setProcessingBusy] = useState(false);
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchConfirmationOpen, setBatchConfirmationOpen] = useState(false);
  const [batchResult, setBatchResult] =
    useState<DubbingLevelMatchBatchResult | null>(null);
  const [buildBusy, setBuildBusy] = useState(false);
  const [buildProgress, setBuildProgress] = useState<DubbingBuildProgress | null>(null);
  const [buildResult, setBuildResult] = useState<TestRomBuildResult | null>(null);
  const [error, setError] = useState("");
  const [announcement, setAnnouncement] = useState("");
  const [lastRecording, setLastRecording] = useState<DubbingTakeResult["summary"] | null>(null);
  const audioRef = useRef<{ audio: HTMLAudioElement; url: string; key: string } | null>(null);
  const audioRequestRef = useRef(0);
  const analysisRequestRef = useRef(0);
  const mountedRef = useRef(true);
  const recordingRef = useRef(false);
  const recordingSymbolRef = useRef("");
  const recordingTokenRef = useRef(0);
  const recordingStartupRef = useRef<Promise<void> | null>(null);
  const savingTargetRef = useRef(false);
  const batchRunRef = useRef(false);
  const batchMatchButtonRef = useRef<HTMLButtonElement | null>(null);
  const batchCancelButtonRef = useRef<HTMLButtonElement | null>(null);

  const currentCue = useMemo(
    () => snapshot?.cues.find((cue) => cue.symbol === selectedSymbol) ?? null,
    [selectedSymbol, snapshot],
  );
  const currentLevelAnalysis = useMemo(() => {
    if (
      !levelAnalysis ||
      levelAnalysis.symbol !== currentCue?.symbol ||
      levelAnalysis.takeId !== currentCue.progress.activeTake ||
      levelAnalysis.referenceLanguage !== referenceLanguage
    ) {
      return null;
    }
    return levelAnalysis;
  }, [currentCue?.progress.activeTake, currentCue?.symbol, levelAnalysis, referenceLanguage]);

  const filteredCues = useMemo(() => {
    if (!snapshot) return [];
    const needle = normalizeSearch(deferredSearch.trim());
    return snapshot.cues.filter((cue) => {
      if (!cueMatchesFilter(cue, filter)) return false;
      if (!needle) return true;
      return normalizeSearch(
        `${cue.speaker} ${cue.speakerRaw} ${cue.text} ${cue.symbol} ${cue.scriptPath} ${cue.functionName}`,
      ).includes(needle);
    });
  }, [deferredSearch, filter, snapshot]);

  const filteredPosition = useMemo(
    () => filteredCues.findIndex((cue) => cue.symbol === selectedSymbol),
    [filteredCues, selectedSymbol],
  );
  const detailsDirty = currentCue
    ? notesDraft !== currentCue.progress.notes || statusDraft !== visibleStatus(currentCue)
    : false;
  const gainDirty = currentCue
    ? Math.abs(gainDraft - currentCue.progress.gainDb) >= 0.05
    : false;
  const targetDirty = detailsDirty || gainDirty;
  const workspaceBusy = buildBusy || processingBusy || batchBusy;
  const exclusiveOperationActive =
    recordingState !== "idle" ||
    buildBusy ||
    batchBusy ||
    Boolean(audioBusyKey || playingAudioKey);
  const missingCount = useMemo(
    () =>
      snapshot?.cues.filter(
        (cue) => !cue.progress.activeTake && cue.progress.status !== "skipped",
      ).length ?? 0,
    [snapshot?.cues],
  );
  const cueStatusLabels = useMemo<Record<TargetStatus, string>>(
    () => ({
      missing: copy.statusMissing,
      recorded: copy.statusRecorded,
      needs_review: copy.statusNeedsReview,
      approved: copy.statusApproved,
      skipped: copy.statusSkipped,
    }),
    [
      copy.statusApproved,
      copy.statusMissing,
      copy.statusNeedsReview,
      copy.statusRecorded,
      copy.statusSkipped,
    ],
  );

  const stopPlayback = useCallback(() => {
    audioRequestRef.current += 1;
    const current = audioRef.current;
    if (current) {
      current.audio.pause();
      current.audio.removeAttribute("src");
      URL.revokeObjectURL(current.url);
      audioRef.current = null;
    }
    setPlayingAudioKey("");
    setAudioBusyKey("");
  }, []);

  const invalidateLevelAnalysis = useCallback(() => {
    analysisRequestRef.current += 1;
    setLevelAnalysis(null);
    setAnalysisBusy(false);
  }, []);

  const replaceProgress = useCallback((symbol: string, progress: TargetProgress) => {
    const normalized = normalizeProgress(progress);
    // A derived ROM represents one exact set of active takes. Any project mutation makes the
    // previous success card misleading even when its output file still exists on disk.
    setBuildResult(null);
    setBuildProgress(null);
    setLastRecording(null);
    setBatchResult(null);
    setSnapshot((current) => {
      if (!current) return current;
      const cues = current.cues.map((cue) =>
        cue.symbol === symbol ? { ...cue, progress: normalized } : cue,
      );
      return {
        ...current,
        cues,
        summary: summarize(cues),
        manifest: {
          ...current.manifest,
          targets: { ...current.manifest.targets, [symbol]: normalized },
        },
      };
    });
  }, []);

  const replaceProgresses = useCallback((updated: DubbingLevelMatchBatchResult["updated"]) => {
    const bySymbol = new Map(
      updated.map(({ symbol, progress }) => [symbol, normalizeProgress(progress)]),
    );
    setBuildResult(null);
    setBuildProgress(null);
    setLastRecording(null);
    setSnapshot((current) => {
      if (!current) return current;
      const targets = { ...current.manifest.targets };
      const cues = current.cues.map((cue) => {
        const progress = bySymbol.get(cue.symbol);
        if (!progress) return cue;
        targets[cue.symbol] = progress;
        return { ...cue, progress };
      });
      return {
        ...current,
        cues,
        summary: summarize(cues),
        manifest: { ...current.manifest, targets },
      };
    });
  }, []);

  const loadDevices = useCallback(async () => {
    if (!isTauri()) return;
    setDevicesBusy(true);
    try {
      const available = await invoke<AudioInputDevice[]>("list_dubbing_input_devices");
      setDevices(available);
      setDeviceId((current) => {
        if (available.some((device) => device.id === current)) return current;
        return available.find((device) => device.isDefault)?.id ?? available[0]?.id ?? "";
      });
    } catch (caught) {
      setError(`${copy.devicesError} ${rawError(caught)}`);
    } finally {
      setDevicesBusy(false);
    }
  }, [copy.devicesError]);

  const persistCurrentDraft = useCallback(
    async (announce = true): Promise<boolean> => {
      const project = snapshot;
      const cue = currentCue;
      if (!project || !cue || !targetDirty) return true;
      if (
        savingTargetRef.current ||
        recordingState !== "idle" ||
        workspaceBusy
      ) {
        return false;
      }

      savingTargetRef.current = true;
      setSavingTarget(true);
      if (gainDirty) {
        stopPlayback();
        setProcessingBusy(true);
      }
      try {
        let progress = cue.progress;
        if (detailsDirty) {
          progress = await invoke<TargetProgress>("update_dubbing_target", {
            projectDir: project.projectDir,
            symbol: cue.symbol,
            status: statusDraft,
            notes: notesDraft,
          });
          if (!mountedRef.current) return false;
          replaceProgress(cue.symbol, progress);
        }
        if (gainDirty) {
          const statusBeforeGain = progress.status;
          progress = await invoke<TargetProgress>("update_dubbing_processing", {
            projectDir: project.projectDir,
            symbol: cue.symbol,
            gainDb: clampGain(gainDraft),
            trimStartMs: progress.trimStartMs,
            trimEndMs: progress.trimEndMs,
          });
          if (!mountedRef.current) return false;
          replaceProgress(cue.symbol, progress);
          setLevelAnalysis(null);
          if (announce && statusBeforeGain === "approved" && progress.status === "needs_review") {
            setAnnouncement(copy.approvalResetDone);
          } else if (announce) {
            setAnnouncement(copy.saved);
          }
        } else if (announce) {
          setAnnouncement(copy.saved);
        }
        setError("");
        return true;
      } catch (caught) {
        if (mountedRef.current) {
          setError(`${copy.saveError} ${rawError(caught)}`);
        }
        return false;
      } finally {
        savingTargetRef.current = false;
        if (mountedRef.current) {
          setSavingTarget(false);
          setProcessingBusy(false);
        }
      }
    },
    [
      copy.approvalResetDone,
      copy.saveError,
      copy.saved,
      currentCue,
      detailsDirty,
      gainDirty,
      gainDraft,
      notesDraft,
      recordingState,
      replaceProgress,
      snapshot,
      statusDraft,
      stopPlayback,
      targetDirty,
      workspaceBusy,
    ],
  );

  const selectCue = useCallback(
    async (symbol: string): Promise<boolean> => {
      if (symbol === selectedSymbol) return true;
      if (
        recordingState !== "idle" ||
        recordingRef.current ||
        savingTargetRef.current ||
        workspaceBusy
      ) {
        return false;
      }
      if (!(await persistCurrentDraft(false)) || !mountedRef.current) return false;
      stopPlayback();
      invalidateLevelAnalysis();
      setSelectedSymbol(symbol);
      setError("");
      setLastRecording(null);
      return true;
    },
    [
      persistCurrentDraft,
      recordingState,
      selectedSymbol,
      invalidateLevelAnalysis,
      stopPlayback,
      workspaceBusy,
    ],
  );

  const navigate = useCallback(
    (direction: -1 | 1) => {
      if (!filteredCues.length || recordingState !== "idle" || workspaceBusy) return;
      const origin = filteredPosition < 0 ? (direction > 0 ? -1 : 0) : filteredPosition;
      const next = Math.min(filteredCues.length - 1, Math.max(0, origin + direction));
      void selectCue(filteredCues[next].symbol);
    },
    [filteredCues, filteredPosition, recordingState, selectCue, workspaceBusy],
  );

  const startRecording = useCallback(async () => {
    const project = snapshot;
    const cue = currentCue;
    if (
      !project ||
      !cue ||
      !deviceId ||
      recordingState !== "idle" ||
      recordingRef.current ||
      workspaceBusy
    ) {
      return;
    }
    if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;

    const token = recordingTokenRef.current + 1;
    recordingTokenRef.current = token;
    let settleStartup!: () => void;
    const startupSettled = new Promise<void>((resolve) => {
      settleStartup = resolve;
    });
    recordingStartupRef.current = startupSettled;
    recordingRef.current = true;
    recordingSymbolRef.current = cue.symbol;
    stopPlayback();
    setError("");
    setLastRecording(null);
    setRecordingState("starting");
    try {
      // Let React commit the exclusive state so app music is paused before the native stream opens.
      await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
      if (
        !mountedRef.current ||
        recordingTokenRef.current !== token ||
        !recordingRef.current
      ) {
        return;
      }
      await invoke<RecordingStartInfo>("start_dubbing_recording", {
        projectDir: project.projectDir,
        symbol: cue.symbol,
        deviceId,
      });
      if (
        !mountedRef.current ||
        recordingTokenRef.current !== token ||
        !recordingRef.current
      ) {
        // Cleanup may run while macOS is still resolving microphone permission. A second cancel
        // after native startup closes the stream that did not exist during the first attempt.
        await invoke<void>("cancel_dubbing_recording").catch(() => undefined);
        return;
      }
      setRecordingStartedAt(Date.now());
      setRecordingElapsed(0);
      setRecordingState("recording");
      setAnnouncement(copy.recording);
    } catch (caught) {
      if (!mountedRef.current || recordingTokenRef.current !== token) return;
      recordingRef.current = false;
      recordingSymbolRef.current = "";
      setRecordingState("idle");
      setError(`${copy.recordError} ${rawError(caught)}`);
    } finally {
      if (recordingStartupRef.current === startupSettled) {
        recordingStartupRef.current = null;
      }
      settleStartup();
    }
  }, [
    copy.recordError,
    copy.recording,
    currentCue,
    deviceId,
    persistCurrentDraft,
    recordingState,
    snapshot,
    stopPlayback,
    workspaceBusy,
  ]);

  const stopRecording = useCallback(async () => {
    if (recordingState !== "recording" || !recordingRef.current) return;
    const symbol = recordingSymbolRef.current;
    const token = recordingTokenRef.current + 1;
    recordingTokenRef.current = token;
    setRecordingState("stopping");
    try {
      const result = await invoke<DubbingTakeResult>("stop_dubbing_recording");
      if (!mountedRef.current || recordingTokenRef.current !== token) return;
      replaceProgress(symbol, result.progress);
      setLastRecording(result.summary);
      setAnnouncement(
        `${copy.recordedAnnouncement}: ${formatDuration(result.summary.durationMs)}, ${copy.peak} ${result.summary.peakDbfs.toFixed(1)} dBFS.`,
      );
    } catch (caught) {
      if (mountedRef.current && recordingTokenRef.current === token) {
        setError(`${copy.stopError} ${rawError(caught)}`);
      }
    } finally {
      if (mountedRef.current && recordingTokenRef.current === token) {
        recordingRef.current = false;
        recordingSymbolRef.current = "";
        setRecordingState("idle");
        setRecordingElapsed(0);
      }
    }
  }, [
    copy.peak,
    copy.recordedAnnouncement,
    copy.stopError,
    recordingState,
    replaceProgress,
  ]);

  const cancelRecording = useCallback(async () => {
    if (
      !recordingRef.current ||
      (recordingState !== "starting" && recordingState !== "recording")
    ) {
      return;
    }
    const pendingStartup = recordingStartupRef.current;
    const token = recordingTokenRef.current + 1;
    recordingTokenRef.current = token;
    recordingRef.current = false;
    recordingSymbolRef.current = "";
    setRecordingState("stopping");
    try {
      if (pendingStartup) {
        // The stale startup path owns cleanup so the UI cannot unlock between the late native
        // start and its compensating cancel, when a newer attempt could otherwise be targeted.
        await pendingStartup;
      } else {
        await invoke<void>("cancel_dubbing_recording");
      }
      if (mountedRef.current && recordingTokenRef.current === token) {
        setAnnouncement(copy.cancelledAnnouncement);
      }
    } catch (caught) {
      if (mountedRef.current && recordingTokenRef.current === token) {
        setError(rawError(caught));
      }
    } finally {
      if (mountedRef.current && recordingTokenRef.current === token) {
        setRecordingState("idle");
        setRecordingElapsed(0);
      }
    }
  }, [copy.cancelledAnnouncement, recordingState]);

  const playStreamedAudio = useCallback(
    async (
      key: string,
      command: string,
      args: Record<string, unknown>,
      errorMessage: string,
    ) => {
      if (recordingState !== "idle" || workspaceBusy) return;
      if (audioRef.current?.key === key) {
        if (audioRef.current.audio.paused) {
          const current = audioRef.current;
          const request = audioRequestRef.current;
          setAudioBusyKey(key);
          try {
            // Give the derived exclusive-audio state one commit before playback resumes, so the
            // application soundtrack cannot leak into the first resumed samples.
            await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
            if (
              request !== audioRequestRef.current ||
              audioRef.current?.audio !== current.audio
            ) {
              return;
            }
            await current.audio.play();
            if (
              request !== audioRequestRef.current ||
              audioRef.current?.audio !== current.audio
            ) {
              current.audio.pause();
              return;
            }
            setPlayingAudioKey(key);
          } catch (caught) {
            if (request === audioRequestRef.current) {
              stopPlayback();
              setError(`${errorMessage} ${rawError(caught)}`);
            }
          } finally {
            if (request === audioRequestRef.current) setAudioBusyKey("");
          }
        } else {
          audioRef.current.audio.pause();
          setPlayingAudioKey("");
        }
        return;
      }

      stopPlayback();
      setAudioBusyKey(key);
      const request = audioRequestRef.current;
      try {
        const bytes = await new Promise<ArrayBuffer>((resolve, reject) => {
          const audioChannel = new Channel<ArrayBuffer>();
          audioChannel.onmessage = resolve;
          void invoke<void>(command, {
            ...args,
            onData: audioChannel,
          }).catch(reject);
        });
        const url = URL.createObjectURL(
          new Blob([bytes], { type: "audio/wav" }),
        );
        if (request !== audioRequestRef.current) {
          URL.revokeObjectURL(url);
          return;
        }
        const audio = new Audio(url);
        audioRef.current = { audio, url, key };
        audio.addEventListener("ended", () => {
          if (audioRef.current?.audio === audio) setPlayingAudioKey("");
        });
        audio.addEventListener("error", () => {
          if (audioRef.current?.audio === audio) {
            setError(errorMessage);
            stopPlayback();
          }
        });
        await audio.play();
        if (
          request !== audioRequestRef.current ||
          audioRef.current?.audio !== audio
        ) {
          audio.pause();
          return;
        }
        setPlayingAudioKey(audio.ended ? "" : key);
      } catch (caught) {
        if (request === audioRequestRef.current) {
          stopPlayback();
          setError(`${errorMessage} ${rawError(caught)}`);
        }
      } finally {
        if (request === audioRequestRef.current) setAudioBusyKey("");
      }
    },
    [recordingState, stopPlayback, workspaceBusy],
  );

  const playTake = useCallback(
    async (take: TakeMetadata) => {
      if (!snapshot || !currentCue) return;
      await playStreamedAudio(
        rawAudioKey(currentCue.symbol, take.id),
        "read_dubbing_take",
        {
          projectDir: snapshot.projectDir,
          symbol: currentCue.symbol,
          takeId: take.id,
        },
        copy.readError,
      );
    },
    [copy.readError, currentCue, playStreamedAudio, snapshot],
  );

  const selectTake = useCallback(
    async (take: TakeMetadata) => {
      const project = snapshot;
      const cue = currentCue;
      if (!project || !cue || recordingState !== "idle" || workspaceBusy) return;
      if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;
      try {
        const progress = await invoke<TargetProgress>("select_dubbing_take", {
          projectDir: project.projectDir,
          symbol: cue.symbol,
          takeId: take.id,
        });
        if (!mountedRef.current) return;
        replaceProgress(cue.symbol, progress);
        setAnnouncement(copy.activeTake);
      } catch (caught) {
        if (mountedRef.current) setError(`${copy.selectError} ${rawError(caught)}`);
      }
    },
    [
      copy.activeTake,
      copy.selectError,
      currentCue,
      persistCurrentDraft,
      recordingState,
      replaceProgress,
      snapshot,
      workspaceBusy,
    ],
  );

  const saveTarget = useCallback(async () => {
    await persistCurrentDraft(true);
  }, [persistCurrentDraft]);

  const analyzeLevel = useCallback(async () => {
    const project = snapshot;
    const cue = currentCue;
    if (
      !project ||
      !cue?.progress.activeTake ||
      gainDirty ||
      recordingState !== "idle" ||
      workspaceBusy ||
      savingTargetRef.current
    ) {
      return;
    }
    const request = analysisRequestRef.current + 1;
    analysisRequestRef.current = request;
    setAnalysisBusy(true);
    setError("");
    try {
      const analysis = await invoke<DubbingLevelAnalysis>("analyze_dubbing_level", {
        projectDir: project.projectDir,
        symbol: cue.symbol,
        referenceLanguage,
      });
      if (!mountedRef.current || analysisRequestRef.current !== request) return;
      setLevelAnalysis(analysis);
    } catch (caught) {
      if (mountedRef.current && analysisRequestRef.current === request) {
        setError(`${copy.analyzeError} ${rawError(caught)}`);
      }
    } finally {
      if (mountedRef.current && analysisRequestRef.current === request) {
        setAnalysisBusy(false);
      }
    }
  }, [copy.analyzeError, currentCue, gainDirty, recordingState, referenceLanguage, snapshot, workspaceBusy]);

  const applySuggestedGain = useCallback(async () => {
    const project = snapshot;
    const cue = currentCue;
    const analysis = currentLevelAnalysis;
    const recommended = analysis?.recommendedGainDb;
    if (
      !project ||
      !cue?.progress.activeTake ||
      !analysis ||
      analysis.symbol !== cue.symbol ||
      analysis.takeId !== cue.progress.activeTake ||
      analysis.referenceLanguage !== referenceLanguage ||
      recommended === null ||
      recommended === undefined ||
      recordingState !== "idle" ||
      workspaceBusy ||
      savingTargetRef.current
    ) {
      return;
    }

    savingTargetRef.current = true;
    setSavingTarget(true);
    setProcessingBusy(true);
    stopPlayback();
    setError("");
    try {
      let progress = cue.progress;
      if (detailsDirty) {
        progress = await invoke<TargetProgress>("update_dubbing_target", {
          projectDir: project.projectDir,
          symbol: cue.symbol,
          status: statusDraft,
          notes: notesDraft,
        });
        if (!mountedRef.current) return;
        replaceProgress(cue.symbol, progress);
      }
      const statusBeforeGain = progress.status;
      progress = await invoke<TargetProgress>("update_dubbing_processing", {
        projectDir: project.projectDir,
        symbol: cue.symbol,
        gainDb: recommended,
        trimStartMs: progress.trimStartMs,
        trimEndMs: progress.trimEndMs,
      });
      if (!mountedRef.current) return;
      replaceProgress(cue.symbol, progress);
      setGainDraft(progress.gainDb);
      const analysisRequest = analysisRequestRef.current + 1;
      analysisRequestRef.current = analysisRequest;
      setAnalysisBusy(true);
      try {
        const refreshed = await invoke<DubbingLevelAnalysis>("analyze_dubbing_level", {
          projectDir: project.projectDir,
          symbol: cue.symbol,
          referenceLanguage,
        });
        if (mountedRef.current && analysisRequestRef.current === analysisRequest) {
          setLevelAnalysis(refreshed);
        }
      } catch (caught) {
        if (mountedRef.current && analysisRequestRef.current === analysisRequest) {
          setLevelAnalysis(null);
          setError(`${copy.analyzeError} ${rawError(caught)}`);
        }
      } finally {
        if (mountedRef.current && analysisRequestRef.current === analysisRequest) {
          setAnalysisBusy(false);
        }
      }
      setAnnouncement(
        statusBeforeGain === "approved" && progress.status === "needs_review"
          ? copy.approvalResetDone
          : copy.gainSaved,
      );
    } catch (caught) {
      if (mountedRef.current) {
        setError(`${copy.gainSaveError} ${rawError(caught)}`);
      }
    } finally {
      savingTargetRef.current = false;
      if (mountedRef.current) {
        setSavingTarget(false);
        setProcessingBusy(false);
      }
    }
  }, [
    copy.analyzeError,
    copy.approvalResetDone,
    copy.gainSaveError,
    copy.gainSaved,
    currentCue,
    currentLevelAnalysis,
    detailsDirty,
    notesDraft,
    recordingState,
    referenceLanguage,
    replaceProgress,
    snapshot,
    statusDraft,
    stopPlayback,
    workspaceBusy,
  ]);

  const playProcessedPreview = useCallback(async () => {
    const project = snapshot;
    const cue = currentCue;
    if (
      !project ||
      !cue?.progress.activeTake ||
      gainDirty ||
      recordingState !== "idle" ||
      workspaceBusy ||
      savingTargetRef.current
    ) {
      return;
    }
    await playStreamedAudio(
      `${cue.symbol}:processed:${cue.progress.activeTake}:${clampGain(gainDraft)}`,
      "read_dubbing_processed_preview",
      { projectDir: project.projectDir, symbol: cue.symbol },
      copy.previewError,
    );
  }, [
    copy.previewError,
    currentCue,
    gainDraft,
    gainDirty,
    playStreamedAudio,
    recordingState,
    snapshot,
    workspaceBusy,
  ]);

  const playReferencePreview = useCallback(async () => {
    const cue = currentCue;
    if (!cue || recordingState !== "idle" || workspaceBusy) return;
    await playStreamedAudio(
      `${cue.symbol}:reference:${referenceLanguage}`,
      "read_dubbing_reference_preview",
      { symbol: cue.symbol, referenceLanguage },
      copy.previewError,
    );
  }, [copy.previewError, currentCue, playStreamedAudio, recordingState, referenceLanguage, workspaceBusy]);

  const closeMatchAllConfirmation = useCallback(() => {
    setBatchConfirmationOpen(false);
    window.requestAnimationFrame(() => batchMatchButtonRef.current?.focus());
  }, []);

  const openMatchAllConfirmation = useCallback(() => {
    const project = snapshot;
    if (
      !project ||
      project.summary.recorded === 0 ||
      recordingState !== "idle" ||
      analysisBusy ||
      batchConfirmationOpen ||
      workspaceBusy ||
      savingTargetRef.current ||
      batchRunRef.current
    ) {
      return;
    }
    stopPlayback();
    setBatchConfirmationOpen(true);
  }, [analysisBusy, batchConfirmationOpen, recordingState, snapshot, stopPlayback, workspaceBusy]);

  const matchAllLevels = useCallback(async () => {
    const project = snapshot;
    if (
      !project ||
      project.summary.recorded === 0 ||
      recordingState !== "idle" ||
      workspaceBusy ||
      savingTargetRef.current ||
      batchRunRef.current
    ) {
      return;
    }

    batchRunRef.current = true;
    setBatchConfirmationOpen(false);
    setBatchBusy(true);
    setBatchResult(null);
    setLevelAnalysis(null);
    stopPlayback();
    setError("");
    try {
      if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;
      const result = await invoke<DubbingLevelMatchBatchResult>("match_all_dubbing_levels", {
        projectDir: project.projectDir,
        referenceLanguage,
      });
      if (!mountedRef.current) return;
      replaceProgresses(result.updated);
      setBatchResult(result);
      setAnnouncement(`${result.matched.toLocaleString()} ${copy.matched}.`);
    } catch (caught) {
      if (mountedRef.current) {
        setError(`${copy.matchAllError} ${rawError(caught)}`);
      }
    } finally {
      batchRunRef.current = false;
      if (mountedRef.current) {
        setBatchBusy(false);
        window.requestAnimationFrame(() => batchMatchButtonRef.current?.focus());
      }
    }
  }, [
    copy.matchAllError,
    copy.matched,
    persistCurrentDraft,
    recordingState,
    referenceLanguage,
    replaceProgresses,
    snapshot,
    stopPlayback,
    workspaceBusy,
  ]);

  const changeGainDraft = useCallback(
    (value: number) => {
      if (!Number.isFinite(value)) return;
      stopPlayback();
      setGainDraft(clampGain(value));
    },
    [stopPlayback],
  );

  const buildTestRom = useCallback(async () => {
    const project = snapshot;
    if (
      !project ||
      workspaceBusy ||
      recordingState !== "idle" ||
      project.summary.recorded === 0
    ) {
      return;
    }
    if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;
    let outputPath: string | null;
    try {
      outputPath = await save({
        title: copy.buildDialog,
        defaultPath: previewRomPath(project.romPath),
        filters: [{ name: copy.romFilter, extensions: ["nds"] }],
      });
    } catch (caught) {
      setError(`${copy.buildError} ${rawError(caught)}`);
      return;
    }
    if (!outputPath) return;

    stopPlayback();
    setBuildBusy(true);
    setBuildResult(null);
    setBuildProgress({ stage: "validate", completed: 0, total: 1, message: copy.buildingTest });
    setError("");
    try {
      const result = await invoke<TestRomBuildResult>("build_dubbing_test_rom", {
        projectDir: project.projectDir,
        romPath: project.romPath,
        outputPath,
      });
      if (!mountedRef.current) return;
      setBuildResult(result);
      setAnnouncement(`${copy.buildDone}: ${fileName(result.rom.outputPath)}`);
    } catch (caught) {
      setError(`${copy.buildError} ${rawError(caught)}`);
    } finally {
      if (mountedRef.current) {
        setBuildBusy(false);
      }
    }
  }, [
    copy.buildDialog,
    copy.buildError,
    copy.buildDone,
    copy.buildingTest,
    copy.romFilter,
    persistCurrentDraft,
    recordingState,
    snapshot,
    stopPlayback,
    workspaceBusy,
  ]);

  const clearProjectSession = useCallback(() => {
    invalidateLevelAnalysis();
    stopPlayback();
    batchRunRef.current = false;
    setBatchBusy(false);
    setBatchConfirmationOpen(false);
    setBuildProgress(null);
    setBuildResult(null);
    setLastRecording(null);
    setBatchResult(null);
    setReferenceLanguage("japanese");
    setError("");
    setSearch("");
    setFilter("all");
  }, [invalidateLevelAnalysis, stopPlayback]);

  const loadProject = useCallback(
    (loaded: DubbingProjectSnapshot) => {
      clearProjectSession();
      setSnapshot(loaded);
      setSelectedSymbol(loaded.cues[0]?.symbol ?? "");
      setAnnouncement(copy.setupReady);
    },
    [clearProjectSession, copy.setupReady],
  );

  const closeProject = useCallback(async () => {
    if (recordingState !== "idle" || workspaceBusy) return;
    if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;
    clearProjectSession();
    setSnapshot(null);
    setSelectedSymbol("");
  }, [clearProjectSession, persistCurrentDraft, recordingState, workspaceBusy]);

  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<DubbingBuildProgress>("dubbing-build-progress", (event) => {
      setBuildProgress(event.payload);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    if (!snapshot) return;
    void loadDevices();
  }, [loadDevices, snapshot?.projectDir]);

  useEffect(() => {
    if (!snapshot || filteredCues.length === 0) return;
    if (!filteredCues.some((cue) => cue.symbol === selectedSymbol)) {
      void selectCue(filteredCues[0].symbol);
    }
  }, [filteredCues, selectCue, selectedSymbol, snapshot]);

  useEffect(() => {
    if (!batchConfirmationOpen) return;
    const frame = window.requestAnimationFrame(() => batchCancelButtonRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [batchConfirmationOpen]);

  useEffect(() => {
    if (!currentCue) return;
    setNotesDraft(currentCue.progress.notes);
    setStatusDraft(visibleStatus(currentCue));
    setGainDraft(currentCue.progress.gainDb);
  }, [
    currentCue?.progress.activeTake,
    currentCue?.progress.gainDb,
    currentCue?.progress.notes,
    currentCue?.progress.status,
    currentCue?.symbol,
  ]);

  useEffect(() => {
    invalidateLevelAnalysis();
  }, [currentCue?.progress.activeTake, currentCue?.symbol, invalidateLevelAnalysis, referenceLanguage]);

  useEffect(() => {
    onNavigationLockChange?.(
      targetDirty || savingTarget || batchBusy || batchConfirmationOpen,
    );
  }, [batchBusy, batchConfirmationOpen, onNavigationLockChange, savingTarget, targetDirty]);

  useLayoutEffect(() => {
    // One derived owner prevents a preview finishing from resuming app music while a recording,
    // build, or batch update still needs exclusive audio.
    onExclusiveOperationChange?.(exclusiveOperationActive);
  }, [exclusiveOperationActive, onExclusiveOperationChange]);

  useEffect(() => {
    if (!targetDirty) return;
    const protectDraft = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", protectDraft);
    return () => window.removeEventListener("beforeunload", protectDraft);
  }, [targetDirty]);

  useEffect(() => {
    if (recordingState !== "recording") return;
    const update = () => setRecordingElapsed(Date.now() - recordingStartedAt);
    update();
    const interval = window.setInterval(update, 100);
    return () => window.clearInterval(interval);
  }, [recordingStartedAt, recordingState]);

  useEffect(() => {
    if (recordingState !== "recording") return;
    // Stop with enough headroom for the native 45-second hard limit to remain a validator,
    // not a normal end-of-take path on a busy machine.
    const timeout = window.setTimeout(() => void stopRecording(), 44_500);
    return () => window.clearTimeout(timeout);
  }, [recordingState, stopRecording]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        !snapshot ||
        batchConfirmationOpen ||
        workspaceBusy ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        isTypingTarget(event.target)
      ) {
        return;
      }
      if (event.key.toLocaleLowerCase() === "r") {
        event.preventDefault();
        if (recordingState === "recording") void stopRecording();
        else if (recordingState === "idle") void startRecording();
        return;
      }
      if (event.code === "Space") {
        const active = currentCue?.progress.takes.find(
          (take) => take.id === currentCue.progress.activeTake,
        );
        if (active && recordingState === "idle") {
          event.preventDefault();
          void playTake(active);
        }
        return;
      }
      if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        event.preventDefault();
        navigate(-1);
      } else if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        event.preventDefault();
        navigate(1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [batchConfirmationOpen, cancelRecording, currentCue, navigate, playTake, recordingState, snapshot, startRecording, stopRecording, workspaceBusy]);

  useEffect(() => stopPlayback, [selectedSymbol, stopPlayback]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      recordingTokenRef.current += 1;
      stopPlayback();
      if (recordingRef.current) {
        recordingRef.current = false;
        recordingSymbolRef.current = "";
        void invoke("cancel_dubbing_recording");
      }
      onExclusiveOperationChange?.(false);
      onNavigationLockChange?.(false);
    };
  }, [onExclusiveOperationChange, onNavigationLockChange, stopPlayback]);

  if (!snapshot) {
    return (
      <div className="dubbing-studio" lang={language}>
        <SetupView
          language={language}
          onLoaded={loadProject}
        />
        <p className="dub-sr-only" aria-live="polite">{announcement}</p>
      </div>
    );
  }

  const summary = snapshot.summary;
  const completion = summary.total ? (summary.recorded / summary.total) * 100 : 0;
  const activeTake = currentCue?.progress.takes.find(
    (take) => take.id === currentCue.progress.activeTake,
  );
  const rawPlaybackKey = activeTake && currentCue
    ? rawAudioKey(currentCue.symbol, activeTake.id)
    : "";
  const processedPlaybackKey = activeTake && currentCue
    ? `${currentCue.symbol}:processed:${activeTake.id}:${clampGain(gainDraft)}`
    : "";
  const referencePlaybackKey = currentCue
    ? `${currentCue.symbol}:reference:${referenceLanguage}`
    : "";
  const finalPeak = !gainDirty && currentLevelAnalysis
    ? currentLevelAnalysis.processed.peakDbfs
    : null;
  const processedClipping =
    !gainDirty && (currentLevelAnalysis?.processedClippedSamples ?? 0) > 0;
  const peakRisk =
    processedClipping ||
    (finalPeak !== null && finalPeak > -1);

  return (
    <div className="dubbing-studio" lang={language}>
      <p className="dub-sr-only" aria-live="polite">{announcement}</p>

      <header className="dub-header">
        <div className="dub-project-title">
          <span>{copy.studio}</span>
          <h1>{snapshot.manifest.name}</h1>
          <small>{snapshot.manifest.sourceRomFilename}</small>
        </div>
        <div className="dub-summary" aria-label={`${summary.recorded} / ${summary.total}`}>
          <div className="dub-progress-track" aria-hidden="true">
            <span style={{ width: `${completion}%` }} />
          </div>
          <strong>{summary.recorded.toLocaleString()} / {summary.total.toLocaleString()}</strong>
          <span>{copy.summaryRecorded}</span>
        </div>
        <div className="dub-summary-chips">
          <span className="is-missing">{missingCount.toLocaleString()} {copy.missing}</span>
          <span className="is-review">{summary.needsReview.toLocaleString()} {copy.summaryReview}</span>
          <span className="is-approved">{summary.approved.toLocaleString()} {copy.summaryApproved}</span>
        </div>
        <button
          className="dub-ghost"
          type="button"
          disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
          onClick={() => void closeProject()}
        >
          <FolderOpen size={17} aria-hidden="true" />
          {copy.backToProjects}
        </button>
      </header>

      <div className="dub-toolbar">
        <label className="dub-search">
          <MagnifyingGlass size={17} aria-hidden="true" />
          <span className="dub-sr-only">{copy.search}</span>
          <input
            type="search"
            value={search}
            disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
            placeholder={copy.searchShort}
            onChange={(event) => setSearch(event.currentTarget.value)}
          />
          {search && (
            <button
              type="button"
              aria-label={copy.close}
              disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
              onClick={() => setSearch("")}
            >
              <X size={14} aria-hidden="true" />
            </button>
          )}
        </label>

        <div className="dub-filter" role="group" aria-label={copy.filter}>
          {(["all", ...STATUS_ORDER] as DubbingStatusFilter[]).map((value) => (
            <button
              key={value}
              type="button"
              aria-pressed={filter === value}
              disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
              onClick={() => setFilter(value)}
            >
              {copy[value]}
            </button>
          ))}
        </div>

        <label className="dub-device">
          <Microphone size={16} aria-hidden="true" />
          <span className="dub-sr-only">{copy.inputDevice}</span>
          <select
            value={deviceId}
            disabled={recordingState !== "idle" || workspaceBusy || savingTarget || devicesBusy || devices.length === 0}
            onChange={(event) => setDeviceId(event.currentTarget.value)}
          >
            {devices.length === 0 && <option value="">{copy.noInput}</option>}
            {devices.map((device) => (
              <option key={device.id} value={device.id}>{device.name}</option>
            ))}
          </select>
          <button
            type="button"
            aria-label={copy.refreshDevices}
            disabled={recordingState !== "idle" || workspaceBusy || savingTarget || devicesBusy}
            onClick={() => void loadDevices()}
          >
            {devicesBusy ? <SpinnerGap className="dub-spin" size={15} /> : <Circle size={10} weight="fill" />}
          </button>
        </label>

        <button
          className="dub-build"
          type="button"
          disabled={recordingState !== "idle" || workspaceBusy || savingTarget || summary.recorded === 0}
          title={summary.recorded === 0 ? copy.buildNeedsTake : undefined}
          onClick={() => void buildTestRom()}
        >
          {buildBusy ? (
            <SpinnerGap className="dub-spin" size={17} aria-hidden="true" />
          ) : (
            <HardDrives size={17} aria-hidden="true" />
          )}
          {buildBusy ? copy.buildingTest : copy.buildTest}
        </button>
      </div>

      {(buildBusy || buildResult) && (
        <div className={`dub-build-status ${buildResult ? "is-complete" : ""}`} role="status" aria-live="polite">
          {buildResult ? (
            <CheckCircle size={19} weight="fill" aria-hidden="true" />
          ) : (
            <SpinnerGap className="dub-spin" size={19} aria-hidden="true" />
          )}
          <div>
            <strong>{buildResult ? copy.buildDone : copy.buildingTest}</strong>
            <span>
              {buildResult
                ? `${fileName(buildResult.rom.outputPath)} · ${(buildResult.rom.bytes / 1048576).toFixed(1)} MiB · SHA-256 ${buildResult.rom.sha256.slice(0, 12)}…`
                : buildProgress?.message ?? copy.buildingTest}
            </span>
          </div>
          {buildBusy && buildProgress && buildProgress.total > 0 && (
            <div className="dub-build-progress" aria-hidden="true">
              <span style={{ width: `${Math.min(100, (buildProgress.completed / buildProgress.total) * 100)}%` }} />
            </div>
          )}
          {buildResult && (
            <button
              type="button"
              onClick={() => {
                void revealItemInDir(buildResult.rom.outputPath).catch((caught) => {
                  setError(rawError(caught));
                });
              }}
            >
              <FolderOpen size={16} aria-hidden="true" />
              {copy.revealBuild}
            </button>
          )}
        </div>
      )}

      {error && (
        <div className="dub-error dub-global-error" role="alert">
          <Warning size={17} aria-hidden="true" />
          <span>{error}</span>
          <button type="button" aria-label={copy.close} onClick={() => setError("")}>
            <X size={14} aria-hidden="true" />
          </button>
        </div>
      )}

      <div className="dub-workspace">
        <CueBrowser
          cues={filteredCues}
          disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
          linesLabel={copy.lines}
          noResults={copy.noResults}
          onSelect={selectCue}
          selectedSymbol={selectedSymbol}
          statusLabels={cueStatusLabels}
        />

        {currentCue ? (
          <main className="dub-editor" aria-labelledby="dub-current-speaker">
            <div className="dub-cue-heading">
              <div>
                <span>{copy.currentLine}</span>
                <h2 id="dub-current-speaker">{currentCue.speaker || "—"}</h2>
              </div>
              <div className="dub-cue-identity">
                <strong>{currentCue.symbol}</strong>
                <span>{currentCue.scriptPath} · #{currentCue.ordinal}</span>
              </div>
              <div className="dub-nav-buttons">
                <button
                  type="button"
                  aria-label={copy.previous}
                  disabled={filteredPosition <= 0 || recordingState !== "idle" || workspaceBusy || savingTarget}
                  onClick={() => navigate(-1)}
                >
                  <ArrowLeft size={18} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  aria-label={copy.next}
                  disabled={filteredPosition < 0 || filteredPosition >= filteredCues.length - 1 || recordingState !== "idle" || workspaceBusy || savingTarget}
                  onClick={() => navigate(1)}
                >
                  <ArrowRight size={18} aria-hidden="true" />
                </button>
              </div>
            </div>

            <section className="dub-context-panel" aria-labelledby="dub-context-title">
              <div className="dub-section-title">
                <span>01</span>
                <h3 id="dub-context-title">{copy.context}</h3>
              </div>
              <ol className="dub-context-lines">
                {currentCue.context.map((line) => (
                  <li
                    key={`${line.ordinal}-${line.speakerRaw}`}
                    className={`${line.isCurrent ? "is-current" : ""} ${line.isRecordable ? "" : "is-context-only"}`}
                  >
                    <span className="dub-context-speaker">
                      {line.speaker || copy.narration}
                    </span>
                    <p>{line.text}</p>
                    <small>{line.ordinal}</small>
                  </li>
                ))}
              </ol>
            </section>

            <section className={`dub-recorder is-${recordingState}`} aria-labelledby="dub-recorder-title">
              <div className="dub-section-title">
                <span>02</span>
                <h3 id="dub-recorder-title">{copy.record}</h3>
              </div>
              <blockquote>{currentCue.text}</blockquote>
              <div className="dub-recorder-controls">
                <div className="dub-record-clock">
                  <Clock size={18} aria-hidden="true" />
                  <strong>{formatDuration(recordingElapsed, true)}</strong>
                  <span>
                    {recordingState === "starting"
                      ? copy.preparing
                      : recordingState === "stopping"
                        ? copy.stopping
                        : recordingState === "recording"
                          ? copy.recording
                          : copy.recordHint}
                  </span>
                </div>
                {recordingState === "recording" ? (
                  <>
                    <button className="dub-stop" type="button" onClick={() => void stopRecording()}>
                      <Stop size={18} weight="fill" aria-hidden="true" />
                      {copy.stop}
                    </button>
                    <button className="dub-cancel" type="button" onClick={() => void cancelRecording()}>
                      <X size={17} aria-hidden="true" />
                      {copy.cancel}
                    </button>
                  </>
                ) : recordingState === "starting" ? (
                  <button className="dub-cancel" type="button" onClick={() => void cancelRecording()}>
                    <X size={17} aria-hidden="true" />
                    {copy.cancel}
                  </button>
                ) : (
                  <button
                    className="dub-record"
                    type="button"
                    disabled={!deviceId || recordingState !== "idle" || workspaceBusy || savingTarget}
                    onClick={() => void startRecording()}
                  >
                    {recordingState === "idle" ? (
                      <Microphone size={19} weight="fill" aria-hidden="true" />
                    ) : (
                      <SpinnerGap className="dub-spin" size={19} aria-hidden="true" />
                    )}
                    {recordingState === "idle" ? copy.record : copy.loading}
                  </button>
                )}
              </div>
              {lastRecording && (
                <div className={`dub-capture-report ${lastRecording.clippedSamples || lastRecording.overflowed ? "has-warning" : ""}`}>
                  <CheckCircle size={17} weight="fill" aria-hidden="true" />
                  <span>{formatDuration(lastRecording.durationMs)} · {lastRecording.sampleRate.toLocaleString()} Hz · {copy.peak} {lastRecording.peakDbfs.toFixed(1)} dBFS</span>
                  {lastRecording.clippedSamples > 0 && <strong>{copy.clipping}</strong>}
                  {lastRecording.overflowed && <strong>{copy.overflow}</strong>}
                </div>
              )}
            </section>

            <section className="dub-level-panel" aria-labelledby="dub-level-title">
              <div className="dub-level-heading">
                <div className="dub-section-title">
                  <span>03</span>
                  <div>
                    <h3 id="dub-level-title">{copy.audioBalance}</h3>
                    <p>{copy.audioBalanceBody}</p>
                  </div>
                </div>
                <button
                  className="dub-match-all"
                  ref={batchMatchButtonRef}
                  type="button"
                  disabled={
                    summary.recorded === 0 ||
                    recordingState !== "idle" ||
                    workspaceBusy ||
                    savingTarget ||
                    analysisBusy ||
                    batchConfirmationOpen
                  }
                  onClick={openMatchAllConfirmation}
                >
                  {batchBusy ? (
                    <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                  ) : (
                    <ArrowsLeftRight size={16} aria-hidden="true" />
                  )}
                  {batchBusy ? copy.matchingAll : copy.matchAll}
                </button>
              </div>

              {!activeTake ? (
                <div className="dub-level-empty">
                  <SpeakerHigh size={24} aria-hidden="true" />
                  <span>{copy.levelNeedsTake}</span>
                </div>
              ) : (
                <>
                  <div className="dub-level-actions">
                    <fieldset className="dub-reference-picker">
                      <legend>{copy.referenceVoice}</legend>
                      <button
                        type="button"
                        aria-pressed={referenceLanguage === "japanese"}
                        disabled={recordingState !== "idle" || workspaceBusy || analysisBusy}
                        onClick={() => {
                          if (referenceLanguage === "japanese") return;
                          stopPlayback();
                          invalidateLevelAnalysis();
                          setReferenceLanguage("japanese");
                        }}
                      >
                        JP · {copy.japaneseReference}
                      </button>
                      <button
                        type="button"
                        aria-pressed={referenceLanguage === "english"}
                        disabled={recordingState !== "idle" || workspaceBusy || analysisBusy}
                        onClick={() => {
                          if (referenceLanguage === "english") return;
                          stopPlayback();
                          invalidateLevelAnalysis();
                          setReferenceLanguage("english");
                        }}
                      >
                        EN · {copy.englishReference}
                      </button>
                    </fieldset>
                    <button
                      className="dub-analyze-level"
                      type="button"
                      disabled={gainDirty || recordingState !== "idle" || workspaceBusy || analysisBusy}
                      title={gainDirty ? copy.saveGainFirst : undefined}
                      onClick={() => void analyzeLevel()}
                    >
                      {analysisBusy ? (
                        <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                      ) : (
                        <Waveform size={16} aria-hidden="true" />
                      )}
                      {analysisBusy ? copy.analyzingLevels : copy.analyzeLevels}
                    </button>
                  </div>

                  <div className="dub-level-metrics" aria-live="polite">
                    <div>
                      <span>{copy.takeActiveLevel}</span>
                      <strong>{currentLevelAnalysis ? formatDbfs(currentLevelAnalysis.take.activeRmsDbfs) : copy.notMeasured}</strong>
                      <small>{copy.activeRms}</small>
                    </div>
                    <div>
                      <span>{copy.referenceActiveLevel}</span>
                      <strong>{currentLevelAnalysis ? formatDbfs(currentLevelAnalysis.reference.activeRmsDbfs) : copy.notMeasured}</strong>
                      <small>{referenceLanguage === "japanese" ? "JP" : "EN"} · {copy.activeRms}</small>
                    </div>
                    <div>
                      <span>{copy.processedActiveLevel}</span>
                      <strong>{currentLevelAnalysis && !gainDirty ? formatDbfs(currentLevelAnalysis.processed.activeRmsDbfs) : copy.notMeasured}</strong>
                      <small>{copy.activeRms}</small>
                    </div>
                    <div>
                      <span>{copy.currentGain}</span>
                      <strong>{formatDb(currentCue.progress.gainDb, true)}</strong>
                      <small>{activeTake.id}</small>
                    </div>
                    <div>
                      <span>{copy.suggestedGain}</span>
                      <strong>{currentLevelAnalysis ? formatDb(currentLevelAnalysis.recommendedGainDb, true) : copy.notMeasured}</strong>
                      <small>{currentLevelAnalysis?.headroomLimited ? "−1 dBFS" : "RMS"}</small>
                    </div>
                    <div className={peakRisk ? "has-warning" : ""}>
                      <span>{copy.finalPeak}</span>
                      <strong>{finalPeak === null ? copy.notMeasured : formatDbfs(finalPeak, 2)}</strong>
                      <small>
                        {processedClipping
                          ? copy.clipping
                          : finalPeak === null
                            ? copy.notMeasured
                            : peakRisk
                              ? "> −1 dBFS"
                              : "≤ −1 dBFS"}
                      </small>
                    </div>
                  </div>

                  {currentLevelAnalysis?.unavailableReason && (
                    <div className="dub-level-message is-warning" role="status">
                      <Warning size={16} aria-hidden="true" />
                      <span>
                        {currentLevelAnalysis.unavailableReason === "take_silent"
                          ? copy.silentTake
                          : copy.silentReference}
                      </span>
                    </div>
                  )}

                  <div className="dub-gain-controls">
                    <label className="dub-gain-range">
                      <span>{copy.gain}</span>
                      <input
                        type="range"
                        min={MIN_GAIN_DB}
                        max={MAX_GAIN_DB}
                        step="0.1"
                        value={gainDraft}
                        disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
                        onChange={(event) => changeGainDraft(event.currentTarget.valueAsNumber)}
                      />
                      <small>{copy.gainHelp}</small>
                    </label>
                    <label className="dub-gain-number">
                      <span className="dub-sr-only">{copy.gain}</span>
                      <input
                        type="number"
                        min={MIN_GAIN_DB}
                        max={MAX_GAIN_DB}
                        step="0.1"
                        value={gainDraft}
                        disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
                        onChange={(event) => changeGainDraft(event.currentTarget.valueAsNumber)}
                      />
                      <span>dB</span>
                    </label>
                    <button
                      className="dub-apply-level"
                      type="button"
                      disabled={
                        currentLevelAnalysis?.recommendedGainDb === null ||
                        currentLevelAnalysis?.recommendedGainDb === undefined ||
                        recordingState !== "idle" ||
                        workspaceBusy ||
                        savingTarget
                      }
                      onClick={() => void applySuggestedGain()}
                    >
                      {processingBusy ? (
                        <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                      ) : (
                        <ArrowsLeftRight size={16} aria-hidden="true" />
                      )}
                      {processingBusy ? copy.applyingGain : copy.applySuggestedGain}
                    </button>
                  </div>
                  {gainDirty && <span className="dub-gain-unsaved">{copy.saveGainFirst}</span>}

                  {(gainDirty && currentCue.progress.status === "approved") && (
                    <div className="dub-level-message is-warning" role="status">
                      <Warning size={16} aria-hidden="true" />
                      <span>{copy.approvalReset}</span>
                    </div>
                  )}
                  {currentLevelAnalysis?.headroomLimited && (
                    <div className="dub-level-message" role="status">
                      <CheckCircle size={16} aria-hidden="true" />
                      <span>{copy.headroomLimited}</span>
                    </div>
                  )}
                  {currentLevelAnalysis?.gainLimited && (
                    <div className="dub-level-message is-warning" role="status">
                      <Warning size={16} aria-hidden="true" />
                      <span>
                        {currentLevelAnalysis.gainLimit === "minimum"
                          ? copy.minimumGainLimited
                          : copy.maximumGainLimited}
                      </span>
                    </div>
                  )}
                  {peakRisk && (
                    <div className="dub-level-message is-danger" role="alert">
                      <Warning size={16} aria-hidden="true" />
                      <span>{copy.peakRisk}</span>
                    </div>
                  )}

                  <div className="dub-audio-compare" role="group" aria-label={copy.compareAudio}>
                    <span>{copy.compareAudio}</span>
                    <button
                      type="button"
                      aria-pressed={playingAudioKey === rawPlaybackKey}
                      aria-label={playingAudioKey === rawPlaybackKey ? copy.pauseAudio : copy.playRawTake}
                      disabled={workspaceBusy || savingTarget || recordingState !== "idle" || Boolean(audioBusyKey && audioBusyKey !== rawPlaybackKey)}
                      onClick={() => void playTake(activeTake)}
                    >
                      {audioBusyKey === rawPlaybackKey ? (
                        <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                      ) : playingAudioKey === rawPlaybackKey ? (
                        <Pause size={16} weight="fill" aria-hidden="true" />
                      ) : (
                        <Play size={16} weight="fill" aria-hidden="true" />
                      )}
                      {copy.rawTake}
                    </button>
                    <button
                      type="button"
                      aria-pressed={playingAudioKey === processedPlaybackKey}
                      aria-label={playingAudioKey === processedPlaybackKey ? copy.pauseAudio : copy.playFinalDs}
                      disabled={gainDirty || workspaceBusy || savingTarget || recordingState !== "idle" || Boolean(audioBusyKey && audioBusyKey !== processedPlaybackKey)}
                      title={gainDirty ? copy.saveGainFirst : undefined}
                      onClick={() => void playProcessedPreview()}
                    >
                      {audioBusyKey === processedPlaybackKey ? (
                        <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                      ) : playingAudioKey === processedPlaybackKey ? (
                        <Pause size={16} weight="fill" aria-hidden="true" />
                      ) : (
                        <Play size={16} weight="fill" aria-hidden="true" />
                      )}
                      {copy.finalDs}
                    </button>
                    <button
                      type="button"
                      aria-pressed={playingAudioKey === referencePlaybackKey}
                      aria-label={playingAudioKey === referencePlaybackKey ? copy.pauseAudio : copy.playReference}
                      disabled={workspaceBusy || savingTarget || recordingState !== "idle" || Boolean(audioBusyKey && audioBusyKey !== referencePlaybackKey)}
                      onClick={() => void playReferencePreview()}
                    >
                      {audioBusyKey === referencePlaybackKey ? (
                        <SpinnerGap className="dub-spin" size={16} aria-hidden="true" />
                      ) : playingAudioKey === referencePlaybackKey ? (
                        <Pause size={16} weight="fill" aria-hidden="true" />
                      ) : (
                        <Play size={16} weight="fill" aria-hidden="true" />
                      )}
                      {copy.originalReference} · {referenceLanguage === "japanese" ? "JP" : "EN"}
                    </button>
                  </div>
                </>
              )}

              {batchResult && (
                <div className="dub-batch-result" role="status">
                  <CheckCircle size={17} weight="fill" aria-hidden="true" />
                  <strong>{batchResult.matched.toLocaleString()} {copy.matched}</strong>
                  <span>{batchResult.headroomLimited.toLocaleString()} {copy.limitedForHeadroom}</span>
                  <span>{batchResult.gainLimited.toLocaleString()} {copy.limitedByGain}</span>
                  <span>{batchResult.skipped.toLocaleString()} {copy.skippedDuringMatch}</span>
                </div>
              )}
            </section>

            <div className="dub-lower-grid">
              <section className="dub-takes-panel" aria-labelledby="dub-takes-title">
                <div className="dub-section-title">
                  <span>04</span>
                  <h3 id="dub-takes-title">
                    {currentCue.progress.takes.length} {currentCue.progress.takes.length === 1 ? copy.take : copy.takes}
                  </h3>
                </div>
                {currentCue.progress.takes.length === 0 ? (
                  <div className="dub-empty-takes">
                    <Headphones size={24} aria-hidden="true" />
                    {copy.noTakes}
                  </div>
                ) : (
                  <div className="dub-takes-list">
                    {[...currentCue.progress.takes].reverse().map((take, reverseIndex) => {
                      const isActive = take.id === currentCue.progress.activeTake;
                      const takeAudioKey = rawAudioKey(currentCue.symbol, take.id);
                      const isPlaying = takeAudioKey === playingAudioKey;
                      return (
                        <article key={take.id} className={isActive ? "is-active" : ""}>
                          <div className="dub-take-number">
                            <strong>{currentCue.progress.takes.length - reverseIndex}</strong>
                            <span>{formatDuration(take.durationMs)}</span>
                          </div>
                          <div className="dub-take-data">
                            <span>{new Date(take.createdAtMs).toLocaleString(language)}</span>
                            <small>{take.peakDbfs.toFixed(1)} dBFS{take.clippedSamples ? ` · ${copy.clipping}` : ""}</small>
                          </div>
                          <button
                            type="button"
                            className="dub-play-take"
                            aria-label={isPlaying ? copy.pauseTake : copy.playTake}
                            disabled={workspaceBusy || savingTarget || recordingState !== "idle" || Boolean(audioBusyKey && audioBusyKey !== takeAudioKey)}
                            onClick={() => void playTake(take)}
                          >
                            {audioBusyKey === takeAudioKey ? (
                              <SpinnerGap className="dub-spin" size={16} />
                            ) : isPlaying ? (
                              <Pause size={16} weight="fill" />
                            ) : (
                              <Play size={16} weight="fill" />
                            )}
                          </button>
                          {isActive ? (
                            <span className="dub-active-take"><CheckCircle size={15} weight="fill" /> {copy.activeTake}</span>
                          ) : (
                            <button
                              className="dub-select-take"
                              type="button"
                              disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
                              onClick={() => void selectTake(take)}
                            >
                              {copy.selectTake}
                            </button>
                          )}
                        </article>
                      );
                    })}
                  </div>
                )}
              </section>

              <section className="dub-notes-panel" aria-labelledby="dub-notes-title">
                <div className="dub-section-title">
                  <span>05</span>
                  <h3 id="dub-notes-title">{copy.notes}</h3>
                </div>
                <label className="dub-field">
                  <span>{copy.status}</span>
                  <select
                    value={statusDraft}
                    disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
                    onChange={(event) => setStatusDraft(event.currentTarget.value as TargetStatus)}
                  >
                    {STATUS_ORDER.map((status) => (
                      <option
                        key={status}
                        value={status}
                        disabled={
                          (status === "missing" && Boolean(activeTake)) ||
                          (!activeTake && !["missing", "skipped"].includes(status))
                        }
                      >
                        {cueStatusLabels[status]}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="dub-field dub-notes-field">
                  <span>{copy.notes}</span>
                  <textarea
                    value={notesDraft}
                    maxLength={4_000}
                    disabled={recordingState !== "idle" || workspaceBusy || savingTarget}
                    placeholder={copy.notesPlaceholder}
                    onChange={(event) => setNotesDraft(event.currentTarget.value)}
                  />
                  <small>{notesDraft.length.toLocaleString()} / 4,000</small>
                </label>
                <button
                  className="dub-save-target"
                  type="button"
                  disabled={!targetDirty || savingTarget || recordingState !== "idle" || workspaceBusy}
                  onClick={() => void saveTarget()}
                >
                  {savingTarget ? <SpinnerGap className="dub-spin" size={16} /> : <FloppyDisk size={16} />}
                  {copy.save}
                </button>
                {targetDirty && <span className="dub-unsaved">{copy.unsaved}</span>}
              </section>
            </div>
          </main>
        ) : (
          <main className="dub-editor dub-empty-editor">{copy.noResults}</main>
        )}
      </div>

      {batchConfirmationOpen && (
        <div className="dub-confirm-backdrop">
          <section
            className="dub-confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="dub-match-all-confirm-title"
            aria-describedby="dub-match-all-confirm-body dub-match-all-confirm-warning"
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                closeMatchAllConfirmation();
                return;
              }
              if (event.key !== "Tab") return;
              const focusable = Array.from(
                event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"),
              );
              if (focusable.length === 0) return;
              const first = focusable[0];
              const last = focusable[focusable.length - 1];
              if (event.shiftKey && document.activeElement === first) {
                event.preventDefault();
                last.focus();
              } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault();
                first.focus();
              }
            }}
          >
            <div className="dub-confirm-heading">
              <ArrowsLeftRight size={22} aria-hidden="true" />
              <div>
                <span>{copy.audioBalance}</span>
                <h2 id="dub-match-all-confirm-title">{copy.matchAllConfirmTitle}</h2>
              </div>
            </div>
            <p id="dub-match-all-confirm-body">{copy.matchAllConfirm}</p>
            <div className="dub-confirm-reference">
              <span>{copy.referenceVoice}</span>
              <strong>
                {referenceLanguage === "japanese"
                  ? `JP · ${copy.japaneseReference}`
                  : `EN · ${copy.englishReference}`}
              </strong>
            </div>
            <div
              className="dub-confirm-warning"
              id="dub-match-all-confirm-warning"
            >
              <Warning size={18} aria-hidden="true" />
              <span>{copy.matchAllApprovalWarning}</span>
            </div>
            <div className="dub-confirm-actions">
              <button
                className="dub-confirm-cancel"
                ref={batchCancelButtonRef}
                type="button"
                onClick={closeMatchAllConfirmation}
              >
                {copy.cancelMatchAll}
              </button>
              <button
                className="dub-confirm-submit"
                type="button"
                disabled={batchBusy || savingTarget || batchRunRef.current}
                onClick={() => void matchAllLevels()}
              >
                <ArrowsLeftRight size={16} aria-hidden="true" />
                {copy.confirmMatchAll}
              </button>
            </div>
          </section>
        </div>
      )}

      <footer className="dub-footer">
        <span><Circle size={8} weight="fill" aria-hidden="true" /> {snapshot.manifest.gameCode}</span>
        <span>{copy.source}: {fileName(snapshot.manifestPath)}</span>
        <span>{copy.shortcutHelp}</span>
      </footer>
    </div>
  );
}
