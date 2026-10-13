<template>
  <div
    class="taskbar-overlay"
    :class="{
      'is-flying': visual.phase === 'snapping' || visual.phase === 'returning',
      'reduced-motion': snapshot?.reducedMotion,
    }"
    :data-phase="visual.phase"
    :style="{ '--flight-duration': `${visual.durationMs}ms` }"
  >
    <div class="flight-backdrop" aria-hidden="true" />
    <TaskbarPlayer
      v-if="visual.phase !== 'hidden' && snapshot"
      class="player"
      :snapshot="snapshot"
      @command="send"
    />
    <div class="flight-outline" aria-hidden="true" />
  </div>
</template>

<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, shallowRef, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import TaskbarPlayer from '@/components/TaskbarPlayer.vue';
import {
  acknowledgeTaskbarOverlayVisual,
  connectTaskbarOverlay,
  sendTaskbarOverlayCommand,
} from '@/services/windowsTaskbar';
import type {
  TaskbarOverlayCommand,
  TaskbarOverlaySnapshot,
  TaskbarOverlayVisual,
} from '@/services/windowsTaskbar';

const snapshot = shallowRef<TaskbarOverlaySnapshot | null>(null);
const visual = shallowRef<TaskbarOverlayVisual>({
  phase: 'hidden',
  token: 0,
  durationMs: 240,
});
const { locale } = useI18n({ useScope: 'global' });
let disconnect: (() => void) | null = null;
let unmounted = false;

watch(
  () => snapshot.value?.locale,
  next => {
    if (next) locale.value = next;
  }
);

// The controller reveals a flight only after its phase has been painted.
async function applyVisual(next: TaskbarOverlayVisual) {
  if (next.token <= visual.value.token) return;
  visual.value = next;
  await nextTick();
  await new Promise(resolve =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))
  );
  if (visual.value.token !== next.token) return;
  acknowledgeTaskbarOverlayVisual(next.token).catch(error =>
    console.warn('[taskbar-overlay] visual acknowledgement failed', error)
  );
}

function send(command: TaskbarOverlayCommand) {
  sendTaskbarOverlayCommand(command).catch(error =>
    console.warn('[taskbar-overlay] command failed', error)
  );
}

onMounted(async () => {
  try {
    const stop = await connectTaskbarOverlay({
      onSnapshot: next => (snapshot.value = next),
      onVisual: next => void applyVisual(next),
    });
    if (unmounted) stop();
    else disconnect = stop;
  } catch (error) {
    console.warn('[taskbar-overlay] connection failed', error);
  }
});

onBeforeUnmount(() => {
  unmounted = true;
  disconnect?.();
});
</script>

<style scoped>
.taskbar-overlay {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
}

.flight-backdrop,
.player,
.flight-outline {
  position: absolute;
  inset: 0;
}

/* The flying strip keeps the floating player's glass, then lands transparent
   so only the strip remains on the taskbar. */
.flight-backdrop {
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

.player {
  z-index: 1;
}

.flight-outline {
  z-index: 2;
  border: 1px solid rgba(255, 255, 255, 0.9);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.3),
    inset 0 0 14px rgba(255, 255, 255, 0.18);
  opacity: 0;
  pointer-events: none;
  transition: opacity 180ms ease-out;
}

.is-flying .flight-outline {
  opacity: 1;
  transition: none;
}

.is-flying .player {
  pointer-events: none;
}

[data-phase='snapping'] .flight-backdrop {
  animation: fade-out var(--flight-duration) cubic-bezier(0.77, 0, 0.175, 1)
    both;
}

[data-phase='returning'] .flight-backdrop {
  animation: fade-in var(--flight-duration) cubic-bezier(0.77, 0, 0.175, 1) both;
}

/* Reduced motion has no geometric travel: the strip simply fades. */
.reduced-motion .flight-backdrop {
  animation: none;
}

.reduced-motion[data-phase='snapping'] :is(.player, .flight-outline) {
  animation: fade-in var(--flight-duration) ease-out both;
}

.reduced-motion[data-phase='returning'] :is(.player, .flight-outline) {
  animation: fade-out var(--flight-duration) ease-in both;
}

@keyframes fade-in {
  from {
    opacity: 0;
  }

  to {
    opacity: 1;
  }
}

@keyframes fade-out {
  from {
    opacity: 1;
  }

  to {
    opacity: 0;
  }
}
</style>
