import { isTauriRuntime } from '@/utils/runtime';
import { resolveRendererWindowRole } from '@/utils/taskbarOverlayRole';

async function bootstrapWindow() {
  let windowLabel: string | null = null;
  if (isTauriRuntime) {
    try {
      const { getCurrentWebviewWindow } = await import(
        '@tauri-apps/api/webviewWindow'
      );
      windowLabel = getCurrentWebviewWindow().label;
    } catch (error) {
      console.error(
        '[renderer] unable to resolve the Tauri window label',
        error
      );
    }
  }

  const role = resolveRendererWindowRole(
    window.location.hash,
    isTauriRuntime,
    windowLabel
  );
  if (role === 'taskbar-overlay') {
    await import('./taskbarOverlayMain');
  } else if (role === 'main') {
    await import('./bootstrapMain');
  } else {
    console.error('[renderer] refusing to boot an unrecognized Tauri window');
  }
}

void bootstrapWindow();
