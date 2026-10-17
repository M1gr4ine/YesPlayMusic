import { sendDesktop } from '@/services/desktopTransport';

type DesktopSender = (channel: string, payload: unknown) => Promise<unknown>;

export interface OrderedDesktopSettings {
  sync(settings: unknown): Promise<void>;
  sendDiscordPresence(payload: unknown): Promise<void>;
}

const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

function jsonSnapshot(value: unknown): unknown {
  const serialized = JSON.stringify(value);
  if (serialized === undefined) {
    throw new TypeError('Desktop payload must be JSON serializable');
  }
  return JSON.parse(serialized) as unknown;
}

export function createOrderedDesktopSettings(
  send: DesktopSender
): OrderedDesktopSettings {
  let releaseFirstSettings: (() => void) | null = null;
  const firstSettings = new Promise<void>(resolve => {
    releaseFirstSettings = resolve;
  });
  let settingsQueue = Promise.resolve();

  return {
    sync(settings) {
      const snapshot = jsonSnapshot(settings);
      settingsQueue = settingsQueue
        .catch(() => undefined)
        .then(async () => {
          await send('settings', snapshot);
        });
      releaseFirstSettings?.();
      releaseFirstSettings = null;
      return settingsQueue;
    },
    async sendDiscordPresence(payload) {
      const snapshot = jsonSnapshot(payload);
      await firstSettings;
      await settingsQueue;
      await send('discordPresence', snapshot);
    },
  };
}

function reducedMotionQuery(): MediaQueryList | null {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function')
    return null;
  try {
    return window.matchMedia(REDUCED_MOTION_QUERY);
  } catch {
    return null;
  }
}

/**
 * Rust needs the OS motion preference next to the settings (the taskbar
 * strip's flight honours it); only the renderer can read it.
 */
export function withSystemPreferences(
  settings: unknown,
  systemReducedMotion: boolean
): unknown {
  if (typeof settings !== 'object' || settings === null) return settings;
  return { ...settings, systemReducedMotion };
}

const desktopSettings = createOrderedDesktopSettings(sendDesktop);
let lastSettings: unknown;

export function syncDesktopSettings(settings: unknown): Promise<void> {
  lastSettings = settings;
  const reduced = reducedMotionQuery()?.matches ?? false;
  return desktopSettings.sync(withSystemPreferences(settings, reduced));
}

// Re-send when the OS preference flips so Rust never acts on a stale value.
reducedMotionQuery()?.addEventListener('change', () => {
  if (lastSettings !== undefined) void syncDesktopSettings(lastSettings);
});

export function sendConfiguredDiscordPresence(payload: unknown): Promise<void> {
  return desktopSettings.sendDiscordPresence(payload);
}
