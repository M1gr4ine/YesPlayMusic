import { normalizeCacheLimit } from '@/utils/cachePolicy';
import type { LastfmState, UnknownRecord } from '@/types/domain';
import type {
  DataState,
  ProxyConfig,
  SettingsState,
  Shortcut,
} from '@/types/persistence';

export interface StorageReader {
  getItem(key: string): string | null;
}

export function isUnknownRecord(value: unknown): value is UnknownRecord {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function readStoredJson(storage: StorageReader, key: string): unknown {
  const raw = storage.getItem(key);
  if (raw === null) return undefined;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return undefined;
  }
}

export function decodeStoredRecord(value: unknown): UnknownRecord {
  return isUnknownRecord(value) ? value : {};
}

type StoredShortcut = Partial<Shortcut> & Pick<Shortcut, 'id'>;

function isStoredShortcut(value: unknown): value is StoredShortcut {
  return (
    isUnknownRecord(value) &&
    typeof value['id'] === 'string' &&
    (value['name'] === undefined || typeof value['name'] === 'string') &&
    (value['shortcut'] === undefined ||
      typeof value['shortcut'] === 'string') &&
    (value['globalShortcut'] === undefined ||
      typeof value['globalShortcut'] === 'string')
  );
}

function completeStoredShortcut(
  saved: StoredShortcut,
  fallback?: Shortcut
): Shortcut {
  return {
    id: saved.id,
    name: saved.name ?? fallback?.name ?? saved.id,
    shortcut: saved.shortcut ?? fallback?.shortcut ?? '',
    globalShortcut: saved.globalShortcut ?? fallback?.globalShortcut ?? '',
  };
}

function decodeShortcuts(value: unknown, defaults: Shortcut[]): Shortcut[] {
  const saved = Array.isArray(value) ? value.filter(isStoredShortcut) : [];
  const savedIds = new Set(saved.map(shortcut => shortcut.id));
  const defaultsById = new Map(
    defaults.map(shortcut => [shortcut.id, shortcut] as const)
  );
  return [
    ...saved.map(shortcut =>
      completeStoredShortcut(shortcut, defaultsById.get(shortcut.id))
    ),
    ...defaults.filter(shortcut => !savedIds.has(shortcut.id)),
  ];
}

function decodeProxyConfig(value: unknown, fallback: ProxyConfig): ProxyConfig {
  const stored = decodeStoredRecord(value);
  const protocol = stored['protocol'];
  return {
    protocol:
      protocol === 'noProxy' || protocol === 'HTTP' || protocol === 'HTTPS'
        ? protocol
        : fallback.protocol,
    server:
      typeof stored['server'] === 'string' ? stored['server'] : fallback.server,
    port: proxyPortValue(stored['port'], fallback.port),
  };
}

function proxyPortValue(
  value: unknown,
  fallback: number | null
): number | null {
  if (value === null) return null;
  const normalized =
    typeof value === 'string' && /^(?:0|[1-9]\d*)$/.test(value)
      ? Number(value)
      : value;
  return typeof normalized === 'number' &&
    Number.isSafeInteger(normalized) &&
    normalized >= 1 &&
    normalized <= 65_535
    ? normalized
    : fallback;
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function booleanValue(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function numberValue(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function positiveInteger(value: unknown): number | null {
  const normalized =
    typeof value === 'string' && /^(?:0|[1-9]\d*)$/.test(value)
      ? Number(value)
      : value;
  return typeof normalized === 'number' &&
    Number.isSafeInteger(normalized) &&
    normalized > 0
    ? normalized
    : null;
}

export function normalizeMusicQuality(
  value: unknown,
  fallback: SettingsState['musicQuality']
): SettingsState['musicQuality'] {
  if (value === 'flac') return value;
  return positiveInteger(value) ?? fallback;
}

export function normalizeLyricFontSize(
  value: unknown,
  fallback: number
): number {
  return positiveInteger(value) ?? fallback;
}

function cacheLimitValue(
  value: unknown,
  fallback: number | null
): number | null {
  if (
    value === null ||
    value === false ||
    value === 0 ||
    value === '0' ||
    (typeof value === 'number' && Number.isFinite(value) && value > 0)
  ) {
    return normalizeCacheLimit(value);
  }
  return normalizeCacheLimit(fallback);
}

function optionalString(value: unknown, fallback?: string): string | undefined {
  return typeof value === 'string' ? value : fallback;
}

function optionalBoolean(
  value: unknown,
  fallback?: boolean
): boolean | undefined {
  return typeof value === 'boolean' ? value : fallback;
}

export function decodeSettingsState(
  value: unknown,
  defaults: SettingsState
): SettingsState {
  const stored = decodeStoredRecord(value);
  const lang = stored['lang'];
  const lyricsBackground = stored['lyricsBackground'];
  const realIP = stored['realIP'];
  const categories = stored['enabledPlaylistCategories'];
  const unmSource = optionalString(stored['unmSource'], defaults.unmSource);
  const unmEnableFlac = optionalBoolean(
    stored['unmEnableFlac'],
    defaults.unmEnableFlac
  );
  const unmProxyUri = optionalString(
    stored['unmProxyUri'],
    defaults.unmProxyUri
  );
  const unmSearchMode = optionalString(
    stored['unmSearchMode'],
    defaults.unmSearchMode
  );
  const unmJooxCookie = optionalString(
    stored['unmJooxCookie'],
    defaults.unmJooxCookie
  );
  const unmQQCookie = optionalString(
    stored['unmQQCookie'],
    defaults.unmQQCookie
  );
  const unmYtDlExe = optionalString(stored['unmYtDlExe'], defaults.unmYtDlExe);

  return {
    lang: typeof lang === 'string' || lang === null ? lang : defaults.lang,
    musicLanguage: stringValue(stored['musicLanguage'], defaults.musicLanguage),
    appearance: stringValue(stored['appearance'], defaults.appearance),
    themeColor: stringValue(stored['themeColor'], defaults.themeColor),
    musicQuality: normalizeMusicQuality(
      stored['musicQuality'],
      defaults.musicQuality
    ),
    lyricFontSize: normalizeLyricFontSize(
      stored['lyricFontSize'],
      defaults.lyricFontSize
    ),
    outputDevice: stringValue(stored['outputDevice'], defaults.outputDevice),
    showPlaylistsByAppleMusic: booleanValue(
      stored['showPlaylistsByAppleMusic'],
      defaults.showPlaylistsByAppleMusic
    ),
    enableUnblockNeteaseMusic: booleanValue(
      stored['enableUnblockNeteaseMusic'],
      defaults.enableUnblockNeteaseMusic
    ),
    automaticallyCacheSongs: booleanValue(
      stored['automaticallyCacheSongs'],
      defaults.automaticallyCacheSongs
    ),
    shareCacheWithYpm: booleanValue(
      stored['shareCacheWithYpm'],
      defaults.shareCacheWithYpm
    ),
    cacheLimit: cacheLimitValue(stored['cacheLimit'], defaults.cacheLimit),
    enableReversedMode: booleanValue(
      stored['enableReversedMode'],
      defaults.enableReversedMode
    ),
    nyancatStyle: booleanValue(stored['nyancatStyle'], defaults.nyancatStyle),
    anonStyle: booleanValue(stored['anonStyle'], defaults.anonStyle),
    creeperStyle: booleanValue(stored['creeperStyle'], defaults.creeperStyle),
    showLyricsTranslation: booleanValue(
      stored['showLyricsTranslation'],
      defaults.showLyricsTranslation
    ),
    lyricsBackground:
      typeof lyricsBackground === 'boolean' ||
      lyricsBackground === 'blur' ||
      lyricsBackground === 'dynamic'
        ? lyricsBackground
        : defaults.lyricsBackground,
    lyricsPortraitControls:
      stored['lyricsPortraitControls'] === 'bottom' ? 'bottom' : 'top',
    showLyricsTime: booleanValue(
      stored['showLyricsTime'],
      defaults.showLyricsTime
    ),
    closeAppOption:
      stored['closeAppOption'] === 'ask' ||
      stored['closeAppOption'] === 'exit' ||
      stored['closeAppOption'] === 'minimizeToTray'
        ? stored['closeAppOption']
        : defaults.closeAppOption,
    enableDiscordRichPresence: booleanValue(
      stored['enableDiscordRichPresence'],
      defaults.enableDiscordRichPresence
    ),
    enableOsdlyricsSupport: booleanValue(
      stored['enableOsdlyricsSupport'],
      defaults.enableOsdlyricsSupport
    ),
    enableGlobalShortcut: booleanValue(
      stored['enableGlobalShortcut'],
      defaults.enableGlobalShortcut
    ),
    showLibraryDefault: booleanValue(
      stored['showLibraryDefault'],
      defaults.showLibraryDefault
    ),
    subTitleDefault: booleanValue(
      stored['subTitleDefault'],
      defaults.subTitleDefault
    ),
    linuxEnableCustomTitlebar: booleanValue(
      stored['linuxEnableCustomTitlebar'],
      defaults.linuxEnableCustomTitlebar
    ),
    windowsTaskbarDocking: booleanValue(
      stored['windowsTaskbarDocking'],
      defaults.windowsTaskbarDocking
    ),
    windowsWindowAnimations: booleanValue(
      stored['windowsWindowAnimations'],
      defaults.windowsWindowAnimations
    ),
    trayIconTheme:
      stored['trayIconTheme'] === 'auto' ||
      stored['trayIconTheme'] === 'light' ||
      stored['trayIconTheme'] === 'dark'
        ? stored['trayIconTheme']
        : defaults.trayIconTheme,
    enabledPlaylistCategories:
      Array.isArray(categories) &&
      categories.every(category => typeof category === 'string')
        ? categories
        : defaults.enabledPlaylistCategories,
    proxyConfig: decodeProxyConfig(stored['proxyConfig'], defaults.proxyConfig),
    enableRealIP: booleanValue(stored['enableRealIP'], defaults.enableRealIP),
    realIP:
      typeof realIP === 'string' || realIP === null ? realIP : defaults.realIP,
    shortcuts: decodeShortcuts(stored['shortcuts'], defaults.shortcuts),
    ...(unmSource === undefined ? {} : { unmSource }),
    ...(unmEnableFlac === undefined ? {} : { unmEnableFlac }),
    ...(unmProxyUri === undefined ? {} : { unmProxyUri }),
    ...(unmSearchMode === undefined ? {} : { unmSearchMode }),
    ...(unmJooxCookie === undefined ? {} : { unmJooxCookie }),
    ...(unmQQCookie === undefined ? {} : { unmQQCookie }),
    ...(unmYtDlExe === undefined ? {} : { unmYtDlExe }),
  };
}

export function decodeDataState(
  value: unknown,
  defaults: DataState
): DataState {
  const stored = decodeStoredRecord(value);
  const user = stored['user'];
  const playlistId = stored['likedSongPlaylistID'];
  const refreshDate = stored['lastRefreshCookieDate'];
  const loginMode = stored['loginMode'];
  const filter = stored['libraryPlaylistFilter'];
  const libraryPlaylistFilter =
    filter === 'all' || filter === 'mine' || filter === 'liked'
      ? filter
      : defaults.libraryPlaylistFilter;
  return {
    user: isUnknownRecord(user) ? user : defaults.user,
    likedSongPlaylistID:
      typeof playlistId === 'number'
        ? playlistId
        : defaults.likedSongPlaylistID,
    lastRefreshCookieDate:
      typeof refreshDate === 'number' && Number.isFinite(refreshDate)
        ? refreshDate
        : defaults.lastRefreshCookieDate,
    loginMode:
      loginMode === 'account' || loginMode === 'username' || loginMode === null
        ? loginMode
        : defaults.loginMode,
    ...(libraryPlaylistFilter === undefined ? {} : { libraryPlaylistFilter }),
  };
}

export function decodeLastfmState(value: unknown): LastfmState {
  return decodeStoredRecord(value);
}
