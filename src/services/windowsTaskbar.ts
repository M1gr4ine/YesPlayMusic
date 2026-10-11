import { platform } from '@/utils/platform';
import { isTauriRuntime } from '@/utils/runtime';

export type TaskbarDockMode = 'floating' | 'docked' | 'waiting';

export interface TaskbarDockState {
  mode: TaskbarDockMode;
  suspended: boolean;
  dockError: string;
  enabled: boolean;
}

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
  return typeof value === 'object' && value !== null;
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
    (mode !== 'floating' && mode !== 'docked' && mode !== 'waiting') ||
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
    !(state.enabled && (state.mode === 'docked' || state.mode === 'waiting'))
  );
}

export function hasTaskbarDockIntent(state: TaskbarDockState): boolean {
  return state.enabled || state.mode === 'docked' || state.mode === 'waiting';
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
