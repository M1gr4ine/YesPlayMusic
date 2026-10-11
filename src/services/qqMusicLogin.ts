import { isTauriRuntime } from '@/utils/runtime';
import type { Event as TauriEvent } from '@tauri-apps/api/event';

export const QQ_MUSIC_LOGIN_WINDOW_LABEL = 'qq-music-login-auth';
export const QQ_MUSIC_LOGIN_CANDIDATE_EVENT = 'qq-music-login://candidate';

const MAIN_WINDOW_LABEL = 'main';

const QQ_COOKIE_NAMES = [
  'uin',
  'qqmusic_key',
  'qm_keyst',
  'psrf_qqrefresh_token',
  'qqrefresh_token',
  'psrf_musickey_createtime',
  'psrf_qqaccess_token',
  'psrf_access_token',
  'qqaccess_token',
  'psrf_access_token_expiresAt',
  'psrf_qqopenid',
  'psrf_qqunionid',
  'login_type',
  'tmeLoginType',
  'euin',
  'music_ignore_pskey',
  'wxopenid',
  'wxunionid',
  'wxrefresh_token',
] as const;

const QQ_COOKIE_NAME_SET: ReadonlySet<string> = new Set(QQ_COOKIE_NAMES);

export interface QQMusicCookie {
  name: string;
  value: string;
  domain?: string | null;
}

export interface NormalizedQQMusicCookie {
  value: string;
  valid: boolean;
  canRefresh: boolean;
  empty: boolean;
}

export interface QQMusicLoginCandidate {
  attemptId: string;
  value: string;
  canRefresh: boolean;
}

export type QQMusicLoginCommitResult =
  | 'committed'
  | 'conflict'
  | 'invalid'
  | 'persistence-failed';

export interface QQMusicCookieStoreBinding {
  getCurrent(): string | undefined;
  getPersisted(): string | undefined;
  update(value: string): void;
}

export function isAllowedQQMusicCookieDomain(
  domain: string | null | undefined
) {
  const normalized = (domain ?? '').replace(/^\./, '').toLowerCase();
  return normalized === 'qq.com' || normalized === 'y.qq.com';
}

export function normalizeQQMusicCookies(
  cookies: readonly QQMusicCookie[]
): NormalizedQQMusicCookie {
  const selected = cookies
    .filter(
      cookie =>
        QQ_COOKIE_NAME_SET.has(cookie.name) &&
        isAllowedQQMusicCookieDomain(cookie.domain)
    )
    .map(cookie => `${cookie.name}=${cookie.value}`)
    .join('; ');
  return normalizeQQMusicCookieValue(selected);
}

export function normalizeQQMusicCookieValue(
  raw: unknown
): NormalizedQQMusicCookie {
  let text = String(raw ?? '').trim();
  const empty = !text;
  text = text
    .replace(/^```[^\r\n]*[\r\n]+/, '')
    .replace(/[\r\n]+```$/, '')
    .trim();
  if (
    text.length > 1 &&
    ((text.startsWith("'") && text.endsWith("'")) ||
      (text.startsWith('"') && text.endsWith('"')) ||
      (text.startsWith('`') && text.endsWith('`')))
  ) {
    text = text.slice(1, -1).trim();
  }
  text = text.replace(/^cookie\s*:\s*/i, '').replace(/\\([_*@])/g, '$1');

  const fields = new Map<string, string>();
  for (const part of text.split(/[;\r\n]+/)) {
    const separator = part.indexOf('=');
    if (separator < 1) continue;
    const name = part.slice(0, separator).trim();
    const value = part.slice(separator + 1).trim();
    if (
      QQ_COOKIE_NAME_SET.has(name) &&
      value.length > 0 &&
      !/[\s\x00-\x1f\x7f]/.test(value)
    ) {
      fields.set(name, value);
    }
  }

  const key = fields.get('qqmusic_key') || fields.get('qm_keyst');
  if (key) {
    fields.set('qqmusic_key', key);
    fields.set('qm_keyst', key);
  }
  const valid = /^\d+$/.test(fields.get('uin') ?? '') && Boolean(key);
  return {
    value: QQ_COOKIE_NAMES.filter(name => fields.has(name))
      .map(name => `${name}=${fields.get(name)}`)
      .join('; '),
    valid,
    canRefresh:
      valid &&
      Boolean(
        fields.get('psrf_qqrefresh_token') || fields.get('qqrefresh_token')
      ),
    empty,
  };
}

export function persistQQMusicLoginCandidate(
  candidate: unknown,
  expectedCurrent: string,
  store: QQMusicCookieStoreBinding
): QQMusicLoginCommitResult {
  if (
    typeof candidate !== 'object' ||
    candidate === null ||
    Array.isArray(candidate) ||
    !('attemptId' in candidate) ||
    typeof candidate.attemptId !== 'string' ||
    candidate.attemptId.length === 0 ||
    !('value' in candidate) ||
    typeof candidate.value !== 'string' ||
    !('canRefresh' in candidate) ||
    typeof candidate.canRefresh !== 'boolean'
  ) {
    return 'invalid';
  }

  const normalized = normalizeQQMusicCookieValue(candidate.value);
  if (
    !normalized.valid ||
    normalized.value !== candidate.value ||
    normalized.canRefresh !== candidate.canRefresh
  ) {
    return 'invalid';
  }
  if ((store.getCurrent() ?? '') !== expectedCurrent) return 'conflict';

  try {
    store.update(normalized.value);
    if (
      store.getCurrent() === normalized.value &&
      store.getPersisted() === normalized.value
    ) {
      return 'committed';
    }
  } catch {
    // A failed synchronous store or storage write must not replace the prior value.
  }

  if ((store.getCurrent() ?? '') === normalized.value) {
    try {
      store.update(expectedCurrent);
    } catch {
      // Keep the failure result even if the storage layer also rejects rollback.
    }
  }
  return 'persistence-failed';
}

type Unlisten = () => void;
type QQMusicEventHandler = (payload: unknown) => void | Promise<void>;

interface QQMusicLoginRuntime {
  listenForCandidate(handler: QQMusicEventHandler): Promise<Unlisten>;
  open(): Promise<{ attemptId: string; reused: boolean }>;
  finish(request: {
    attemptId: string;
    value?: string;
    committed: boolean;
  }): Promise<void>;
}

interface QQMusicLoginCallbacks {
  onCandidate(
    candidate: QQMusicLoginCandidate
  ): QQMusicLoginCommitResult | Promise<QQMusicLoginCommitResult>;
  onComplete(candidate: QQMusicLoginCandidate): void;
  onNativeRejected?(candidate: QQMusicLoginCandidate): void;
  onClosed?(): void;
  onError?(error: Error): void;
  onRejected?(result: Exclude<QQMusicLoginCommitResult, 'committed'>): void;
}

function decodeQQMusicLoginCandidate(
  value: unknown
): QQMusicLoginCandidate | null {
  if (
    typeof value !== 'object' ||
    value === null ||
    Array.isArray(value) ||
    !('attemptId' in value) ||
    typeof value.attemptId !== 'string' ||
    value.attemptId.length === 0 ||
    !('value' in value) ||
    typeof value.value !== 'string' ||
    !('canRefresh' in value) ||
    typeof value.canRefresh !== 'boolean'
  ) {
    return null;
  }
  const normalized = normalizeQQMusicCookieValue(value.value);
  if (
    !normalized.valid ||
    normalized.value !== value.value ||
    normalized.canRefresh !== value.canRefresh
  ) {
    return null;
  }
  return {
    attemptId: value.attemptId,
    value: normalized.value,
    canRefresh: normalized.canRefresh,
  };
}

async function loadQQMusicLoginRuntime(): Promise<QQMusicLoginRuntime> {
  const [{ invoke }, { listen }] = await Promise.all([
    import('@tauri-apps/api/core'),
    import('@tauri-apps/api/event'),
  ]);

  const listenFor = (eventName: string, handler: QQMusicEventHandler) =>
    listen<unknown>(
      eventName,
      (event: TauriEvent<unknown>) => handler(event.payload),
      { target: { kind: 'WebviewWindow', label: MAIN_WINDOW_LABEL } }
    );

  return {
    listenForCandidate: handler =>
      listenFor(QQ_MUSIC_LOGIN_CANDIDATE_EVENT, handler),
    open: () => invoke('open_qq_music_login'),
    finish: request =>
      invoke('finish_qq_music_login', {
        attemptId: request.attemptId,
        value: request.value ?? null,
        committed: request.committed,
      }),
  };
}

export async function startDesktopQQMusicLogin(
  callbacks: QQMusicLoginCallbacks,
  providedRuntime?: QQMusicLoginRuntime
): Promise<Unlisten> {
  if (!providedRuntime && !isTauriRuntime) {
    throw new Error('QQ Music quick login is available in the Windows app');
  }
  const runtime = providedRuntime ?? (await loadQQMusicLoginRuntime());
  let active = true;
  let attemptId: string | null = null;
  let processing = false;
  let queuedPayload: unknown;
  const disposers: Unlisten[] = [];

  const cleanup = () => {
    if (!active) return;
    active = false;
    disposers.splice(0).forEach(dispose => dispose());
  };

  const handleCandidate = async (payload: unknown) => {
    if (!active) return;
    if (attemptId === null) {
      queuedPayload = payload;
      return;
    }
    if (
      typeof payload === 'object' &&
      payload !== null &&
      !Array.isArray(payload) &&
      'attemptId' in payload &&
      payload.attemptId === attemptId &&
      'value' in payload &&
      payload.value === '' &&
      'canRefresh' in payload &&
      payload.canRefresh === false
    ) {
      cleanup();
      callbacks.onClosed?.();
      return;
    }
    const candidate = decodeQQMusicLoginCandidate(payload);
    if (candidate === null) return;
    if (candidate.attemptId !== attemptId || processing) return;
    processing = true;
    let committed = false;
    try {
      const result = await callbacks.onCandidate(candidate);
      committed = result === 'committed';
      await runtime.finish({
        attemptId,
        value: candidate.value,
        committed,
      });
      if (result === 'committed') {
        callbacks.onComplete(candidate);
      } else {
        callbacks.onRejected?.(result);
      }
    } catch {
      if (committed) callbacks.onNativeRejected?.(candidate);
      try {
        await runtime.finish({
          attemptId,
          value: candidate.value,
          committed: false,
        });
      } catch {
        // The native window also closes and clears its temporary profile on cancellation.
      }
      callbacks.onError?.(new Error('QQ Music login could not be saved'));
    } finally {
      processing = false;
    }
  };

  try {
    disposers.push(
      await runtime.listenForCandidate(payload => {
        void handleCandidate(payload);
      })
    );
    const opened = await runtime.open();
    if (!opened.attemptId) throw new Error('Unable to open QQ Music login');
    attemptId = opened.attemptId;
    if (queuedPayload !== undefined) {
      const queued = queuedPayload;
      queuedPayload = undefined;
      await handleCandidate(queued);
    }
  } catch {
    cleanup();
    throw new Error('Unable to open QQ Music login');
  }

  return () => {
    if (!active) return;
    const currentAttemptId = attemptId;
    cleanup();
    if (currentAttemptId !== null) {
      void runtime
        .finish({ attemptId: currentAttemptId, committed: false })
        .catch(() => {});
    }
  };
}
