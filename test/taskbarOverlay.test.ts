import { expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';

const snapshotFixture = {
  artwork: 'https://example.test/cover.jpg',
  trackTitle: 'Track',
  artistName: 'Artist',
  lyric: 'Current lyric',
  emptyKind: null,
  emptyLabel: '',
  progress: 212,
  duration: 180,
  elapsedLabel: '3:32',
  durationLabel: '3:00',
  playing: true,
  shuffle: false,
  locale: 'zh-CN',
  reducedMotion: false,
};

test('taskbar overlay snapshot exposes only safe UI fields and clamps progress', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const normalizeSnapshot = Reflect.get(
    service,
    'normalizeTaskbarOverlaySnapshot'
  ) as ((value: unknown) => unknown) | undefined;

  expect(
    normalizeSnapshot?.({ ...snapshotFixture, accountToken: 'secret' })
  ).toEqual({ ...snapshotFixture, progress: 180 });
  expect(
    normalizeSnapshot?.({ ...snapshotFixture, locale: 'invalid-locale' })
  ).toBeNull();
  expect(
    normalizeSnapshot?.({ ...snapshotFixture, artistName: '歌'.repeat(171) })
  ).toBeNull();
  expect(
    normalizeSnapshot?.({ ...snapshotFixture, lyric: 'line\u0001break' })
  ).toBeNull();
  expect(
    normalizeSnapshot?.({ ...snapshotFixture, lyric: 'line 1\nline 2' })
  ).toMatchObject({ lyric: 'line 1\nline 2' });
  expect(
    normalizeSnapshot?.({ ...snapshotFixture, duration: 86_401 })
  ).toBeNull();
});

test('taskbar overlay accepts only bounded playback actions from native', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const normalizeAction = Reflect.get(
    service,
    'normalizeTaskbarOverlayAction'
  ) as ((value: unknown) => unknown) | undefined;

  expect(normalizeAction?.({ action: 'play' })).toEqual({ action: 'play' });
  expect(normalizeAction?.({ action: 'seek', value: 42.5 })).toEqual({
    action: 'seek',
    value: 42.5,
  });
  expect(normalizeAction?.({ action: 'seek', value: -1 })).toBeNull();
  expect(normalizeAction?.({ action: 'seek', value: 86_401 })).toBeNull();
  expect(normalizeAction?.({ action: 'close' })).toBeNull();
});

test('taskbar overlay visual state rejects stale or malformed transition tokens', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const normalizeVisual = Reflect.get(
    service,
    'normalizeTaskbarOverlayVisual'
  ) as ((value: unknown) => unknown) | undefined;

  expect(normalizeVisual?.({ phase: 'snapping', token: 7 })).toEqual({
    phase: 'snapping',
    token: 7,
  });
  expect(normalizeVisual?.({ phase: 'docked', token: -1 })).toBeNull();
  expect(normalizeVisual?.({ phase: 'unknown', token: 7 })).toBeNull();
});

test('taskbar overlay flight duration follows native and ignores bad values', async () => {
  const { normalizeTaskbarOverlayVisual } = await import(
    '../src/services/windowsTaskbar'
  );

  expect(
    normalizeTaskbarOverlayVisual({
      phase: 'snapping',
      token: 3,
      durationMs: 240,
    })
  ).toEqual({ phase: 'snapping', token: 3, durationMs: 240 });
  for (const durationMs of [-1, Number.NaN, 2_001, '240']) {
    expect(
      normalizeTaskbarOverlayVisual({
        phase: 'returning',
        token: 3,
        durationMs,
      })
    ).toEqual({ phase: 'returning', token: 3 });
  }
});

test('drag dock hints accept only the three native states', async () => {
  const { normalizeTaskbarDockHint } = await import(
    '../src/services/windowsTaskbar'
  );

  for (const state of ['none', 'ready', 'unavailable'] as const) {
    expect(normalizeTaskbarDockHint({ state })).toBe(state);
  }
  expect(normalizeTaskbarDockHint({ state: 'docked' })).toBeNull();
  expect(normalizeTaskbarDockHint('ready')).toBeNull();
  expect(normalizeTaskbarDockHint(null)).toBeNull();
});

test('the docked strip exposes no WebView2 caption drag region', () => {
  // A caption hit on a taskbar child restyles it with a Win32 title bar.
  const player = readFileSync(
    new URL('../src/components/TaskbarPlayer.vue', import.meta.url),
    'utf8'
  );

  expect(player).not.toMatch(/app-region:\s*drag/);
  expect(player).not.toContain('data-tauri-drag-region');
});

test('taskbar overlay only applies newer visual tokens or a new phase', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const shouldApply = Reflect.get(
    service,
    'shouldApplyTaskbarOverlayVisual'
  ) as
    | ((
        current: { phase: string; token: number },
        next: { phase: string; token: number }
      ) => boolean)
    | undefined;
  const current = { phase: 'snapping', token: 7 };

  expect(shouldApply?.(current, current)).toBe(false);
  expect(shouldApply?.(current, { phase: 'docked', token: 6 })).toBe(false);
  expect(shouldApply?.(current, { phase: 'docked', token: 7 })).toBe(true);
  expect(shouldApply?.(current, { phase: 'returning', token: 8 })).toBe(true);
});

test('taskbar overlay readiness rejects malformed initial state before rendering', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const normalizeReady = Reflect.get(
    service,
    'normalizeTaskbarOverlayReadyState'
  ) as ((value: unknown) => unknown) | undefined;
  const visual = { phase: 'hidden', token: 3 };

  expect(normalizeReady?.({ snapshot: null, visual })).toEqual({
    snapshot: null,
    visual,
  });
  expect(
    normalizeReady?.({ snapshot: snapshotFixture, visual: { phase: 'hidden' } })
  ).toBeNull();
});

test('only the dedicated Tauri overlay label receives the lightweight entry', async () => {
  const { resolveRendererWindowRole } = await import(
    '../src/utils/taskbarOverlayRole'
  );

  expect(resolveRendererWindowRole('#/taskbar-overlay', true, 'main')).toBe(
    'main'
  );
  expect(
    resolveRendererWindowRole('#/taskbar-overlay', true, 'taskbar-lyrics')
  ).toBe('taskbar-overlay');
  expect(resolveRendererWindowRole('#/taskbar-overlay', false, null)).toBe(
    'main'
  );
  expect(resolveRendererWindowRole('#/', true, 'taskbar-lyrics')).toBe(
    'invalid'
  );
  expect(resolveRendererWindowRole('#/taskbar-overlay', true, null)).toBe(
    'invalid'
  );
});

test('overlay seek actions are bounded by the active track duration', async () => {
  const service = await import('../src/services/windowsTaskbar');
  const clampSeek = Reflect.get(service, 'clampTaskbarOverlaySeek') as
    | ((value: number, duration: number) => number | null)
    | undefined;

  expect(clampSeek?.(240, 180)).toBe(180);
  expect(clampSeek?.(42, 180)).toBe(42);
  expect(clampSeek?.(42, 0)).toBeNull();
  expect(clampSeek?.(-1, 180)).toBeNull();
  expect(clampSeek?.(86_400, 86_401)).toBeNull();
});

test('overlay track-time formatting loads from its pure utility module', async () => {
  const { formatTrackTime } = await import('../src/utils/trackTime');

  expect(formatTrackTime(212)).toBe('3:32');
  expect(formatTrackTime(0)).toBe('');
});
