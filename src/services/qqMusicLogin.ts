import { isDesktopRuntime } from '@/utils/runtime';
import { isWindows } from '@/utils/platform';

export const canLoginQQMusic = isDesktopRuntime && isWindows;

let pending: Promise<string | null> | null = null;

// Opens QQ Music's official login in a throwaway window. Resolves with the
// UNM cookie, or null when the user closes the window.
export function loginQQMusic(): Promise<string | null> {
  pending ??= import('@tauri-apps/api/core')
    .then(({ invoke }) => invoke<string | null>('qq_music_login'))
    .finally(() => {
      pending = null;
    });
  return pending;
}
