import {
  memo,
  useCallback,
  useDeferredValue,
  useEffect,
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
  DubbingProjectSnapshot,
  DubbingProjectSummary,
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
    save: "Enregistrer les informations",
    saved: "Informations enregistrées.",
    saveError: "Les informations n’ont pas pu être enregistrées.",
    unsaved: "Modifications non enregistrées",
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
    save: "Save line details",
    saved: "Line details saved.",
    saveError: "Line details could not be saved.",
    unsaved: "Unsaved changes",
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
  const [playingTakeId, setPlayingTakeId] = useState("");
  const [audioBusyTakeId, setAudioBusyTakeId] = useState("");
  const [notesDraft, setNotesDraft] = useState("");
  const [statusDraft, setStatusDraft] = useState<TargetStatus>("missing");
  const [savingTarget, setSavingTarget] = useState(false);
  const [buildBusy, setBuildBusy] = useState(false);
  const [buildProgress, setBuildProgress] = useState<DubbingBuildProgress | null>(null);
  const [buildResult, setBuildResult] = useState<TestRomBuildResult | null>(null);
  const [error, setError] = useState("");
  const [announcement, setAnnouncement] = useState("");
  const [lastRecording, setLastRecording] = useState<DubbingTakeResult["summary"] | null>(null);
  const audioRef = useRef<{ audio: HTMLAudioElement; url: string; takeId: string } | null>(null);
  const audioRequestRef = useRef(0);
  const mountedRef = useRef(true);
  const recordingRef = useRef(false);
  const recordingSymbolRef = useRef("");
  const recordingTokenRef = useRef(0);
  const recordingStartupRef = useRef<Promise<void> | null>(null);
  const savingTargetRef = useRef(false);

  const currentCue = useMemo(
    () => snapshot?.cues.find((cue) => cue.symbol === selectedSymbol) ?? null,
    [selectedSymbol, snapshot],
  );

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
  const notesDirty = currentCue
    ? notesDraft !== currentCue.progress.notes || statusDraft !== visibleStatus(currentCue)
    : false;
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
    setPlayingTakeId("");
    setAudioBusyTakeId("");
  }, []);

  const replaceProgress = useCallback((symbol: string, progress: TargetProgress) => {
    const normalized = normalizeProgress(progress);
    // A derived ROM represents one exact set of active takes. Any project mutation makes the
    // previous success card misleading even when its output file still exists on disk.
    setBuildResult(null);
    setBuildProgress(null);
    setLastRecording(null);
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
      if (!project || !cue || !notesDirty) return true;
      if (
        savingTargetRef.current ||
        recordingState !== "idle" ||
        buildBusy
      ) {
        return false;
      }

      savingTargetRef.current = true;
      setSavingTarget(true);
      try {
        const progress = await invoke<TargetProgress>("update_dubbing_target", {
          projectDir: project.projectDir,
          symbol: cue.symbol,
          status: statusDraft,
          notes: notesDraft,
        });
        if (!mountedRef.current) return false;
        replaceProgress(cue.symbol, progress);
        if (announce) setAnnouncement(copy.saved);
        setError("");
        return true;
      } catch (caught) {
        if (mountedRef.current) {
          setError(`${copy.saveError} ${rawError(caught)}`);
        }
        return false;
      } finally {
        savingTargetRef.current = false;
        if (mountedRef.current) setSavingTarget(false);
      }
    },
    [
      buildBusy,
      copy.saveError,
      copy.saved,
      currentCue,
      notesDirty,
      notesDraft,
      recordingState,
      replaceProgress,
      snapshot,
      statusDraft,
    ],
  );

  const selectCue = useCallback(
    async (symbol: string): Promise<boolean> => {
      if (symbol === selectedSymbol) return true;
      if (
        recordingState !== "idle" ||
        recordingRef.current ||
        savingTargetRef.current ||
        buildBusy
      ) {
        return false;
      }
      if (!(await persistCurrentDraft(false)) || !mountedRef.current) return false;
      stopPlayback();
      setSelectedSymbol(symbol);
      setError("");
      setLastRecording(null);
      return true;
    },
    [
      buildBusy,
      persistCurrentDraft,
      recordingState,
      selectedSymbol,
      stopPlayback,
    ],
  );

  const navigate = useCallback(
    (direction: -1 | 1) => {
      if (!filteredCues.length || recordingState !== "idle" || buildBusy) return;
      const origin = filteredPosition < 0 ? (direction > 0 ? -1 : 0) : filteredPosition;
      const next = Math.min(filteredCues.length - 1, Math.max(0, origin + direction));
      void selectCue(filteredCues[next].symbol);
    },
    [buildBusy, filteredCues, filteredPosition, recordingState, selectCue],
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
      buildBusy
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
    onExclusiveOperationChange?.(true);
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
      onExclusiveOperationChange?.(false);
    } finally {
      if (recordingStartupRef.current === startupSettled) {
        recordingStartupRef.current = null;
      }
      settleStartup();
    }
  }, [
    buildBusy,
    copy.recordError,
    copy.recording,
    currentCue,
    deviceId,
    onExclusiveOperationChange,
    persistCurrentDraft,
    recordingState,
    snapshot,
    stopPlayback,
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
        onExclusiveOperationChange?.(false);
      }
    }
  }, [
    copy.peak,
    copy.recordedAnnouncement,
    copy.stopError,
    onExclusiveOperationChange,
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
        onExclusiveOperationChange?.(false);
      }
    }
  }, [copy.cancelledAnnouncement, onExclusiveOperationChange, recordingState]);

  const playTake = useCallback(
    async (take: TakeMetadata) => {
      if (!snapshot || !currentCue || recordingState !== "idle" || buildBusy) return;
      if (audioRef.current?.takeId === take.id) {
        if (audioRef.current.audio.paused) {
          await audioRef.current.audio.play();
          setPlayingTakeId(take.id);
        } else {
          audioRef.current.audio.pause();
          setPlayingTakeId("");
        }
        return;
      }

      stopPlayback();
      setAudioBusyTakeId(take.id);
      const request = audioRequestRef.current;
      try {
        const bytes = await new Promise<ArrayBuffer>((resolve, reject) => {
          const audioChannel = new Channel<ArrayBuffer>();
          audioChannel.onmessage = resolve;
          void invoke<void>("read_dubbing_take", {
            projectDir: snapshot.projectDir,
            symbol: currentCue.symbol,
            takeId: take.id,
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
        audioRef.current = { audio, url, takeId: take.id };
        audio.addEventListener("ended", () => {
          if (audioRef.current?.audio === audio) setPlayingTakeId("");
        });
        audio.addEventListener("error", () => {
          if (audioRef.current?.audio === audio) {
            setError(copy.readError);
            stopPlayback();
          }
        });
        await audio.play();
        setPlayingTakeId(take.id);
      } catch (caught) {
        stopPlayback();
        setError(`${copy.readError} ${rawError(caught)}`);
      } finally {
        setAudioBusyTakeId("");
      }
    },
    [buildBusy, copy.readError, currentCue, recordingState, snapshot, stopPlayback],
  );

  const selectTake = useCallback(
    async (take: TakeMetadata) => {
      const project = snapshot;
      const cue = currentCue;
      if (!project || !cue || recordingState !== "idle" || buildBusy) return;
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
      buildBusy,
      copy.activeTake,
      copy.selectError,
      currentCue,
      persistCurrentDraft,
      recordingState,
      replaceProgress,
      snapshot,
    ],
  );

  const saveTarget = useCallback(async () => {
    await persistCurrentDraft(true);
  }, [persistCurrentDraft]);

  const buildTestRom = useCallback(async () => {
    const project = snapshot;
    if (
      !project ||
      buildBusy ||
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
    onExclusiveOperationChange?.(true);
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
        onExclusiveOperationChange?.(false);
      }
    }
  }, [
    buildBusy,
    copy.buildDialog,
    copy.buildError,
    copy.buildDone,
    copy.buildingTest,
    copy.romFilter,
    onExclusiveOperationChange,
    persistCurrentDraft,
    recordingState,
    snapshot,
    stopPlayback,
  ]);

  const clearProjectSession = useCallback(() => {
    stopPlayback();
    setBuildProgress(null);
    setBuildResult(null);
    setLastRecording(null);
    setError("");
    setSearch("");
    setFilter("all");
  }, [stopPlayback]);

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
    if (recordingState !== "idle" || buildBusy) return;
    if (!(await persistCurrentDraft(false)) || !mountedRef.current) return;
    clearProjectSession();
    setSnapshot(null);
    setSelectedSymbol("");
  }, [buildBusy, clearProjectSession, persistCurrentDraft, recordingState]);

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
    if (!currentCue) return;
    setNotesDraft(currentCue.progress.notes);
    setStatusDraft(visibleStatus(currentCue));
  }, [
    currentCue?.progress.activeTake,
    currentCue?.progress.notes,
    currentCue?.progress.status,
    currentCue?.symbol,
  ]);

  useEffect(() => {
    onNavigationLockChange?.(notesDirty || savingTarget);
  }, [notesDirty, onNavigationLockChange, savingTarget]);

  useEffect(() => {
    if (!notesDirty) return;
    const protectDraft = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", protectDraft);
    return () => window.removeEventListener("beforeunload", protectDraft);
  }, [notesDirty]);

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
      if (!snapshot || buildBusy || event.metaKey || event.ctrlKey || event.altKey || isTypingTarget(event.target)) {
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
  }, [buildBusy, cancelRecording, currentCue, navigate, playTake, recordingState, snapshot, startRecording, stopRecording]);

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
          disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
            disabled={recordingState !== "idle" || buildBusy || savingTarget}
            placeholder={copy.searchShort}
            onChange={(event) => setSearch(event.currentTarget.value)}
          />
          {search && (
            <button
              type="button"
              aria-label={copy.close}
              disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
              disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
            disabled={recordingState !== "idle" || buildBusy || savingTarget || devicesBusy || devices.length === 0}
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
            disabled={recordingState !== "idle" || buildBusy || savingTarget || devicesBusy}
            onClick={() => void loadDevices()}
          >
            {devicesBusy ? <SpinnerGap className="dub-spin" size={15} /> : <Circle size={10} weight="fill" />}
          </button>
        </label>

        <button
          className="dub-build"
          type="button"
          disabled={recordingState !== "idle" || buildBusy || savingTarget || summary.recorded === 0}
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
          disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
                  disabled={filteredPosition <= 0 || recordingState !== "idle" || buildBusy || savingTarget}
                  onClick={() => navigate(-1)}
                >
                  <ArrowLeft size={18} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  aria-label={copy.next}
                  disabled={filteredPosition < 0 || filteredPosition >= filteredCues.length - 1 || recordingState !== "idle" || buildBusy || savingTarget}
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
                    disabled={!deviceId || recordingState !== "idle" || buildBusy || savingTarget}
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

            <div className="dub-lower-grid">
              <section className="dub-takes-panel" aria-labelledby="dub-takes-title">
                <div className="dub-section-title">
                  <span>03</span>
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
                      const isPlaying = take.id === playingTakeId;
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
                            disabled={buildBusy || savingTarget || recordingState !== "idle" || Boolean(audioBusyTakeId && audioBusyTakeId !== take.id)}
                            onClick={() => void playTake(take)}
                          >
                            {audioBusyTakeId === take.id ? (
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
                              disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
                  <span>04</span>
                  <h3 id="dub-notes-title">{copy.notes}</h3>
                </div>
                <label className="dub-field">
                  <span>{copy.status}</span>
                  <select
                    value={statusDraft}
                    disabled={recordingState !== "idle" || buildBusy || savingTarget}
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
                    disabled={recordingState !== "idle" || buildBusy || savingTarget}
                    placeholder={copy.notesPlaceholder}
                    onChange={(event) => setNotesDraft(event.currentTarget.value)}
                  />
                  <small>{notesDraft.length.toLocaleString()} / 4,000</small>
                </label>
                <button
                  className="dub-save-target"
                  type="button"
                  disabled={!notesDirty || savingTarget || recordingState !== "idle" || buildBusy}
                  onClick={() => void saveTarget()}
                >
                  {savingTarget ? <SpinnerGap className="dub-spin" size={16} /> : <FloppyDisk size={16} />}
                  {copy.save}
                </button>
                {notesDirty && <span className="dub-unsaved">{copy.unsaved}</span>}
              </section>
            </div>
          </main>
        ) : (
          <main className="dub-editor dub-empty-editor">{copy.noResults}</main>
        )}
      </div>

      <footer className="dub-footer">
        <span><Circle size={8} weight="fill" aria-hidden="true" /> {snapshot.manifest.gameCode}</span>
        <span>{copy.source}: {fileName(snapshot.manifestPath)}</span>
        <span>{copy.shortcutHelp}</span>
      </footer>
    </div>
  );
}
