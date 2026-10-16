import { describe, expect, test } from 'bun:test';
import {
  compactBrowseStage,
  COMPACT_ENTER_MS,
  compactExpansionDeadline,
  COMPACT_EXPANSION_WATCHDOG_MS,
  IDLE_COMPACT_EXPANSION,
  nextCompactExpansion,
  resolveAutoLyrics,
  resolveSettledLyrics,
  type CompactExpansionEffect,
  type CompactExpansionEvent,
  type CompactExpansionPhase,
  type CompactExpansionState,
} from '../src/utils/compactExpansion';

function state(
  phase: CompactExpansionPhase,
  patch: Partial<CompactExpansionState> = {}
): CompactExpansionState {
  return { ...IDLE_COMPACT_EXPANSION, motion: 'full', phase, ...patch };
}

const plan = { origin: 'bottom', width: 920, height: 620 } as const;
const queueSize = { width: 920, height: 620 };

function run(start: CompactExpansionState, events: CompactExpansionEvent[]) {
  let current = start;
  const trace: Array<[CompactExpansionPhase, CompactExpansionEffect[]]> = [];
  for (const event of events) {
    const { state: next, effects } = nextCompactExpansion(current, event);
    trace.push([next.phase, effects]);
    current = next;
  }
  return { state: current, trace };
}

describe('小窗展开状态机', () => {
  test('Windows 分阶段：先淡出播放条，缩放成功后才进队列再入场', () => {
    const { state: end, trace } = run(IDLE_COMPACT_EXPANSION, [
      { type: 'expand', motion: 'full' },
      { type: 'planned', plan },
      { type: 'barHidden' },
      { type: 'resized', size: queueSize },
      { type: 'queueShown', onQueue: true },
      { type: 'entered' },
    ]);
    expect(trace).toEqual([
      ['planning', ['plan']],
      ['leaving', ['hideBar']],
      ['resizing', ['resize']],
      ['showing', ['showQueue']],
      ['entering', []],
      ['idle', ['settle']],
    ]);
    expect(end).toEqual({
      phase: 'idle',
      motion: 'full',
      plan,
      applied: queueSize,
      queueHeld: true,
    });
  });

  test('macOS/Linux 不淡出也不入场，直接缩放后进队列', () => {
    const { state: end, trace } = run(IDLE_COMPACT_EXPANSION, [
      { type: 'expand', motion: 'none' },
      { type: 'planned', plan },
      { type: 'resized', size: queueSize },
      { type: 'queueShown', onQueue: true },
    ]);
    expect(trace).toEqual([
      ['planning', ['plan']],
      ['resizing', ['resize']],
      ['showing', ['showQueue']],
      ['idle', ['settle']],
    ]);
    expect(end.queueHeld).toBe(true);
  });

  const busyPhases: CompactExpansionPhase[] = [
    'planning',
    'leaving',
    'resizing',
    'showing',
    'entering',
    'reverting',
    'restoring',
  ];

  test.each(busyPhases)('%s 期间重复双击被忽略', phase => {
    const busy = state(phase);
    const result = nextCompactExpansion(busy, {
      type: 'expand',
      motion: 'full',
    });
    expect(result.state).toBe(busy);
    expect(result.effects).toEqual([]);
  });

  test.each(busyPhases)('%s 期间按 Esc 不会触发原生缩回', phase => {
    const busy = state(phase);
    const result = nextCompactExpansion(busy, { type: 'restore' });
    expect(result.state).toBe(busy);
    expect(result.effects).toEqual([]);
  });

  test('空闲时 Esc 缩回，完成后回到空闲', () => {
    const { trace } = run(state('idle'), [
      { type: 'restore' },
      { type: 'restored' },
    ]);
    expect(trace).toEqual([
      ['restoring', ['restore']],
      ['idle', []],
    ]);
  });

  test.each<[CompactExpansionPhase, CompactExpansionEvent]>([
    // A normal lyrics close also fires after-leave.
    ['idle', { type: 'barHidden' }],
    ['idle', { type: 'barShown' }],
    ['idle', { type: 'entered' }],
    // Fallback timers racing a real end event.
    ['resizing', { type: 'barHidden' }],
    ['resizing', { type: 'entered' }],
    ['leaving', { type: 'barShown' }],
    ['entering', { type: 'resized', size: queueSize }],
    // A watchdog 'restored' outliving its restore, then a new expansion.
    ['idle', { type: 'restored' }],
    ['planning', { type: 'restored' }],
    ['leaving', { type: 'restored' }],
    // Watchdog failures firing after the phase already advanced.
    ['showing', { type: 'resizeFailed' }],
    ['leaving', { type: 'planFailed' }],
    ['idle', { type: 'queueShown', onQueue: false }],
  ])('%s 阶段收到过期的 %o 不改变状态', (phase, event) => {
    const current = state(phase);
    const result = nextCompactExpansion(current, event);
    expect(result.state).toBe(current);
    expect(result.effects).toEqual([]);
  });

  test('原生缩放失败时淡回播放条，不推队列路由', () => {
    const { state: end, trace } = run(state('resizing'), [
      { type: 'resizeFailed' },
      { type: 'barShown' },
    ]);
    expect(trace).toEqual([
      ['reverting', ['showBar']],
      ['idle', ['settle']],
    ]);
    expect(trace.flatMap(([, effects]) => effects)).not.toContain('showQueue');
    expect(end.queueHeld).toBe(false);
  });

  test('直接缩放失败时只做一次对齐', () => {
    expect(
      nextCompactExpansion(state('resizing', { motion: 'none' }), {
        type: 'resizeFailed',
      })
    ).toEqual({
      state: state('idle', { motion: 'none' }),
      effects: ['settle'],
    });
  });

  test('拿不到展开计划时不动界面', () => {
    expect(
      nextCompactExpansion(state('planning'), { type: 'planFailed' })
    ).toEqual({ state: state('idle'), effects: [] });
  });

  test('导航没落到队列时不保留队列', () => {
    expect(
      nextCompactExpansion(state('showing'), {
        type: 'queueShown',
        onQueue: false,
      }).state.queueHeld
    ).toBe(false);
  });

  test('展开途中的 becameBar 和进队列的路由变化不打断流程', () => {
    const { state: end, trace } = run(IDLE_COMPACT_EXPANSION, [
      { type: 'expand', motion: 'full' },
      // Still bar-sized until the native resize lands.
      { type: 'becameBar' },
      { type: 'planned', plan },
      { type: 'becameBar' },
      { type: 'barHidden' },
      { type: 'resized', size: queueSize },
      { type: 'routeChanged', name: 'next' },
      { type: 'queueShown', onQueue: true },
      { type: 'routeChanged', name: 'next' },
      { type: 'entered' },
    ]);
    expect(trace.map(([phase]) => phase)).toEqual([
      'planning',
      'planning',
      'leaving',
      'leaving',
      'resizing',
      'showing',
      'showing',
      'entering',
      'entering',
      'idle',
    ]);
    expect(end.queueHeld).toBe(true);
  });

  test('入场途中离开队列只结束队列选择，不改变阶段', () => {
    const { state: end } = run(state('entering', { queueHeld: true }), [
      { type: 'routeChanged', name: 'home' },
    ]);
    expect(end).toEqual(state('entering', { queueHeld: false }));
  });

  test('离开队列或缩回播放条才结束显式的队列选择', () => {
    const held = state('idle', { queueHeld: true });
    expect(
      nextCompactExpansion(held, { type: 'routeChanged', name: 'next' }).state
    ).toBe(held);
    expect(
      nextCompactExpansion(held, { type: 'routeChanged', name: 'home' }).state
        .queueHeld
    ).toBe(false);
    expect(
      nextCompactExpansion(held, { type: 'becameBar' }).state.queueHeld
    ).toBe(false);
  });
});

describe('展开阶段的超时兜底', () => {
  test('异步阶段卡住时走既有的失败事件，淡出的播放条会被淡回', () => {
    const stuck = state('resizing');
    const deadline = compactExpansionDeadline(stuck);
    expect(deadline).toEqual({
      event: { type: 'resizeFailed' },
      ms: COMPACT_EXPANSION_WATCHDOG_MS,
    });
    if (!deadline) throw new Error('resizing must have a deadline');
    const { state: next, effects } = nextCompactExpansion(
      stuck,
      deadline.event
    );
    expect(next.phase).toBe('reverting');
    expect(effects).toEqual(['showBar']);
  });

  test('每个忙碌阶段都有且只有一个收尾事件，空闲没有', () => {
    const expected: Record<
      CompactExpansionPhase,
      CompactExpansionEvent['type'] | null
    > = {
      idle: null,
      planning: 'planFailed',
      leaving: 'barHidden',
      resizing: 'resizeFailed',
      showing: 'queueShown',
      entering: 'entered',
      reverting: 'barShown',
      restoring: 'restored',
    };
    for (const [phase, type] of Object.entries(expected)) {
      const current = state(phase as CompactExpansionPhase);
      const deadline = compactExpansionDeadline(current);
      expect(deadline?.event.type ?? null).toBe(type);
      // Each one actually leaves the phase it guards.
      if (deadline) {
        expect(
          nextCompactExpansion(current, deadline.event).state.phase
        ).not.toBe(phase);
      }
    }
  });

  test('动画兜底比动画本身长', () => {
    const entering = compactExpansionDeadline(state('entering'));
    expect(entering?.ms).toBeGreaterThan(COMPACT_ENTER_MS.full);
  });
});

describe('浏览视图的展开阶段', () => {
  test('macOS/Linux 从不隐藏浏览视图', () => {
    for (const phase of [
      'leaving',
      'resizing',
      'showing',
      'entering',
    ] as const) {
      expect(compactBrowseStage(state(phase, { motion: 'none' }))).toBeNull();
    }
  });

  test('Windows 在淡出、缩放、回退期间隐藏，入场时播放动画', () => {
    expect(compactBrowseStage(state('planning'))).toBeNull();
    expect(compactBrowseStage(state('leaving'))).toBe('hidden');
    expect(compactBrowseStage(state('resizing'))).toBe('hidden');
    expect(compactBrowseStage(state('showing'))).toBe('hidden');
    expect(compactBrowseStage(state('reverting'))).toBe('hidden');
    expect(compactBrowseStage(state('entering', { motion: 'fade' }))).toBe(
      'entering'
    );
    expect(compactBrowseStage(state('idle'))).toBeNull();
  });
});

describe('窄窗自动打开歌词', () => {
  const narrowQueue = { isMini: true, showLyrics: false, autoOpened: false };

  test('双击展开到队列后，窄窗不再被切回歌词', () => {
    expect(
      resolveAutoLyrics(state('idle', { queueHeld: true }), narrowQueue)
    ).toBeNull();
  });

  test('没有显式选择队列时，窄窗照旧打开歌词', () => {
    expect(resolveAutoLyrics(state('idle'), narrowQueue)).toBe('open');
  });

  test('离开队列后恢复原有规则', () => {
    const left = nextCompactExpansion(state('idle', { queueHeld: true }), {
      type: 'routeChanged',
      name: 'home',
    }).state;
    expect(resolveAutoLyrics(left, narrowQueue)).toBe('open');
  });

  test('展开过程中由状态机接管歌词页，不随中间尺寸翻转', () => {
    for (const phase of [
      'planning',
      'leaving',
      'resizing',
      'showing',
    ] as const) {
      expect(resolveAutoLyrics(state(phase), narrowQueue)).toBeNull();
      expect(
        resolveAutoLyrics(state(phase), {
          isMini: false,
          showLyrics: true,
          autoOpened: true,
        })
      ).toBeNull();
    }
  });

  test('收尾时按已应用的尺寸判断，窗口尺寸没跟上也不会在队列上重开歌词', () => {
    // macOS: native set_size returned, window.inner* still reports the bar.
    const { state: settled } = run(IDLE_COMPACT_EXPANSION, [
      { type: 'expand', motion: 'none' },
      { type: 'planned', plan },
      { type: 'resized', size: queueSize },
      { type: 'queueShown', onQueue: false },
    ]);
    const staleBar = { width: 520, height: 80 };
    const view = { showLyrics: false, autoOpened: false };
    expect(resolveSettledLyrics(settled, staleBar, view)).toBeNull();
    // Without an applied size (failed resize) the live size still decides.
    expect(
      resolveSettledLyrics({ ...settled, applied: null }, staleBar, view)
    ).toBe('open');
  });

  test('变宽只关闭自动打开的歌词', () => {
    const wide = { isMini: false, showLyrics: true };
    expect(
      resolveAutoLyrics(state('idle'), { ...wide, autoOpened: true })
    ).toBe('close');
    expect(
      resolveAutoLyrics(state('idle'), { ...wide, autoOpened: false })
    ).toBeNull();
  });
});
