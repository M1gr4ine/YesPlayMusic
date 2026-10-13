import { isWindows } from '@/utils/platform';
import { isTauriRuntime } from '@/utils/runtime';
import type { LocaleCode } from '@/locale/catalog';

// Event payloads come from the Rust taskbar controller, which owns their schema.

export const isTaskbarDockingAvailable = isWindows && isTauriRuntime;

/** Feedback while the compact player is dragged near a taskbar. */
export type TaskbarDockHint = 'none' | 'ready' | 'unavailable';

export type TaskbarPlaybackAction =
  | { action: 'previous' | 'play' | 'next' | 'shuffle' }
  | { action: 'seek'; value: number };

export type TaskbarOverlayCommand =
  | TaskbarPlaybackAction
  | { action: 'undock' | 'close' };

/** Presentation-only playback state mirrored into the taskbar strip. */
export interface TaskbarOverlaySnapshot {
  artwork: string;
  trackTitle: string;
  artistName: string;
  lyric: string;
  emptyKind: 'loading' | 'track' | null;
  emptyLabel: string;
  progress: number;
  duration: number;
  playing: boolean;
  shuffle: boolean;
  locale: LocaleCode;
  reducedMotion: boolean;
}

export interface TaskbarOverlayVisual {
  phase: 'hidden' | 'snapping' | 'docked' | 'returning';
  // Increases with every phase; the controller ignores stale acknowledgements.
  token: number;
  durationMs: number;
}

async function invoke<T>(command: string, args?: Record<string, unknown>) {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(command, args);
}

async function listen<T>(event: string, handler: (payload: T) => void) {
  const { listen } = await import('@tauri-apps/api/event');
  return listen<T>(`desktop://${event}`, ({ payload }) => handler(payload));
}

async function listenAll(
  subscriptions: Promise<() => void>[]
): Promise<() => void> {
  const results = await Promise.allSettled(subscriptions);
  const stops = results.flatMap(result =>
    result.status === 'fulfilled' ? [result.value] : []
  );
  const stopAll = () => stops.forEach(stop => stop());
  const failure = results.find(
    (result): result is PromiseRejectedResult => result.status === 'rejected'
  );
  if (failure) {
    stopAll();
    throw failure.reason;
  }
  return stopAll;
}

export interface TaskbarDockSession {
  /** Reports the compact layout; failures retry on the next report. */
  syncCompact(compact: boolean): void;
  /** Mirrors playback into the strip, coalescing bursts of updates. */
  publish(snapshot: Omit<TaskbarOverlaySnapshot, 'reducedMotion'>): void;
  dispose(): void;
}

/** Main window side of docking: listens first, then syncs and publishes. */
export function connectTaskbarDock(handlers: {
  onHint(hint: TaskbarDockHint): void;
  onAction(action: TaskbarPlaybackAction): void;
}): TaskbarDockSession {
  let disposed = false;
  const listening = listenAll([
    listen<{ state: TaskbarDockHint }>('taskbarDockHint', ({ state }) =>
      handlers.onHint(state)
    ),
    listen<TaskbarPlaybackAction>('taskbarOverlayAction', handlers.onAction),
  ]);

  let wanted: boolean | null = null;
  let synced: boolean | null = null;
  let syncing = false;
  async function sync() {
    if (syncing) return;
    syncing = true;
    try {
      // Docking can emit hints and actions at once; listen before syncing.
      await listening;
      while (!disposed && wanted !== null && wanted !== synced) {
        const compact: boolean = wanted;
        await invoke('sync_taskbar_compact_mode', { compact });
        synced = compact;
      }
    } catch (error) {
      synced = null;
      console.warn('[taskbar-dock] compact mode sync failed', error);
    } finally {
      syncing = false;
    }
  }

  const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let latest: Omit<TaskbarOverlaySnapshot, 'reducedMotion'> | null = null;
  let publishTimer: ReturnType<typeof setTimeout> | null = null;
  function schedulePublish() {
    if (publishTimer !== null) clearTimeout(publishTimer);
    publishTimer = setTimeout(() => {
      publishTimer = null;
      if (!latest || disposed) return;
      invoke('publish_taskbar_overlay_snapshot', {
        snapshot: { ...latest, reducedMotion: motion.matches },
      }).catch(error =>
        console.warn('[taskbar-dock] snapshot publish failed', error)
      );
    }, 100);
  }
  motion.addEventListener('change', schedulePublish);

  return {
    syncCompact(compact) {
      wanted = compact;
      void sync();
    },
    publish(snapshot) {
      latest = snapshot;
      schedulePublish();
    },
    dispose() {
      disposed = true;
      if (publishTimer !== null) clearTimeout(publishTimer);
      motion.removeEventListener('change', schedulePublish);
      listening.then(stop => stop()).catch(() => undefined);
    },
  };
}

/** Overlay window: subscribes, then asks the controller to replay its state. */
export async function connectTaskbarOverlay(handlers: {
  onSnapshot(snapshot: TaskbarOverlaySnapshot): void;
  onVisual(visual: TaskbarOverlayVisual): void;
}): Promise<() => void> {
  const stop = await listenAll([
    listen('taskbarOverlaySnapshot', handlers.onSnapshot),
    listen('taskbarOverlayVisual', handlers.onVisual),
  ]);
  try {
    await invoke('taskbar_overlay_ready');
  } catch (error) {
    stop();
    throw error;
  }
  return stop;
}

export function sendTaskbarOverlayCommand(
  command: TaskbarOverlayCommand
): Promise<void> {
  return invoke('taskbar_overlay_action', command);
}

/** Reports that the overlay has painted a flight phase. */
export function acknowledgeTaskbarOverlayVisual(token: number): Promise<void> {
  return invoke('taskbar_overlay_visual_ready', { token });
}
