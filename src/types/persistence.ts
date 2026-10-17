import type { UserProfile } from './domain';

export interface Shortcut {
  id: string;
  name: string;
  shortcut: string;
  globalShortcut: string;
}

export interface ProxyConfig {
  protocol: 'noProxy' | 'HTTP' | 'HTTPS';
  server: string;
  port: number | null;
}

export interface SettingsState {
  lang: string | null;
  musicLanguage: string;
  appearance: string;
  themeColor: string;
  musicQuality: number | 'flac';
  lyricFontSize: number;
  outputDevice: string;
  showPlaylistsByAppleMusic: boolean;
  enableUnblockNeteaseMusic: boolean;
  automaticallyCacheSongs: boolean;
  shareCacheWithYpm: boolean;
  cacheLimit: number | null;
  enableReversedMode: boolean;
  nyancatStyle: boolean;
  anonStyle: boolean;
  creeperStyle: boolean;
  showLyricsTranslation: boolean;
  lyricsBackground: boolean | 'blur' | 'dynamic';
  showLyricsTime: boolean;
  closeAppOption: 'ask' | 'exit' | 'minimizeToTray';
  enableDiscordRichPresence: boolean;
  enableOsdlyricsSupport: boolean;
  enableGlobalShortcut: boolean;
  showLibraryDefault: boolean;
  subTitleDefault: boolean;
  linuxEnableCustomTitlebar: boolean;
  windowsTaskbarDocking: boolean;
  windowsWindowAnimations: boolean;
  trayIconTheme: 'auto' | 'light' | 'dark';
  enabledPlaylistCategories: string[];
  proxyConfig: ProxyConfig;
  enableRealIP: boolean;
  realIP: string | null;
  shortcuts: Shortcut[];
  unmSource?: string;
  unmEnableFlac?: boolean;
  unmProxyUri?: string;
  unmSearchMode?: string;
  unmJooxCookie?: string;
  unmQQCookie?: string;
  unmYtDlExe?: string;
}

export interface DataState {
  user: UserProfile;
  likedSongPlaylistID: number | undefined;
  lastRefreshCookieDate: number;
  loginMode: 'account' | 'username' | null;
  libraryPlaylistFilter?: 'all' | 'mine' | 'liked';
}
