<template>
  <div
    class="taskbar-overlay-root"
    :class="{
      'taskbar-reduced-motion': snapshot?.reducedMotion,
      'is-flying': flying,
    }"
    :data-phase="visual.phase"
    :data-visual-token="visual.token"
    :style="{ '--flight-duration': `${flightDurationMs}ms` }"
  >
    <div class="taskbar-flight-backdrop" aria-hidden="true" />
    <TaskbarPlayer
      v-if="visual.phase !== 'hidden' && playerProps"
      class="taskbar-overlay-player"
      v-bind="playerProps"
      @previous="invokeCommand({ action: 'previous' })"
      @play="invokeCommand({ action: 'play' })"
      @next="invokeCommand({ action: 'next' })"
      @shuffle="invokeCommand({ action: 'shuffle' })"
      @undock="invokeCommand({ action: 'undock' })"
      @close="invokeCommand({ action: 'close' })"
      @seek-pointerdown="startSeek"
      @seek-pointermove="moveSeek"
      @seek-pointerup="commitSeek"
      @seek-pointercancel="commitSeek"
      @nudge-seek="nudgeSeek"
    />
    <div class="taskbar-flight-outline" aria-hidden="true" />
  </div>
</template>

<script setup lang="ts">
import {
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  ref,
  shallowRef,
  watch,
} from 'vue';
import { useI18n } from 'vue-i18n';
import TaskbarPlayer from '@/components/TaskbarPlayer.vue';
import {
  acknowledgeTaskbarOverlayVisual,
  getTaskbarOverlayReadyState,
  invokeTaskbarOverlayCommand,
  listenTaskbarOverlayEvents,
  normalizeTaskbarOverlayReadyState,
  shouldApplyTaskbarOverlayVisual,
} from '@/services/windowsTaskbar';
import type {
  TaskbarOverlayCommand,
  TaskbarOverlaySnapshot,
  TaskbarOverlayVisual,
} from '@/services/windowsTaskbar';
import { formatTrackTime } from '@/utils/trackTime';
import { calculateMiniSeekTime } from '@/utils/miniPlayer';

const snapshot = shallowRef<TaskbarOverlaySnapshot | null>(null);
const visual = shallowRef<TaskbarOverlayVisual>({
  phase: 'hidden',
  token: -1,
});
const seekPreview = ref<number | null>(null);
const { locale } = useI18n({ useScope: 'global' });

const DEFAULT_FLIGHT_DURATION_MS = 240;
const flying = computed(
  () => visual.value.phase === 'snapping' || visual.value.phase === 'returning'
);
const flightDurationMs = computed(
  () => visual.value.durationMs ?? DEFAULT_FLIGHT_DURATION_MS
);

const playerProps = computed(() => {
  const current = snapshot.value;
  if (!current) return null;
  const progress = seekPreview.value ?? current.progress;
  return {
    artwork: current.artwork,
    trackTitle: current.trackTitle,
    artistName: current.artistName,
    lyric: current.lyric,
    emptyKind: current.emptyKind,
    emptyLabel: current.emptyLabel,
    progress,
    duration: current.duration,
    elapsedLabel:
      seekPreview.value === null
        ? current.elapsedLabel
        : formatTrackTime(progress) || '0:00',
    durationLabel: current.durationLabel,
    playing: current.playing,
    shuffle: current.shuffle,
  };
});

let cleanup: (() => void) | null = null;
let disposed = false;
let snapshotVersion = 0;
let visualAckGeneration = 0;
let lastAcknowledgedVisualToken = -1;
let visualAckRetryTimer: ReturnType<typeof setTimeout> | null = null;

watch(
  () => snapshot.value?.locale,
  nextLocale => {
    if (nextLocale) locale.value = nextLocale;
  },
  { immediate: true }
);

function applySnapshot(next: TaskbarOverlaySnapshot) {
  snapshotVersion += 1;
  snapshot.value = next;
}

function waitForTwoFrames(): Promise<void> {
  return new Promise(resolve => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

function scheduleVisualAcknowledgement(next: TaskbarOverlayVisual) {
  if (next.token === 0 || next.token <= lastAcknowledgedVisualToken) return;
  const generation = ++visualAckGeneration;
  void (async () => {
    await nextTick();
    await waitForTwoFrames();
    if (disposed || generation !== visualAckGeneration) return;
    const latest = visual.value;
    if (latest.token !== next.token || latest.phase !== next.phase) return;
    try {
      const acknowledged = await acknowledgeTaskbarOverlayVisual(next.token);
      if (
        acknowledged &&
        visual.value.token === next.token &&
        visual.value.phase === next.phase
      ) {
        lastAcknowledgedVisualToken = next.token;
      }
    } catch (error) {
      console.warn('[taskbar-overlay] visual acknowledgement failed', error);
      if (visualAckRetryTimer !== null) clearTimeout(visualAckRetryTimer);
      visualAckRetryTimer = setTimeout(() => {
        visualAckRetryTimer = null;
        if (
          !disposed &&
          visual.value.token === next.token &&
          visual.value.phase === next.phase
        ) {
          scheduleVisualAcknowledgement(next);
        }
      }, 100);
    }
  })();
}

function applyVisual(next: TaskbarOverlayVisual) {
  const current = visual.value;
  if (!shouldApplyTaskbarOverlayVisual(current, next)) return;
  visual.value = next;
  scheduleVisualAcknowledgement(next);
}

async function connectOverlay() {
  try {
    const unlisten = await listenTaskbarOverlayEvents({
      onSnapshot: applySnapshot,
      onVisual: applyVisual,
    });
    if (disposed) {
      unlisten();
      return;
    }
    cleanup = unlisten;

    const versionBeforeReady = snapshotVersion;
    const ready = normalizeTaskbarOverlayReadyState(
      await getTaskbarOverlayReadyState()
    );
    if (!ready) throw new Error('native overlay readiness state is invalid');
    if (snapshotVersion === versionBeforeReady && ready.snapshot) {
      applySnapshot(ready.snapshot);
    }
    applyVisual(ready.visual);
  } catch (error) {
    console.warn('[taskbar-overlay] connection failed', error);
  }
}

async function invokeCommand(command: TaskbarOverlayCommand) {
  try {
    await invokeTaskbarOverlayCommand(command);
  } catch (error) {
    console.warn('[taskbar-overlay] command failed', error);
  }
}

function getSeekTime(event: PointerEvent): number | null {
  const current = snapshot.value;
  const target = event.currentTarget;
  if (!current || current.duration <= 0 || !(target instanceof HTMLElement)) {
    return null;
  }
  const bounds = target.getBoundingClientRect();
  return calculateMiniSeekTime(
    event.clientX,
    bounds.left,
    bounds.width,
    current.duration
  );
}

function startSeek(event: PointerEvent) {
  seekPreview.value = getSeekTime(event);
}

function moveSeek(event: PointerEvent) {
  if (seekPreview.value === null) return;
  const next = getSeekTime(event);
  if (next !== null) seekPreview.value = next;
}

function commitSeek(event: PointerEvent) {
  if (seekPreview.value === null) return;
  moveSeek(event);
  const value = seekPreview.value;
  seekPreview.value = null;
  if (value !== null) void invokeCommand({ action: 'seek', value });
}

function nudgeSeek(offset: number) {
  const current = snapshot.value;
  if (!current || current.duration <= 0) return;
  const position = Math.min(
    current.duration,
    Math.max(0, (seekPreview.value ?? current.progress) + offset)
  );
  void invokeCommand({ action: 'seek', value: position });
}

onMounted(() => void connectOverlay());

onBeforeUnmount(() => {
  disposed = true;
  visualAckGeneration += 1;
  if (visualAckRetryTimer !== null) clearTimeout(visualAckRetryTimer);
  cleanup?.();
});
</script>

<style scoped>
.taskbar-overlay-root {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: transparent;
}

/* The flying strip keeps the floating player's glass and glow, then lands
   transparent so only the strip remains on the taskbar. */
.taskbar-flight-backdrop {
  position: absolute;
  z-index: 0;
  inset: 0;
  border: 1px solid rgba(255, 255, 255, 0.16);
  background: linear-gradient(
    135deg,
    rgba(37, 37, 37, 0.94),
    rgba(22, 22, 22, 0.96)
  );
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06);
  opacity: 0;
  pointer-events: none;
}

.taskbar-overlay-player {
  position: absolute;
  z-index: 1;
  inset: 0;
}

.taskbar-flight-outline {
  position: absolute;
  z-index: 2;
  inset: 0;
  border: 1px solid rgba(255, 255, 255, 0.9);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.3),
    inset 0 0 14px rgba(255, 255, 255, 0.18);
  opacity: 0;
  pointer-events: none;
  transition: opacity 180ms ease-out;
}

.is-flying .taskbar-flight-outline {
  opacity: 1;
  transition: none;
}

.is-flying .taskbar-overlay-player {
  pointer-events: none;
}

[data-phase='snapping'] .taskbar-flight-backdrop {
  animation: taskbar-glass-out var(--flight-duration)
    cubic-bezier(0.77, 0, 0.175, 1) both;
}

[data-phase='returning'] .taskbar-flight-backdrop {
  animation: taskbar-glass-in var(--flight-duration)
    cubic-bezier(0.77, 0, 0.175, 1) both;
}

/* Reduced motion has no geometric travel: the strip simply fades. */
.taskbar-reduced-motion .taskbar-flight-backdrop {
  animation: none;
}

.taskbar-reduced-motion[data-phase='snapping'] .taskbar-overlay-player,
.taskbar-reduced-motion[data-phase='snapping'] .taskbar-flight-outline {
  animation: taskbar-fade-in var(--flight-duration) ease-out both;
}

.taskbar-reduced-motion[data-phase='returning'] .taskbar-overlay-player,
.taskbar-reduced-motion[data-phase='returning'] .taskbar-flight-outline {
  animation: taskbar-fade-out var(--flight-duration) ease-in both;
}

@keyframes taskbar-glass-out {
  from {
    opacity: 1;
  }

  to {
    opacity: 0;
  }
}

@keyframes taskbar-glass-in {
  from {
    opacity: 0;
  }

  to {
    opacity: 1;
  }
}

@keyframes taskbar-fade-in {
  from {
    opacity: 0;
  }

  to {
    opacity: 1;
  }
}

@keyframes taskbar-fade-out {
  from {
    opacity: 1;
  }

  to {
    opacity: 0;
  }
}
</style>
