import { afterAll, expect, mock, test } from 'bun:test';
import { createMemoryStorage } from './helpers/memoryStorage';

interface Frame {
  x: number;
  y: number;
  width: number;
  height: number;
}

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
const storage = createMemoryStorage();
let currentFrame: Frame = { x: 100, y: 100, width: 920, height: 620 };
const appliedFrames: Frame[] = [];
let signalSyncStarted: () => void = () => {};
let resolveCompactSync: (state: unknown) => void = () => {};
const compactSyncStarted = new Promise<void>(resolve => {
  signalSyncStarted = resolve;
});
const compactSyncResult = new Promise<unknown>(resolve => {
  resolveCompactSync = resolve;
});

Object.defineProperty(globalThis, 'window', {
  configurable: true,
  value: { localStorage: storage },
});

const nativeWindow = {
  innerSize: async () => ({
    width: currentFrame.width,
    height: currentFrame.height,
  }),
  outerPosition: async () => ({ x: currentFrame.x, y: currentFrame.y }),
  scaleFactor: async () => 1,
  isMaximized: async () => false,
  isFullscreen: async () => false,
  setFullscreen: async () => undefined,
};

mock.module('@/utils/platform', () => ({
  platform: 'win32',
  isWindows: true,
  isMac: false,
  isLinux: false,
}));
mock.module('@/utils/runtime', () => ({ isTauriRuntime: true }));
mock.module('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => nativeWindow,
}));
mock.module('@tauri-apps/api/core', () => ({
  invoke: async (command: string, args?: Record<string, unknown>) => {
    if (command === 'sync_taskbar_compact_mode') {
      signalSyncStarted();
      return compactSyncResult;
    }
    if (command === 'restore_compact_window') {
      const frame = args as unknown as Frame;
      currentFrame = { ...frame };
      appliedFrames.push({ ...frame });
      return undefined;
    }
    throw new Error(`unexpected native command: ${command}`);
  },
}));

const {
  expandCompactWindow,
  rememberCompactWindowFrame,
  restoreCompactWindow,
} = await import('../src/services/compactWindow');

afterAll(() => {
  mock.restore();
  if (originalWindow) {
    Object.defineProperty(globalThis, 'window', originalWindow);
  } else {
    Reflect.deleteProperty(globalThis, 'window');
  }
});

test('expand owns the transition lock while taskbar sync is pending', async () => {
  rememberCompactWindowFrame({
    x: 100,
    y: 100,
    width: 500,
    height: 72,
  });
  rememberCompactWindowFrame({
    x: 100,
    y: 100,
    width: 920,
    height: 620,
  });

  const expanding = expandCompactWindow();
  await compactSyncStarted;

  const restoring = await restoreCompactWindow();
  resolveCompactSync({
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  });

  expect(restoring).toBe(false);
  expect(await expanding).toBe(false);
  expect(appliedFrames).toEqual([]);
});
