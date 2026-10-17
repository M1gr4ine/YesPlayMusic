import { playlistCategories } from '@/utils/staticData';
import shortcuts from '@/utils/shortcuts';
import { isDesktopRuntime } from '@/utils/runtime';
import type { DataState, SettingsState } from '@/types/persistence';

const enabledPlaylistCategories = playlistCategories
  .filter(category => category.enable)
  .map(category => category.name);

const settings = {
  lang: null,
  musicLanguage: 'all',
  appearance: 'auto',
  themeColor: 'default',
  musicQuality: 320000,
  lyricFontSize: 28,
  outputDevice: 'default',
  showPlaylistsByAppleMusic: true,
  enableUnblockNeteaseMusic: true,
  automaticallyCacheSongs: true,
  shareCacheWithYpm: false,
  cacheLimit: 8192,
  enableReversedMode: false,
  nyancatStyle: false,
  anonStyle: false,
  creeperStyle: false,
  showLyricsTranslation: true,
  lyricsBackground: true,
  showLyricsTime: false,
  closeAppOption: 'ask',
  enableDiscordRichPresence: false,
  enableOsdlyricsSupport: false,
  enableGlobalShortcut: true,
  showLibraryDefault: false,
  subTitleDefault: false,
  linuxEnableCustomTitlebar: false,
  windowsTaskbarDocking: true,
  windowsWindowAnimations: true,
  trayIconTheme: 'auto',
  enabledPlaylistCategories,
  proxyConfig: {
    protocol: 'noProxy',
    server: '',
    port: null,
  },
  enableRealIP: false,
  realIP: null,
  shortcuts,
} satisfies SettingsState;

if (isDesktopRuntime) {
  settings.automaticallyCacheSongs = true;
}

const data = {
  user: {},
  likedSongPlaylistID: 0,
  lastRefreshCookieDate: 0,
  loginMode: null,
} satisfies DataState;

const defaultStorageState = {
  player: {},
  settings,
  data,
};

export default defaultStorageState;
