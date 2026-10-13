import { platform } from '@/utils/platform';
import { isTauriRuntime } from '@/utils/runtime';
import { isLocaleCode, type LocaleCode } from '@/locale/catalog';

export type TaskbarDockMode =
  | 'floating'
  | 'docked'
  | 'waiting'
  | 'docking'
  | 'undocking';

export interface TaskbarDockState {
  mode: TaskbarDockMode;
  suspended: boolean;
  dockError: string;
  enabled: boolean;
}

export interface TaskbarOverlaySnapshot {
  artwork: string;
  trackTitle: string;
  artistName: string;
  lyric: string;
  emptyKind: 'loading' | 'track' | null;
  emptyLabel: string;
  progress: number;
  duration: number;
  elapsedLabel: string;
  durationLabel: string;
  playing: boolean;
  shuffle: boolean;
  locale: LocaleCode;
  reducedMotion: boolean;
}

export type TaskbarOverlayAction =
  | { action: 'previous' }
  | { action: 'play' }
  | { action: 'next' }
  | { action: 'shuffle' }
  | { action: 'seek'; value: number };

export type TaskbarOverlayVisualPhase =
  | 'hidden'
  | 'snapping'
  | 'docked'
  | 'returning';

export interface TaskbarOverlayVisual {
  phase: TaskbarOverlayVisualPhase;
  token: number;
  durationMs?: number;
}

/** Feedback while the compact player is dragged near a taskbar. */
export type TaskbarDockHint = 'none' | 'ready' | 'unavailable';

export interface TaskbarOverlayReadyState {
  snapshot: TaskbarOverlaySnapshot | null;
  visual: TaskbarOverlayVisual;
}

export type TaskbarOverlayCommand =
  | TaskbarOverlayAction
  | { action: 'undock' }
  | { action: 'close' };

export type TaskbarDockCommand =
  | { command: 'set_taskbar_docking'; args: { enabled: boolean } }
  | { command: 'sync_taskbar_compact_mode'; args: { compact: boolean } };

type TaskbarDockInvoker = (request: TaskbarDockCommand) => Promise<unknown>;

export const DEFAULT_TASKBAR_DOCK_STATE: TaskbarDockState = {
  mode: 'floating',
  suspended: false,
  dockError: '',
  enabled: false,
};

let currentTaskbarDockState: TaskbarDockState = {
  ...DEFAULT_TASKBAR_DOCK_STATE,
};
const stateListeners = new Set<(state: TaskbarDockState) => void>();
let pendingTaskbarDockCommandCount = 0;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function normalizeTaskbarDockState(
  value: unknown
): TaskbarDockState | null {
  if (!isRecord(value)) return null;
  const mode = value['mode'];
  const suspended = value['suspended'];
  const dockError = value['dockError'];
  const enabled = value['enabled'];
  if (
    (mode !== 'floating' &&
      mode !== 'docked' &&
      mode !== 'waiting' &&
      mode !== 'docking' &&
      mode !== 'undocking') ||
    typeof suspended !== 'boolean' ||
    typeof dockError !== 'string' ||
    typeof enabled !== 'boolean'
  ) {
    return null;
  }
  return { mode, suspended, dockError, enabled };
}

export function applyTaskbarDockState(value: unknown): TaskbarDockState | null {
  const state = normalizeTaskbarDockState(value);
  if (!state) return null;
  currentTaskbarDockState = state;
  stateListeners.forEach(listener => listener(state));
  return state;
}

export function getTaskbarDockState(): TaskbarDockState {
  return { ...currentTaskbarDockState };
}

export function canPersistCompactWindowFrame(state: TaskbarDockState): boolean {
  return (
    pendingTaskbarDockCommandCount === 0 &&
    state.mode !== 'docking' &&
    state.mode !== 'undocking' &&
    !(state.enabled && (state.mode === 'docked' || state.mode === 'waiting'))
  );
}

export function hasTaskbarDockIntent(state: TaskbarDockState): boolean {
  return (
    state.enabled ||
    state.mode === 'docked' ||
    state.mode === 'waiting' ||
    state.mode === 'docking' ||
    state.mode === 'undocking'
  );
}

function isFiniteNonnegative(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

function isBoundedTaskbarOverlayText(
  value: unknown,
  maxBytes: number,
  allowLineBreaks = false
): value is string {
  if (
    typeof value !== 'string' ||
    new TextEncoder().encode(value).byteLength > maxBytes
  ) {
    return false;
  }
  return Array.from(value).every(character => {
    const codePoint = character.codePointAt(0) ?? 0;
    const isControl =
      codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
    return (
      !isControl ||
      (allowLineBreaks &&
        (character === '\n' || character === '\r' || character === '\t'))
    );
  });
}

function isBoundedOverlayTime(value: unknown): value is number {
  return isFiniteNonnegative(value) && value <= 86_400;
}

export function normalizeTaskbarOverlaySnapshot(
  value: unknown
): TaskbarOverlaySnapshot | null {
  if (!isRecord(value)) return null;
  const {
    artwork,
    trackTitle,
    artistName,
    lyric,
    emptyKind,
    emptyLabel,
    progress,
    duration,
    elapsedLabel,
    durationLabel,
    playing,
    shuffle,
    locale,
    reducedMotion,
  } = value;
  if (
    !isBoundedTaskbarOverlayText(artwork, 4096) ||
    !isBoundedTaskbarOverlayText(trackTitle, 512) ||
    !isBoundedTaskbarOverlayText(artistName, 512) ||
    !isBoundedTaskbarOverlayText(lyric, 2048, true) ||
    (emptyKind !== null && emptyKind !== 'loading' && emptyKind !== 'track') ||
    !isBoundedTaskbarOverlayText(emptyLabel, 256) ||
    !isBoundedOverlayTime(progress) ||
    !isBoundedOverlayTime(duration) ||
    !isBoundedTaskbarOverlayText(elapsedLabel, 32) ||
    !isBoundedTaskbarOverlayText(durationLabel, 32) ||
    typeof playing !== 'boolean' ||
    typeof shuffle !== 'boolean' ||
    !isLocaleCode(locale) ||
    typeof reducedMotion !== 'boolean'
  ) {
    return null;
  }

  return {
    artwork,
    trackTitle,
    artistName,
    lyric,
    emptyKind,
    emptyLabel,
    progress: Math.min(progress, duration),
    duration,
    elapsedLabel,
    durationLabel,
    playing,
    shuffle,
    locale,
    reducedMotion,
  };
}

export function normalizeTaskbarOverlayAction(
  value: unknown
): TaskbarOverlayAction | null {
  if (!isRecord(value)) return null;
  const action = value['action'];
  if (
    action === 'previous' ||
    action === 'play' ||
    action === 'next' ||
    action === 'shuffle'
  ) {
    return { action };
  }
  if (action === 'seek' && isBoundedOverlayTime(value['value'])) {
    return { action, value: value['value'] };
  }
  return null;
}

export function normalizeTaskbarOverlayVisual(
  value: unknown
): TaskbarOverlayVisual | null {
  if (!isRecord(value)) return null;
  const phase = value['phase'];
  const token = value['token'];
  const durationMs = value['durationMs'];
  if (
    (phase !== 'hidden' &&
      phase !== 'snapping' &&
      phase !== 'docked' &&
      phase !== 'returning') ||
    typeof token !== 'number' ||
    !Number.isSafeInteger(token) ||
    token < 0
  ) {
    return null;
  }
  return isFiniteNonnegative(durationMs) && durationMs <= 2_000
    ? { phase, token, durationMs }
    : { phase, token };
}

export function normalizeTaskbarDockHint(
  value: unknown
): TaskbarDockHint | null {
  if (!isRecord(value)) return null;
  const state = value['state'];
  return state === 'none' || state === 'ready' || state === 'unavailable'
    ? state
    : null;
}

export async function listenTaskbarDockHint(
  listener: (hint: TaskbarDockHint) => void
): Promise<() => void> {
  if (!isTaskbarDockingAvailable()) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<unknown>('desktop://taskbarDockHint', event => {
    const hint = normalizeTaskbarDockHint(event.payload);
    if (hint) listener(hint);
  });
}

export function normalizeTaskbarOverlayReadyState(
  value: unknown
): TaskbarOverlayReadyState | null {
  if (!isRecord(value)) return null;
  const snapshot = value['snapshot'];
  const visual = normalizeTaskbarOverlayVisual(value['visual']);
  if (!visual) return null;
  if (snapshot !== null && !normalizeTaskbarOverlaySnapshot(snapshot)) {
    return null;
  }
  return {
    snapshot:
      snapshot === null ? null : normalizeTaskbarOverlaySnapshot(snapshot),
    visual,
  };
}

export function shouldApplyTaskbarOverlayVisual(
  current: TaskbarOverlayVisual,
  next: TaskbarOverlayVisual
): boolean {
  return (
    next.token > current.token ||
    (next.token === current.token && next.phase !== current.phase)
  );
}

export function clampTaskbarOverlaySeek(
  value: number,
  duration: number
): number | null {
  if (!isBoundedOverlayTime(value) || !isBoundedOverlayTime(duration)) {
    return null;
  }
  if (duration === 0) return null;
  return Math.min(duration, value);
}

export async function publishTaskbarOverlaySnapshot(
  value: unknown
): Promise<void> {
  if (!isTaskbarDockingAvailable()) return;
  const snapshot = normalizeTaskbarOverlaySnapshot(value);
  if (!snapshot) return;
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('publish_taskbar_overlay_snapshot', { snapshot });
}

export async function listenTaskbarOverlayActions(
  listener: (action: TaskbarOverlayAction) => void
): Promise<() => void> {
  if (!isTaskbarDockingAvailable()) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<unknown>('desktop://taskbarOverlayAction', event => {
    const action = normalizeTaskbarOverlayAction(event.payload);
    if (action) listener(action);
  });
}

export async function listenTaskbarOverlayEvents(handlers: {
  onSnapshot(snapshot: TaskbarOverlaySnapshot): void;
  onVisual(visual: TaskbarOverlayVisual): void;
}): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  const unlistenSnapshot = await listen<unknown>(
    'desktop://taskbarOverlaySnapshot',
    event => {
      const snapshot = normalizeTaskbarOverlaySnapshot(event.payload);
      if (snapshot) handlers.onSnapshot(snapshot);
    }
  );
  try {
    const unlistenVisual = await listen<unknown>(
      'desktop://taskbarOverlayVisual',
      event => {
        const visual = normalizeTaskbarOverlayVisual(event.payload);
        if (visual) handlers.onVisual(visual);
      }
    );
    return () => {
      unlistenSnapshot();
      unlistenVisual();
    };
  } catch (error) {
    unlistenSnapshot();
    throw error;
  }
}

export async function getTaskbarOverlayReadyState(): Promise<unknown> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke('taskbar_overlay_ready');
}

export async function invokeTaskbarOverlayCommand(
  command: TaskbarOverlayCommand
): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('taskbar_overlay_action', command);
}

export async function acknowledgeTaskbarOverlayVisual(
  token: number
): Promise<boolean> {
  const { invoke } = await import('@tauri-apps/api/core');
  return (
    (await invoke<unknown>('taskbar_overlay_visual_ready', { token })) === true
  );
}

export function shouldShowTaskbarReturnButton(
  state: TaskbarDockState
): boolean {
  return state.mode === 'docked';
}

export function canOpenMainPlayerFromCompactDoubleClick(
  state: TaskbarDockState
): boolean {
  return pendingTaskbarDockCommandCount === 0 && !hasTaskbarDockIntent(state);
}

export function isTaskbarDockingAvailable(
  runtimePlatform: string = platform,
  tauriRuntime: boolean = isTauriRuntime
): boolean {
  return runtimePlatform === 'win32' && tauriRuntime;
}

export async function invokeTaskbarDockCommand(
  request: TaskbarDockCommand,
  invoke: TaskbarDockInvoker
): Promise<TaskbarDockState> {
  pendingTaskbarDockCommandCount += 1;
  try {
    const result = await invoke(request);
    return applyTaskbarDockState(result) ?? getTaskbarDockState();
  } finally {
    pendingTaskbarDockCommandCount -= 1;
  }
}

export async function listenTaskbarDockState(
  listener: (state: TaskbarDockState) => void
): Promise<() => void> {
  if (!isTaskbarDockingAvailable()) return () => {};
  stateListeners.add(listener);
  listener(getTaskbarDockState());
  try {
    const { listen } = await import('@tauri-apps/api/event');
    const unlisten = await listen<unknown>(
      'desktop://taskbarDockState',
      event => {
        applyTaskbarDockState(event.payload);
      }
    );
    return () => {
      stateListeners.delete(listener);
      unlisten();
    };
  } catch (error) {
    stateListeners.delete(listener);
    throw error;
  }
}

async function invokeTaskbarDockingCommand(
  request: TaskbarDockCommand
): Promise<TaskbarDockState> {
  if (!isTaskbarDockingAvailable()) return getTaskbarDockState();
  const { invoke } = await import('@tauri-apps/api/core');
  return invokeTaskbarDockCommand(request, ({ command, args }) =>
    invoke(command, args)
  );
}

export function setTaskbarDocking(enabled: boolean): Promise<TaskbarDockState> {
  return invokeTaskbarDockingCommand({
    command: 'set_taskbar_docking',
    args: { enabled },
  });
}

export function syncTaskbarCompactMode(
  compact: boolean
): Promise<TaskbarDockState> {
  return invokeTaskbarDockingCommand({
    command: 'sync_taskbar_compact_mode',
    args: { compact },
  });
}

export async function syncTaskbarCompactModeWithReadiness(
  sync: () => Promise<TaskbarDockState>,
  applyState: (state: TaskbarDockState) => void,
  markReady: (ready: boolean) => void,
  onError: (error: unknown) => void
): Promise<boolean> {
  markReady(false);
  try {
    applyState(await sync());
    markReady(true);
    return true;
  } catch (error) {
    markReady(false);
    onError(error);
    return false;
  }
}
