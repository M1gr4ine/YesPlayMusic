import { isTauriRuntime } from '@/utils/runtime';
import { isBarWindowSize } from '@/utils/miniWindow';

export const COMPACT_EXPANDED_SIZE = Object.freeze({ width: 920, height: 620 });
export const COMPACT_RESIZE_SETTLE_MS = 250;
export const COMPACT_WINDOW_MEMORY_KEY = 'compactWindowFrames.v1';

const MIN_WINDOW_SIZE = Object.freeze({ width: 300, height: 48 });
const MAX_WINDOW_EDGE = 8192;
let compactWindowTransitioning = false;

export interface CompactWindowFrame {
  x: number | null;
  y: number | null;
  width: number;
  height: number;
}

interface CompactWindowFrameInput {
  x?: unknown;
  y?: unknown;
  width?: unknown;
  height?: unknown;
}

type CompactWindowMode = 'bar' | 'browse';

interface CompactWindowMemory {
  bar: CompactWindowFrame | null;
  browse: CompactWindowFrame | null;
  lastMode: CompactWindowMode | null;
}

type CompactWindowStorage = Pick<Storage, 'getItem' | 'setItem'>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function browserStorage(): CompactWindowStorage | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage;
  } catch (_) {
    return null;
  }
}

function normalizeCompactWindowFrame(
  frame: CompactWindowFrameInput | null | undefined
): CompactWindowFrame | null {
  const width = Number(frame?.width);
  const height = Number(frame?.height);
  if (
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width < MIN_WINDOW_SIZE.width ||
    height < MIN_WINDOW_SIZE.height ||
    width > MAX_WINDOW_EDGE ||
    height > MAX_WINDOW_EDGE
  ) {
    return null;
  }
  const rawX = frame?.x == null ? null : Number(frame.x);
  const rawY = frame?.y == null ? null : Number(frame.y);
  return {
    x: rawX !== null && Number.isFinite(rawX) ? Math.round(rawX) : null,
    y: rawY !== null && Number.isFinite(rawY) ? Math.round(rawY) : null,
    width: Math.round(width),
    height: Math.round(height),
  };
}

export function loadCompactWindowMemory(
  storage: CompactWindowStorage | null = browserStorage()
): CompactWindowMemory {
  const empty: CompactWindowMemory = {
    bar: null,
    browse: null,
    lastMode: null,
  };
  if (!storage) return empty;
  try {
    const raw = storage.getItem(COMPACT_WINDOW_MEMORY_KEY);
    if (raw === null) return empty;
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value)) return empty;
    const bar = isRecord(value['bar']) ? value['bar'] : null;
    const browse = isRecord(value['browse']) ? value['browse'] : null;
    const lastMode = value['lastMode'];
    return {
      bar: normalizeCompactWindowFrame(bar),
      browse: normalizeCompactWindowFrame(browse),
      lastMode: lastMode === 'bar' || lastMode === 'browse' ? lastMode : null,
    };
  } catch (_) {
    return empty;
  }
}

export function rememberCompactWindowFrame(
  frame: CompactWindowFrameInput,
  storage: CompactWindowStorage | null = browserStorage()
): CompactWindowMemory {
  const normalized = normalizeCompactWindowFrame(frame);
  const memory = loadCompactWindowMemory(storage);
  if (!normalized || !storage) return memory;
  const mode: CompactWindowMode = isBarWindowSize(normalized)
    ? 'bar'
    : 'browse';
  const next: CompactWindowMemory = {
    ...memory,
    [mode]: normalized,
    lastMode: mode,
  };
  try {
    storage.setItem(COMPACT_WINDOW_MEMORY_KEY, JSON.stringify(next));
  } catch (_) {
    // Storage failure must not block the current window transition.
  }
  return next;
}

export function hasRememberedBarFrame(
  storage: CompactWindowStorage | null = browserStorage()
): boolean {
  return Boolean(loadCompactWindowMemory(storage).bar);
}

export function buildCompactWindowTransitionFrame(
  currentFrame: CompactWindowFrameInput,
  targetFrame: CompactWindowFrameInput
): CompactWindowFrame | null {
  const current = normalizeCompactWindowFrame(currentFrame);
  const target = normalizeCompactWindowFrame(targetFrame);
  if (!current || !target) return null;

  return {
    x: current.x,
    y: current.y,
    width: target.width,
    height: target.height,
  };
}

export function isCompactWindowPhysicalSize(
  size: { width?: unknown; height?: unknown } | null | undefined,
  scaleFactor = 1
): boolean {
  const scale = Number(scaleFactor);
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  return isBarWindowSize({
    width: Number(size?.width) / safeScale,
    height: Number(size?.height) / safeScale,
  });
}

async function captureCurrentCompactWindowSnapshot() {
  if (!isTauriRuntime) return null;

  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  const window = getCurrentWindow();
  const [size, position, scaleFactor, maximized, fullscreen] =
    await Promise.all([
      window.innerSize(),
      window.outerPosition(),
      window.scaleFactor(),
      window.isMaximized(),
      window.isFullscreen(),
    ]);
  return {
    frame: normalizeCompactWindowFrame({
      x: position.x,
      y: position.y,
      // Persist logical pixels across displays with different scale factors.
      width: size.width / scaleFactor,
      height: size.height / scaleFactor,
    }),
    normal: !maximized && !fullscreen,
  };
}

async function applyTauriCompactWindowFrame(
  frame: CompactWindowFrame
): Promise<boolean> {
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  const window = getCurrentWindow();
  if (await window.isFullscreen()) {
    await window.setFullscreen(false);
    // Wait for the native fullscreen transition before resizing.
    const deadline = Date.now() + 1500;
    while ((await window.isFullscreen()) && Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    await new Promise(resolve => setTimeout(resolve, 350));
  }
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('restore_compact_window', { ...frame });
  return true;
}

async function applyCompactWindowFrame(
  frame: CompactWindowFrame
): Promise<boolean> {
  if (!isTauriRuntime) return false;
  return applyTauriCompactWindowFrame(frame);
}

export async function restoreRememberedCompactWindowFrame() {
  const memory = loadCompactWindowMemory();
  const mode =
    memory.lastMode || (memory.browse ? 'browse' : memory.bar ? 'bar' : null);
  const frame = mode ? memory[mode] : null;
  if (!frame) return null;
  const current = (await captureCurrentCompactWindowSnapshot())?.frame;
  // Native position is newer than the renderer's mode-specific size memory.
  const target = current
    ? buildCompactWindowTransitionFrame(current, frame)
    : frame;
  if (!target) return null;
  const restored = await applyCompactWindowFrame(target);
  return restored ? { mode, frame: target } : null;
}

export async function signalInitialWindowReady(): Promise<boolean> {
  if (!isTauriRuntime) return false;
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<boolean>('renderer_ready');
}

export async function rememberCurrentCompactWindowFrame() {
  const snapshot = await captureCurrentCompactWindowSnapshot();
  const frame = snapshot?.frame;
  // Maximized and fullscreen frames are not reusable browse sizes.
  if (!frame || !snapshot.normal) return null;
  rememberCompactWindowFrame(frame);
  return {
    mode: isBarWindowSize(frame) ? 'bar' : 'browse',
    frame,
  };
}

export async function expandCompactWindow() {
  if (compactWindowTransitioning) return false;
  const snapshot = await captureCurrentCompactWindowSnapshot();
  const current = snapshot?.frame;
  // Ignore transient maximized dimensions from the window manager.
  if (!current || (snapshot.normal && !isBarWindowSize(current))) return false;

  const memory = snapshot.normal
    ? rememberCompactWindowFrame(current)
    : loadCompactWindowMemory();
  const rememberedTarget =
    memory.browse ||
    normalizeCompactWindowFrame({ ...COMPACT_EXPANDED_SIZE, x: null, y: null });
  if (!rememberedTarget) return false;
  // Keep the current display position when restoring a remembered size.
  const target = buildCompactWindowTransitionFrame(current, rememberedTarget);
  if (!target) return false;
  compactWindowTransitioning = true;
  try {
    const expanded = await applyCompactWindowFrame(target);
    if (expanded) rememberCompactWindowFrame(target);
    return expanded;
  } finally {
    compactWindowTransitioning = false;
  }
}

export async function restoreCompactWindow() {
  if (compactWindowTransitioning) return false;
  const snapshot = await captureCurrentCompactWindowSnapshot();
  const current = snapshot?.frame;
  if (!current || (snapshot.normal && isBarWindowSize(current))) return false;

  const memory = snapshot.normal
    ? rememberCompactWindowFrame(current)
    : loadCompactWindowMemory();
  if (!memory.bar) return false;
  // Keep the compact bar on the display currently in use.
  const target = buildCompactWindowTransitionFrame(current, memory.bar);
  if (!target) return false;
  compactWindowTransitioning = true;
  try {
    const restored = await applyCompactWindowFrame(target);
    if (restored) rememberCompactWindowFrame(target);
    return restored;
  } finally {
    compactWindowTransitioning = false;
  }
}
