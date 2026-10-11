export const TASKBAR_OVERLAY_HASH = '#/taskbar-overlay';
export const TASKBAR_OVERLAY_WINDOW_LABEL = 'taskbar-lyrics';
export const MAIN_WINDOW_LABEL = 'main';

export type RendererWindowRole = 'main' | 'taskbar-overlay' | 'invalid';

export function resolveRendererWindowRole(
  hash: string,
  isTauri: boolean,
  windowLabel: string | null
): RendererWindowRole {
  if (!isTauri) return 'main';
  if (!windowLabel) return 'invalid';
  if (windowLabel === TASKBAR_OVERLAY_WINDOW_LABEL) {
    return hash === TASKBAR_OVERLAY_HASH ? 'taskbar-overlay' : 'invalid';
  }
  if (windowLabel === MAIN_WINDOW_LABEL) return 'main';
  return 'invalid';
}
