<template>
  <div class="taskbar-player" :style="{ '--progress': `${progressPercent}%` }">
    <div class="artwork">
      <img
        v-if="snapshot.artwork"
        :src="snapshot.artwork"
        alt=""
        decoding="async"
        draggable="false"
      />
      <svg v-else viewBox="0 0 128 128" aria-hidden="true" focusable="false">
        <defs>
          <linearGradient id="artworkFallback" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="#555555"></stop>
            <stop offset="1" stop-color="#202020"></stop>
          </linearGradient>
        </defs>
        <rect width="128" height="128" rx="24" fill="url(#artworkFallback)" />
        <g
          fill="none"
          stroke="rgba(255,255,255,.72)"
          stroke-width="3"
          stroke-linecap="round"
        >
          <circle cx="64" cy="64" r="27" />
          <path d="M64 37v27M64 37c10 0 18-3 24-8" />
        </g>
        <circle cx="64" cy="64" r="7" fill="rgba(255,255,255,.86)" />
      </svg>
    </div>

    <div class="track">
      <span class="track-title" :title="snapshot.trackTitle">
        {{ snapshot.trackTitle }}
      </span>
      <span class="track-artist" :title="snapshot.artistName">
        {{ snapshot.artistName }}
      </span>
    </div>

    <div class="lyric" :title="snapshot.lyric">
      <div v-if="snapshot.emptyKind" class="lyric-empty">
        <span
          v-if="snapshot.emptyKind === 'loading'"
          class="loading-orb"
          aria-hidden="true"
        ></span>
        <svg v-else viewBox="0 0 24 24" aria-hidden="true" focusable="false">
          <path
            d="M8.2 18.2a2.8 2.8 0 1 0 0-5.6 2.8 2.8 0 0 0 0 5.6Zm2.8-2.8V6.2l8-1.8v8.1"
          />
        </svg>
        <span class="lyric-empty-label">{{ snapshot.emptyLabel }}</span>
      </div>
      <div v-else class="lyric-line">{{ snapshot.lyric }}</div>
    </div>

    <div class="transport">
      <button
        type="button"
        :aria-label="$t('player.previous')"
        :title="$t('player.previous')"
        @click.stop="send({ action: 'previous' })"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M5 5h2v14H5zM18.5 5.5v13L8.5 12z" />
        </svg>
      </button>
      <button
        type="button"
        :aria-label="$t(snapshot.playing ? 'player.pause' : 'player.play')"
        :title="$t(snapshot.playing ? 'player.pause' : 'player.play')"
        @click.stop="send({ action: 'play' })"
      >
        <svg v-if="snapshot.playing" class="icon-pause" viewBox="0 0 24 24">
          <path d="M8.2 6.3v11.4M15.8 6.3v11.4" />
        </svg>
        <svg v-else class="icon-play" viewBox="0 0 24 24" aria-hidden="true">
          <path d="m9 6.7 8.2 5.3L9 17.3V6.7Z" />
        </svg>
      </button>
      <button
        type="button"
        :aria-label="$t('player.next')"
        :title="$t('player.next')"
        @click.stop="send({ action: 'next' })"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M17 5h2v14h-2zM5.5 5.5v13l10-6.5z" />
        </svg>
      </button>
      <button
        type="button"
        class="shuffle"
        :aria-label="$t('player.shuffle')"
        :aria-pressed="snapshot.shuffle"
        :title="$t('player.shuffle')"
        @click.stop="send({ action: 'shuffle' })"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            d="M3 6h2c5 0 7 12 12 12h4m-4-3 4 3-4 3M3 18h2c2.3 0 4-2.5 5.5-5m3-3C15 7.5 16 6 18 6h3m-4-3 4 3-4 3"
          />
        </svg>
      </button>
    </div>

    <div class="window-actions">
      <button
        type="button"
        :aria-label="$t('common.floatMiniPlayer')"
        :title="$t('common.floatMiniPlayer')"
        @click.stop="send({ action: 'undock' })"
      >
        <svg class="icon-undock" viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 17.5h16v3H4zM12 14V4m-4 4 4-4 4 4" />
        </svg>
      </button>
      <button
        type="button"
        :aria-label="$t('common.closeWindow')"
        :title="$t('common.closeWindow')"
        @click.stop="send({ action: 'close' })"
      >
        <svg class="icon-close" viewBox="0 0 24 24" aria-hidden="true">
          <path d="m6.7 6.7 10.6 10.6M17.3 6.7 6.7 17.3" />
        </svg>
      </button>
    </div>

    <input
      class="progress"
      type="range"
      min="0"
      step="0.1"
      :max="snapshot.duration"
      :value="progress"
      :disabled="snapshot.duration <= 0"
      :aria-valuetext="formatTrackTime(progress) || '0:00'"
      @input="seekPreview = rangeValue($event)"
      @change="commitSeek"
      @keydown.left.prevent="nudgeSeek(-5)"
      @keydown.right.prevent="nudgeSeek(5)"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';
import { formatTrackTime } from '@/utils/trackTime';
import type {
  TaskbarOverlayCommand,
  TaskbarOverlaySnapshot,
} from '@/services/windowsTaskbar';

const props = defineProps<{ snapshot: TaskbarOverlaySnapshot }>();
const emit = defineEmits<{ command: [command: TaskbarOverlayCommand] }>();

const seekPreview = ref<number | null>(null);
const progress = computed(() => seekPreview.value ?? props.snapshot.progress);
const progressPercent = computed(() => {
  const { duration } = props.snapshot;
  return duration > 0 ? Math.min(100, (progress.value / duration) * 100) : 0;
});

function send(command: TaskbarOverlayCommand) {
  emit('command', command);
}

function rangeValue(event: Event): number | null {
  return event.target instanceof HTMLInputElement
    ? Number(event.target.value)
    : null;
}

function commitSeek(event: Event) {
  const value = rangeValue(event);
  seekPreview.value = null;
  if (value !== null) send({ action: 'seek', value });
}

function nudgeSeek(offset: number) {
  const { duration } = props.snapshot;
  if (duration <= 0) return;
  const value = Math.min(duration, Math.max(0, progress.value + offset));
  send({ action: 'seek', value });
}
</script>

<style scoped>
.taskbar-player {
  --cover-size: 100vh;
  position: relative;
  display: grid;
  grid-template-columns: var(--cover-size) minmax(0, 1fr) 96px;
  grid-template-rows: 100%;
  align-items: center;
  column-gap: 10px;
  width: 100%;
  height: 100%;
  padding-right: 53px;
  overflow: hidden;
  color-scheme: dark;
  color: #fff;
  font: 100%/1.4 -apple-system, BlinkMacSystemFont, 'Segoe UI',
    'Microsoft YaHei UI', sans-serif;
  -webkit-font-smoothing: antialiased;
  /* WebView2 treats a drag region on a taskbar child as its caption. */
  -webkit-app-region: no-drag;
}

.taskbar-player *,
.taskbar-player {
  box-sizing: border-box;
  user-select: none;
  -webkit-user-drag: none;
}

button {
  display: grid;
  place-items: center;
  padding: 0;
  border: 0;
  background: transparent;
  color: inherit;
  font: inherit;
  cursor: pointer;
}

button:active {
  transform: scale(0.92);
}

.artwork {
  position: relative;
  z-index: 2;
  grid-area: 1 / 1;
  width: var(--cover-size);
  height: var(--cover-size);
  overflow: hidden;
  background: #242424;
}

.artwork img,
.artwork svg {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.track {
  grid-area: 1 / 2;
  display: flex;
  align-self: start;
  align-items: baseline;
  gap: 5px;
  min-width: 0;
  padding-top: 3px;
  font-size: 10px;
  line-height: 13px;
}

.track-title,
.track-artist {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.track-title {
  color: rgba(255, 255, 255, 0.56);
  font-weight: 500;
  letter-spacing: -0.02em;
}

.track-artist {
  color: rgba(255, 255, 255, 0.5);
  opacity: 0.72;
}

.track-artist::before {
  content: '· ';
}

.lyric {
  grid-area: 1 / 2;
  align-self: end;
  height: 22px;
  min-width: 0;
  margin-bottom: 2px;
  overflow: hidden;
  cursor: default;
}

.lyric-line {
  overflow: hidden;
  font-size: 14px;
  font-weight: 500;
  letter-spacing: 0.015em;
  line-height: 22px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.lyric-empty {
  display: flex;
  height: 22px;
  align-items: center;
  gap: 4px;
  overflow: hidden;
  color: rgba(255, 255, 255, 0.5);
}

.lyric-empty svg {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.5;
}

.lyric-empty-label {
  overflow: hidden;
  font-size: 13px;
  font-weight: 500;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.loading-orb {
  width: 14px;
  height: 14px;
  border: 1.5px solid rgba(255, 255, 255, 0.18);
  border-top-color: rgba(255, 255, 255, 0.7);
  border-radius: 50%;
  animation: spin 900ms linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.transport {
  grid-area: 1 / 3;
  display: flex;
  align-items: center;
  gap: 2px;
}

.transport button {
  flex: 0 0 22px;
  width: 22px;
  height: 32px;
  padding: 2px;
  border-radius: 5px;
  color: rgba(255, 255, 255, 0.66);
}

.transport svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}

.transport .icon-play,
.transport .icon-pause {
  width: 12px;
  height: 12px;
}

.transport .icon-pause {
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-width: 2.8;
}

.transport .shuffle {
  color: rgba(255, 255, 255, 0.32);
}

.transport .shuffle svg {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.transport .shuffle[aria-pressed='true'] {
  background: rgba(255, 255, 255, 0.14);
  color: #fff;
}

.transport button:hover {
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
}

.window-actions {
  position: absolute;
  z-index: 4;
  top: 0;
  right: 3px;
  bottom: 0;
  display: flex;
  align-items: center;
}

.window-actions button {
  width: 23px;
  height: 30px;
  border-radius: 7px;
  color: rgba(255, 255, 255, 0.65);
  transition: color 140ms, background 140ms;
}

.taskbar-player:hover .window-actions button,
.taskbar-player:focus-within .window-actions button {
  color: rgba(255, 255, 255, 0.62);
}

.window-actions button:hover {
  color: #fff !important;
  background: rgba(255, 255, 255, 0.1);
}

.window-actions svg {
  display: block;
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
}

.icon-undock {
  width: 14px;
  height: 14px;
  stroke-width: 1.5;
  stroke-linejoin: round;
}

.icon-close {
  width: 12px;
  height: 12px;
  stroke-width: 1.7;
}

/* A one-pixel line after the cover; the range above it takes the input. */
.taskbar-player::after {
  position: absolute;
  z-index: 3;
  right: 0;
  bottom: 0;
  left: var(--cover-size);
  height: 1px;
  background: linear-gradient(
    to right,
    rgba(255, 255, 255, 0.8) var(--progress),
    rgba(255, 255, 255, 0.12) var(--progress)
  );
  opacity: 0.4;
  content: '';
  pointer-events: none;
}

.progress {
  position: absolute;
  z-index: 3;
  bottom: 0;
  left: var(--cover-size);
  width: calc(100% - var(--cover-size));
  height: 3px;
  margin: 0;
  padding: 0;
  border: 0;
  outline: none;
  background: transparent;
  cursor: pointer;
  appearance: none;
}

.progress:disabled {
  cursor: default;
}

.progress::-webkit-slider-runnable-track {
  height: 2px;
  background: transparent;
}

.progress::-webkit-slider-thumb {
  width: 1px;
  height: 14px;
  margin-top: -6px;
  opacity: 0;
  appearance: none;
}

@media (prefers-reduced-motion: reduce) {
  .taskbar-player *,
  .taskbar-player {
    animation: none !important;
    transition: none !important;
  }
}
</style>
