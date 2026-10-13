// Only these mini-player elements keep their native pointer behavior.
// .mini-copyable wraps text tightly so only the text remains selectable;
// a text-locked player (Windows) lets text drag and expand like blank space.
export const MINI_WINDOW_INTERACTIVE_SELECTOR = [
  '.mini-player:not(.mini-text-locked) .mini-copyable',
  '.mini-controls',
  '.mini-progress-track',
  'button',
  'a',
  'input',
  'textarea',
  'select',
  '[contenteditable="true"]',
].join(', ');

interface MiniWindowPointerEvent {
  button?: number;
  detail?: number;
  target?: EventTarget | null;
  clientX?: number;
  clientY?: number;
  preventDefault?: () => void;
}

interface WindowSize {
  width?: number;
  height?: number;
}

interface ClosestTarget {
  closest(selector: string): unknown;
}

function hasClosest(
  target: EventTarget | null | undefined
): target is EventTarget & ClosestTarget {
  return (
    typeof target === 'object' &&
    target !== null &&
    'closest' in target &&
    typeof target.closest === 'function'
  );
}

export function isMiniWindowInteractiveTarget(
  target: EventTarget | null | undefined
): boolean {
  return hasClosest(target)
    ? target.closest(MINI_WINDOW_INTERACTIVE_SELECTOR) !== null
    : false;
}

export function shouldStartMiniWindowDrag(
  event: MiniWindowPointerEvent
): boolean {
  return (
    event?.button === 0 &&
    event?.detail === 1 &&
    !isMiniWindowInteractiveTarget(event.target)
  );
}

// Cancel selection on mousedown before WebKit starts a drag selection. Clear
// existing selection explicitly because preventDefault also blocks that action.
export function beginMiniWindowDragGesture(
  event: MiniWindowPointerEvent,
  selection: Selection | null = null
): boolean {
  if (!shouldStartMiniWindowDrag(event)) return false;
  selection?.removeAllRanges?.();
  event.preventDefault?.();
  return true;
}

export function hasCrossedMiniWindowDragThreshold(
  start: MiniWindowPointerEvent,
  current: MiniWindowPointerEvent,
  threshold = 4
): boolean {
  const deltaX = Number(current?.clientX) - Number(start?.clientX);
  const deltaY = Number(current?.clientY) - Number(start?.clientY);
  return (
    Number.isFinite(deltaX) &&
    Number.isFinite(deltaY) &&
    deltaX * deltaX + deltaY * deltaY >= threshold * threshold
  );
}

export function shouldToggleMiniWindow(event: MiniWindowPointerEvent): boolean {
  return event?.button === 0 && !isMiniWindowInteractiveTarget(event.target);
}

export function isMiniWindowSize(size: WindowSize | null | undefined): boolean {
  return Number(size?.width) < 620 || Number(size?.height) < 340;
}

// The compact bar is for short windows only; a narrow-but-tall window keeps
// the full player view (isMiniWindowSize still routes it to the lyrics page).
export function isBarWindowSize(size: WindowSize | null | undefined): boolean {
  return Number(size?.height) < 340;
}
