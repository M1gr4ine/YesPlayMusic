// Keep controls interactive even when Windows text is used for window dragging.
const MINI_WINDOW_CONTROL_SELECTOR = [
  '.mini-controls',
  '.mini-progress-track',
  'button',
  'a',
  'input',
  'textarea',
  'select',
  '[contenteditable="true"]',
].join(', ');

export const MINI_WINDOW_INTERACTIVE_SELECTOR =
  '.mini-copyable, ' + MINI_WINDOW_CONTROL_SELECTOR;

interface MiniWindowGestureOptions {
  allowTextDrag?: boolean;
}

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
  target: EventTarget | null | undefined,
  options: MiniWindowGestureOptions = {}
): boolean {
  const selector = options.allowTextDrag
    ? MINI_WINDOW_CONTROL_SELECTOR
    : MINI_WINDOW_INTERACTIVE_SELECTOR;
  return hasClosest(target) ? target.closest(selector) !== null : false;
}

export function shouldStartMiniWindowDrag(
  event: MiniWindowPointerEvent,
  options: MiniWindowGestureOptions = {}
): boolean {
  return (
    event?.button === 0 &&
    event?.detail === 1 &&
    !isMiniWindowInteractiveTarget(event.target, options)
  );
}

// Cancel selection on mousedown before WebKit starts a drag selection. Clear
// existing selection explicitly because preventDefault also blocks that action.
export function beginMiniWindowDragGesture(
  event: MiniWindowPointerEvent,
  selection: Selection | null = null,
  options: MiniWindowGestureOptions = {}
): boolean {
  if (!shouldStartMiniWindowDrag(event, options)) return false;
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

export function shouldToggleMiniWindow(
  event: MiniWindowPointerEvent,
  options: MiniWindowGestureOptions = {}
): boolean {
  return (
    event?.button === 0 && !isMiniWindowInteractiveTarget(event.target, options)
  );
}

export function isMiniWindowSize(size: WindowSize | null | undefined): boolean {
  return Number(size?.width) < 620 || Number(size?.height) < 340;
}

// The compact bar is for short windows only; a narrow-but-tall window keeps
// the full player view (isMiniWindowSize still routes it to the lyrics page).
export function isBarWindowSize(size: WindowSize | null | undefined): boolean {
  return Number(size?.height) < 340;
}
