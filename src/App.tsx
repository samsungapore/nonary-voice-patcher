import { useEffect, useMemo, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ArrowCounterClockwise,
  CaretRight,
  Check,
  CheckCircle,
  Circle,
  Copy,
  FileArrowUp,
  FolderOpen,
  GameController,
  Globe,
  SpeakerHigh,
  SpeakerSlash,
  SpinnerGap,
  Warning,
  Waveform,
  X,
} from "@phosphor-icons/react";
import logoImage from "./assets/999/logo.png";
import startSubImage from "./assets/999/start-sub.png";
import corridorImage from "./assets/999/flooded-corridor.png";
import doorImage from "./assets/999/numbered-door.png";
import musicTrack from "./assets/999/bgm-mystery.mp3";
import "./App.css";

type UiLanguage = "fr" | "en";
type VoiceLanguage = "japanese" | "english";
type RomPatchState =
  | "clean"
  | "japanese"
  | "english"
  | "legacy_voice_patch"
  | "unsupported";

interface Capabilities {
  appVersion: string;
}

interface RomInfo {
  path: string;
  title: string;
  gameCode: string;
  bytes: number;
  sha256: string;
  state: RomPatchState;
  compatible: boolean;
  detail: string;
}

interface PatchProgress {
  stage: string;
  completed: number;
  total: number;
  message: string;
}

interface PatchResult {
  outputPath: string;
  bytes: number;
  sha256: string;
  language: VoiceLanguage | null;
  voices: number;
  scripts: number;
  resetExact: boolean;
}

const COPY = {
  fr: {
    appName: "NONARY VOICE PATCHER",
    appSubtitle: "999 · Nintendo DS",
    audioOff: "Musique coupée",
    audioOn: "Musique active",
    chooseLanguage: "Langue de l’interface",
    navigation: "Commandes de l’application",
    heroEyebrow: "PATCH DOUBLAGE",
    heroTitle: "Doublage 999 sur Nintendo DS.",
    stepRom: "ROM 999",
    stepRomHint: "Version USA ou traduction compatible",
    chooseRom: "Choisir une ROM",
    stepVoices: "Langue des voix",
    japanese: "Japonais",
    english: "Anglais",
    reset: "Retirer les voix",
    create: "Créer la ROM",
    processing: "Traitement…",
    operationStopped: "Opération interrompue",
    restored: "Voix retirées",
    patched: "ROM créée",
    hashCopied: "Copié",
    copyHash: "Copier le hash",
    openFolder: "Afficher",
    selectRomTitle: "Choisir la ROM 999",
    saveRomTitle: "Enregistrer la ROM",
    resetRomTitle: "Enregistrer la ROM restaurée",
    japaneseFileSuffix: "[Voix japonaises]",
    englishFileSuffix: "[Voix anglaises]",
    resetFileSuffix: "[Voix retirées]",
    romFilter: "ROM Nintendo DS",
    progressLabel: "Progression du patch",
    removeRom: "Retirer la ROM",
    close: "Fermer",
  },
  en: {
    appName: "NONARY VOICE PATCHER",
    appSubtitle: "999 · Nintendo DS",
    audioOff: "Music muted",
    audioOn: "Music playing",
    chooseLanguage: "Interface language",
    navigation: "Application controls",
    heroEyebrow: "VOICE DUBBING",
    heroTitle: "999 dubbing on Nintendo DS.",
    stepRom: "999 ROM",
    stepRomHint: "USA release or compatible translation",
    chooseRom: "Choose a ROM",
    stepVoices: "Voice language",
    japanese: "Japanese",
    english: "English",
    reset: "Remove voices",
    create: "Create ROM",
    processing: "Processing…",
    operationStopped: "Operation stopped",
    restored: "Voices removed",
    patched: "ROM created",
    hashCopied: "Copied",
    copyHash: "Copy hash",
    openFolder: "Show file",
    selectRomTitle: "Choose the 999 ROM",
    saveRomTitle: "Save the ROM",
    resetRomTitle: "Save the restored ROM",
    japaneseFileSuffix: "[Japanese Voices]",
    englishFileSuffix: "[English Voices]",
    resetFileSuffix: "[Voices Removed]",
    romFilter: "Nintendo DS ROM",
    progressLabel: "Patch progress",
    removeRom: "Remove ROM",
    close: "Close",
  },
} as const;

const fallbackCapabilities: Capabilities = { appVersion: "0.1.0" };

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function outputName(path: string, suffix: string): string {
  return `${path.replace(/\.nds$/i, "")} ${suffix}.nds`;
}

function rawError(caught: unknown): string {
  if (typeof caught === "string") return caught;
  if (caught instanceof Error) return caught.message;
  try {
    return JSON.stringify(caught);
  } catch {
    return String(caught);
  }
}

function localizedError(caught: unknown, language: UiLanguage): string {
  const raw = rawError(caught);
  if (language === "en") return raw;
  const lower = raw.toLocaleLowerCase();
  // Backend diagnostics stay precise and English; the French UI groups them
  // by user action so implementation wording does not become a second API.
  if (lower.includes("output file must be different"))
    return "La sortie doit être différente de la ROM source.";
  if (lower.includes("invalid nintendo ds rom") || lower.includes("unrecognized file"))
    return "Ce fichier n’est pas une ROM Nintendo DS valide.";
  if (lower.includes("script") && lower.includes("incompatible"))
    return "La structure de cette ROM n’est pas compatible.";
  if (lower.includes("no nonary voice patcher patch detected"))
    return "Aucun patch vocal réversible n’a été trouvé.";
  if (lower.includes("legacy voice patch"))
    return "Un ancien patch vocal non réversible a été détecté.";
  if (lower.includes("voice pack") || lower.includes("voice profile") || lower.includes("resource"))
    return "Les ressources vocales requises sont absentes ou invalides.";
  if (lower.includes("cannot access") || lower.includes("failed to create"))
    return "Le fichier ne peut pas être lu ou écrit. Vérifiez ses autorisations.";
  if (lower.includes("512 mib"))
    return "La ROM dépasserait la taille maximale prise en charge.";
  return "L’opération a échoué. Vérifiez la ROM et la destination.";
}

function stateLabel(state: RomPatchState, language: UiLanguage): string {
  const labels = {
    fr: {
      clean: "Compatible",
      japanese: "Voix japonaises",
      english: "Voix anglaises",
      legacy_voice_patch: "Ancien patch",
      unsupported: "Non compatible",
    },
    en: {
      clean: "Compatible",
      japanese: "Japanese voices",
      english: "English voices",
      legacy_voice_patch: "Legacy patch",
      unsupported: "Unsupported",
    },
  } as const;
  return labels[language][state];
}

function progressMessage(progress: PatchProgress, language: UiLanguage): string {
  const messages: Record<UiLanguage, Record<string, string>> = {
    fr: {
      read: "Lecture de la ROM…",
      inspect: "Analyse de la ROM…",
      prepare: "Préparation du patch…",
      scripts: "Modification des scripts…",
      voices: "Ajout des voix…",
      write: "Écriture de la ROM…",
      verify: "Vérification de la ROM…",
      reset: "Retrait des voix…",
      done: "Terminé.",
    },
    en: {
      read: "Reading the ROM…",
      inspect: "Inspecting the ROM…",
      prepare: "Preparing the patch…",
      scripts: "Patching scripts…",
      voices: "Adding voices…",
      write: "Writing the ROM…",
      verify: "Verifying the ROM…",
      reset: "Removing voices…",
      done: "Complete.",
    },
  };
  return messages[language][progress.stage] ?? (language === "fr" ? "Traitement…" : "Processing…");
}

function LanguageGate({ onSelect }: { onSelect: (language: UiLanguage) => void }) {
  return (
    <main className="language-gate">
      <img className="gate-texture" src={startSubImage} alt="" />
      <section className="gate-console" aria-labelledby="gate-title">
        <img
          className="gate-logo"
          src={logoImage}
          alt="999: Nine Hours, Nine Persons, Nine Doors"
        />
        <div className="gate-copy">
          <div className="gate-kicker" lang="en">SYSTEM / LANGUAGE</div>
          <h1 id="gate-title" lang="en">Choose your language</h1>
          <p>Choisissez votre langue.</p>
        </div>
        <div className="gate-actions">
          <button onClick={() => onSelect("fr")} type="button">
            <span>FR</span><strong>Français</strong><CaretRight size={20} weight="bold" />
          </button>
          <button onClick={() => onSelect("en")} type="button">
            <span>EN</span><strong>English</strong><CaretRight size={20} weight="bold" />
          </button>
        </div>
      </section>
    </main>
  );
}

function App() {
  // QA state is development-only so a screenshot shortcut cannot change the
  // startup behavior shipped to users.
  const qaParameters = import.meta.env.DEV
    ? new URLSearchParams(window.location.search)
    : null;
  const qaLanguage = qaParameters?.get("qa-language");
  const [uiLanguage, setUiLanguage] = useState<UiLanguage | null>(
    qaLanguage === "fr" || qaLanguage === "en" ? qaLanguage : null,
  );
  const [capabilities, setCapabilities] = useState<Capabilities>(fallbackCapabilities);
  const [romPath, setRomPath] = useState<string | null>(null);
  const [romInfo, setRomInfo] = useState<RomInfo | null>(null);
  const [voiceLanguage, setVoiceLanguage] = useState<VoiceLanguage>("japanese");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<PatchProgress | null>(null);
  const [result, setResult] = useState<PatchResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [audioEnabled, setAudioEnabled] = useState(false);
  const audioRef = useRef<HTMLAudioElement>(null);
  const japaneseVoiceRef = useRef<HTMLButtonElement>(null);
  const englishVoiceRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    document.documentElement.lang = uiLanguage ?? "fr";
  }, [uiLanguage]);

  useEffect(() => {
    // Native events are unavailable in browser builds, so fallback capabilities
    // avoid registering a listener against a missing bridge.
    if (!isTauri()) {
      setCapabilities(fallbackCapabilities);
      return;
    }

    invoke<Capabilities>("get_capabilities")
      .then(setCapabilities)
      .catch(() => setCapabilities(fallbackCapabilities));
    const unlisten = listen<PatchProgress>("patch-progress", (event) => {
      setProgress(event.payload);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.volume = 0.15;
    if (audioEnabled) {
      void audio.play().catch(() => setAudioEnabled(false));
    } else {
      audio.pause();
    }
  }, [audioEnabled]);

  const language = uiLanguage ?? "fr";
  const t = COPY[language];
  const canPatch = Boolean(romInfo?.compatible && !busy);
  const canReset = Boolean(
    romInfo &&
      (romInfo.state === "japanese" || romInfo.state === "english") &&
      !busy,
  );
  const progressPercent = useMemo(() => {
    if (!progress || progress.total <= 0) return 0;
    return Math.min(100, Math.round((progress.completed / progress.total) * 100));
  }, [progress]);

  async function inspectRom(path: string) {
    setBusy(true);
    setError(null);
    setResult(null);
    setProgress({ stage: "inspect", completed: 0, total: 1, message: "Analyse de la ROM…" });
    try {
      const info = await invoke<RomInfo>("inspect_rom", { path });
      setRomPath(path);
      setRomInfo(info);
      if (!info.compatible) setError(localizedError(info.detail, language));
    } catch (caught) {
      setRomPath(path);
      setRomInfo(null);
      setError(localizedError(caught, language));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  async function chooseRom() {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: t.selectRomTitle,
        filters: [{ name: t.romFilter, extensions: ["nds"] }],
      });
      if (typeof selected === "string") await inspectRom(selected);
    } catch (caught) {
      setError(localizedError(caught, language));
    }
  }

  function clearRom() {
    if (busy) return;
    setRomPath(null);
    setRomInfo(null);
    setResult(null);
    setError(null);
  }

  async function applyPatch() {
    if (!romPath || !romInfo?.compatible) return;
    const suffix =
      voiceLanguage === "japanese"
        ? t.japaneseFileSuffix
        : t.englishFileSuffix;
    let outputPath: string | null;
    try {
      outputPath = await save({
        title: t.saveRomTitle,
        defaultPath: outputName(romPath, suffix),
        filters: [{ name: t.romFilter, extensions: ["nds"] }],
      });
    } catch (caught) {
      setError(localizedError(caught, language));
      return;
    }
    if (!outputPath) return;
    setBusy(true);
    setError(null);
    setResult(null);
    setProgress({ stage: "prepare", completed: 0, total: 1, message: "Préparation du patch…" });
    try {
      const patchResult = await invoke<PatchResult>("apply_voice_patch", {
        inputPath: romPath,
        outputPath,
        language: voiceLanguage,
      });
      setResult(patchResult);
    } catch (caught) {
      setError(localizedError(caught, language));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  async function resetPatch() {
    if (!romPath || !canReset) return;
    let outputPath: string | null;
    try {
      outputPath = await save({
        title: t.resetRomTitle,
        defaultPath: outputName(romPath, t.resetFileSuffix),
        filters: [{ name: t.romFilter, extensions: ["nds"] }],
      });
    } catch (caught) {
      setError(localizedError(caught, language));
      return;
    }
    if (!outputPath) return;
    setBusy(true);
    setError(null);
    setResult(null);
    setProgress({ stage: "reset", completed: 0, total: 1, message: "Retrait des voix…" });
    try {
      const patchResult = await invoke<PatchResult>("reset_voice_patch", {
        inputPath: romPath,
        outputPath,
      });
      setResult(patchResult);
    } catch (caught) {
      setError(localizedError(caught, language));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  async function copyHash() {
    if (!result?.sha256) return;
    try {
      await navigator.clipboard.writeText(result.sha256);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch (caught) {
      setError(localizedError(caught, language));
    }
  }

  async function reveal(path: string) {
    try {
      // Revealing the output keeps the permission narrow and selects the ROM
      // users just created instead of opening an unrelated default handler.
      await revealItemInDir(path);
    } catch (caught) {
      setError(localizedError(caught, language));
    }
  }

  function loopMusic() {
    const audio = audioRef.current;
    if (!audio || !audioEnabled) return;
    // The retail cue has a one-shot intro; returning to its authored loop point
    // avoids replaying that intro every cycle.
    audio.currentTime = 32.025;
    void audio.play();
  }

  function handleVoiceKey(event: React.KeyboardEvent<HTMLButtonElement>) {
    let next: VoiceLanguage | null = null;
    if (event.key === "ArrowLeft" || event.key === "ArrowUp" || event.key === "Home") {
      next = "japanese";
    } else if (event.key === "ArrowRight" || event.key === "ArrowDown" || event.key === "End") {
      next = "english";
    }
    if (!next) return;
    event.preventDefault();
    setVoiceLanguage(next);
    (next === "japanese" ? japaneseVoiceRef : englishVoiceRef).current?.focus();
  }

  if (uiLanguage === null) return <LanguageGate onSelect={setUiLanguage} />;

  return (
    <div className="app-shell">
      <audio onEnded={loopMusic} preload="metadata" ref={audioRef} src={musicTrack} />
      <aside className="scene-sidebar">
        <img className="scene-corridor" src={corridorImage} alt="" />
        <div className="scene-overlay" />
        <div className="sidebar-content">
          <div>
            <img className="sidebar-logo" src={logoImage} alt="999" />
            <div className="product-name">{t.appName}</div>
            <div className="product-subtitle">{t.appSubtitle}</div>
          </div>
          <div className="utility-actions" aria-label={t.navigation}>
            <button
              aria-pressed={audioEnabled}
              onClick={() => setAudioEnabled((value) => !value)}
              title={audioEnabled ? t.audioOn : t.audioOff}
              type="button"
            >
              {audioEnabled ? <SpeakerHigh size={18} /> : <SpeakerSlash size={18} />}
              <span>{audioEnabled ? t.audioOn : t.audioOff}</span>
            </button>
            <button
              aria-label={t.chooseLanguage}
              onClick={() => setUiLanguage(null)}
              title={t.chooseLanguage}
              type="button"
            >
              <Globe size={18} /><span>{uiLanguage.toUpperCase()}</span>
            </button>
          </div>
        </div>
      </aside>

      <section className="app-stage">
        <header className="topbar">
          <div><Circle size={7} weight="fill" /> NONARY SYSTEM</div>
          <div>BSKE / v{capabilities.appVersion}</div>
        </header>

        <main className="content voices-content">
          <section className="hero-card">
            <div className="hero-copy">
              <span>{t.heroEyebrow}</span>
              <h1>{t.heroTitle}</h1>
            </div>
            <div className="hero-visual">
              <img className="hero-door" src={doorImage} alt="" />
            </div>
          </section>

          <div className="steps-grid">
            <section className="console-card">
              <StepHeader index="01" title={t.stepRom} hint={t.stepRomHint} />
              {!romPath ? (
                <button className="file-picker" disabled={busy} onClick={chooseRom} type="button">
                  <span className="picker-icon"><FileArrowUp size={25} weight="duotone" /></span>
                  <span><strong>{t.chooseRom}</strong></span>
                  <CaretRight size={20} weight="bold" />
                </button>
              ) : (
                <div className={`selected-file ${romInfo?.compatible ? "valid" : "invalid"}`}>
                  <span className="picker-icon"><GameController size={24} weight="duotone" /></span>
                  <span className="file-copy">
                    <span className="file-name" title={romPath}>{fileName(romPath)}</span>
                    <small>
                      {romInfo
                        ? `${romInfo.gameCode} · ${formatBytes(romInfo.bytes)} · ${stateLabel(romInfo.state, language)}`
                        : t.operationStopped}
                    </small>
                  </span>
                  {romInfo && (
                    <span className={`state-badge ${romInfo.compatible ? "ok" : "bad"}`}>
                      {stateLabel(romInfo.state, language)}
                    </span>
                  )}
                  <button
                    aria-label={t.removeRom}
                    className="remove-button"
                    disabled={busy}
                    onClick={clearRom}
                    type="button"
                  ><X size={17} /></button>
                </div>
              )}
            </section>

            <section className="console-card">
              <StepHeader index="02" title={t.stepVoices} />
              <div className="voice-options" role="radiogroup" aria-label={t.stepVoices}>
                <button
                  aria-checked={voiceLanguage === "japanese"}
                  className={voiceLanguage === "japanese" ? "selected" : ""}
                  onClick={() => setVoiceLanguage("japanese")}
                  onKeyDown={handleVoiceKey}
                  ref={japaneseVoiceRef}
                  role="radio"
                  tabIndex={voiceLanguage === "japanese" ? 0 : -1}
                  type="button"
                >
                  <b>JP</b><span><strong>{t.japanese}</strong></span>
                  <i>{voiceLanguage === "japanese" && <Check size={12} weight="bold" />}</i>
                </button>
                <button
                  aria-checked={voiceLanguage === "english"}
                  className={voiceLanguage === "english" ? "selected" : ""}
                  onClick={() => setVoiceLanguage("english")}
                  onKeyDown={handleVoiceKey}
                  ref={englishVoiceRef}
                  role="radio"
                  tabIndex={voiceLanguage === "english" ? 0 : -1}
                  type="button"
                >
                  <b>EN</b><span><strong>{t.english}</strong></span>
                  <i>{voiceLanguage === "english" && <Check size={12} weight="bold" />}</i>
                </button>
              </div>
            </section>
          </div>

          {error && (
            <Alert
              closeLabel={t.close}
              text={error}
              title={t.operationStopped}
              onClose={() => setError(null)}
            />
          )}
          {busy && progress && (
            <section
              aria-label={t.progressLabel}
              aria-live="polite"
              aria-valuemax={100}
              aria-valuemin={0}
              aria-valuenow={progressPercent}
              className="progress-panel"
              role="progressbar"
            >
              <div>
                <SpinnerGap className="spin" size={21} weight="bold" />
                <strong>{progressMessage(progress, language)}</strong>
                <b>{progressPercent}%</b>
              </div>
              <i><span style={{ width: `${progressPercent}%` }} /></i>
            </section>
          )}
          {result && (
            <ResultPanel
              hash={result.sha256}
              meta={`${fileName(result.outputPath)} · ${formatBytes(result.bytes)}`}
              onCopy={copyHash}
              onReveal={() => reveal(result.outputPath)}
              copied={copied}
              title={result.resetExact ? t.restored : t.patched}
              t={t}
            />
          )}

          <footer className="action-bar">
            <button className="secondary-action" disabled={!canReset} onClick={resetPatch} type="button">
              <ArrowCounterClockwise size={18} weight="bold" /> {t.reset}
            </button>
            <button className="primary-action" disabled={!canPatch} onClick={applyPatch} type="button">
              {busy ? <SpinnerGap className="spin" size={19} /> : <Waveform size={19} weight="bold" />}
              {busy ? t.processing : t.create}
            </button>
          </footer>
        </main>
      </section>
    </div>
  );
}

function StepHeader({ index, title, hint }: { index: string; title: string; hint?: string }) {
  return (
    <div className="step-header">
      <span>{index}</span><div><h2>{title}</h2>{hint && <p>{hint}</p>}</div>
    </div>
  );
}

function Alert({
  title,
  text,
  closeLabel,
  onClose,
}: {
  title: string;
  text: string;
  closeLabel: string;
  onClose: () => void;
}) {
  return (
    <div className="alert-panel" role="alert">
      <Warning size={20} weight="fill" />
      <div><strong>{title}</strong><span>{text}</span></div>
      <button aria-label={closeLabel} onClick={onClose} type="button"><X size={16} /></button>
    </div>
  );
}

function ResultPanel({
  title,
  meta,
  hash,
  copied,
  onCopy,
  onReveal,
  t,
}: {
  title: string;
  meta: string;
  hash: string;
  copied: boolean;
  onCopy: () => void;
  onReveal: () => void;
  t: (typeof COPY)[UiLanguage];
}) {
  return (
    <section className="result-panel" aria-live="polite">
      <CheckCircle size={26} weight="fill" />
      <div><strong>{title}</strong><span>{meta}</span><code>SHA-256 {hash}</code></div>
      <button onClick={onCopy} type="button">
        {copied ? <Check size={16} /> : <Copy size={16} />}
        {copied ? t.hashCopied : t.copyHash}
      </button>
      <button onClick={onReveal} type="button"><FolderOpen size={16} />{t.openFolder}</button>
    </section>
  );
}

export default App;
