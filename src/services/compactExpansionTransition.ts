import { isBarWindowSize, isMiniWindowSize } from '@/utils/miniWindow';

export const WINDOWS_COMPACT_EXPANSION_STAGE_MS = 80;
// Match the compact lyrics leave transition; successful expansion overlaps its entrance.
export const WINDOWS_COMPACT_EXPANSION_EXIT_MS = 180;
export const WINDOWS_COMPACT_EXPANSION_ENTRANCE_MS = 436;
export const WINDOWS_REDUCED_MOTION_ENTRANCE_MS = 120;

export interface StagedCompactExpansionSteps {
  prepareRoute(): Promise<void>;
  beginExit(): void;
  expand(): Promise<boolean>;
  beginEntrance(): void;
  finish(): void;
}

export interface StagedCompactExpansionOptions {
  reducedMotion: boolean;
  wait?: (milliseconds: number) => Promise<void>;
  waitForPaint?: () => Promise<void>;
  now?: () => number;
}

function waitForStage(milliseconds: number) {
  return new Promise<void>(resolve => setTimeout(resolve, milliseconds));
}

function waitForNextPaint() {
  return new Promise<void>(resolve => {
    if (typeof window === 'undefined') {
      setTimeout(resolve, 0);
      return;
    }
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => resolve());
    });
  });
}

export async function runStagedCompactExpansion(
  steps: StagedCompactExpansionSteps,
  options: StagedCompactExpansionOptions
): Promise<boolean> {
  const wait = options.wait ?? waitForStage;
  const waitForPaint = options.waitForPaint ?? waitForNextPaint;
  const now = options.now ?? Date.now;
  let exitStartedAt: number | null = null;
  let exitSettled = false;

  const settleExit = async () => {
    if (exitStartedAt === null || exitSettled || options.reducedMotion) {
      exitSettled = true;
      return;
    }
    const remaining =
      WINDOWS_COMPACT_EXPANSION_EXIT_MS - (now() - exitStartedAt);
    if (remaining > 0) await wait(remaining);
    exitSettled = true;
  };

  try {
    await steps.prepareRoute();
    exitStartedAt = now();
    steps.beginExit();
    if (!options.reducedMotion) {
      await wait(WINDOWS_COMPACT_EXPANSION_STAGE_MS);
    }
    const expanded = await steps.expand();
    if (!expanded) return false;

    await waitForPaint();
    steps.beginEntrance();
    await wait(
      options.reducedMotion
        ? WINDOWS_REDUCED_MOTION_ENTRANCE_MS
        : WINDOWS_COMPACT_EXPANSION_ENTRANCE_MS
    );
    return true;
  } finally {
    try {
      await settleExit();
    } finally {
      steps.finish();
    }
  }
}

export function resolveMiniModeDuringCompactExpansion(
  currentMini: boolean,
  exitActive: boolean,
  size: { width?: unknown; height?: unknown }
): boolean {
  return exitActive
    ? currentMini
    : isBarWindowSize({
        width: Number(size.width),
        height: Number(size.height),
      });
}

export function shouldAutoOpenLyricsOnResize(
  windowsFeatureActive: boolean,
  expansionBusy: boolean,
  explicitQueueAfterExpansion: boolean,
  size: { width?: unknown; height?: unknown }
): boolean {
  const normalizedSize = {
    width: Number(size.width),
    height: Number(size.height),
  };
  if (!isMiniWindowSize(normalizedSize)) return false;
  if (!windowsFeatureActive) return true;
  if (expansionBusy) return false;

  return !(explicitQueueAfterExpansion && !isBarWindowSize(normalizedSize));
}
