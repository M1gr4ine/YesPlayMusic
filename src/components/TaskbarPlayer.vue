<template>
  <div class="taskbar-mode taskbar-player-shell" data-testid="taskbar-player">
    <div
      id="lyricsWindow"
      class="lyrics-window mini-player"
      aria-label="桌面歌词窗口"
    >
      <header class="window-header drag-region" aria-label="窗口标题栏">
        <div class="window-actions">
          <button
            id="dockButton"
            class="icon-button no-drag mini-taskbar-dock"
            data-action="return-floating"
            type="button"
            :aria-label="locale.t('common.floatMiniPlayer', translateLabel)"
            :title="locale.t('common.floatMiniPlayer', translateLabel)"
            @click.stop="emit('undock')"
          >
            <svg
              class="icon icon-undock"
              viewBox="0 0 24 24"
              aria-hidden="true"
              focusable="false"
            >
              <path d="M4 17.5h16v3H4zM12 14V4m-4 4 4-4 4 4"></path>
            </svg>
          </button>
          <button
            id="closeButton"
            class="icon-button no-drag"
            data-action="close"
            type="button"
            :aria-label="locale.t('common.closeWindow', translateLabel)"
            :title="locale.t('common.closeWindow', translateLabel)"
            @click.stop="emit('close')"
          >
            <svg
              class="icon icon-close"
              viewBox="0 0 24 24"
              aria-hidden="true"
              focusable="false"
            >
              <path d="m6.7 6.7 10.6 10.6M17.3 6.7 6.7 17.3"></path>
            </svg>
          </button>
        </div>
      </header>

      <div class="window-content">
        <aside class="track-panel" aria-label="当前播放">
          <div class="artwork-shell" id="artworkShell">
            <img
              v-if="artwork"
              id="artwork"
              class="artwork"
              :src="artwork"
              alt="专辑封面"
              decoding="async"
              draggable="false"
            />
            <div v-else class="artwork-fallback" aria-hidden="true">
              <svg viewBox="0 0 128 128" focusable="false">
                <defs>
                  <linearGradient
                    id="fallbackGradient"
                    x1="0"
                    y1="0"
                    x2="1"
                    y2="1"
                  >
                    <stop offset="0" stop-color="#555555"></stop>
                    <stop offset="1" stop-color="#202020"></stop>
                  </linearGradient>
                </defs>
                <rect
                  width="128"
                  height="128"
                  rx="24"
                  fill="url(#fallbackGradient)"
                ></rect>
                <circle
                  cx="64"
                  cy="64"
                  r="27"
                  fill="none"
                  stroke="rgba(255,255,255,.72)"
                  stroke-width="3"
                ></circle>
                <circle
                  cx="64"
                  cy="64"
                  r="7"
                  fill="rgba(255,255,255,.86)"
                ></circle>
                <path
                  d="M64 37v27"
                  fill="none"
                  stroke="rgba(255,255,255,.72)"
                  stroke-width="3"
                  stroke-linecap="round"
                ></path>
                <path
                  d="M64 37c10 0 18-3 24-8"
                  fill="none"
                  stroke="rgba(255,255,255,.72)"
                  stroke-width="3"
                  stroke-linecap="round"
                ></path>
              </svg>
            </div>
          </div>

          <div class="track-copy">
            <h1 class="track-title" id="trackTitle" :title="trackTitle">
              {{ trackTitle }}
            </h1>
            <p class="track-artist" id="trackArtist" :title="artistName">
              {{ artistName }}
            </p>
          </div>

          <div class="transport-controls no-drag" aria-label="播放控制">
            <button
              class="transport-button no-drag"
              id="previousButton"
              data-action="previous"
              type="button"
              :aria-label="t('player.previous')"
              :title="t('player.previous')"
              @click.stop="emit('previous')"
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M5 5h2v14H5zM18.5 5.5v13L8.5 12z"></path>
              </svg>
            </button>
            <button
              class="transport-button no-drag"
              id="playButton"
              data-action="play"
              type="button"
              :aria-label="playing ? t('player.pause') : t('player.play')"
              :title="playing ? t('player.pause') : t('player.play')"
              :data-playing="playing"
              @click.stop="emit('play')"
            >
              <svg
                class="transport-icon icon-play"
                viewBox="0 0 24 24"
                aria-hidden="true"
                focusable="false"
              >
                <path d="m9 6.7 8.2 5.3L9 17.3V6.7Z"></path>
              </svg>
              <svg
                class="transport-icon icon-pause"
                viewBox="0 0 24 24"
                aria-hidden="true"
                focusable="false"
              >
                <path d="M8.2 6.3v11.4M15.8 6.3v11.4"></path>
              </svg>
            </button>
            <button
              class="transport-button no-drag"
              id="nextButton"
              data-action="next"
              type="button"
              :aria-label="t('player.next')"
              :title="t('player.next')"
              @click.stop="emit('next')"
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M17 5h2v14h-2zM5.5 5.5v13l10-6.5z"></path>
              </svg>
            </button>
            <button
              class="transport-button shuffle-button no-drag"
              id="shuffleButton"
              data-action="shuffle"
              type="button"
              :aria-label="t('player.shuffle')"
              :aria-pressed="shuffle"
              :title="t('player.shuffle')"
              @click.stop="emit('shuffle')"
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M3 6h2c5 0 7 12 12 12h4m-4-3 4 3-4 3M3 18h2c2.3 0 4-2.5 5.5-5m3-3C15 7.5 16 6 18 6h3m-4-3 4 3-4 3"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.8"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                ></path>
              </svg>
            </button>
          </div>
        </aside>

        <section class="lyrics-panel" aria-label="歌词">
          <div
            class="lyrics-viewport no-drag"
            id="lyricsViewport"
            :title="lyric"
            aria-label="当前歌词"
          >
            <div class="lyrics-stack" id="lyricsList">
              <div
                v-if="emptyKind"
                class="empty-state"
                id="currentLyric"
                data-testid="current-lyric"
              >
                <span
                  v-if="emptyKind === 'loading'"
                  class="loading-orb"
                  aria-hidden="true"
                ></span>
                <svg
                  v-else
                  class="empty-state-icon"
                  viewBox="0 0 24 24"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path
                    d="M8.2 18.2a2.8 2.8 0 1 0 0-5.6 2.8 2.8 0 0 0 0 5.6Zm2.8-2.8V6.2l8-1.8v8.1"
                  ></path>
                </svg>
                <div class="empty-state-copy">{{ emptyLabel }}</div>
              </div>
              <div
                v-else
                class="lyric-line is-active"
                id="currentLyric"
                data-testid="current-lyric"
              >
                {{ lyric }}
              </div>
            </div>
          </div>
        </section>
      </div>

      <footer class="timeline-panel" aria-label="播放进度">
        <div
          class="timeline-row"
          :style="{ '--range-progress': `${progressPercent}%` }"
        >
          <span class="timeline-time" id="elapsedTime">
            {{ elapsedLabel }} / {{ durationLabel }}
          </span>
          <input
            id="progressRange"
            class="progress-range no-drag"
            data-testid="taskbar-progress"
            type="range"
            min="0"
            :max="duration"
            step="0.1"
            :value="progress"
            :disabled="duration <= 0"
            aria-label="播放进度"
            :aria-valuetext="elapsedLabel"
            :aria-valuenow="Math.round(progress)"
            @pointerdown="emit('seek-pointerdown', $event)"
            @pointermove="emit('seek-pointermove', $event)"
            @pointerup="emit('seek-pointerup', $event)"
            @pointercancel="emit('seek-pointercancel', $event)"
            @keydown.left.prevent="emit('nudge-seek', -5)"
            @keydown.right.prevent="emit('nudge-seek', 5)"
          />
        </div>
      </footer>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import locale from '@/utils/taskbar/locale';
import type { MessageKey } from '@/locale/catalog';

const props = defineProps<{
  artwork: string;
  trackTitle: string;
  artistName: string;
  lyric: string;
  emptyKind: 'loading' | 'track' | null;
  emptyLabel: string;
  progress: number;
  duration: number;
  elapsedLabel: string;
  durationLabel: string;
  playing: boolean;
  shuffle: boolean;
}>();

const emit = defineEmits<{
  (event: 'previous'): void;
  (event: 'play'): void;
  (event: 'next'): void;
  (event: 'shuffle'): void;
  (event: 'undock'): void;
  (event: 'close'): void;
  (event: 'seek-pointerdown', value: PointerEvent): void;
  (event: 'seek-pointermove', value: PointerEvent): void;
  (event: 'seek-pointerup', value: PointerEvent): void;
  (event: 'seek-pointercancel', value: PointerEvent): void;
  (event: 'nudge-seek', value: number): void;
}>();

const { t } = useI18n();
const translateLabel = (key: MessageKey): string => String(t(key));
const progressPercent = computed(() => {
  if (props.duration <= 0) return 0;
  return Math.min(100, Math.max(0, (props.progress / props.duration) * 100));
});
</script>

<style scoped>
.taskbar-player-shell {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
}

.taskbar-mode {
  --cover-size: 100vh;
  color-scheme: dark;
  color: #fff;
  cursor: auto;
  font: 100%/1.4 -apple-system, BlinkMacSystemFont, 'Segoe UI',
    'Microsoft YaHei UI', sans-serif;
  -webkit-font-smoothing: antialiased;
}

.taskbar-mode *,
.taskbar-mode *::before,
.taskbar-mode *::after {
  box-sizing: border-box;
  user-select: none;
  -webkit-user-select: none;
  -webkit-user-drag: none;
}

.taskbar-mode button,
.taskbar-mode input {
  font: inherit;
  -webkit-app-region: no-drag;
}

button {
  color: inherit;
}

button:not(:disabled) {
  cursor: pointer !important;
}

button:disabled {
  cursor: default !important;
}

.lyrics-window {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
  border-radius: 14px;
  clip-path: inset(0 round 14px);
  background: transparent;
}

.lyrics-window::before {
  position: absolute;
  inset: 0;
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 14px;
  clip-path: inset(0 round 14px);
  background: linear-gradient(
    135deg,
    rgba(37, 37, 37, 0.94),
    rgba(22, 22, 22, 0.96)
  );
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06);
  content: '';
}

/* The strip is a child of Explorer's taskbar. WebView2 treats app-region drag
   as a caption hit on that child, so no surface may be draggable. */
.drag-region {
  -webkit-app-region: no-drag;
}

.no-drag {
  -webkit-app-region: no-drag;
}

.window-header {
  position: absolute;
  z-index: 4;
  top: 0;
  left: var(--cover-size);
  right: 0;
  height: 12px;
}

.window-actions {
  position: absolute;
  top: 3px;
  right: 10px;
  display: flex;
  gap: 2px;
}

.icon-button {
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  border-radius: 7px;
  background: transparent;
  color: rgba(255, 255, 255, 0.22);
  transition: color 140ms, background 140ms;
}

.lyrics-window:hover .icon-button,
.lyrics-window:focus-within .icon-button {
  color: rgba(255, 255, 255, 0.62);
}

.icon-button:hover {
  color: #fff !important;
  background: rgba(255, 255, 255, 0.1);
}

.icon {
  width: 14px;
  height: 14px;
  fill: currentColor;
}

.icon-close {
  display: block;
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-width: 1.7;
}

.icon-button:active,
.transport-button:active {
  transform: scale(0.92);
}

.window-content {
  position: absolute;
  inset: 0;
  display: grid;
  grid-template-columns: var(--cover-size) 108px minmax(0, 1fr);
  grid-template-rows: 100%;
  align-items: center;
  column-gap: 14px;
  padding: 0 22px 2px 0;
  -webkit-app-region: no-drag;
}

.track-panel {
  display: contents;
}

.artwork-shell {
  position: relative;
  z-index: 2;
  grid-column: 1;
  grid-row: 1;
  width: var(--cover-size);
  height: var(--cover-size);
  overflow: hidden;
  border-radius: 14px 10px 0 0;
  background: #242424;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
  -webkit-app-region: no-drag;
}

.artwork,
.artwork-fallback,
.artwork-fallback svg {
  width: 100%;
  height: 100%;
}

.artwork {
  position: relative;
  z-index: 1;
  display: block;
  object-fit: cover;
}

.artwork-fallback {
  position: absolute;
  inset: 0;
}

.track-copy {
  grid-column: 2;
  grid-row: 1;
  align-self: center;
  min-width: 0;
  margin-top: -34px;
  padding-left: 5px;
  -webkit-app-region: no-drag;
}

.track-title,
.track-artist {
  overflow: hidden;
  margin: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.track-title {
  font-size: 14px;
  font-weight: 600;
  letter-spacing: -0.02em;
  line-height: 1.35;
}

.track-artist {
  margin-top: 2px;
  color: rgba(255, 255, 255, 0.5);
  font-size: 11px;
  font-weight: 400;
}

.transport-controls {
  grid-column: 2;
  grid-row: 1;
  display: flex;
  align-items: center;
  align-self: center;
  gap: 8px;
  margin-top: 30px;
  -webkit-app-region: no-drag;
}

.transport-button {
  display: flex;
  flex: 0 0 18px;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 2px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: rgba(255, 255, 255, 0.66);
}

.transport-button svg {
  width: 14px;
  height: 14px;
  fill: currentColor;
}

.transport-button:disabled {
  opacity: 0.24;
  cursor: default;
}

.shuffle-button {
  color: rgba(255, 255, 255, 0.32);
}

.shuffle-button[aria-pressed='true'] {
  background: rgba(255, 255, 255, 0.14);
  color: #fff;
}

.transport-button:hover {
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
}

.transport-icon {
  display: none;
  width: 12px;
  height: 12px;
  fill: currentColor;
}

.transport-icon.icon-pause {
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-width: 2.8;
}

.transport-button[data-playing='false'] .icon-play,
.transport-button[data-playing='true'] .icon-pause {
  display: block;
}

.lyrics-panel {
  position: relative;
  grid-column: 3;
  grid-row: 1;
  width: 100%;
  height: 56px;
  min-width: 0;
  -webkit-app-region: no-drag;
}

.lyrics-viewport {
  position: absolute;
  inset: 0;
  overflow: hidden;
  padding: 15px 0;
  outline: none;
  overscroll-behavior: contain;
  scrollbar-width: none;
  cursor: default;
  -webkit-mask-image: linear-gradient(
    to bottom,
    transparent 0%,
    rgba(0, 0, 0, 0.55) 22%,
    #000 37%,
    #000 63%,
    rgba(0, 0, 0, 0.55) 78%,
    transparent 100%
  );
  mask-image: linear-gradient(
    to bottom,
    transparent 0%,
    rgba(0, 0, 0, 0.55) 22%,
    #000 37%,
    #000 63%,
    rgba(0, 0, 0, 0.55) 78%,
    transparent 100%
  );
}

.lyrics-viewport::-webkit-scrollbar {
  display: none;
  width: 0;
  height: 0;
}

.lyrics-stack {
  position: relative;
  display: block;
  width: 100%;
}

.lyric-line {
  display: block;
  width: 100%;
  height: 26px;
  min-height: 26px;
  margin: 0;
  padding: 0 4px;
  overflow: hidden;
  border: 0;
  outline: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.16);
  font-size: 17px;
  font-weight: 600;
  letter-spacing: 0.065em;
  line-height: 26px;
  text-align: center;
  text-overflow: ellipsis;
  white-space: nowrap;
  transform: scale(0.9);
  transform-origin: center;
  transition: color 280ms ease, transform 350ms cubic-bezier(0.2, 0.7, 0.2, 1);
}

.lyric-line.is-active {
  color: #fff;
  transform: scale(1);
}

.empty-state {
  display: flex;
  height: 26px;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: rgba(255, 255, 255, 0.5);
}

.empty-state-icon {
  width: 20px;
  height: 20px;
}

.empty-state-icon path,
.empty-state-icon circle {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.5;
}

.empty-state-copy {
  font-size: 17px;
  font-weight: 500;
  white-space: nowrap;
}

.empty-state-detail {
  display: none;
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

.timeline-panel {
  position: absolute;
  z-index: 3;
  right: 0;
  bottom: 0;
  left: 0;
  height: 8px;
}

.timeline-row {
  position: absolute;
  inset: 0;
}

.timeline-row::before {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 2px;
  background: linear-gradient(
    to right,
    rgba(255, 255, 255, 0.8) var(--range-progress, 0%),
    rgba(255, 255, 255, 0.12) var(--range-progress, 0%)
  );
  content: '';
  pointer-events: none;
}

.timeline-time {
  position: absolute;
  bottom: 5px;
  left: calc(var(--cover-size) + 14px);
  color: rgba(255, 255, 255, 0.3);
  font-size: 8px;
  font-weight: 400;
  line-height: 1;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
  pointer-events: none;
}

.progress-range {
  position: absolute;
  z-index: 2;
  bottom: 0;
  left: 0;
  width: 100%;
  height: 14px;
  padding: 0;
  margin: 0;
  border: 0;
  outline: none;
  background: transparent;
  cursor: pointer;
  appearance: none;
  -webkit-appearance: none;
}

.progress-range::-webkit-slider-runnable-track {
  height: 2px;
  background: transparent;
}

.progress-range::-webkit-slider-thumb {
  width: 1px;
  height: 14px;
  margin-top: -6px;
  border: 0;
  background: transparent;
  opacity: 0;
  -webkit-appearance: none;
}

.progress-range::-moz-range-track {
  height: 2px;
  background: transparent;
}

.progress-range::-moz-range-thumb {
  width: 1px;
  height: 14px;
  border: 0;
  background: transparent;
  opacity: 0;
}

.progress-range:disabled {
  cursor: default;
}

.taskbar-mode .lyrics-window {
  border-radius: 0;
  clip-path: none;
}

.taskbar-mode .lyrics-window::before {
  border: 0;
  border-radius: 0;
  clip-path: none;
  background: transparent;
  box-shadow: none;
}

.taskbar-mode .window-header {
  left: 0;
  height: 100%;
  pointer-events: none;
}

.taskbar-mode .window-actions {
  top: 0;
  right: 3px;
  bottom: 0;
  align-items: center;
  gap: 0;
  pointer-events: auto;
}

.taskbar-mode .icon-button {
  width: 23px;
  height: 30px;
  color: rgba(255, 255, 255, 0.65);
}

.taskbar-mode .icon-undock {
  display: block;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.taskbar-mode .window-content {
  grid-template-columns: var(--cover-size) minmax(0, 1fr) 96px;
  grid-template-rows: 100%;
  gap: 10px;
  padding: 0 53px 0 0;
}

.taskbar-mode .artwork-shell {
  width: 100vh;
  height: 100vh;
  border-radius: 0;
  box-shadow: none;
}

.taskbar-mode .track-copy {
  grid-column: 2;
  display: flex;
  align-self: start;
  align-items: baseline;
  gap: 5px;
  margin: 0;
  padding: 3px 0 0;
}

.taskbar-mode .track-title {
  color: rgba(255, 255, 255, 0.56);
  font-size: 10px;
  font-weight: 500;
  line-height: 13px;
}

.taskbar-mode .track-artist {
  margin: 0;
  font-size: 10px;
  line-height: 13px;
  opacity: 0.72;
}

.taskbar-mode .track-artist::before {
  content: '· ';
}

.taskbar-mode .transport-controls {
  grid-column: 3;
  margin: 0;
  gap: 2px;
}

.taskbar-mode .transport-button {
  flex-basis: 22px;
  width: 22px;
  height: 32px;
}

.taskbar-mode .lyrics-panel {
  grid-column: 2;
  align-self: end;
  height: 22px;
  margin-bottom: 2px;
}

.taskbar-mode .lyrics-viewport {
  padding: 0;
  -webkit-mask-image: none;
  mask-image: none;
}

.taskbar-mode .lyrics-stack {
  padding: 0;
}

.taskbar-mode .empty-state {
  height: 22px;
  gap: 4px;
  justify-content: flex-start;
  overflow: hidden;
}

.taskbar-mode .empty-state-copy {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
}

.taskbar-mode .empty-state-icon {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}

.taskbar-mode .lyric-line {
  height: 22px;
  min-height: 22px;
  padding: 0;
  font-size: 14px;
  font-weight: 500;
  letter-spacing: 0.015em;
  line-height: 22px;
  text-align: left;
  transform: none;
  transition: color 160ms ease;
}

.taskbar-mode .timeline-panel {
  left: var(--cover-size);
  height: 2px;
}

.taskbar-mode .timeline-time {
  display: none;
}

.taskbar-mode .progress-range {
  height: 3px;
}

.taskbar-mode .timeline-row::before {
  height: 1px;
  opacity: 0.4;
}

@media (max-width: 580px) {
  .window-content {
    grid-template-columns: var(--cover-size) 96px minmax(0, 1fr);
    column-gap: 12px;
    padding-right: 18px;
  }

  .lyric-line {
    font-size: 16px;
  }
}

@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation: none !important;
    scroll-behavior: auto !important;
    transition: none !important;
  }

  .lyric-line {
    transform: none;
  }
}
</style>
