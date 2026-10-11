import { expect, test } from 'bun:test';
import {
  WINDOWS_COMPACT_EXPANSION_EXIT_MS,
  WINDOWS_COMPACT_EXPANSION_ENTRANCE_MS,
  WINDOWS_REDUCED_MOTION_ENTRANCE_MS,
  shouldAutoOpenLyricsOnResize,
  resolveMiniModeDuringCompactExpansion,
  runStagedCompactExpansion,
} from '../src/services/compactExpansionTransition';

test('staged expansion overlaps browse entrance with the lyric exit and holds through it', async () => {
  const steps: string[] = [];
  let now = 0;
  const expanded = await runStagedCompactExpansion(
    {
      prepareRoute: async () => {
        steps.push('route');
      },
      beginExit: () => {
        steps.push('exit');
      },
      expand: async () => {
        steps.push('expand');
        return true;
      },
      beginEntrance: () => {
        steps.push('entrance');
      },
      finish: () => {
        steps.push('finish');
      },
    },
    {
      reducedMotion: false,
      now: () => now,
      wait: async milliseconds => {
        steps.push(`wait:${milliseconds}`);
        now += milliseconds;
      },
      waitForPaint: async () => {
        steps.push('paint');
      },
    }
  );

  expect(expanded).toBe(true);
  expect(steps).toEqual([
    'route',
    'exit',
    'wait:80',
    'expand',
    'paint',
    'entrance',
    'wait:436',
    'finish',
  ]);
});

test('reduced motion reveals after paint with a shorter entrance', async () => {
  const steps: string[] = [];
  const expanded = await runStagedCompactExpansion(
    {
      prepareRoute: async () => {
        steps.push('route');
      },
      beginExit: () => {
        steps.push('exit');
      },
      expand: async () => {
        steps.push('expand');
        return true;
      },
      beginEntrance: () => {
        steps.push('entrance');
      },
      finish: () => {
        steps.push('finish');
      },
    },
    {
      reducedMotion: true,
      waitForPaint: async () => {
        steps.push('paint');
      },
      wait: async () => {
        steps.push('reduced-entrance');
      },
    }
  );

  expect(expanded).toBe(true);
  expect(WINDOWS_REDUCED_MOTION_ENTRANCE_MS).toBe(120);
  expect(steps).toEqual([
    'route',
    'exit',
    'expand',
    'paint',
    'entrance',
    'reduced-entrance',
    'finish',
  ]);
});

test('starts the browse entrance before lyric exit ends after a painted resize frame', async () => {
  const steps: string[] = [];
  let now = 0;
  const expanded = await runStagedCompactExpansion(
    {
      prepareRoute: async () => {
        steps.push('route');
      },
      beginExit: () => {
        steps.push('exit');
      },
      expand: async () => {
        steps.push('native-resize');
        now += 20;
        return true;
      },
      beginEntrance: () => {
        expect(now).toBeLessThan(WINDOWS_COMPACT_EXPANSION_EXIT_MS);
        steps.push('entrance');
      },
      finish: () => {
        steps.push('finish');
      },
    },
    {
      reducedMotion: false,
      now: () => now,
      wait: async milliseconds => {
        steps.push(`wait:${milliseconds}`);
        now += milliseconds;
      },
      waitForPaint: async () => {
        steps.push('paint');
      },
    }
  );

  expect(expanded).toBe(true);
  expect(WINDOWS_COMPACT_EXPANSION_EXIT_MS).toBe(180);
  expect(WINDOWS_COMPACT_EXPANSION_ENTRANCE_MS).toBe(436);
  expect(steps).toEqual([
    'route',
    'exit',
    'wait:80',
    'native-resize',
    'paint',
    'entrance',
    'wait:436',
    'finish',
  ]);
});

test('failed native expansion still ends the staged UI state', async () => {
  const steps: string[] = [];
  const expanded = await runStagedCompactExpansion(
    {
      prepareRoute: async () => {
        steps.push('route');
      },
      beginExit: () => {
        steps.push('exit');
      },
      expand: async () => {
        steps.push('expand');
        return false;
      },
      beginEntrance: () => {
        steps.push('entrance');
      },
      finish: () => {
        steps.push('finish');
      },
    },
    { reducedMotion: true }
  );

  expect(expanded).toBe(false);
  expect(steps).toEqual(['route', 'exit', 'expand', 'finish']);
});

test('failed resize settles the lyric exit without starting the browse entrance', async () => {
  const steps: string[] = [];
  let now = 0;
  const expanded = await runStagedCompactExpansion(
    {
      prepareRoute: async () => {
        steps.push('route');
      },
      beginExit: () => {
        steps.push('exit');
      },
      expand: async () => {
        steps.push('expand');
        now += 20;
        return false;
      },
      beginEntrance: () => {
        steps.push('entrance');
      },
      finish: () => {
        steps.push('finish');
      },
    },
    {
      reducedMotion: false,
      now: () => now,
      wait: async milliseconds => {
        steps.push(`wait:${milliseconds}`);
        now += milliseconds;
      },
      waitForPaint: async () => {
        steps.push('paint');
      },
    }
  );

  expect(expanded).toBe(false);
  expect(steps).toEqual([
    'route',
    'exit',
    'wait:80',
    'expand',
    'wait:80',
    'finish',
  ]);
});

test('native expansion errors still release the staged UI state', async () => {
  const steps: string[] = [];
  await expect(
    runStagedCompactExpansion(
      {
        prepareRoute: async () => {
          steps.push('route');
        },
        beginExit: () => {
          steps.push('exit');
        },
        expand: async () => {
          steps.push('expand');
          throw new Error('resize failed');
        },
        beginEntrance: () => {
          steps.push('entrance');
        },
        finish: () => {
          steps.push('finish');
        },
      },
      { reducedMotion: true }
    )
  ).rejects.toThrow('resize failed');
  expect(steps).toEqual(['route', 'exit', 'expand', 'finish']);
});

test('throws during route preparation still releases the busy transition state', async () => {
  const steps: string[] = [];
  await expect(
    runStagedCompactExpansion(
      {
        prepareRoute: async () => {
          steps.push('route');
          throw new Error('route failed');
        },
        beginExit: () => {
          steps.push('exit');
        },
        expand: async () => {
          steps.push('expand');
          return true;
        },
        beginEntrance: () => {
          steps.push('entrance');
        },
        finish: () => {
          steps.push('finish');
        },
      },
      { reducedMotion: true }
    )
  ).rejects.toThrow('route failed');
  expect(steps).toEqual(['route', 'finish']);
});

test('keeps compact rendering during expansion resize and reevaluates after exit', () => {
  const expandedSize = { width: 920, height: 620 };
  expect(resolveMiniModeDuringCompactExpansion(true, true, expandedSize)).toBe(
    true
  );
  expect(resolveMiniModeDuringCompactExpansion(true, false, expandedSize)).toBe(
    false
  );
});

test('suppresses automatic lyrics for a narrow tall Windows queue after explicit expansion', () => {
  expect(
    shouldAutoOpenLyricsOnResize(true, false, true, {
      width: 560,
      height: 620,
    })
  ).toBe(false);
});

test('allows automatic lyrics after returning to the compact bar or on other platforms', () => {
  expect(
    shouldAutoOpenLyricsOnResize(true, false, true, {
      width: 560,
      height: 120,
    })
  ).toBe(true);
  expect(
    shouldAutoOpenLyricsOnResize(false, false, true, {
      width: 560,
      height: 620,
    })
  ).toBe(true);
});

test('suppresses automatic lyrics while the Windows expansion resize is active', () => {
  expect(
    shouldAutoOpenLyricsOnResize(true, true, false, {
      width: 560,
      height: 120,
    })
  ).toBe(false);
});
