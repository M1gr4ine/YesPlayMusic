import { expect, test } from 'bun:test';
import {
  applyTaskbarDockState,
  canPersistCompactWindowFrame,
  getTaskbarDockState,
  hasTaskbarDockIntent,
  invokeTaskbarDockCommand,
  isTaskbarDockingAvailable,
  normalizeTaskbarDockState,
  syncTaskbarCompactModeWithReadiness,
  shouldShowTaskbarReturnButton,
} from '../src/services/windowsTaskbar';
import {
  COMPACT_WINDOW_MEMORY_KEY,
  rememberCompactWindowSnapshot,
} from '../src/services/compactWindow';
import { createMemoryStorage } from './helpers/memoryStorage';

test('native taskbar state is accepted only when every field has the expected type', () => {
  expect(
    normalizeTaskbarDockState({
      mode: 'docked',
      suspended: false,
      dockError: '',
      enabled: true,
    })
  ).toEqual({
    mode: 'docked',
    suspended: false,
    dockError: '',
    enabled: true,
  });
  expect(
    normalizeTaskbarDockState({
      mode: 'docked',
      suspended: false,
      dockError: '',
      enabled: 'true',
    })
  ).toBeNull();
});

test('failed initial compact sync stays unready and a later retry can recover', async () => {
  const initialState = getTaskbarDockState();
  const readyStates: boolean[] = [];
  const appliedStates: import('../src/services/windowsTaskbar').TaskbarDockState[] =
    [];
  const floatingState = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  } as const;
  const retryState = {
    ...floatingState,
    mode: 'docked',
    enabled: true,
  } as const;

  const firstSync = await syncTaskbarCompactModeWithReadiness(
    async () => {
      throw new Error('native sync unavailable');
    },
    state => appliedStates.push(state),
    ready => readyStates.push(ready),
    () => {}
  );

  expect(firstSync).toBe(false);
  expect(readyStates).toEqual([false, false]);
  expect(appliedStates).toEqual([]);

  const retrySync = await syncTaskbarCompactModeWithReadiness(
    async () => retryState,
    state => appliedStates.push(state),
    ready => readyStates.push(ready),
    () => {}
  );

  expect(retrySync).toBe(true);
  expect(readyStates).toEqual([false, false, false, true]);
  expect(appliedStates).toEqual([retryState]);
  applyTaskbarDockState(initialState);
});

test('invalid native events cannot replace the last valid docking state', () => {
  const initial = getTaskbarDockState();
  const next = {
    mode: 'waiting',
    suspended: true,
    dockError: 'fullscreen window',
    enabled: true,
  } as const;

  expect(applyTaskbarDockState(next)).toEqual(next);
  expect(
    applyTaskbarDockState({ mode: 'broken', suspended: false })
  ).toBeNull();
  expect(getTaskbarDockState()).toEqual(next);
  applyTaskbarDockState(initial);
});

test('docked and waiting compact frames are not saved as the floating bar frame', () => {
  expect(
    canPersistCompactWindowFrame({
      mode: 'docked',
      suspended: false,
      dockError: '',
      enabled: true,
    })
  ).toBe(false);
  expect(
    canPersistCompactWindowFrame({
      mode: 'waiting',
      suspended: true,
      dockError: '',
      enabled: true,
    })
  ).toBe(false);
  expect(
    canPersistCompactWindowFrame({
      mode: 'floating',
      suspended: false,
      dockError: '',
      enabled: true,
    })
  ).toBe(true);
  expect(
    canPersistCompactWindowFrame({
      mode: 'docked',
      suspended: false,
      dockError: '',
      enabled: false,
    })
  ).toBe(true);
});

test('taskbar docking is enabled only inside the Windows Tauri runtime', () => {
  expect(isTaskbarDockingAvailable('win32', true)).toBe(true);
  expect(isTaskbarDockingAvailable('darwin', true)).toBe(false);
  expect(isTaskbarDockingAvailable('win32', false)).toBe(false);
});

test('the taskbar return button appears only after docking completes', () => {
  const floating = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  } as const;

  expect(shouldShowTaskbarReturnButton(floating)).toBe(false);
  expect(shouldShowTaskbarReturnButton({ ...floating, enabled: true })).toBe(
    false
  );
  expect(
    shouldShowTaskbarReturnButton({
      ...floating,
      mode: 'waiting',
      enabled: true,
    })
  ).toBe(false);
  expect(
    shouldShowTaskbarReturnButton({
      ...floating,
      mode: 'docked',
      enabled: true,
    })
  ).toBe(true);
});

test('docking transitions retain dock intent and block compact-frame actions', () => {
  for (const mode of ['docking', 'undocking'] as const) {
    const state = {
      mode,
      suspended: false,
      dockError: '',
      enabled: false,
    };

    expect(normalizeTaskbarDockState(state)).toEqual(state);
    expect(hasTaskbarDockIntent(state)).toBe(true);
    expect(canPersistCompactWindowFrame(state)).toBe(false);
    expect(shouldShowTaskbarReturnButton(state)).toBe(false);
  }
});

test('compact mode sync forwards its native argument and refreshes cached state', async () => {
  const calls: Array<[string, unknown]> = [];
  const next = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: true,
  } as const;

  const result = await invokeTaskbarDockCommand(
    { command: 'sync_taskbar_compact_mode', args: { compact: true } },
    async request => {
      calls.push([request.command, request.args]);
      return next;
    }
  );

  expect(calls).toEqual([['sync_taskbar_compact_mode', { compact: true }]]);
  expect(result).toEqual(next);
  expect(getTaskbarDockState()).toEqual(next);
  applyTaskbarDockState({
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  });
});

test('compact-window persistence skips Explorer-parented dock bounds', () => {
  const storage = createMemoryStorage();
  const dockedFrame = { x: 1280, y: 1040, width: 380, height: 64 };
  const dockedState = {
    mode: 'docked',
    suspended: false,
    dockError: '',
    enabled: true,
  } as const;

  expect(
    rememberCompactWindowSnapshot(
      { frame: dockedFrame, normal: true },
      dockedState,
      storage
    )
  ).toBeNull();
  expect(storage.getItem(COMPACT_WINDOW_MEMORY_KEY)).toBeNull();

  expect(
    rememberCompactWindowSnapshot(
      { frame: dockedFrame, normal: true },
      { ...dockedState, mode: 'floating' },
      storage
    )
  ).toEqual({ mode: 'bar', frame: dockedFrame });
  expect(
    JSON.parse(storage.getItem(COMPACT_WINDOW_MEMORY_KEY) ?? 'null')
  ).toEqual({
    bar: dockedFrame,
    browse: null,
    lastMode: 'bar',
  });
});

test('compact frame persistence waits for an in-flight docking command', async () => {
  const storage = createMemoryStorage();
  const frame = { x: 440, y: 720, width: 420, height: 96 };
  const floatingState = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  } as const;
  const dockedState = {
    mode: 'docked',
    suspended: false,
    dockError: '',
    enabled: true,
  } as const;
  const initialState = getTaskbarDockState();
  applyTaskbarDockState(floatingState);

  let finishCommand!: (value: unknown) => void;
  const nativeResult = new Promise<unknown>(resolve => {
    finishCommand = resolve;
  });
  const command = invokeTaskbarDockCommand(
    { command: 'set_taskbar_docking', args: { enabled: true } },
    async () => nativeResult
  );

  try {
    expect(
      rememberCompactWindowSnapshot(
        { frame, normal: true },
        floatingState,
        storage
      )
    ).toBeNull();
    expect(storage.getItem(COMPACT_WINDOW_MEMORY_KEY)).toBeNull();
  } finally {
    finishCommand(dockedState);
    await command;
    applyTaskbarDockState(initialState);
  }
});

test('a rejected dock command does not clear another pending persistence guard', async () => {
  const storage = createMemoryStorage();
  const frame = { x: 440, y: 720, width: 420, height: 96 };
  const floatingState = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  } as const;
  const initialState = getTaskbarDockState();
  applyTaskbarDockState(floatingState);

  let finishCommand!: (value: unknown) => void;
  const nativeResult = new Promise<unknown>(resolve => {
    finishCommand = resolve;
  });
  const pendingCommand = invokeTaskbarDockCommand(
    { command: 'sync_taskbar_compact_mode', args: { compact: true } },
    async () => nativeResult
  );
  const rejectedCommand = invokeTaskbarDockCommand(
    { command: 'set_taskbar_docking', args: { enabled: true } },
    async () => {
      throw new Error('native attach failed');
    }
  );

  try {
    await expect(rejectedCommand).rejects.toThrow('native attach failed');
    expect(
      rememberCompactWindowSnapshot(
        { frame, normal: true },
        floatingState,
        storage
      )
    ).toBeNull();
    expect(storage.getItem(COMPACT_WINDOW_MEMORY_KEY)).toBeNull();

    finishCommand(floatingState);
    await pendingCommand;
    expect(
      rememberCompactWindowSnapshot(
        { frame, normal: true },
        floatingState,
        storage
      )
    ).toEqual({ mode: 'bar', frame });
    expect(storage.getItem(COMPACT_WINDOW_MEMORY_KEY)).not.toBeNull();
  } finally {
    finishCommand(floatingState);
    await pendingCommand;
    applyTaskbarDockState(initialState);
  }
});

test('compact double-click opens the main window only while floating and no dock command is pending', async () => {
  const taskbar = await import('../src/services/windowsTaskbar');
  const canOpen = taskbar.canOpenMainPlayerFromCompactDoubleClick as
    | ((
        state: import('../src/services/windowsTaskbar').TaskbarDockState
      ) => boolean)
    | undefined;
  const floating = {
    mode: 'floating',
    suspended: false,
    dockError: '',
    enabled: false,
  } as const;

  expect(canOpen?.(floating)).toBe(true);
  expect(canOpen?.({ ...floating, enabled: true })).toBe(false);
  expect(canOpen?.({ ...floating, mode: 'docked' })).toBe(false);
  expect(canOpen?.({ ...floating, mode: 'waiting' })).toBe(false);

  const initialState = getTaskbarDockState();
  let finishCommand!: (value: unknown) => void;
  const nativeResult = new Promise<unknown>(resolve => {
    finishCommand = resolve;
  });
  const command = invokeTaskbarDockCommand(
    { command: 'sync_taskbar_compact_mode', args: { compact: true } },
    async () => nativeResult
  );

  try {
    expect(canOpen?.(floating)).toBe(false);
  } finally {
    finishCommand(floating);
    await command;
    applyTaskbarDockState(initialState);
  }

  expect(canOpen?.(floating)).toBe(true);
});
