import { isMiniWindowSize } from '@/utils/miniWindow';

// Compact bar -> play queue expansion as a pure state machine. App.vue owns the
// side effects; this module only decides what happens next.

// The edge the bar stays on while the window grows away from it.
export type CompactExpansionOrigin = 'top' | 'bottom';

// 'none' resizes directly (macOS/Linux); the others stage the Windows fade.
export type CompactExpansionMotion = 'none' | 'fade' | 'full';

export type CompactExpansionPhase =
  | 'idle'
  | 'planning'
  | 'leaving'
  | 'resizing'
  | 'showing'
  | 'entering'
  | 'reverting'
  | 'restoring';

interface WindowSize {
  width: number;
  height: number;
}

// Only the direction and logical size are planned: the bar stays draggable
// during the fade, so the position is read again right before resizing.
export interface CompactExpansionPlan extends WindowSize {
  origin: CompactExpansionOrigin;
}

export interface CompactExpansionState {
  phase: CompactExpansionPhase;
  motion: CompactExpansionMotion;
  plan: CompactExpansionPlan | null;
  // Logical size the native resize applied; window.inner* may still lag.
  applied: WindowSize | null;
  // The user expanded into the queue; narrow windows must not flip to lyrics.
  queueHeld: boolean;
}

export type CompactExpansionEvent =
  | { type: 'expand'; motion: CompactExpansionMotion }
  | { type: 'planned'; plan: CompactExpansionPlan }
  | { type: 'planFailed' }
  | { type: 'barHidden' }
  | { type: 'resized'; size: WindowSize }
  | { type: 'resizeFailed' }
  | { type: 'queueShown'; onQueue: boolean }
  | { type: 'entered' }
  | { type: 'barShown' }
  | { type: 'restore' }
  | { type: 'restored' }
  | { type: 'routeChanged'; name: unknown }
  | { type: 'becameBar' };

export type CompactExpansionEffect =
  | 'plan'
  | 'hideBar'
  | 'resize'
  | 'showQueue'
  | 'showBar'
  | 'restore'
  | 'settle';

export interface CompactExpansionTransition {
  state: CompactExpansionState;
  effects: CompactExpansionEffect[];
}

export const COMPACT_BAR_FADE_MS = 120;
export const COMPACT_ENTER_MS: Readonly<
  Record<CompactExpansionMotion, number>
> = Object.freeze({ none: 0, fade: 120, full: 420 });
// End events never fire if the element is hidden, removed or the transition
// is cancelled.
const END_EVENT_GRACE_MS = 250;
// Native calls and navigation have no inherent bound; never leave the bar
// faded out because one of them hung.
export const COMPACT_EXPANSION_WATCHDOG_MS = 10_000;

export const IDLE_COMPACT_EXPANSION: CompactExpansionState = Object.freeze({
  phase: 'idle',
  motion: 'none',
  plan: null,
  applied: null,
  queueHeld: false,
});

export function nextCompactExpansion(
  state: CompactExpansionState,
  event: CompactExpansionEvent
): CompactExpansionTransition {
  const to = (
    patch: Partial<CompactExpansionState>,
    ...effects: CompactExpansionEffect[]
  ): CompactExpansionTransition => ({ state: { ...state, ...patch }, effects });
  const stay: CompactExpansionTransition = { state, effects: [] };
  const staged = state.motion !== 'none';

  // Leaving the queue or shrinking back to a bar ends the explicit choice.
  if (event.type === 'routeChanged' || event.type === 'becameBar') {
    const left = event.type === 'becameBar' || event.name !== 'next';
    return state.queueHeld && left ? to({ queueHeld: false }) : stay;
  }

  switch (state.phase) {
    case 'idle':
      if (event.type === 'expand') {
        return to(
          {
            phase: 'planning',
            motion: event.motion,
            plan: null,
            applied: null,
          },
          'plan'
        );
      }
      if (event.type === 'restore') {
        return to({ phase: 'restoring' }, 'restore');
      }
      break;
    case 'planning':
      if (event.type === 'planned') {
        return staged
          ? to({ phase: 'leaving', plan: event.plan }, 'hideBar')
          : to({ phase: 'resizing', plan: event.plan }, 'resize');
      }
      if (event.type === 'planFailed') {
        return to({ phase: 'idle' });
      }
      break;
    case 'leaving':
      if (event.type === 'barHidden') {
        return to({ phase: 'resizing' }, 'resize');
      }
      break;
    case 'resizing':
      if (event.type === 'resized') {
        return to({ phase: 'showing', applied: event.size }, 'showQueue');
      }
      if (event.type === 'resizeFailed') {
        return staged
          ? to({ phase: 'reverting' }, 'showBar')
          : to({ phase: 'idle' }, 'settle');
      }
      break;
    case 'showing':
      if (event.type === 'queueShown') {
        return staged
          ? to({ phase: 'entering', queueHeld: event.onQueue })
          : to({ phase: 'idle', queueHeld: event.onQueue }, 'settle');
      }
      break;
    case 'entering':
      if (event.type === 'entered') {
        return to({ phase: 'idle' }, 'settle');
      }
      break;
    case 'reverting':
      if (event.type === 'barShown') {
        return to({ phase: 'idle' }, 'settle');
      }
      break;
    case 'restoring':
      if (event.type === 'restored') {
        return to({ phase: 'idle' });
      }
      break;
  }
  // Double triggers, Esc mid-animation and stale end events are ignored.
  return stay;
}

// Every busy phase has exactly one event that ends it: the real completion
// signal, or a failure. Firing it late is harmless because the machine drops
// events that do not belong to the current phase.
export function compactExpansionDeadline(
  state: CompactExpansionState
): { event: CompactExpansionEvent; ms: number } | null {
  const grace = (ms: number) => ms + END_EVENT_GRACE_MS;
  const watchdog = COMPACT_EXPANSION_WATCHDOG_MS;
  switch (state.phase) {
    case 'planning':
      return { event: { type: 'planFailed' }, ms: watchdog };
    case 'leaving':
      return { event: { type: 'barHidden' }, ms: grace(COMPACT_BAR_FADE_MS) };
    case 'resizing':
      return { event: { type: 'resizeFailed' }, ms: watchdog };
    case 'showing':
      // Navigation hung after a good resize: settle without holding the queue.
      return { event: { type: 'queueShown', onQueue: false }, ms: watchdog };
    case 'entering':
      return {
        event: { type: 'entered' },
        ms: grace(COMPACT_ENTER_MS[state.motion]),
      };
    case 'reverting':
      return { event: { type: 'barShown' }, ms: grace(COMPACT_BAR_FADE_MS) };
    case 'restoring':
      return { event: { type: 'restored' }, ms: watchdog };
    case 'idle':
      return null;
  }
}

// Which stage the browse chrome (main, nav, player) is in on Windows.
export function compactBrowseStage(
  state: CompactExpansionState
): 'hidden' | 'entering' | null {
  if (state.motion === 'none') return null;
  switch (state.phase) {
    case 'leaving':
    case 'resizing':
    case 'showing':
    case 'reverting':
      return 'hidden';
    case 'entering':
      return 'entering';
    default:
      return null;
  }
}

export type AutoLyricsAction = 'open' | 'close' | null;

// Narrow or short windows open the lyrics page and widening closes it again,
// except while an expansion runs or after the user explicitly chose the queue.
export function resolveAutoLyrics(
  state: CompactExpansionState,
  view: { isMini: boolean; showLyrics: boolean; autoOpened: boolean }
): AutoLyricsAction {
  if (state.phase !== 'idle' && state.phase !== 'restoring') return null;
  if (view.isMini && !view.showLyrics && !state.queueHeld) return 'open';
  if (!view.isMini && view.showLyrics && view.autoOpened) return 'close';
  return null;
}

// Settling runs right after native calls return, when window.inner* may not
// have caught up yet. Judge by the applied frame so a stale bar-sized read
// cannot reopen lyrics over the queue the user asked for.
export function resolveSettledLyrics(
  state: CompactExpansionState,
  liveSize: WindowSize,
  view: { showLyrics: boolean; autoOpened: boolean }
): AutoLyricsAction {
  return resolveAutoLyrics(state, {
    isMini: isMiniWindowSize(state.applied ?? liveSize),
    ...view,
  });
}
