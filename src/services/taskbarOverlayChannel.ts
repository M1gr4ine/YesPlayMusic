import { isLocaleCode } from '@/locale/catalog';
import type { LocaleCode } from '@/locale/catalog';

// Wire protocol between the player page and the Windows taskbar strip.
// Both pages share the sidecar origin, so a BroadcastChannel carries the
// playback mirror, the strip's phase and its button presses without any
// IPC: the strip window has no Tauri capability at all. Rust tells the
// player page the phase, and the player page relays it.

export const TASKBAR_STRIP_CHANNEL = 'yesplaymusic-taskbar-strip';

export interface TaskbarStripSnapshot {
  artwork: string;
  title: string;
  artist: string;
  lyric: string;
  lyricLoading: boolean;
  progress: number;
  duration: number;
  playing: boolean;
  locale: LocaleCode;
}

export type TaskbarStripAction =
  | 'previous'
  | 'playPause'
  | 'next'
  | 'undock'
  | 'hide';

const STRIP_ACTIONS: ReadonlyArray<TaskbarStripAction> = [
  'previous',
  'playPause',
  'next',
  'undock',
  'hide',
];

export type StripToMainMessage =
  | { type: 'ready' }
  | { type: 'action'; action: TaskbarStripAction }
  | { type: 'seek'; seconds: number };

export type MainToStripMessage =
  | { type: 'snapshot'; snapshot: TaskbarStripSnapshot }
  | { type: 'phase'; phase: TaskbarStripPhase };

export type TaskbarDockHint = 'none' | 'ready' | 'unavailable';

export type TaskbarStripMotion = 'fly' | 'fade' | 'instant';

export interface TaskbarStripPhase {
  phase: 'hidden' | 'docking' | 'docked' | 'undocking';
  motion: TaskbarStripMotion | null;
  durationMs: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function isStripAction(value: unknown): value is TaskbarStripAction {
  return (STRIP_ACTIONS as readonly unknown[]).includes(value);
}

export function decodeStripMessage(data: unknown): StripToMainMessage | null {
  if (!isRecord(data)) return null;
  switch (data['type']) {
    case 'ready':
      return { type: 'ready' };
    case 'action':
      return isStripAction(data['action'])
        ? { type: 'action', action: data['action'] }
        : null;
    case 'seek': {
      const seconds = finiteNumber(data['seconds']);
      return seconds !== null && seconds >= 0
        ? { type: 'seek', seconds }
        : null;
    }
    default:
      return null;
  }
}

export function decodeSnapshot(data: unknown): TaskbarStripSnapshot | null {
  if (!isRecord(data)) return null;
  const strings = ['artwork', 'title', 'artist', 'lyric'] as const;
  if (strings.some(key => typeof data[key] !== 'string')) return null;
  const progress = finiteNumber(data['progress']);
  const duration = finiteNumber(data['duration']);
  if (
    progress === null ||
    duration === null ||
    progress < 0 ||
    duration < 0 ||
    typeof data['lyricLoading'] !== 'boolean' ||
    typeof data['playing'] !== 'boolean' ||
    !isLocaleCode(data['locale'])
  ) {
    return null;
  }
  return {
    artwork: String(data['artwork']),
    title: String(data['title']),
    artist: String(data['artist']),
    lyric: String(data['lyric']),
    lyricLoading: data['lyricLoading'],
    progress: Math.min(progress, duration),
    duration,
    playing: data['playing'],
    locale: data['locale'],
  };
}

export function decodeMainMessage(data: unknown): MainToStripMessage | null {
  if (!isRecord(data)) return null;
  switch (data['type']) {
    case 'snapshot': {
      const snapshot = decodeSnapshot(data['snapshot']);
      return snapshot ? { type: 'snapshot', snapshot } : null;
    }
    case 'phase': {
      const phase = decodeStripPhase(data['phase']);
      return phase ? { type: 'phase', phase } : null;
    }
    default:
      return null;
  }
}

export function decodeStripPhase(data: unknown): TaskbarStripPhase | null {
  if (!isRecord(data)) return null;
  const phase = data['phase'];
  const motion = data['motion'] ?? null;
  const durationMs = finiteNumber(data['durationMs']);
  if (
    (phase !== 'hidden' &&
      phase !== 'docking' &&
      phase !== 'docked' &&
      phase !== 'undocking') ||
    (motion !== null &&
      motion !== 'fly' &&
      motion !== 'fade' &&
      motion !== 'instant') ||
    durationMs === null ||
    durationMs < 0
  ) {
    return null;
  }
  return { phase, motion, durationMs };
}

export function decodeDockHint(data: unknown): TaskbarDockHint | null {
  if (!isRecord(data)) return null;
  const state = data['state'];
  return state === 'none' || state === 'ready' || state === 'unavailable'
    ? state
    : null;
}

export function decodeDockState(data: unknown): boolean | null {
  return isRecord(data) && typeof data['docked'] === 'boolean'
    ? data['docked']
    : null;
}

export function openStripChannel(): BroadcastChannel | null {
  try {
    return new BroadcastChannel(TASKBAR_STRIP_CHANNEL);
  } catch (error) {
    console.warn('[taskbar-strip] channel unavailable', error);
    return null;
  }
}
